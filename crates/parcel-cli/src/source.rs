//! Reading a contract and working out the column types it is checked against.
//!
//! Types come from the first of: the file given with `--schema` (or `--data`), the Parquet
//! data the contract's binding names when it is on this machine, and the types the contract
//! declares in `expose`.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use datafusion::arrow::array::RecordBatch;
use datafusion::arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use datafusion::prelude::{CsvReadOptions, ParquetReadOptions, SessionContext};
use parcel_core::document::Source;
use parcel_core::inherit::Resolved;
use parcel_core::structure::Lines;
use parcel_core::{Code, Compilation, ContractDoc, Diagnostic, Registry};

use crate::TypeHints;

/// A contract read from disk, with the documents next to it (where parents are found).
pub struct Contract {
    pub path: PathBuf,
    pub source: String,
    pub doc: ContractDoc,
    siblings: BTreeMap<String, ContractDoc>,
}

impl Contract {
    /// `Err` for a file that cannot be read; `Ok(Err(..))` for a document with problems.
    pub fn read(path: &Path) -> Result<Result<Contract, (String, Vec<Diagnostic>)>, String> {
        let source =
            std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(match ContractDoc::parse_all(&source) {
            Ok(doc) => Ok(Contract {
                path: path.to_owned(),
                source,
                doc,
                siblings: sibling_documents(path),
            }),
            Err(d) => Err((source, d)),
        })
    }

    pub fn lines(&self) -> Lines<'_> {
        Lines::new(&self.source)
    }

    /// The contract with its inheritance chain flattened, when the chain resolves.
    pub fn resolve(&self) -> Result<Resolved, Vec<Diagnostic>> {
        parcel_core::inherit::resolve(&self.doc, &|n| self.siblings.get(n).cloned())
    }

    pub fn compile(
        &self,
        schema: &Schema,
        registry: &Registry,
    ) -> Result<Compilation, Vec<Diagnostic>> {
        parcel_core::compile::compile_resolved(&self.resolve()?, schema, registry)
    }

    /// The directory relative paths in the contract are read from.
    fn dir(&self) -> PathBuf {
        match self.path.parent() {
            Some(d) if !d.as_os_str().is_empty() => d.to_path_buf(),
            _ => PathBuf::from("."),
        }
    }
}

/// Every contract document next to `contract`, by name: where parents are found.
fn sibling_documents(contract: &Path) -> BTreeMap<String, ContractDoc> {
    let mut out = BTreeMap::new();
    let dir = match contract.parent() {
        Some(d) if !d.as_os_str().is_empty() => d.to_path_buf(),
        _ => PathBuf::from("."),
    };
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return out;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.extension()
            .is_some_and(|x| x == "yaml" || x == "yml" || x == "json")
            && let Ok(text) = std::fs::read_to_string(&p)
            && let Ok(doc) = ContractDoc::parse(&text)
        {
            out.entry(doc.contract.clone()).or_insert(doc);
        }
    }
    out
}

/// Where the column types a contract was checked against came from.
#[derive(Clone, Debug)]
pub enum TypesFrom {
    /// A data file named on the command line.
    File(PathBuf),
    /// The Parquet data the binding names, found on this machine.
    Binding(PathBuf),
    /// The types declared in `expose`, because no data was available; `why` says why not.
    Expose { why: String },
}

impl fmt::Display for TypesFrom {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TypesFrom::File(p) => write!(f, "{}", p.display()),
            TypesFrom::Binding(p) => write!(f, "{} (binding)", p.display()),
            TypesFrom::Expose { why } => write!(f, "expose ({why})"),
        }
    }
}

/// The schema to check `contract` against: `explicit` when given, else the binding's data, else
/// the types `expose` declares.
pub async fn schema_for(
    contract: &Contract,
    explicit: Option<&Path>,
    hints: &TypeHints,
) -> Result<(SchemaRef, TypesFrom), String> {
    if let Some(p) = explicit {
        return Ok((read_schema(p, hints).await?, TypesFrom::File(p.to_owned())));
    }
    let resolved = contract.resolve().ok().map(|r| r.doc);
    let doc = resolved.as_ref().unwrap_or(&contract.doc);
    let why = match doc.binding.as_ref().map(|b| &b.source) {
        Some(Source::Parquet(loc)) => {
            let local = loc.strip_prefix("file://").unwrap_or(loc);
            if local.contains("://") {
                format!("binding {loc} is remote")
            } else {
                let dir = contract.dir();
                let path = if dir == Path::new(".") {
                    PathBuf::from(local)
                } else {
                    dir.join(local)
                };
                if path.is_dir() && !has_parquet(&path) {
                    format!("binding {} has no Parquet files", path.display())
                } else if path.exists() {
                    match read_schema(&path, hints).await {
                        Ok(s) => return Ok((s, TypesFrom::Binding(path))),
                        Err(e) => {
                            let first = e.split(['.', '\n']).next().unwrap_or_default();
                            format!("binding {} unreadable: {first}", path.display())
                        }
                    }
                } else {
                    format!("binding {} not found", path.display())
                }
            }
        }
        Some(Source::Iceberg(t)) => format!("binding is Iceberg table {t}"),
        None => "no binding".into(),
    };
    Ok((Arc::new(expose_schema(doc)), TypesFrom::Expose { why }))
}

/// The Hive partition columns of a Parquet directory, outermost first: `region` for
/// `data/customers/region=North/part-0.parquet`.
fn hive_partitions(dir: &Path) -> Vec<String> {
    let mut keys = Vec::new();
    let mut at = dir.to_path_buf();
    loop {
        let Ok(rd) = std::fs::read_dir(&at) else {
            return keys;
        };
        let next = rd.flatten().map(|e| e.path()).find(|p| {
            p.is_dir()
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.contains('=') && !n.starts_with('.'))
        });
        let Some(next) = next else {
            return keys;
        };
        let name = next
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        keys.push(name.split('=').next().unwrap_or_default().to_owned());
        at = next;
    }
}

/// Whether a directory holds Parquet files, at any depth.
fn has_parquet(dir: &Path) -> bool {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .any(|e| {
            let p = e.path();
            if p.is_dir() {
                has_parquet(&p)
            } else {
                p.extension().is_some_and(|x| x == "parquet")
            }
        })
}

/// The schema `expose` declares. A column whose type does not parse is kept as a placeholder,
/// so the only message about it is the one about its type.
fn expose_schema(doc: &ContractDoc) -> Schema {
    let fields: Vec<Field> = doc
        .expose
        .iter()
        .flatten()
        .map(|c| {
            let ty = parcel_core::types::parse_type_name(&c.type_name).unwrap_or(DataType::Null);
            Field::new(&c.name, ty, true)
        })
        .collect();
    Schema::new(fields)
}

/// A Parquet or CSV file's schema, without reading its rows.
pub async fn read_schema(path: &Path, hints: &TypeHints) -> Result<SchemaRef, String> {
    let (ctx, df) = open(path, hints).await?;
    drop(ctx);
    Ok(Arc::new(df.schema().as_arrow().clone()))
}

/// A Parquet or CSV file (or a directory of Parquet files): its schema and its rows,
/// counted into `t` as they arrive.
pub async fn read_data_tracked(
    path: &Path,
    hints: &TypeHints,
    t: &crate::progress::Tracker,
) -> Result<(SchemaRef, Vec<RecordBatch>), String> {
    use futures::StreamExt;
    let (_ctx, df) = open(path, hints).await?;
    let schema: SchemaRef = Arc::new(df.schema().as_arrow().clone());
    let mut stream = df.execute_stream().await.map_err(|e| e.to_string())?;
    let mut batches = Vec::new();
    while let Some(b) = stream.next().await {
        let b = b.map_err(|e| e.to_string())?;
        t.add(b.num_rows() as u64);
        batches.push(b);
    }
    t.finish();
    Ok((schema, batches))
}

async fn open(
    path: &Path,
    hints: &TypeHints,
) -> Result<(SessionContext, datafusion::dataframe::DataFrame), String> {
    if !path.exists() {
        return Err(format!("{}: no such file or directory", path.display()));
    }
    let ctx = SessionContext::new();
    let p = path.to_str().ok_or("path is not UTF-8")?;
    let hints = hints.parse()?;
    let df = if path.extension().is_some_and(|e| e == "csv") {
        let inferred = ctx
            .read_csv(p, CsvReadOptions::new())
            .await
            .map_err(|e| format!("{}: {e}", path.display()))?;
        let names: Vec<String> = inferred
            .schema()
            .fields()
            .iter()
            .map(|f| f.name().clone())
            .collect();
        if let Some((c, _)) = hints.iter().find(|(c, _)| !names.contains(c)) {
            let help = parcel_core::diag::did_you_mean(c, names.iter().map(String::as_str))
                .map(|n| format!("; did you mean `{n}`?"))
                .unwrap_or_default();
            return Err(format!(
                "--type {c}=...: {} has no column `{c}`{help}",
                path.display()
            ));
        }
        let fields: Vec<Field> = inferred
            .schema()
            .as_arrow()
            .fields()
            .iter()
            .map(|f| match hints.iter().find(|(c, _)| c == f.name()) {
                Some((_, t)) => f.as_ref().clone().with_data_type(t.clone()),
                None => f.as_ref().clone(),
            })
            .collect();
        let schema = Schema::new(fields);
        ctx.read_csv(p, CsvReadOptions::new().schema(&schema)).await
    } else {
        let partitions = hive_partitions(path);
        let options = ParquetReadOptions::default().table_partition_cols(
            partitions
                .into_iter()
                .map(|k| (k, DataType::Utf8))
                .collect(),
        );
        ctx.read_parquet(p, options).await
    }
    .map_err(|e| format!("{}: {e}", path.display()))?;
    Ok((ctx, df))
}

/// Give each diagnostic the line it is about, and say what to do when the types came from
/// `expose` and a rule reads a column it does not list.
pub fn locate(diags: &mut Vec<Diagnostic>, contract_source: &str, from: Option<&TypesFrom>) {
    // A column whose declared type does not parse is a placeholder; its type error is enough.
    diags.retain(|d| !(d.code == Code::UnreadableColumn && d.message.contains("(Null)")));
    let csv = matches!(from, Some(TypesFrom::File(p)) if p.extension().is_some_and(|e| e == "csv"));
    let lines = Lines::new(contract_source);
    for d in diags.iter_mut() {
        if csv
            && d.code == Code::ExposeTypeMismatch
            && let (Some(col), Some(declared)) =
                (first_quoted(&d.message), d.message.rsplit(" as ").next())
        {
            let found = d
                .message
                .split(" is ")
                .nth(1)
                .and_then(|r| r.split(' ').next())
                .unwrap_or("its type");
            d.help = Some(format!(
                "pass `--type {col}={declared}`, or expose it as `{found}`"
            ));
        }
        if d.line.is_none() {
            let m = &d.message;
            d.line = match &d.rule {
                Some(r) if in_expression(d.code) => lines.rule_expr(r),
                Some(r) => lines.rule(r),
                None if m.starts_with("expose `")
                    || m.starts_with("exposed column `")
                    || m.starts_with("column `") =>
                {
                    first_quoted(m)
                        .and_then(|c| lines.exposed(c))
                        .or_else(|| lines.top("expose"))
                }
                None if m.starts_with("partition column") => lines.top("binding"),
                None if m.starts_with("extensions.") => lines.top("extensions"),
                None if m.contains("residency") => lines.top("residency"),
                None => None,
            };
        }
        if let Some(TypesFrom::Expose { .. }) = from
            && d.code == Code::UnknownColumn
            && let Some(col) = last_quoted(&d.message)
        {
            let col = col.to_owned();
            d.message = format!("type of `{col}` unknown: not exposed, no data");
            d.help = Some(format!("pass --schema <data file>, or expose `{col}`"));
        }
    }
}

/// Codes about a rule's expression, which point at its `expr:` line.
fn in_expression(code: Code) -> bool {
    matches!(
        code,
        Code::Parse
            | Code::OutsideProfile
            | Code::UnknownIdentifier
            | Code::UnknownField
            | Code::UnknownColumn
            | Code::UnreadableColumn
            | Code::UnknownFunction
            | Code::NoMatchingOverload
            | Code::TypeMismatch
            | Code::Namespace
            | Code::NonDeterministic
    )
}

fn first_quoted(s: &str) -> Option<&str> {
    let start = s.find('`')? + 1;
    let len = s[start..].find('`')?;
    Some(&s[start..start + len])
}

fn last_quoted(s: &str) -> Option<&str> {
    let end = s.rfind('`')?;
    let start = s[..end].rfind('`')? + 1;
    Some(&s[start..end])
}
