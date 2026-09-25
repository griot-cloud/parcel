//! `parcel`: author, check and compile data contracts.
//!
//! parcel compiles a contract into artifacts; an engine such as peQL runs them. This command
//! checks a contract against sample data in memory and writes the portable artifacts. It
//! stores nothing and serves no queries.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use clap::{Args, Parser, Subcommand};
use datafusion::arrow::array::RecordBatch;
use datafusion::arrow::datatypes::{Schema, SchemaRef};
use datafusion::datasource::MemTable;
use datafusion::prelude::{CsvReadOptions, ParquetReadOptions, SessionContext};
use parcel_core::compile::{ReportEntry, Tier};
use parcel_core::registry::FunctionManifest;
use parcel_core::{Compilation, ContractDoc, Registry};
use parcel_runtime::Caller;
use parcel_runtime::bundle::{Bundle, BundledFunction};
use parcel_runtime::differential::differential;
use parcel_runtime::plan::{refusal, selectivity, validate};
use serde_json::json;

#[derive(Parser)]
#[command(
    name = "parcel",
    version,
    about = "Data contracts in CEL, compiled into query plans"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Compile a contract against a data schema and print how each rule will execute.
    Compile {
        contract: PathBuf,
        /// A Parquet or CSV file whose schema the contract binds.
        #[arg(long)]
        schema: PathBuf,
        /// Write the compiled bundle (JSON) here: what an engine loads.
        #[arg(long, short)]
        out: Option<PathBuf>,
        /// Print the compiled artifacts as JSON instead of the report.
        #[arg(long)]
        json: bool,
        /// Print the validation plan as SQL in this dialect instead of the report
        /// (datafusion, duckdb, postgres, mysql, sqlite, bigquery, snowflake).
        #[arg(long, value_name = "DIALECT")]
        sql: Option<String>,
        /// The table the exported SQL reads.
        #[arg(long, default_value = "contract_data")]
        table: String,
        /// Write the validation plan as a Substrait plan (protobuf) to this file.
        #[cfg(feature = "substrait")]
        #[arg(long, value_name = "FILE")]
        substrait: Option<PathBuf>,
        #[command(flatten)]
        functions: Functions,
        #[command(flatten)]
        types: TypeHints,
    },
    /// Check a contract against a sample of its data: the verdict, what each caller would
    /// see, and the differential test between the CEL interpreter and DataFusion.
    Check {
        contract: PathBuf,
        /// A Parquet or CSV file of sample data.
        #[arg(long)]
        data: PathBuf,
        /// Callers to evaluate decide, admit and transform rules for (YAML files).
        /// Defaults to one analyst.
        #[arg(long = "caller")]
        callers: Vec<PathBuf>,
        /// Rows used for the differential test.
        #[arg(long, default_value_t = 1000)]
        sample: usize,
        /// Print the result as JSON.
        #[arg(long)]
        json: bool,
        #[command(flatten)]
        functions: Functions,
        #[command(flatten)]
        types: TypeHints,
    },
    /// Print the JSON Schema of contract documents (for editors and CI).
    Schema,
    /// Import a contract from another standard.
    Import {
        #[command(subcommand)]
        from: ImportCommand,
    },
    /// Tenants' WebAssembly functions.
    Function {
        #[command(subcommand)]
        action: FunctionCommand,
    },
}

/// The contract owner's WebAssembly functions: `--function MODULE.wasm=MANIFEST.yaml`.
#[derive(Args, Clone, Default)]
struct Functions {
    #[arg(long = "function", value_name = "MODULE=MANIFEST")]
    functions: Vec<String>,
}

impl Functions {
    /// Load each function for `owner`; the registry a contract of that owner compiles against,
    /// and the functions to carry in a bundle.
    fn load(&self, owner: Option<&str>) -> Result<(Registry, Vec<BundledFunction>), String> {
        let mut registry = Registry::builtin();
        let mut bundled = Vec::new();
        for f in &self.functions {
            let (module, manifest) = f
                .split_once('=')
                .ok_or_else(|| format!("--function expects MODULE=MANIFEST, got `{f}`"))?;
            let owner = owner.ok_or("a contract that calls tenant functions needs an `owner`")?;
            let (bytes, m) = read_function(Path::new(module), Path::new(manifest))?;
            let entry = parcel_runtime::wasm::install(&bytes, &m, owner)?;
            registry.insert(entry);
            bundled.push(BundledFunction {
                owner: owner.to_owned(),
                manifest: m,
                module: hex_encode(&bytes),
            });
        }
        Ok((registry.visible_to(owner), bundled))
    }
}

fn read_function(module: &Path, manifest: &Path) -> Result<(Vec<u8>, FunctionManifest), String> {
    let bytes = std::fs::read(module).map_err(|e| format!("{}: {e}", module.display()))?;
    let text =
        std::fs::read_to_string(manifest).map_err(|e| format!("{}: {e}", manifest.display()))?;
    let m = yaml_serde::from_str(&text).map_err(|e| format!("{}: {e}", manifest.display()))?;
    Ok((bytes, m))
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Column types for CSV input, overriding inference: `--type msisdn=utf8`.
#[derive(Args, Clone, Default)]
struct TypeHints {
    #[arg(long = "type", value_name = "COLUMN=TYPE")]
    types: Vec<String>,
}

impl TypeHints {
    fn parse(&self) -> Result<Vec<(String, datafusion::arrow::datatypes::DataType)>, String> {
        self.types
            .iter()
            .map(|t| {
                let (c, ty) = t
                    .split_once('=')
                    .ok_or_else(|| format!("--type expects COLUMN=TYPE, got `{t}`"))?;
                Ok((
                    c.trim().to_owned(),
                    parcel_core::types::parse_type_name(ty)?,
                ))
            })
            .collect()
    }
}

#[derive(Subcommand)]
enum ImportCommand {
    /// An Open Data Contract Standard (ODCS v3) document.
    Odcs {
        file: PathBuf,
        /// The schema object to import when the document has several.
        #[arg(long)]
        object: Option<String>,
        /// Write the parcel contract here instead of printing it.
        #[arg(long, short)]
        out: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum FunctionCommand {
    /// Load a module the way an engine will: imports nothing, speaks the ABI, passes a smoke
    /// batch. Prints the hash contracts pin it by.
    Verify {
        /// The compiled module (.wasm), built with parcel_udf::export!.
        module: PathBuf,
        /// The function's manifest (YAML): name, version, signatures, deterministic, cost.
        #[arg(long)]
        manifest: PathBuf,
        /// The tenant whose contracts will call it.
        #[arg(long)]
        owner: String,
    },
}

fn load_caller(p: &Path) -> Result<Caller, String> {
    let text = std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))?;
    yaml_serde::from_str(&text).map_err(|e| format!("{}: {e}", p.display()))
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli.command).await {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(2)
        }
    }
}

type R = Result<ExitCode, String>;

async fn run(cmd: Command) -> R {
    match cmd {
        Command::Compile {
            contract,
            schema,
            out,
            json,
            sql,
            table,
            functions,
            types,
            #[cfg(feature = "substrait")]
            substrait,
        } => {
            let (schema, _) = read_data(&schema, &types).await?;
            let Some(loaded) = load_and_compile(&contract, &schema, &functions)? else {
                return Ok(ExitCode::from(1));
            };
            let c = &loaded.compilation;
            #[cfg(feature = "substrait")]
            if let Some(path) = &substrait {
                let (bytes, warnings) = parcel_runtime::export::validation_substrait(c, &table)?;
                std::fs::write(path, bytes).map_err(|e| e.to_string())?;
                for w in warnings {
                    eprintln!("warning: {w}");
                }
                eprintln!("wrote {}", path.display());
                return Ok(ExitCode::SUCCESS);
            }
            if let Some(dialect) = &sql {
                let export = parcel_runtime::export::validation_sql(c, dialect, &table)?;
                println!("{};", export.sql);
                for w in &export.warnings {
                    eprintln!("warning: {w}");
                }
            } else if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(c).map_err(|e| e.to_string())?
                );
            } else {
                print_report(
                    &c.contract.name,
                    c.contract.version,
                    &c.contract.compilation_hash,
                    &c.contract.report,
                );
            }
            if let Some(out) = out {
                let bundle = Bundle::with_functions(
                    &loaded.doc,
                    &loaded.ancestors,
                    &schema,
                    c,
                    loaded.functions,
                )
                .map_err(|e| e.to_string())?;
                std::fs::write(&out, bundle.to_json().map_err(|e| e.to_string())?)
                    .map_err(|e| e.to_string())?;
                eprintln!("wrote {}", out.display());
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Check {
            contract,
            data,
            callers,
            sample,
            json,
            functions,
            types,
        } => cmd_check(&contract, &data, &callers, sample, json, &functions, &types).await,
        Command::Import {
            from: ImportCommand::Odcs { file, object, out },
        } => {
            let text =
                std::fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?;
            let imported = parcel_core::odcs::import(&text, object.as_deref())?;
            let yaml = format!(
                "# Imported from {} (ODCS). Review the binding, then check it against data:\n#   parcel check <this file> --data <sample>\n{}",
                file.display(),
                yaml_serde::to_string(&imported.doc).map_err(|e| e.to_string())?
            );
            match out {
                Some(p) => std::fs::write(&p, yaml).map_err(|e| e.to_string())?,
                None => print!("{yaml}"),
            }
            for n in &imported.notes {
                eprintln!("note: {n}");
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Function {
            action:
                FunctionCommand::Verify {
                    module,
                    manifest,
                    owner,
                },
        } => {
            let (bytes, m) = read_function(&module, &manifest)?;
            let entry = parcel_runtime::wasm::install(&bytes, &m, &owner)?;
            let sigs: Vec<String> = entry
                .signatures
                .iter()
                .map(|s| {
                    let args: Vec<String> = s.args.iter().map(|t| t.to_string()).collect();
                    format!("({}) -> {}", args.join(", "), s.ret)
                })
                .collect();
            println!(
                "{} v{} for {owner}: verified\n  signatures {}\n  pinned as  {}",
                entry.name,
                entry.version,
                sigs.join(", "),
                entry.hash
            );
            Ok(ExitCode::SUCCESS)
        }
        Command::Schema => {
            let schema = parcel_core::document::json_schema();
            println!(
                "{}",
                serde_json::to_string_pretty(&schema).map_err(|e| e.to_string())?
            );
            Ok(ExitCode::SUCCESS)
        }
    }
}

/// A compiled contract with what produced it.
struct Loaded {
    doc: ContractDoc,
    ancestors: Vec<ContractDoc>,
    functions: Vec<BundledFunction>,
    compilation: Compilation,
}

/// Parse, load the owner's functions, and compile against `schema`. Parents named by
/// `inherits` are found among the documents next to the contract. `None` after printing
/// diagnostics.
fn load_and_compile(
    contract: &Path,
    schema: &Schema,
    functions: &Functions,
) -> Result<Option<Loaded>, String> {
    let source =
        std::fs::read_to_string(contract).map_err(|e| format!("{}: {e}", contract.display()))?;
    let doc = ContractDoc::parse(&source).map_err(|d| d.to_string())?;
    let docs = sibling_documents(contract);
    let (registry, bundled) = functions.load(doc.owner.as_deref())?;
    let compilation =
        match parcel_core::compile_with(&doc, schema, &registry, &|n| docs.get(n).cloned()) {
            Ok(c) => c,
            Err(diags) => {
                for d in diags {
                    eprintln!("{d}");
                }
                return Ok(None);
            }
        };
    let mut ancestors = Vec::new();
    let mut next = doc.inherits.clone();
    while let Some(p) = next.and_then(|n| docs.get(&n).cloned()) {
        next = p.inherits.clone();
        ancestors.push(p);
    }
    Ok(Some(Loaded {
        doc,
        ancestors,
        functions: bundled,
        compilation,
    }))
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

/// Read a Parquet or CSV file (or a directory of Parquet files) into batches.
async fn read_data(
    path: &Path,
    hints: &TypeHints,
) -> Result<(SchemaRef, Vec<RecordBatch>), String> {
    let ctx = SessionContext::new();
    let p = path.to_str().ok_or("path is not UTF-8")?;
    let hints = hints.parse()?;
    let df = if path.extension().is_some_and(|e| e == "csv") {
        let inferred = ctx
            .read_csv(p, CsvReadOptions::new())
            .await
            .map_err(|e| format!("{}: {e}", path.display()))?;
        let fields: Vec<datafusion::arrow::datatypes::Field> = inferred
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
        ctx.read_parquet(p, ParquetReadOptions::default()).await
    }
    .map_err(|e| format!("{}: {e}", path.display()))?;
    let schema: SchemaRef = std::sync::Arc::new(df.schema().as_arrow().clone());
    let batches = df.collect().await.map_err(|e| e.to_string())?;
    Ok((schema, batches))
}

fn print_report(name: &str, version: u32, hash: &str, report: &[ReportEntry]) {
    println!("{name} v{version}  compilation {}", &hash[..16]);
    println!();
    println!(
        "  {:<24} {:<10} {:<12} how it executes",
        "rule", "op", "tier"
    );
    for r in report {
        let tier = match r.tier {
            Tier::PlanTime => "plan time",
            Tier::Prunes => "prunes",
            Tier::ScanFilter => "scan filter",
            Tier::WriteTime => "write time",
            Tier::Projection => "projection",
            Tier::Operator => "operator",
        };
        println!("  {:<24} {:<10} {:<12} {}", r.rule, r.op, tier, r.reason);
    }
}

async fn cmd_check(
    contract: &Path,
    data: &Path,
    callers: &[PathBuf],
    sample: usize,
    json: bool,
    functions: &Functions,
    types: &TypeHints,
) -> R {
    let (schema, batches) = read_data(data, types).await?;
    let Some(loaded) = load_and_compile(contract, &schema, functions)? else {
        return Ok(ExitCode::from(1));
    };
    let c = &loaded.compilation;
    let cc = &c.contract;
    let table = Arc::new(
        MemTable::try_new(schema.clone(), vec![batches.clone()]).map_err(|e| e.to_string())?,
    );

    let v = validate(c, c.validation.plan.clone(), table.clone())
        .await
        .map_err(|e| e.to_string())?;

    let callers: Vec<Caller> = if callers.is_empty() {
        vec![Caller::new("check", "", "analytics")]
    } else {
        callers
            .iter()
            .map(|p| load_caller(p))
            .collect::<Result<_, _>>()?
    };
    // What each caller would see: refused by a decision, or the rows its admits let through.
    let mut seen = Vec::new();
    for caller in &callers {
        let who = format!("{}/{}", caller.tenant, caller.id);
        let refused = refusal(cc, caller).map_err(|e| e.to_string())?;
        let rows = match &refused {
            Some(_) => None,
            None => Some(
                selectivity(c, table.clone(), caller)
                    .await
                    .map_err(|e| e.to_string())?,
            ),
        };
        seen.push((who, refused, rows));
    }

    let mut rows = Vec::new();
    let mut left = sample;
    for b in &batches {
        if left == 0 {
            break;
        }
        let take = left.min(b.num_rows());
        rows.push(b.slice(0, take));
        left -= take;
    }
    let sample_batch =
        datafusion::arrow::compute::concat_batches(&schema, &rows).map_err(|e| e.to_string())?;
    let diff = differential(c, &sample_batch, &callers)
        .await
        .map_err(|e| e.to_string())?;
    let ok = v.valid && diff.passed();

    if json {
        let out = json!({
            "contract": cc.name,
            "version": cc.version,
            "compilation_hash": cc.compilation_hash,
            "report": cc.report,
            "verdict": v,
            "query_time_guarantees": c.validation.query_time_guarantees,
            "callers": seen.iter().map(|(who, refused, rows)| json!({
                "caller": who, "refused_by": refused, "rows": rows,
            })).collect::<Vec<_>>(),
            "differential": diff,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&out).map_err(|e| e.to_string())?
        );
        return Ok(if ok {
            ExitCode::SUCCESS
        } else {
            ExitCode::from(1)
        });
    }

    print_report(&cc.name, cc.version, &cc.compilation_hash, &cc.report);
    println!();
    println!(
        "verdict over {} rows: {}",
        v.row_count,
        if v.valid {
            "VALID".to_owned()
        } else {
            format!("INVALID ({})", v.breached.join(", "))
        }
    );
    for (id, fails) in &v.failures {
        let rate = if v.row_count > 0 {
            100.0 * (1.0 - *fails as f64 / v.row_count as f64)
        } else {
            100.0
        };
        println!("  assert    {id:<24} {rate:>7.2}% pass  ({fails} failing)");
    }
    for (id, ok) in &v.guarantees {
        println!(
            "  guarantee {id:<24} {}",
            if *ok { "holds" } else { "FAILS" }
        );
    }
    for g in &c.validation.query_time_guarantees {
        println!("  guarantee {g:<24} decided per query (reads write time or ctx.now)");
    }
    println!();
    for (who, refused, rows) in &seen {
        match (refused, rows) {
            (Some(rule), _) => println!("  caller {who:<20} refused by `{rule}`"),
            (None, Some(n)) => println!("  caller {who:<20} sees {n} of {} rows", v.row_count),
            (None, None) => {}
        }
    }
    println!();
    println!(
        "differential test: {} evaluations over {} rows and {} caller(s): {}",
        diff.evaluations,
        diff.rows,
        diff.callers,
        if diff.passed() {
            "interpreter and DataFusion agree".to_owned()
        } else {
            format!("{} MISMATCHES", diff.mismatches.len())
        }
    );
    for m in diff.mismatches.iter().take(10) {
        println!(
            "  {} row {} ({}): cel {} vs datafusion {}",
            m.rule, m.row, m.caller, m.reference, m.datafusion
        );
    }
    Ok(if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    })
}
