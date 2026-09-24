//! Binding resolution: a contract's binding as a DataFusion table provider (peQL design 4.4).

use std::collections::BTreeMap;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use datafusion::arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use datafusion::datasource::TableProvider;
use datafusion::datasource::file_format::parquet::ParquetFormat;
use datafusion::datasource::listing::{
    ListingOptions, ListingTable, ListingTableConfig, ListingTableUrl,
};
use datafusion::parquet::file::reader::{FileReader, SerializedFileReader};
use datafusion::parquet::file::statistics::Statistics;
use parcel_core::CompiledContract;
use sha2_compat::sha256_hex;

use crate::error::Result;
use crate::manifest::{FileEntry, FlagStatus};

/// Key-value metadata key stamped into every Parquet file (design 10, stage 6).
pub const CONTRACT_HASH_KEY: &str = "parcel.contract_hash";

/// The directory a contract's binding points at.
pub fn root(contract: &CompiledContract, base: &Path) -> PathBuf {
    let raw = contract.binding.parquet.trim_start_matches("file://");
    let p = Path::new(raw);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        base.join(p)
    }
}

/// Schema of the files on disk: row columns minus partition columns, plus flag columns.
pub fn file_schema(contract: &CompiledContract) -> SchemaRef {
    let parts = &contract.binding.partitioned_by;
    let mut fields: Vec<Field> = contract
        .scan_schema
        .fields()
        .iter()
        .filter(|f| !parts.contains(f.name()))
        .map(|f| f.as_ref().clone())
        .collect();
    for flag in &contract.flags {
        fields.push(Field::new(&flag.column, DataType::Boolean, true));
    }
    for d in &contract.derived {
        fields.push(Field::new(&d.column, d.ty.to_arrow(), true));
    }
    Arc::new(Schema::new(fields))
}

/// A listing table over every Parquet file under the binding, with hive partition columns.
pub fn provider(contract: &CompiledContract, root: &Path) -> Result<Arc<dyn TableProvider>> {
    std::fs::create_dir_all(root)?;
    let url = ListingTableUrl::parse(format!("{}/", root.canonicalize()?.display()))?;
    let partition_cols: Vec<(String, DataType)> = contract
        .binding
        .partitioned_by
        .iter()
        .map(|p| {
            let f = contract
                .row_schema
                .field_with_name(p)
                .expect("checker verified partition columns");
            (p.clone(), f.data_type().clone())
        })
        .collect();
    let options = ListingOptions::new(Arc::new(ParquetFormat::default().with_enable_pruning(true)))
        .with_file_extension(".parquet")
        .with_table_partition_cols(partition_cols);
    let config = ListingTableConfig::new(url)
        .with_listing_options(options)
        .with_schema(file_schema(contract));
    Ok(Arc::new(ListingTable::try_new(config)?))
}

/// Every Parquet file under the binding, relative to its root, in path order.
pub fn list_files(root: &Path) -> Result<Vec<PathBuf>> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let p = entry?.path();
            if p.is_dir() {
                walk(&p, out)?;
            } else if p.extension().is_some_and(|e| e == "parquet") {
                out.push(p);
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    if root.exists() {
        walk(root, &mut out)?;
    }
    out.sort();
    Ok(out)
}

/// Read one file's footer: its row count, contract hash and per-flag status from column statistics.
pub fn file_entry(root: &Path, path: &Path, flag_columns: &[String]) -> Result<FileEntry> {
    let reader = SerializedFileReader::new(File::open(path)?)
        .map_err(|e| crate::EngineError::Invalid(e.to_string()))?;
    let meta = reader.metadata();
    let file_meta = meta.file_metadata();
    let contract_hash = file_meta
        .key_value_metadata()
        .and_then(|kv| kv.iter().find(|k| k.key == CONTRACT_HASH_KEY))
        .and_then(|k| k.value.clone())
        .unwrap_or_default();
    let schema = file_meta.schema_descr();
    let mut flags = BTreeMap::new();
    for flag in flag_columns {
        let Some(idx) = (0..schema.num_columns()).find(|i| schema.column(*i).name() == flag) else {
            continue;
        };
        let (mut any_true, mut any_false) = (false, false);
        for rg in meta.row_groups() {
            match rg.column(idx).statistics() {
                Some(Statistics::Boolean(s)) => {
                    // A null flag never happens (flags are null-safe), so min/max decide the status.
                    if s.max_opt() == Some(&true) {
                        any_true = true;
                    }
                    if s.min_opt() == Some(&false) {
                        any_false = true;
                    }
                }
                _ => {
                    any_true = true;
                    any_false = true;
                }
            }
        }
        let status = match (any_true, any_false) {
            (true, false) => FlagStatus::AllPass,
            (false, true) => FlagStatus::AllFail,
            _ => FlagStatus::Mixed,
        };
        flags.insert(flag.clone(), status);
    }
    Ok(FileEntry {
        path: path
            .strip_prefix(root)
            .unwrap_or(path)
            .display()
            .to_string(),
        rows: file_meta.num_rows(),
        bytes: std::fs::metadata(path)?.len(),
        contract_hash,
        flags,
    })
}

/// sha256 over (relative path, sha256 of bytes) for every file, in path order.
pub fn data_hash(root: &Path, files: &[PathBuf]) -> Result<String> {
    let mut acc = String::new();
    for f in files {
        let bytes = std::fs::read(f)?;
        acc.push_str(&f.strip_prefix(root).unwrap_or(f).display().to_string());
        acc.push(':');
        acc.push_str(&sha256_hex(&bytes));
        acc.push('\n');
    }
    Ok(sha256_hex(acc.as_bytes()))
}

mod sha2_compat {
    pub fn sha256_hex(bytes: &[u8]) -> String {
        parcel_core::hash::sha256_hex(bytes)
    }
}
