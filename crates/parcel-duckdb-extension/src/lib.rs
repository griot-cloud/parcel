//! The `parcel` DuckDB extension: query a compiled contract by name.
//!
//! ```sql
//! LOAD 'parcel.duckdb_extension';
//! SET VARIABLE parcel_tenant = 'our_company';
//! SET VARIABLE parcel_purpose = 'analytics';
//! SELECT region, count(*) FROM "sales/customers@1.0.0" GROUP BY region;
//! SELECT * FROM 'contracts/customers.parcel';
//! ```
//!
//! A replacement scan resolves the name. DuckDB asks it about any table it does not know;
//! for `<contract>@<version>`, or a path ending in `.parcel`, the extension reads the compiled
//! contract and answers with `query('<governed query>')`. DuckDB inlines that query and plans
//! it like any other: nothing is created per contract, the data stays where it is, and filters
//! reach the scan. Contracts (`.parcel` files, or `.duckdb.json` artifacts written with
//! `parcel compile --target duckdb --source`) are found in the directories `PARCEL_CONTRACTS` lists
//! (separated by `:`), else in `./contracts` and the working directory, and are read again
//! whenever their file changes. `parcel_contracts()` lists them.

use std::collections::HashMap;
use std::ffi::{CStr, CString, c_char, c_void};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

use duckdb::Connection;
use duckdb::core::{DataChunkHandle, Inserter, LogicalTypeHandle, LogicalTypeId};
use duckdb::ffi;
use duckdb::vtab::{BindInfo, InitInfo, TableFunctionInfo, VTab};
use parcel_core::Compilation;
use parcel_runtime::compiled::CompiledBytes;

/// The C API version the extension needs.
const MIN_DUCKDB_VERSION: &str = "v1.2.0";

/// One compiled contract on disk, decoded.
#[derive(Clone)]
struct Loaded {
    name: String,
    contract: String,
    version: String,
    compilation_hash: String,
    /// The governed DuckDB query, or why there is none.
    query: Result<String, String>,
}

/// Decoded contracts by path, with the modification time they were read at.
fn cache() -> &'static Mutex<HashMap<PathBuf, (SystemTime, Loaded)>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, (SystemTime, Loaded)>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

fn load(path: &Path) -> Result<Loaded, String> {
    let modified = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .map_err(|e| format!("{}: {e}", path.display()))?;
    if let Some((at, l)) = cache().lock().map_err(|e| e.to_string())?.get(path)
        && *at == modified
    {
        return Ok(l.clone());
    }
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let loaded = if path.extension().is_some_and(|x| x == "json") {
        // A DuckDB artifact: `parcel compile --target duckdb --source ... -o x.duckdb.json`.
        let a: parcel_sql::Artifact =
            serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
        if a.format != parcel_sql::FORMAT || a.engine != "duckdb" {
            return Err(format!(
                "{}: not a DuckDB artifact ({} for {})",
                path.display(),
                a.format,
                a.engine
            ));
        }
        Loaded {
            name: a.name,
            contract: a.contract,
            version: a.version,
            compilation_hash: a.compilation_hash,
            query: Ok(a.query),
        }
    } else {
        let c = Compilation::from_bytes(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
        let cc = &c.contract;
        Loaded {
            name: parcel_sql::target_name(&cc.name, &cc.version.to_string()),
            contract: cc.name.clone(),
            version: cc.version.to_string(),
            compilation_hash: cc.compilation_hash.clone(),
            query: parcel_duckdb::artifact(&c, None).map(|a| a.query),
        }
    };
    cache()
        .lock()
        .map_err(|e| e.to_string())?
        .insert(path.to_owned(), (modified, loaded.clone()));
    Ok(loaded)
}

/// The directories contracts are found in.
fn directories() -> Vec<PathBuf> {
    match std::env::var("PARCEL_CONTRACTS") {
        Ok(v) if !v.is_empty() => v.split(':').map(PathBuf::from).collect(),
        _ => vec![PathBuf::from("contracts"), PathBuf::from(".")],
    }
}

/// Every `.parcel` file in the contract directories.
fn files() -> Vec<PathBuf> {
    let mut out = Vec::new();
    for dir in directories() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            let ours = p.extension().is_some_and(|x| x == "parcel")
                || p.to_string_lossy().ends_with(".duckdb.json");
            if ours && !out.contains(&p) {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

/// The governed query for `name`; `None` when the name is not a contract's.
fn resolve(name: &str) -> Result<Option<String>, String> {
    if name.ends_with(".parcel") || name.ends_with(".duckdb.json") {
        return load(Path::new(name))?.query.map(Some);
    }
    if !name.contains('@') {
        return Ok(None);
    }
    for f in files() {
        if let Ok(l) = load(&f)
            && l.name == name
        {
            return l.query.map(Some).map_err(|e| format!("{name}: {e}"));
        }
    }
    Err(format!(
        "no compiled contract named `{name}` in {}",
        directories()
            .iter()
            .map(|d| d.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    ))
}

/// The replacement scan: a contract's name becomes `query('<governed query>')`.
unsafe extern "C" fn replacement(
    info: ffi::duckdb_replacement_scan_info,
    table_name: *const c_char,
    _data: *mut c_void,
) {
    let name = unsafe { CStr::from_ptr(table_name) }
        .to_string_lossy()
        .into_owned();
    let answer = std::panic::catch_unwind(|| resolve(&name))
        .unwrap_or_else(|_| Err("the parcel extension failed reading the contract".into()));
    let set_error = |message: String| {
        let m = CString::new(format!("parcel: {message}")).unwrap_or_default();
        unsafe { ffi::duckdb_replacement_scan_set_error(info, m.as_ptr()) };
    };
    match answer {
        Ok(None) => {}
        Err(e) => set_error(e),
        Ok(Some(sql)) => {
            let Ok(sql) = CString::new(sql) else {
                return set_error("the governed query has a NUL byte".into());
            };
            unsafe {
                ffi::duckdb_replacement_scan_set_function_name(info, c"query".as_ptr());
                let mut value = ffi::duckdb_create_varchar(sql.as_ptr());
                ffi::duckdb_replacement_scan_add_parameter(info, value);
                ffi::duckdb_destroy_value(&mut value);
            }
        }
    }
}

/// `parcel_contracts()`: every contract the extension can resolve.
struct Contracts;

#[repr(C)]
struct ContractsBind {
    rows: Vec<[String; 6]>,
}

#[repr(C)]
struct ContractsInit {
    done: std::sync::atomic::AtomicBool,
}

impl VTab for Contracts {
    type InitData = ContractsInit;
    type BindData = ContractsBind;

    fn bind(bind: &BindInfo) -> Result<Self::BindData, Box<dyn std::error::Error>> {
        for c in [
            "name",
            "contract",
            "version",
            "compilation_hash",
            "file",
            "error",
        ] {
            bind.add_result_column(c, LogicalTypeHandle::from(LogicalTypeId::Varchar));
        }
        let rows = files()
            .into_iter()
            .map(|f| {
                let file = f.display().to_string();
                match load(&f) {
                    Ok(l) => [
                        l.name,
                        l.contract,
                        l.version,
                        l.compilation_hash,
                        file,
                        l.query.err().unwrap_or_default(),
                    ],
                    Err(e) => [
                        String::new(),
                        String::new(),
                        String::new(),
                        String::new(),
                        file,
                        e,
                    ],
                }
            })
            .collect();
        Ok(ContractsBind { rows })
    }

    fn init(_: &InitInfo) -> Result<Self::InitData, Box<dyn std::error::Error>> {
        Ok(ContractsInit {
            done: std::sync::atomic::AtomicBool::new(false),
        })
    }

    fn func(
        func: &TableFunctionInfo<Self>,
        output: &mut DataChunkHandle,
    ) -> Result<(), Box<dyn std::error::Error>> {
        if func
            .get_init_data()
            .done
            .swap(true, std::sync::atomic::Ordering::Relaxed)
        {
            output.set_len(0);
            return Ok(());
        }
        let rows = &func.get_bind_data().rows;
        for col in 0..6 {
            let v = output.flat_vector(col);
            for (i, r) in rows.iter().enumerate() {
                v.insert(i, CString::new(r[col].clone())?);
            }
        }
        output.set_len(rows.len());
        Ok(())
    }
}

/// # Safety
///
/// Called by DuckDB when the extension loads.
unsafe fn init(
    info: ffi::duckdb_extension_info,
    access: *const ffi::duckdb_extension_access,
) -> Result<bool, Box<dyn std::error::Error>> {
    unsafe {
        if !ffi::duckdb_rs_extension_api_init(info, access, MIN_DUCKDB_VERSION)? {
            return Ok(false);
        }
        let get_database = (*access)
            .get_database
            .ok_or("get_database is missing from duckdb_extension_access")?;
        let db_ptr = get_database(info);
        if db_ptr.is_null() {
            return Ok(false);
        }
        let db: ffi::duckdb_database = *db_ptr;
        ffi::duckdb_add_replacement_scan(db, Some(replacement), std::ptr::null_mut(), None);
        let con = Connection::open_from_raw(db.cast())?;
        con.register_table_function::<Contracts>("parcel_contracts")?;
        Ok(true)
    }
}

/// # Safety
///
/// The entry point DuckDB calls when the extension loads.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn parcel_init_c_api(
    info: ffi::duckdb_extension_info,
    access: *const ffi::duckdb_extension_access,
) -> bool {
    match unsafe { init(info, access) } {
        Ok(v) => v,
        Err(e) => {
            if let Some(set_error) = unsafe { (*access).set_error } {
                let m = CString::new(e.to_string()).unwrap_or_default();
                unsafe { set_error(info, m.as_ptr()) };
            }
            false
        }
    }
}
