//! `parcel check`: a contract against sample data, one section per caller.
//!
//! Every count is its own query over the data, and they all run at once, each with a line on
//! the progress board.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use datafusion::arrow::array::{Array, BooleanArray, RecordBatch};
use datafusion::catalog::TableProvider;
use datafusion::common::{ParamValues, ScalarValue};
use datafusion::datasource::{MemTable, provider_as_source};
use datafusion::functions_aggregate::expr_fn::count;
use datafusion::logical_expr::{
    Expr, LogicalPlan, LogicalPlanBuilder, Operator, binary_expr, col, lit,
};
use futures::StreamExt;
use parcel_core::Compilation;
use parcel_core::compile::RowRuleKind;
use parcel_core::document::AssertOnFail;
use parcel_runtime::Caller;
use parcel_runtime::differential::{DiffReport, differential};
use parcel_runtime::plan::{Verdict, enrich_plan, param_values, session, validate};
use parcel_runtime::reference;
use serde_json::json;
use tokio::task::JoinHandle;

use crate::progress::{Board, Line, Tracker};
use crate::ui::{self, BOLD, CYAN, DIM, FAIL, GREEN, INFO, OK, Out, RED, WARN, YELLOW, paint};
use crate::{Functions, R, TypeHints, load_caller, ready, source, status};

type Table = Arc<dyn TableProvider>;
type Job<T> = JoinHandle<Result<T, String>>;
/// Passing rows, and the commonest failing values with their counts.
type AssertResult = (u64, Vec<(String, u64)>);

/// Marks a part of an item line as its operation.
const OP: &str = "\u{0}op:";

/// One caller's tasks, or the rules that refused them.
enum CallerRun {
    Refused {
        /// Every decide rule and whether it allowed this caller.
        decisions: Vec<(String, bool)>,
    },
    Reads {
        admitted: Job<u64>,
        visible: Job<u64>,
        /// Rows each admit keeps on its own.
        admits: Vec<(String, Job<u64>)>,
        /// Passing rows among those admitted, and the commonest failing values.
        asserts: Vec<(String, Job<AssertResult>)>,
        /// Rows whose value each transform changes.
        transforms: Vec<(String, String, Job<u64>)>,
    },
}

/// What `parcel check` does besides checking: the differential sample size, where to write
/// each caller's rows, and whether to print JSON.
pub struct Options<'a> {
    pub sample: usize,
    pub rows: Option<&'a Path>,
    pub json: bool,
}

pub async fn run(
    contract: &Path,
    data: &Path,
    caller_files: &[PathBuf],
    options: Options<'_>,
    functions: &Functions,
    types: &TypeHints,
) -> R {
    let Options {
        sample,
        rows: rows_dir,
        json,
    } = options;
    let Some(ready) = ready(contract, Some(data), functions, types, false).await? else {
        return Ok(ExitCode::from(1));
    };
    let c = Arc::new(ready.compiled);
    let cc = &c.contract;

    let reading = Tracker::new(0);
    let board = Board::start(vec![Line::task(
        format!("{}  ", paint(DIM, data.display())),
        &reading,
    )]);
    let loaded = source::read_data_tracked(data, types, &reading).await;
    board.stop().await;
    let (schema, batches) = loaded?;
    let total: u64 = batches.iter().map(|b| b.num_rows() as u64).sum();
    let table: Table = Arc::new(
        MemTable::try_new(schema.clone(), vec![batches.clone()]).map_err(|e| e.to_string())?,
    );

    let callers: Vec<Caller> = if caller_files.is_empty() {
        vec![Caller::new("check", "", "analytics")]
    } else {
        caller_files
            .iter()
            .map(|p| load_caller(p))
            .collect::<Result<_, _>>()?
    };
    let label = |c: &Caller| {
        if caller_files.is_empty() {
            "default caller".to_owned()
        } else {
            format!("{}/{}", c.tenant, c.id)
        }
    };

    // Rows for the differential test.
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

    // Everything starts now and runs at once.
    let verdict_t = Tracker::new(0);
    let verdict: Job<Verdict> = {
        let (c, table, t) = (c.clone(), table.clone(), verdict_t.clone());
        tokio::spawn(async move {
            let v = validate(&c, c.validation.plan.clone(), table)
                .await
                .map_err(|e| e.to_string());
            t.finish();
            v
        })
    };
    let diff_t = Tracker::new(0);
    let diff: Job<DiffReport> = {
        let (c, b, callers, t) = (c.clone(), sample_batch, callers.clone(), diff_t.clone());
        tokio::spawn(async move {
            let d = differential(&c, &b, &callers)
                .await
                .map_err(|e| e.to_string());
            t.finish();
            d
        })
    };

    let scan = scan(&c, &table)?;
    let width = cc
        .report
        .iter()
        .map(|r| r.rule.chars().count())
        .max()
        .unwrap_or(4)
        .max(12);
    let mut lines = vec![
        Line::text(paint(BOLD, format!("{} v{}", cc.name, cc.version))),
        Line::task(
            format!("{} rows  {}  ", ui::count(total), data.display()),
            &verdict_t,
        ),
    ];
    let mut runs = Vec::new();
    for caller in &callers {
        lines.push(Line::text(""));
        lines.push(Line::text(paint(BOLD, rule_line(&label(caller)))));
        let decisions = decisions(&c, caller)?;
        if decisions.iter().any(|(_, ok)| !ok) {
            runs.push(CallerRun::Refused { decisions });
            continue;
        }
        let params = param_values(cc, caller).map_err(|e| e.to_string())?;
        let all_admits = cc
            .admits
            .iter()
            .map(|(_, e)| e.clone())
            .reduce(Expr::and)
            .unwrap_or(lit(true));
        let drops = cc
            .flags
            .iter()
            .filter(|f| f.on_fail == AssertOnFail::Drop)
            .map(|f| f.expr.clone())
            .reduce(Expr::and)
            .unwrap_or(lit(true));

        let rows_t = Tracker::new(total);
        lines.push(Line::task(
            format!("rows     {:width$}", "", width = 0),
            &rows_t,
        ));
        let visible = count_job(&scan, all_admits.clone().and(drops), &params, &rows_t)?;
        let admitted = count_job(&scan, all_admits.clone(), &params, &Tracker::new(total))?;

        lines.push(Line::text(""));
        lines.push(Line::text(paint(BOLD, "Policies")));
        let mut admits = Vec::new();
        for (id, e) in &cc.admits {
            let t = Tracker::new(total);
            lines.push(Line::task(format!("  {id:<width$}     admit      "), &t));
            admits.push((id.clone(), count_job(&scan, e.clone(), &params, &t)?));
        }
        let mut transforms = Vec::new();
        for r in cc.report.iter().filter(|r| r.op == "transform") {
            let Some(column) = transform_column(&c, &r.rule) else {
                continue;
            };
            let Some((_, proj)) = cc.projection.iter().find(|(n, _)| n == &column) else {
                continue;
            };
            let t = Tracker::new(total);
            lines.push(Line::task(
                format!("  {:<width$}     transform  ", r.rule),
                &t,
            ));
            let changed = binary_expr(proj.clone(), Operator::IsDistinctFrom, col(&column));
            transforms.push((
                r.rule.clone(),
                column,
                count_job(&scan, all_admits.clone().and(changed), &params, &t)?,
            ));
        }

        lines.push(Line::text(""));
        lines.push(Line::text(paint(BOLD, "Quality")));
        let mut asserts = Vec::new();
        for f in &cc.flags {
            let t = Tracker::new(total);
            lines.push(Line::task(format!("  {:<width$}     ", f.assert_id), &t));
            asserts.push((
                f.assert_id.clone(),
                assert_job(
                    &c,
                    &scan,
                    all_admits.clone(),
                    f.expr.clone(),
                    &f.assert_id,
                    &params,
                    &t,
                )?,
            ));
        }
        runs.push(CallerRun::Reads {
            admitted,
            visible,
            admits,
            asserts,
            transforms,
        });
    }
    lines.push(Line::text(""));
    lines.push(Line::task("differential  ", &diff_t));

    let board = if json {
        None
    } else {
        Some(Board::start(lines))
    };
    let v = verdict.await.map_err(|e| e.to_string())??;
    let diff = diff.await.map_err(|e| e.to_string())??;
    let mut results = Vec::new();
    for r in runs {
        results.push(match r {
            CallerRun::Refused { decisions } => Done::Refused { decisions },
            CallerRun::Reads {
                admitted,
                visible,
                admits,
                asserts,
                transforms,
            } => Done::Reads {
                admitted: join(admitted).await?,
                visible: join(visible).await?,
                admits: join_all(admits).await?,
                asserts: join_all(asserts).await?,
                transforms: {
                    let mut out = Vec::new();
                    for (id, column, j) in transforms {
                        out.push((id, column, join(j).await?));
                    }
                    out
                },
            },
        });
    }
    if let Some(b) = board {
        b.stop().await;
    }
    if let Some(dir) = rows_dir {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        for (caller, r) in callers.iter().zip(&results) {
            if matches!(r, Done::Reads { .. }) {
                let file = dir.join(format!("{}.parquet", file_name(&caller.id)));
                write_rows(&c, &scan, caller, &file).await?;
            }
        }
    }
    let ok = v.valid && diff.passed();

    if json {
        let out = json!({
            "contract": cc.name,
            "version": cc.version,
            "compilation_hash": cc.compilation_hash,
            "report": cc.report,
            "verdict": v,
            "query_time_guarantees": c.validation.query_time_guarantees,
            "callers": callers.iter().zip(&results).map(|(caller, r)| match r {
                Done::Refused { decisions } => json!({
                    "caller": label(caller),
                    "refused_by": decisions.iter().find(|(_, ok)| !ok).map(|(id, _)| id),
                    "rows": null,
                }),
                Done::Reads { visible, .. } => json!({
                    "caller": label(caller), "refused_by": null, "rows": visible,
                }),
            }).collect::<Vec<_>>(),
            "differential": diff,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&out).map_err(|e| e.to_string())?
        );
        return Ok(status(ok));
    }

    let mut o = Out::default();
    o.line(paint(BOLD, format!("{} v{}", cc.name, cc.version)));
    let state = if v.valid {
        paint(GREEN, "valid")
    } else {
        format!("{}  {}", paint(RED, "INVALID"), v.breached.join(", "))
    };
    o.line(format!(
        "{} rows  {}  {state}",
        ui::count(total),
        data.display()
    ));

    let msg = |id: &str| {
        cc.report
            .iter()
            .find(|r| r.rule == id)
            .and_then(|r| r.message.clone())
    };
    let cel = |id: &str| {
        cc.report
            .iter()
            .find(|r| r.rule == id)
            .and_then(|r| r.cel.clone())
            .map(|s| strip_parens(&s))
            .unwrap_or_default()
    };
    let exposed = cc.exposed_schema.fields().len();
    let columns = schema.fields().len();
    // `  id  mark  rest…`, with the parts that are empty left out.
    // An op is padded to line up what follows it, and left unpadded when nothing does.
    let item = |id: &str, mark: String, parts: Vec<String>| {
        let parts: Vec<String> = parts.into_iter().filter(|p| !p.is_empty()).collect();
        let mut line = format!("  {id:<width$}  {mark}");
        let last = parts.len().saturating_sub(1);
        for (i, p) in parts.into_iter().enumerate() {
            line.push_str("  ");
            match p.strip_prefix(OP) {
                Some(op) if i < last => line.push_str(&paint(CYAN, format!("{op:<9}"))),
                Some(op) => line.push_str(&paint(CYAN, op)),
                None => line.push_str(&p),
            }
        }
        line
    };
    let dim = |s: Option<String>| {
        s.filter(|s| !s.is_empty())
            .map(|s| paint(DIM, s))
            .unwrap_or_default()
    };
    let op = |s: &str| format!("{OP}{s}");
    // Where text after the op column starts, for lines that continue an item.
    let under_op = " ".repeat(width + 7);
    let under_rate = " ".repeat(width + 15);
    for (caller, r) in callers.iter().zip(&results) {
        o.blank();
        o.line(paint(BOLD, rule_line(&label(caller))));
        match r {
            Done::Refused { decisions } => {
                o.line(format!("{}  {exposed}/{columns}", paint(DIM, "columns")));
                o.line(format!(
                    "{}     0/{}  {}",
                    paint(DIM, "rows"),
                    ui::count(total),
                    paint(RED, "refused")
                ));
                o.blank();
                o.line(paint(BOLD, "Policies"));
                for (id, allowed) in decisions {
                    let mark = if *allowed {
                        paint(GREEN, OK)
                    } else {
                        paint(RED, FAIL)
                    };
                    o.line(item(id, mark, vec![op("decide"), dim(msg(id))]));
                    if !allowed {
                        o.line(format!("{under_op}{}", paint(DIM, cel(id))));
                        o.line(format!(
                            "{under_op}{}",
                            paint(DIM, values_read(&cel(id), caller))
                        ));
                    }
                }
            }
            Done::Reads {
                admitted,
                visible,
                admits,
                asserts,
                transforms,
            } => {
                let changed: Vec<&str> = transforms
                    .iter()
                    .filter(|(_, _, n)| *n > 0)
                    .map(|(_, col, _)| col.as_str())
                    .collect();
                let changed = if changed.is_empty() {
                    String::new()
                } else {
                    format!("  {} transformed", changed.join(", "))
                };
                o.line(format!(
                    "{}  {exposed}/{columns}{changed}",
                    paint(DIM, "columns")
                ));
                o.line(format!(
                    "{}     {}/{}",
                    paint(DIM, "rows"),
                    ui::count(*visible),
                    ui::count(total)
                ));
                o.blank();
                o.line(paint(BOLD, "Policies"));
                for r in cc.report.iter().filter(|r| r.op == "decide") {
                    o.line(item(
                        &r.rule,
                        paint(GREEN, OK),
                        vec![op("decide"), dim(r.message.clone())],
                    ));
                }
                for (id, kept) in admits {
                    let removed = total - (*kept).min(total);
                    let mark = if *kept == 0 && total > 0 {
                        paint(RED, FAIL)
                    } else {
                        paint(GREEN, OK)
                    };
                    let what = if removed == 0 {
                        String::new()
                    } else {
                        format!("{} removed", ui::count(removed))
                    };
                    o.line(item(id, mark, vec![op("admit"), what, dim(msg(id))]));
                }
                for (id, column, n) in transforms {
                    let mark = if *n > 0 {
                        paint(GREEN, OK)
                    } else {
                        paint(DIM, INFO)
                    };
                    o.line(item(
                        id,
                        mark,
                        vec![
                            op("transform"),
                            format!("{column}  {} changed", ui::count(*n)),
                            dim(msg(id)),
                        ],
                    ));
                }
                for s in &cc.shapes {
                    let skipped = shape_skipped(&c, &s.unless, caller)?;
                    let mark = if skipped {
                        paint(DIM, INFO)
                    } else {
                        paint(GREEN, OK)
                    };
                    let what = if skipped {
                        "skipped (unless)".to_owned()
                    } else {
                        shape_label(&s.shape)
                    };
                    o.line(item(&s.id, mark, vec![op("shape"), what, dim(msg(&s.id))]));
                }

                if *admitted == 0 {
                    continue; // no rows reach this caller, so no quality to report
                }
                o.blank();
                o.line(format!(
                    "{}{}{}",
                    paint(BOLD, "Quality"),
                    " ".repeat(width + 2),
                    paint(DIM, "pass")
                ));
                for (id, (pass, values)) in asserts {
                    let fails = admitted - (*pass).min(*admitted);
                    let on_fail = cc
                        .flags
                        .iter()
                        .find(|f| &f.assert_id == id)
                        .map(|f| f.on_fail);
                    let (mark, outcome) = match (fails, on_fail) {
                        (0, _) => (paint(GREEN, OK), String::new()),
                        (_, Some(AssertOnFail::Drop)) => {
                            (paint(YELLOW, WARN), format!("{} dropped", ui::count(fails)))
                        }
                        (_, Some(AssertOnFail::Deny)) => (
                            paint(RED, FAIL),
                            format!("{} failed  deny", ui::count(fails)),
                        ),
                        _ => (paint(YELLOW, WARN), format!("{} failed", ui::count(fails))),
                    };
                    o.line(item(
                        id,
                        mark,
                        vec![
                            format!("{:>6}", pct(*pass, *admitted)),
                            outcome,
                            dim(msg(id)),
                        ],
                    ));
                    if fails > 0 && !values.is_empty() {
                        o.line(format!("{under_rate}{}", paint(DIM, show_values(values))));
                    }
                }
                for (id, held) in &v.guarantees {
                    let mark = if *held {
                        paint(GREEN, OK)
                    } else {
                        paint(RED, FAIL)
                    };
                    o.line(item(id, mark, vec![op("guarantee"), dim(msg(id))]));
                }
                for id in &c.validation.query_time_guarantees {
                    o.line(item(
                        id,
                        paint(DIM, INFO),
                        vec![op("guarantee"), "per query".into(), dim(msg(id))],
                    ));
                }
            }
        }
    }

    o.blank();
    if diff.passed() {
        o.line(format!(
            "{}  {}  {}",
            paint(DIM, "differential"),
            paint(GREEN, OK),
            paint(DIM, ui::count(diff.evaluations as u64))
        ));
    } else {
        o.line(format!(
            "{}  {}  {} mismatches",
            paint(DIM, "differential"),
            paint(RED, FAIL),
            diff.mismatches.len()
        ));
        for m in diff.mismatches.iter().take(10) {
            o.line(paint(
                DIM,
                format!(
                    "  {} row {} {}: cel {} datafusion {}",
                    m.rule, m.row, m.caller, m.reference, m.datafusion
                ),
            ));
        }
    }
    o.print();
    Ok(status(ok))
}

enum Done {
    Refused {
        decisions: Vec<(String, bool)>,
    },
    Reads {
        admitted: u64,
        visible: u64,
        admits: Vec<(String, u64)>,
        asserts: Vec<(String, AssertResult)>,
        transforms: Vec<(String, String, u64)>,
    },
}

async fn join<T>(j: Job<T>) -> Result<T, String> {
    j.await.map_err(|e| e.to_string())?
}

async fn join_all<T>(jobs: Vec<(String, Job<T>)>) -> Result<Vec<(String, T)>, String> {
    let mut out = Vec::new();
    for (id, j) in jobs {
        out.push((id, join(j).await?));
    }
    Ok(out)
}

/// A caller id as a file name.
fn file_name(id: &str) -> String {
    let s: String = id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if s.is_empty() { "caller".into() } else { s }
}

/// The rows `caller` receives, as parcel's runtime computes them, written as Parquet: the
/// reference an engine target's results are compared with.
async fn write_rows(
    c: &Compilation,
    scan: &LogicalPlan,
    caller: &Caller,
    file: &Path,
) -> Result<(), String> {
    let cc = &c.contract;
    let mut filters: Vec<Expr> = cc.admits.iter().map(|(_, e)| e.clone()).collect();
    filters.extend(
        cc.flags
            .iter()
            .filter(|f| f.on_fail == AssertOnFail::Drop)
            .map(|f| f.expr.clone()),
    );
    let mut b = LogicalPlanBuilder::from(scan.clone());
    if let Some(f) = filters.into_iter().reduce(Expr::and) {
        b = b.filter(f).map_err(|e| e.to_string())?;
    }
    let plan = b
        .project(
            cc.projection
                .iter()
                .map(|(name, e)| e.clone().alias(name))
                .collect::<Vec<_>>(),
        )
        .and_then(|b| b.build())
        .map_err(|e| e.to_string())?;
    let params = param_values(cc, caller).map_err(|e| e.to_string())?;
    let plan = bind(plan, &params)?;
    session()
        .execute_logical_plan(plan)
        .await
        .map_err(|e| e.to_string())?
        .write_parquet(
            &file.display().to_string(),
            datafusion::dataframe::DataFrameWriteOptions::new().with_single_file_output(true),
            None,
        )
        .await
        .map_err(|e| format!("{}: {e}", file.display()))?;
    Ok(())
}

/// `==== globex/wanjiru ====…`, 60 wide.
fn rule_line(who: &str) -> String {
    let head = format!("==== {who} ");
    let fill = 60usize.saturating_sub(head.chars().count()).max(4);
    format!("{head}{}", "=".repeat(fill))
}

/// Each decide rule, evaluated for `caller`.
fn decisions(c: &Compilation, caller: &Caller) -> Result<Vec<(String, bool)>, String> {
    let cc = &c.contract;
    let ctx = reference::context(&reference::Scope {
        ctx: Some(reference::ctx_value_typed(caller, &cc.ctx_other)),
        pins: cc.functions.clone(),
        ..Default::default()
    });
    cc.decisions
        .iter()
        .map(|d| Ok((d.id.clone(), reference::eval_bool(&d.cel, &ctx)?)))
        .collect()
}

fn scan(c: &Compilation, table: &Table) -> Result<LogicalPlan, String> {
    let scan = LogicalPlanBuilder::scan("__parcel_data", provider_as_source(table.clone()), None)
        .and_then(|b| b.build())
        .map_err(|e| e.to_string())?;
    enrich_plan(scan, &c.contract).map_err(|e| e.to_string())
}

fn bind(plan: LogicalPlan, params: &ParamValues) -> Result<LogicalPlan, String> {
    parcel_core::compile::resolve(plan)
        .and_then(|p| p.with_param_values(params.clone()))
        .map_err(|e| e.to_string())
}

/// Rows where `e` is true, counted as the data streams past.
fn count_job(
    scan: &LogicalPlan,
    e: Expr,
    params: &ParamValues,
    t: &Arc<Tracker>,
) -> Result<Job<u64>, String> {
    let plan = LogicalPlanBuilder::from(scan.clone())
        .project(vec![e.is_true().alias("__ok")])
        .and_then(|b| b.build())
        .map_err(|e| e.to_string())?;
    let plan = bind(plan, params)?;
    let t = t.clone();
    Ok(tokio::spawn(async move {
        let n = stream_true(plan, &t).await;
        t.finish();
        n
    }))
}

async fn stream_true(plan: LogicalPlan, t: &Tracker) -> Result<u64, String> {
    let mut stream = session()
        .execute_logical_plan(plan)
        .await
        .map_err(|e| e.to_string())?
        .execute_stream()
        .await
        .map_err(|e| e.to_string())?;
    let mut yes = 0u64;
    while let Some(b) = stream.next().await {
        let b: RecordBatch = b.map_err(|e| e.to_string())?;
        if let Some(a) = b.column(0).as_any().downcast_ref::<BooleanArray>() {
            yes += a.true_count() as u64;
        }
        t.add(b.num_rows() as u64);
    }
    Ok(yes)
}

/// Rows among those admitted that pass the assert, then its commonest failing values.
fn assert_job(
    c: &Compilation,
    scan: &LogicalPlan,
    admitted: Expr,
    flag: Expr,
    id: &str,
    params: &ParamValues,
    t: &Arc<Tracker>,
) -> Result<Job<AssertResult>, String> {
    let pass = LogicalPlanBuilder::from(scan.clone())
        .project(vec![
            admitted
                .clone()
                .is_true()
                .and(flag.clone().is_true())
                .alias("__ok"),
        ])
        .and_then(|b| b.build())
        .map_err(|e| e.to_string())?;
    let pass = bind(pass, params)?;
    let reads: Vec<String> = c
        .contract
        .row_rules
        .iter()
        .find(|r| r.id == id && r.kind == RowRuleKind::Assert)
        .map(|r| r.reads.clone())
        .unwrap_or_default();
    // Values are shown for plain columns only, not enriched `row.other` fields.
    let plain = reads
        .iter()
        .all(|r| scan.schema().has_column_with_unqualified_name(r));
    let values = if plain && (1..=2).contains(&reads.len()) {
        let plan = LogicalPlanBuilder::from(scan.clone())
            .filter(admitted.is_true().and(flag.is_not_true()))
            .and_then(|b| {
                b.aggregate(
                    reads.iter().map(col).collect::<Vec<_>>(),
                    vec![count(lit(1i64)).alias("__n")],
                )
            })
            .and_then(|b| b.sort(vec![col("__n").sort(false, false)]))
            .and_then(|b| b.limit(0, Some(3)))
            .and_then(|b| b.build())
            .map_err(|e| e.to_string())?;
        Some(bind(plan, params)?)
    } else {
        None
    };
    let t = t.clone();
    Ok(tokio::spawn(async move {
        let n = stream_true(pass, &t).await?;
        let mut shown = Vec::new();
        if let Some(plan) = values {
            let batches = session()
                .execute_logical_plan(plan)
                .await
                .map_err(|e| e.to_string())?
                .collect()
                .await
                .map_err(|e| e.to_string())?;
            for b in batches {
                for row in 0..b.num_rows() {
                    let k = b.num_columns() - 1;
                    let parts: Vec<String> = (0..k).map(|i| value(b.column(i), row)).collect();
                    let n = match ScalarValue::try_from_array(b.column(k), row) {
                        Ok(ScalarValue::Int64(Some(n))) => n.max(0) as u64,
                        _ => 0,
                    };
                    shown.push((parts.join(", "), n));
                }
            }
        }
        t.finish();
        Ok((n, shown))
    }))
}

fn value(a: &datafusion::arrow::array::ArrayRef, row: usize) -> String {
    match ScalarValue::try_from_array(a, row) {
        Ok(v) if v.is_null() => "null".into(),
        Ok(
            ScalarValue::Utf8(Some(s))
            | ScalarValue::LargeUtf8(Some(s))
            | ScalarValue::Utf8View(Some(s)),
        ) if s.is_empty() => "\"\"".into(),
        Ok(v) => v.to_string(),
        Err(_) => "?".into(),
    }
}

/// `Non-Binary (976)`, or `a, b, c` when no value repeats.
fn show_values(v: &[(String, u64)]) -> String {
    if v.iter().all(|(_, n)| *n == 1) {
        v.iter()
            .map(|(s, _)| s.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    } else {
        v.iter()
            .map(|(s, n)| format!("{s} ({})", ui::count(*n)))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

fn transform_column(c: &Compilation, id: &str) -> Option<String> {
    c.contract
        .row_rules
        .iter()
        .find(|r| r.id == id)
        .and_then(|r| match &r.kind {
            RowRuleKind::Transform { column } => Some(column.clone()),
            _ => None,
        })
}

/// The caller's values for the `ctx` fields a condition reads: `ctx.purpose = "sales"`.
fn values_read(cel: &str, caller: &Caller) -> String {
    let json = serde_json::to_value(caller).unwrap_or_default();
    let mut seen: Vec<String> = Vec::new();
    let mut rest = cel;
    while let Some(i) = rest.find("ctx.") {
        let after = &rest[i + 4..];
        let field: String = after
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        rest = &after[field.len()..];
        if field.is_empty() {
            continue;
        }
        let (name, v) = if field == "other" {
            let sub: String = rest
                .trim_start_matches('.')
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            (format!("other.{sub}"), json["other"][&sub].clone())
        } else {
            (field.clone(), json[&field].clone())
        };
        let shown = format!(
            "ctx.{name} = {}",
            if v.is_null() {
                "unset".into()
            } else {
                v.to_string()
            }
        );
        if !seen.contains(&shown) {
            seen.push(shown);
        }
    }
    seen.join("  ")
}

/// `(a && b)` as `a && b`.
fn strip_parens(s: &str) -> String {
    let t = s.trim();
    if t.starts_with('(') && t.ends_with(')') {
        let inner = &t[1..t.len() - 1];
        let mut depth = 0i32;
        let balanced = inner.chars().all(|c| {
            match c {
                '(' => depth += 1,
                ')' => depth -= 1,
                _ => {}
            }
            depth >= 0
        });
        if balanced && depth == 0 {
            return inner.to_owned();
        }
    }
    t.to_owned()
}

/// Whether a shape's `unless` lets this caller skip it.
fn shape_skipped(
    c: &Compilation,
    unless: &Option<String>,
    caller: &Caller,
) -> Result<bool, String> {
    let Some(cel) = unless else {
        return Ok(false);
    };
    let cc = &c.contract;
    let ctx = reference::context(&reference::Scope {
        ctx: Some(reference::ctx_value_typed(caller, &cc.ctx_other)),
        pins: cc.functions.clone(),
        ..Default::default()
    });
    reference::eval_bool(cel, &ctx)
}

/// `suppress k=5`, `sample 10% key=order_id`, `noise revenue ε=1 budget=partner_reports`.
fn shape_label(s: &parcel_core::ShapeOp) -> String {
    use parcel_core::ShapeOp::*;
    match s {
        Suppress { k } => format!("suppress k={k}"),
        Sample { fraction, key } => format!("sample {}% key={key}", fraction * 100.0),
        Noise {
            column,
            epsilon,
            budget,
            at,
            ..
        } => format!(
            "noise {column} ε={epsilon} budget={budget}{}",
            if matches!(at, parcel_core::NoiseAt::Row) {
                " at=row"
            } else {
                ""
            }
        ),
    }
}

/// `n` of `total` as a percentage, one decimal unless whole.
fn pct(n: u64, total: u64) -> String {
    if total == 0 {
        return "-".into();
    }
    let p = 100.0 * n as f64 / total as f64;
    if p == 100.0 || p == 0.0 {
        format!("{p:.0}%")
    } else {
        format!("{p:.1}%")
    }
}
