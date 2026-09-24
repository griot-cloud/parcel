//! `parcel`: author, check, compile, write, validate and query data contracts.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use chrono::{DateTime, Utc};
use clap::{Args, Parser, Subcommand};
use datafusion::arrow::array::RecordBatch;
use datafusion::arrow::datatypes::{Schema, SchemaRef};
use datafusion::arrow::util::pretty::pretty_format_batches;
use datafusion::prelude::{CsvReadOptions, ParquetReadOptions, SessionContext};
use parcel_core::compile::{ReportEntry, Tier};
use parcel_core::{ContractDoc, Registry, compile};
use parcel_engine::differential::differential;
use parcel_engine::{Caller, Engine, EngineError, WriteMode};

#[derive(Parser)]
#[command(
    name = "parcel",
    version,
    about = "Data contracts in CEL, compiled for DataFusion"
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
        /// Write the compiled bundle (JSON) here.
        #[arg(long, short)]
        out: Option<PathBuf>,
        /// Print the compiled artifacts as JSON instead of the report.
        #[arg(long)]
        json: bool,
        #[command(flatten)]
        types: TypeHints,
    },
    /// Check a contract against a sample of its data: pass rates, selectivity, verdict,
    /// and the differential test between the CEL interpreter and DataFusion.
    Check {
        contract: PathBuf,
        /// A Parquet or CSV file of sample data.
        #[arg(long)]
        data: PathBuf,
        /// Callers to evaluate admit and transform rules for (YAML files). Defaults to one analyst.
        #[arg(long = "caller")]
        callers: Vec<PathBuf>,
        /// Rows used for the differential test.
        #[arg(long, default_value_t = 1000)]
        sample: usize,
        #[command(flatten)]
        types: TypeHints,
    },
    /// Write data under a contract: flags, layout, manifest and verdict.
    Write {
        /// Contract file (registered on first write).
        contract: PathBuf,
        /// A Parquet or CSV file.
        #[arg(long)]
        input: PathBuf,
        #[command(flatten)]
        ws: Workspace,
        /// Add to existing data instead of replacing it.
        #[arg(long)]
        append: bool,
        #[command(flatten)]
        types: TypeHints,
    },
    /// Run a contract's validation plan over its data and print the verdict.
    Validate {
        /// Contract name, e.g. sales/orders.
        name: String,
        #[command(flatten)]
        ws: Workspace,
    },
    /// Run SQL in which every table is a contract.
    Query {
        sql: String,
        #[command(flatten)]
        ws: Workspace,
        #[command(flatten)]
        caller: CallerArgs,
        /// Also print the result envelope.
        #[arg(long)]
        envelope: bool,
        /// Print the physical plan instead of running the query.
        #[arg(long)]
        explain: bool,
    },
    /// Show the schema a caller would see.
    Describe {
        name: String,
        #[command(flatten)]
        ws: Workspace,
        #[command(flatten)]
        caller: CallerArgs,
    },
    /// List the contracts in a workspace and the state of their data.
    List {
        #[command(flatten)]
        ws: Workspace,
    },
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

#[derive(Args)]
struct Workspace {
    /// Workspace root: contracts live in <root>/contracts, relative bindings resolve under it.
    #[arg(long, default_value = ".")]
    root: PathBuf,
}

#[derive(Args)]
struct CallerArgs {
    /// A caller YAML file (id, tenant, purpose, tier, clearance, roles, now).
    #[arg(long)]
    caller: Option<PathBuf>,
    #[arg(long)]
    tenant: Option<String>,
    #[arg(long)]
    purpose: Option<String>,
    #[arg(long)]
    id: Option<String>,
    #[arg(long = "role")]
    roles: Vec<String>,
    #[arg(long)]
    clearance: Option<i64>,
    #[arg(long)]
    tier: Option<String>,
    /// Query time, RFC 3339. Defaults to now.
    #[arg(long)]
    now: Option<DateTime<Utc>>,
}

impl CallerArgs {
    fn resolve(&self) -> Result<Caller, String> {
        let mut c = match &self.caller {
            Some(p) => load_caller(p)?,
            None => Caller::new("cli", "", ""),
        };
        if let Some(t) = &self.tenant {
            c.tenant = t.clone();
        }
        if let Some(p) = &self.purpose {
            c.purpose = p.clone();
        }
        if let Some(i) = &self.id {
            c.id = i.clone();
        }
        if !self.roles.is_empty() {
            c.roles = self.roles.clone();
        }
        if let Some(x) = self.clearance {
            c.clearance = x;
        }
        if let Some(t) = &self.tier {
            c.tier = t.clone();
        }
        if let Some(n) = self.now {
            c.now = n;
        }
        Ok(c)
    }
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
            types,
        } => cmd_compile(&contract, &schema, out.as_deref(), json, &types).await,
        Command::Check {
            contract,
            data,
            callers,
            sample,
            types,
        } => cmd_check(&contract, &data, &callers, sample, &types).await,
        Command::Write {
            contract,
            input,
            ws,
            append,
            types,
        } => cmd_write(&contract, &input, &ws.root, append, &types).await,
        Command::Validate { name, ws } => {
            let engine = open(&ws.root)?;
            let v = engine.validate(&name).await.map_err(|e| e.to_string())?;
            println!(
                "{}",
                serde_json::to_string_pretty(&v).map_err(|e| e.to_string())?
            );
            Ok(if v.valid {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            })
        }
        Command::Query {
            sql,
            ws,
            caller,
            envelope,
            explain,
        } => {
            let engine = open(&ws.root)?;
            let caller = caller.resolve()?;
            if explain {
                println!(
                    "{}",
                    engine
                        .explain(&sql, &caller)
                        .await
                        .map_err(|e| e.to_string())?
                );
                return Ok(ExitCode::SUCCESS);
            }
            match engine.query(&sql, &caller).await {
                Ok(res) => {
                    println!(
                        "{}",
                        pretty_format_batches(&res.batches).map_err(|e| e.to_string())?
                    );
                    if envelope {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&res.envelope)
                                .map_err(|e| e.to_string())?
                        );
                    } else {
                        for c in &res.envelope.contracts {
                            if !c.annotations.is_empty() {
                                eprintln!(
                                    "note: `{}` guarantees not met: {}",
                                    c.contract,
                                    c.annotations.join(", ")
                                );
                            }
                        }
                    }
                    Ok(ExitCode::SUCCESS)
                }
                Err(
                    e @ (EngineError::Denied { .. }
                    | EngineError::GuaranteeFailed { .. }
                    | EngineError::NotServable { .. }),
                ) => {
                    eprintln!("refused: {e}");
                    Ok(ExitCode::from(1))
                }
                Err(e) => Err(e.to_string()),
            }
        }
        Command::Describe { name, ws, caller } => {
            let engine = open(&ws.root)?;
            let schema = engine
                .describe(&name, &caller.resolve()?)
                .map_err(|e| e.to_string())?;
            for f in schema.fields() {
                println!("{:<24} {}", f.name(), f.data_type());
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::List { ws } => {
            let (engine, pending) = Engine::open(&ws.root).map_err(|e| e.to_string())?;
            for r in engine.contracts() {
                let cc = &r.compilation.contract;
                let m = engine.manifest(&cc.name).map_err(|e| e.to_string())?;
                let state = match m {
                    Some(m) if m.valid => format!(
                        "{} rows, valid, written {}",
                        m.row_count,
                        m.written_at.format("%Y-%m-%d %H:%M")
                    ),
                    Some(m) => format!(
                        "{} rows, NOT SERVABLE ({})",
                        m.row_count,
                        m.breached.join(", ")
                    ),
                    None => "no data".into(),
                };
                println!("{:<28} v{:<4} {state}", cc.name, cc.version);
            }
            for (p, _) in pending {
                println!("{:<28} {:<5} no data yet ({})", "-", "", p.display());
            }
            Ok(ExitCode::SUCCESS)
        }
    }
}

fn open(root: &Path) -> Result<Engine, String> {
    Engine::open(root)
        .map(|(e, _)| e)
        .map_err(|e| e.to_string())
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

/// The data's schema without any parcel flag columns, which the contract did not bind.
fn row_schema(schema: &Schema) -> Schema {
    Schema::new(
        schema
            .fields()
            .iter()
            .filter(|f| !f.name().starts_with("_c_"))
            .cloned()
            .collect::<Vec<_>>(),
    )
}

async fn cmd_compile(
    contract: &Path,
    schema_from: &Path,
    out: Option<&Path>,
    json: bool,
    types: &TypeHints,
) -> R {
    let source =
        std::fs::read_to_string(contract).map_err(|e| format!("{}: {e}", contract.display()))?;
    let (schema, _) = read_schema_only(schema_from, types).await?;
    let doc = ContractDoc::parse(&source).map_err(|d| d.to_string())?;
    let c = match compile(&doc, &schema, &Registry::builtin()) {
        Ok(c) => c,
        Err(diags) => {
            for d in diags {
                eprintln!("{d}");
            }
            return Ok(ExitCode::from(1));
        }
    };
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&c).map_err(|e| e.to_string())?
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
        let bundle =
            parcel_engine::bundle::Bundle::new(&doc, &schema, &c).map_err(|e| e.to_string())?;
        std::fs::write(out, bundle.to_json().map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        eprintln!("wrote {}", out.display());
    }
    Ok(ExitCode::SUCCESS)
}

async fn read_schema_only(path: &Path, types: &TypeHints) -> Result<(Schema, ()), String> {
    let (schema, _) = read_data(path, types).await?;
    Ok((row_schema(&schema), ()))
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
    types: &TypeHints,
) -> R {
    let source =
        std::fs::read_to_string(contract).map_err(|e| format!("{}: {e}", contract.display()))?;
    let (schema, batches) = read_data(data, types).await?;
    let schema = row_schema(&schema);
    let doc = ContractDoc::parse(&source).map_err(|d| d.to_string())?;
    let c = match compile(&doc, &schema, &Registry::builtin()) {
        Ok(c) => c,
        Err(diags) => {
            for d in diags {
                eprintln!("{d}");
            }
            return Ok(ExitCode::from(1));
        }
    };
    print_report(
        &c.contract.name,
        c.contract.version,
        &c.contract.compilation_hash,
        &c.contract.report,
    );

    // Validate the sample in a scratch workspace with the binding pointed at it.
    let scratch = std::env::temp_dir().join(format!("parcel-check-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    let mut engine = Engine::new(&scratch);
    let mut local = doc.clone();
    local.binding.parquet = "data/".into();
    let local_src = yaml_serde::to_string(&local).map_err(|e| e.to_string())?;
    engine
        .register_contract(&local_src, &schema)
        .map_err(|e| e.to_string())?;
    let report = engine
        .write(&doc.contract, batches.clone(), WriteMode::Overwrite)
        .await
        .map_err(|e| e.to_string())?;
    let v = &report.verdict;
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
    let comp = &engine
        .get(&doc.contract)
        .map_err(|e| e.to_string())?
        .compilation;
    for g in &c.validation.query_time_guarantees {
        println!("  guarantee {g:<24} decided per query (reads write time or ctx.now)");
    }

    let callers: Vec<Caller> = if callers.is_empty() {
        vec![Caller::new("check", "", "analytics")]
    } else {
        callers
            .iter()
            .map(|p| load_caller(p))
            .collect::<Result<_, _>>()?
    };
    // Selectivity of each admit, per caller, over the whole sample.
    if !c.contract.admits.is_empty() {
        println!();
        for caller in &callers {
            match engine
                .query(
                    &format!("SELECT COUNT(*) AS n FROM \"{}\"", doc.contract),
                    caller,
                )
                .await
            {
                Ok(res) => {
                    let n = res
                        .batches
                        .first()
                        .map(|b| {
                            datafusion::arrow::util::display::array_value_to_string(b.column(0), 0)
                                .unwrap_or_default()
                        })
                        .unwrap_or_default();
                    println!(
                        "  caller {:<20} sees {n} of {} rows",
                        format!("{}/{}", caller.tenant, caller.id),
                        v.row_count
                    );
                }
                Err(e) => println!(
                    "  caller {:<20} {e}",
                    format!("{}/{}", caller.tenant, caller.id)
                ),
            }
        }
    }

    // Differential test.
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
    let sample_batch = datafusion::arrow::compute::concat_batches(&batches[0].schema(), &rows)
        .map_err(|e| e.to_string())?;
    let diff = differential(comp, &sample_batch, &callers)
        .await
        .map_err(|e| e.to_string())?;
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
    let _ = std::fs::remove_dir_all(&scratch);
    Ok(if v.valid && diff.passed() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    })
}

async fn cmd_write(
    contract: &Path,
    input: &Path,
    root: &Path,
    append: bool,
    types: &TypeHints,
) -> R {
    let source =
        std::fs::read_to_string(contract).map_err(|e| format!("{}: {e}", contract.display()))?;
    let doc = ContractDoc::parse(&source).map_err(|d| d.to_string())?;
    let (mut engine, _) = Engine::open(root).map_err(|e| e.to_string())?;
    let (schema, batches) = read_data(input, types).await?;
    let schema = row_schema(&schema);
    // (Re)register from the file given, so an edited contract takes effect on this write.
    engine
        .register_contract(&source, &schema)
        .map_err(|e| e.to_string())?;
    let mode = if append {
        WriteMode::Append
    } else {
        WriteMode::Overwrite
    };
    let report = engine
        .write(&doc.contract, batches, mode)
        .await
        .map_err(|e| e.to_string())?;
    let contracts_dir = root.join("contracts");
    let target = contracts_dir.join(format!("{}.yaml", doc.contract.replace('/', "__")));
    if !contract.starts_with(&contracts_dir) {
        std::fs::create_dir_all(&contracts_dir).map_err(|e| e.to_string())?;
        std::fs::write(&target, &source).map_err(|e| e.to_string())?;
    }
    let v = &report.verdict;
    println!(
        "wrote {} rows to {} ({} files): {}",
        report.rows_written,
        doc.contract,
        report.files,
        if v.valid {
            "valid".to_owned()
        } else {
            format!("NOT SERVABLE ({})", v.breached.join(", "))
        }
    );
    for (id, fails) in &v.failures {
        if *fails > 0 {
            println!("  {id}: {fails} rows fail");
        }
    }
    Ok(if v.valid {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    })
}
