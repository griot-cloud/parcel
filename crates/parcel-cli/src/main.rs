//! `parcel`: author, check and compile data contracts.
//!
//! parcel compiles a contract into artifacts; an engine such as peQL runs them. This command
//! checks a contract against sample data in memory and writes the portable artifacts. It
//! stores nothing and serves no queries.

mod check;
mod progress;
mod source;
mod ui;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use datafusion::arrow::datatypes::SchemaRef;
use parcel_core::compile::ReportEntry;
use parcel_core::document::Source;
use parcel_core::lint::{Lint, Severity};
use parcel_core::registry::FunctionManifest;
use parcel_core::{Compilation, Diagnostic, Registry};
use parcel_runtime::Caller;
use parcel_runtime::compiled::CompiledBytes;
use serde_json::json;

use source::{Contract, TypesFrom};
use ui::{BOLD, CYAN, DIM, GREEN, OK, Out, Place, RED, WARN, YELLOW, paint};

#[derive(Parser)]
#[command(
    name = "parcel",
    version,
    about = "Data contract compiler",
    arg_required_else_help = true
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Check a contract compiles. No data needed.
    Validate {
        contract: PathBuf,
        #[command(flatten)]
        schema: SchemaArg,
        /// Print the result as JSON.
        #[arg(long)]
        json: bool,
        #[command(flatten)]
        functions: Functions,
        #[command(flatten)]
        types: TypeHints,
    },
    /// Validate a contract and report likely mistakes.
    Lint {
        contract: PathBuf,
        #[command(flatten)]
        schema: SchemaArg,
        /// Exit 1 on warnings.
        #[arg(long)]
        deny_warnings: bool,
        /// Print the result as JSON.
        #[arg(long)]
        json: bool,
        #[command(flatten)]
        functions: Functions,
        #[command(flatten)]
        types: TypeHints,
    },
    /// Compile a contract.
    Compile {
        contract: PathBuf,
        #[command(flatten)]
        schema: SchemaArg,
        /// Write the compiled contract to this file.
        #[arg(long, short)]
        out: Option<PathBuf>,
        /// Print the compilation as JSON.
        #[arg(long)]
        json: bool,
        /// Print validation SQL: datafusion, duckdb, postgres, mysql, sqlite, bigquery, snowflake.
        #[arg(long, value_name = "DIALECT")]
        sql: Option<String>,
        /// The table the exported SQL reads.
        #[arg(long, default_value = "contract_data")]
        table: String,
        /// Write the validation plan as Substrait to this file.
        #[cfg(feature = "substrait")]
        #[arg(long, value_name = "FILE")]
        substrait: Option<PathBuf>,
        #[command(flatten)]
        functions: Functions,
        #[command(flatten)]
        types: TypeHints,
    },
    /// Check a contract against sample data and callers.
    Check {
        contract: PathBuf,
        /// Sample data: CSV, Parquet, or a Parquet directory.
        #[arg(long)]
        data: PathBuf,
        /// Caller file (YAML or JSON). Repeatable.
        #[arg(long = "caller")]
        callers: Vec<PathBuf>,
        /// Rows for the differential test.
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
    /// Print the contract JSON Schema.
    Schema,
    /// Convert a contract from another standard.
    Import {
        #[command(subcommand)]
        from: ImportCommand,
    },
    /// Custom WebAssembly functions.
    Function {
        #[command(subcommand)]
        action: FunctionCommand,
    },
}

/// Where column types come from: `--schema FILE`, else the binding's data, else `expose`.
#[derive(Args, Clone, Default)]
struct SchemaArg {
    /// Column types from this CSV or Parquet file. Default: the binding's local Parquet
    /// data, then `expose`.
    #[arg(long, value_name = "FILE")]
    schema: Option<PathBuf>,
}

/// The contract owner's WebAssembly functions: `--function MODULE.wasm=MANIFEST.yaml`.
#[derive(Args, Clone, Default)]
pub(crate) struct Functions {
    #[arg(long = "function", value_name = "MODULE=MANIFEST")]
    functions: Vec<String>,
}

impl Functions {
    /// Load each function for `owner`: the registry a contract of that owner compiles against.
    fn load(&self, owner: Option<&str>) -> Result<Registry, String> {
        let mut registry = Registry::builtin();
        for f in &self.functions {
            let (module, manifest) = f
                .split_once('=')
                .ok_or_else(|| format!("--function expects MODULE=MANIFEST, got `{f}`"))?;
            let owner = owner.ok_or("--function needs `owner:` in the contract")?;
            let (bytes, m) = read_function(Path::new(module), Path::new(manifest))?;
            let entry = parcel_runtime::wasm::install(&bytes, &m, owner)?;
            registry.insert(entry);
        }
        Ok(registry.visible_to(owner))
    }
}

fn read_function(module: &Path, manifest: &Path) -> Result<(Vec<u8>, FunctionManifest), String> {
    let bytes = std::fs::read(module).map_err(|e| format!("{}: {e}", module.display()))?;
    let text =
        std::fs::read_to_string(manifest).map_err(|e| format!("{}: {e}", manifest.display()))?;
    let m = yaml_serde::from_str(&text).map_err(|e| format!("{}: {e}", manifest.display()))?;
    Ok((bytes, m))
}

/// Column types for CSV input, overriding inference: `--type msisdn=utf8`.
#[derive(Args, Clone, Default)]
pub struct TypeHints {
    /// CSV column type, e.g. `--type zip=utf8`. Repeatable.
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
                let parsed = parcel_core::types::parse_type_name(ty).map_err(|e| {
                    match parcel_core::types::suggest_type_name(ty) {
                        Some(s) => format!("--type {t}: {e}; did you mean `{s}`?"),
                        None => format!(
                            "--type {t}: {e}; common types: {}",
                            parcel_core::types::COMMON_TYPE_NAMES
                        ),
                    }
                })?;
                Ok((c.trim().to_owned(), parsed))
            })
            .collect()
    }
}

#[derive(Subcommand)]
enum ImportCommand {
    /// Convert an ODCS v3 contract.
    Odcs {
        file: PathBuf,
        /// Schema object to import.
        #[arg(long)]
        object: Option<String>,
        /// Output file. Default: stdout.
        #[arg(long, short)]
        out: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum FunctionCommand {
    /// Verify a WebAssembly function and print its pin.
    Verify {
        /// The .wasm module.
        module: PathBuf,
        /// The manifest (YAML).
        #[arg(long)]
        manifest: PathBuf,
        /// Owner whose contracts call it.
        #[arg(long)]
        owner: String,
    },
}

pub(crate) fn load_caller(p: &Path) -> Result<Caller, String> {
    let text = std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))?;
    yaml_serde::from_str(&text).map_err(|e| {
        format!(
            "{}: {e}\n  help: required: id, tenant, purpose; optional: tier, clearance, \
             classification, roles, now, other",
            p.display()
        )
    })
}

#[tokio::main]
async fn main() -> ExitCode {
    // Rust ignores SIGPIPE, so a closed stdout (`parcel ... | head -1`) makes every print
    // panic. Restore the default: stop quietly, as other command-line tools do.
    #[cfg(unix)]
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
    let cli = Cli::parse();
    match run(cli.command).await {
        Ok(code) => code,
        Err(e) => {
            anstream::eprintln!("{} {e}", paint(RED, "error:"));
            ExitCode::from(2)
        }
    }
}

pub(crate) type R = Result<ExitCode, String>;

pub(crate) fn status(ok: bool) -> ExitCode {
    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

async fn run(cmd: Command) -> R {
    match cmd {
        Command::Validate {
            contract,
            schema,
            json,
            functions,
            types,
        } => cmd_validate(&contract, &schema, json, None, &functions, &types).await,
        Command::Lint {
            contract,
            schema,
            deny_warnings,
            json,
            functions,
            types,
        } => {
            cmd_validate(
                &contract,
                &schema,
                json,
                Some(deny_warnings),
                &functions,
                &types,
            )
            .await
        }
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
            let Some(ready) = ready(
                &contract,
                schema.schema.as_deref(),
                &functions,
                &types,
                false,
            )
            .await?
            else {
                return Ok(ExitCode::from(1));
            };
            let c = &ready.compiled;
            #[cfg(feature = "substrait")]
            if let Some(path) = &substrait {
                let (bytes, warnings) = parcel_runtime::export::validation_substrait(c, &table)?;
                std::fs::write(path, bytes).map_err(|e| e.to_string())?;
                for w in warnings {
                    anstream::eprintln!("{} {w}", paint(YELLOW, "warning:"));
                }
                anstream::eprintln!("{} wrote {}", paint(GREEN, OK), path.display());
                return Ok(ExitCode::SUCCESS);
            }
            if let Some(dialect) = &sql {
                let export = parcel_runtime::export::validation_sql(c, dialect, &table)?;
                println!("{};", export.sql);
                for w in &export.warnings {
                    anstream::eprintln!("{} {w}", paint(YELLOW, "warning:"));
                }
            } else if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(c).map_err(|e| e.to_string())?
                );
            } else {
                let mut o = Out::default();
                report_header(&mut o, c, &ready.from);
                ui::rules(&mut o, &c.contract.report);
                o.print();
            }
            if let Some(out) = out {
                std::fs::write(&out, c.to_bytes().map_err(|e| e.to_string())?)
                    .map_err(|e| e.to_string())?;
                anstream::eprintln!(
                    "{} wrote {}  {}",
                    paint(GREEN, OK),
                    out.display(),
                    paint(DIM, size(&out))
                );
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
        } => check::run(&contract, &data, &callers, sample, json, &functions, &types).await,
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
                anstream::eprintln!("{} {n}", paint(CYAN, "note:"));
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
            let mut o = Out::default();
            o.line(format!(
                "{} {}",
                paint(GREEN, OK),
                paint(BOLD, format!("{} v{}", entry.name, entry.version))
            ));
            ui::field(&mut o, "owner", &owner);
            ui::field(&mut o, "takes", sigs.join(", "));
            ui::field(&mut o, "pinned", &entry.hash);
            o.print();
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

/// A contract compiled against its column types, ready to report on or run.
pub(crate) struct Ready {
    pub contract: Contract,
    pub schema: SchemaRef,
    pub from: TypesFrom,
    pub registry: Registry,
    pub compiled: Compilation,
}

/// Read, type and compile a contract. `None` after printing its problems, to stderr, or to
/// stdout when `to_stdout`.
pub(crate) async fn ready(
    path: &Path,
    schema: Option<&Path>,
    functions: &Functions,
    types: &TypeHints,
    to_stdout: bool,
) -> Result<Option<Ready>, String> {
    let emit = |o: Out| if to_stdout { o.print() } else { o.eprint() };
    let file = path.display().to_string();
    let contract = match Contract::read(path)? {
        Ok(c) => c,
        Err((source, diags)) => {
            emit(problems(&file, &source, None, &diags));
            return Ok(None);
        }
    };
    let (schema, from) = source::schema_for(&contract, schema, types).await?;
    let registry = functions.load(contract.doc.owner.as_deref())?;
    match contract.compile(&schema, &registry) {
        Ok(compiled) => Ok(Some(Ready {
            contract,
            schema,
            from,
            registry,
            compiled,
        })),
        Err(mut diags) => {
            source::locate(&mut diags, &contract.source, Some(&from));
            emit(problems(&file, &contract.source, Some(&from), &diags));
            Ok(None)
        }
    }
}

/// Every problem, then a closing line saying how many and where the types came from.
fn problems(file: &str, source: &str, from: Option<&TypesFrom>, diags: &[Diagnostic]) -> Out {
    let mut o = Out::default();
    let place = Place {
        file,
        source: Some(source),
    };
    ui::diagnostics(&mut o, &place, diags);
    ui::verdict(
        &mut o,
        false,
        format!("{file}  {}", ui::plural(diags.len(), "problem", "problems")),
    );
    if let Some(f) = from {
        ui::field(&mut o, "types", f);
    }
    o
}

/// `validate`, or `lint` when `lint` is set (with whether warnings fail the command).
async fn cmd_validate(
    path: &Path,
    schema: &SchemaArg,
    json: bool,
    lint: Option<bool>,
    functions: &Functions,
    types: &TypeHints,
) -> R {
    if json {
        return validate_json(path, schema, lint.is_some(), functions, types).await;
    }
    let Some(ready) = ready(path, schema.schema.as_deref(), functions, types, true).await? else {
        return Ok(ExitCode::from(1));
    };
    let file = path.display().to_string();
    let cc = &ready.compiled.contract;
    let mut o = Out::default();

    let Some(deny_warnings) = lint else {
        o.line(format!(
            "{} {}",
            paint(GREEN, OK),
            paint(BOLD, format!("{} v{}", cc.name, cc.version))
        ));
        rule_summary(&mut o, &cc.report);
        ui::field(
            &mut o,
            "columns",
            format!(
                "{}/{}",
                cc.exposed_schema.fields().len(),
                ready.schema.fields().len()
            ),
        );
        ui::field(&mut o, "types", &ready.from);
        o.print();
        return Ok(ExitCode::SUCCESS);
    };

    let lints = lints_for(&ready);
    let place = Place {
        file: &file,
        source: Some(&ready.contract.source),
    };
    ui::lints(&mut o, &place, &lints);
    let warnings = lints
        .iter()
        .filter(|l| l.severity == Severity::Warning)
        .count();
    let notes = lints.len() - warnings;
    let summary = format!(
        "{file}  {}  {}",
        ui::plural(warnings, "warning", "warnings"),
        ui::plural(notes, "note", "notes")
    );
    o.blank();
    let mark = match (warnings, deny_warnings) {
        (0, _) => paint(GREEN, OK),
        (_, true) => paint(RED, ui::FAIL),
        (_, false) => paint(YELLOW, WARN),
    };
    o.line(format!("{mark} {}", paint(BOLD, summary)));
    ui::field(&mut o, "types", &ready.from);
    o.print();
    Ok(status(!(deny_warnings && warnings > 0)))
}

/// The lints for a compiled contract, located in its source, with a note when the binding
/// is not where it says.
fn lints_for(ready: &Ready) -> Vec<Lint> {
    let c = &ready.contract;
    let mut lints = match c.checked(&ready.schema, &ready.registry) {
        Ok(checked) => parcel_core::lint::lint(&c.doc, &checked),
        Err(_) => Vec::new(),
    };
    if let (TypesFrom::Expose { why }, Some(Source::Parquet(loc))) =
        (&ready.from, c.doc.binding.as_ref().map(|b| &b.source))
        && why.contains("not found")
    {
        lints.push(Lint {
            name: "binding-not-found",
            severity: Severity::Note,
            rule: None,
            message: format!("binding `{loc}` not found"),
            help: "paths are relative to the contract".into(),
            line: None,
        });
    }
    let lines = c.lines();
    for l in &mut lints {
        l.line = match (&l.rule, l.name) {
            (Some(r), "admit-reads-only-ctx" | "constant-rule" | "duplicate-condition") => {
                lines.rule_expr(r)
            }
            (Some(r), _) => lines.rule(r),
            (None, "unused-extension") => lines.top("extensions"),
            (None, "open-access") => lines.top("rules").or_else(|| lines.top("contract")),
            (None, "binding-not-found") => lines.top("binding"),
            (None, _) => lines.top("contract"),
        };
    }
    lints.sort_by_key(|l| (l.severity, l.line));
    lints
}

async fn validate_json(
    path: &Path,
    schema: &SchemaArg,
    lint: bool,
    functions: &Functions,
    types: &TypeHints,
) -> R {
    let file = path.display().to_string();
    let print = |v: serde_json::Value| -> Result<(), String> {
        println!(
            "{}",
            serde_json::to_string_pretty(&v).map_err(|e| e.to_string())?
        );
        Ok(())
    };
    let contract = match Contract::read(path)? {
        Ok(c) => c,
        Err((_, diags)) => {
            print(json!({"contract": file, "valid": false, "diagnostics": diags}))?;
            return Ok(ExitCode::from(1));
        }
    };
    let (schema, from) = source::schema_for(&contract, schema.schema.as_deref(), types).await?;
    let registry = functions.load(contract.doc.owner.as_deref())?;
    match contract.compile(&schema, &registry) {
        Err(mut diags) => {
            source::locate(&mut diags, &contract.source, Some(&from));
            print(json!({
                "contract": file, "valid": false, "types_from": from.to_string(), "diagnostics": diags,
            }))?;
            Ok(ExitCode::from(1))
        }
        Ok(compiled) => {
            let ready = Ready {
                contract,
                schema,
                from,
                registry,
                compiled,
            };
            let mut out = json!({
                "contract": file,
                "name": ready.compiled.contract.name,
                "version": ready.compiled.contract.version,
                "valid": true,
                "types_from": ready.from.to_string(),
                "diagnostics": [],
            });
            if lint {
                out["lints"] = json!(lints_for(&ready));
            }
            print(out)?;
            Ok(ExitCode::SUCCESS)
        }
    }
}

/// `rules  5 · 2 decide, 3 assert`
fn rule_summary(o: &mut Out, report: &[ReportEntry]) {
    let mut by_op: Vec<(&str, usize)> = Vec::new();
    for r in report {
        match by_op.iter_mut().find(|(op, _)| *op == r.op) {
            Some((_, n)) => *n += 1,
            None => by_op.push((&r.op, 1)),
        }
    }
    let ops: Vec<String> = by_op.iter().map(|(op, n)| format!("{n} {op}")).collect();
    ui::field(o, "rules", format!("{}  {}", report.len(), ops.join(", ")));
}

/// The contract's name, version and hash, and where its column types came from.
fn report_header(o: &mut Out, c: &Compilation, from: &TypesFrom) {
    let cc = &c.contract;
    ui::title_line(o, &cc.name, &cc.version, &cc.compilation_hash[..16]);
    ui::field(o, "types", from);
}

/// A file's size: `4.9 KB`.
fn size(p: &Path) -> String {
    match std::fs::metadata(p).map(|m| m.len()) {
        Ok(n) if n >= 1024 * 1024 => format!("{:.1} MB", n as f64 / (1024.0 * 1024.0)),
        Ok(n) if n >= 1024 => format!("{:.1} KB", n as f64 / 1024.0),
        Ok(n) => format!("{n} B"),
        Err(_) => String::new(),
    }
}
