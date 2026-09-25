//! The differential test (design 11): every per-row rule is evaluated twice over a
//! sample, once by the reference CEL interpreter and once by the translated
//! DataFusion expression, and any disagreement is a hard failure. Enrichers are
//! compared the same way, and each side feeds its own enriched values to the rules.

use std::collections::HashMap;
use std::sync::Arc;

use crate::Caller;
use crate::reference::{self, Scope};
use cel::Value;
use datafusion::arrow::array::RecordBatch;
use datafusion::common::ScalarValue;
use datafusion::datasource::{MemTable, provider_as_source};
use datafusion::logical_expr::{Expr, LogicalPlanBuilder};
use datafusion::prelude::SessionContext;
use parcel_core::Compilation;
use parcel_core::compile::RowRuleKind;
use serde::Serialize;

use crate::plan::{Result, RuntimeError, enrich_plan, param_values};

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Mismatch {
    pub rule: String,
    pub caller: String,
    pub row: usize,
    pub reference: String,
    pub datafusion: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DiffReport {
    pub rows: usize,
    pub callers: usize,
    pub evaluations: usize,
    /// Evaluations where both engines refused the input (overflow, division by zero).
    pub both_errored: usize,
    pub mismatches: Vec<Mismatch>,
}

impl DiffReport {
    pub fn passed(&self) -> bool {
        self.mismatches.is_empty()
    }
}

/// One thing to compare: a rule or an enricher.
struct Item {
    id: String,
    /// Predicates are false on a null read; values are null.
    predicate: bool,
    /// Independent of the caller: compare once.
    caller_free: bool,
    cel: String,
    reads: Vec<String>,
    expr: Expr,
}

type Cell = std::result::Result<Option<Value>, String>;

/// Compare interpreter and DataFusion over `batch` for each caller.
pub async fn differential(
    c: &Compilation,
    batch: &RecordBatch,
    callers: &[Caller],
) -> Result<DiffReport> {
    let cc = &c.contract;
    let batch = crate::plan::conform(batch.clone(), &cc.row_schema)?;
    let rows = batch.num_rows();
    let mut report = DiffReport {
        rows,
        callers: callers.len(),
        evaluations: 0,
        both_errored: 0,
        mismatches: Vec::new(),
    };

    let mut items: Vec<Item> = Vec::new();
    for e in &cc.enrich {
        let id = format!("enrich.{}", e.field);
        items.push(Item {
            id,
            predicate: false,
            caller_free: true,
            cel: e.cel.clone(),
            reads: e.reads.clone(),
            expr: e.expr.clone(),
        });
    }
    for r in &cc.row_rules {
        let expr = match &r.kind {
            RowRuleKind::Admit => cc
                .admits
                .iter()
                .find(|(id, _)| *id == r.id)
                .map(|(_, e)| e.clone()),
            RowRuleKind::Assert => cc
                .flags
                .iter()
                .find(|f| f.assert_id == r.id)
                .map(|f| f.expr.clone()),
            RowRuleKind::Transform { column } => cc
                .projection
                .iter()
                .find(|(n, _)| n == column)
                .map(|(_, e)| e.clone()),
        }
        .ok_or_else(|| RuntimeError::Invalid(format!("rule `{}` has no expression", r.id)))?;
        items.push(Item {
            id: r.id.clone(),
            predicate: !matches!(r.kind, RowRuleKind::Transform { .. }),
            caller_free: matches!(r.kind, RowRuleKind::Assert),
            cel: r.cel.clone(),
            reads: r.reads.clone(),
            expr,
        });
    }
    if items.is_empty() {
        return Ok(report);
    }

    // The reference side's rows, each with its own enriched `other` map.
    let mut row_values = row_maps(&batch);
    if !cc.enrich.is_empty() {
        for row in &mut row_values {
            let mut other: HashMap<String, Value> = HashMap::new();
            for e in &cc.enrich {
                if e.reads.iter().all(|r| row.contains_key(r)) {
                    let scope = Scope {
                        row: Some(row.clone().into()),
                        pins: cc.functions.clone(),
                        ..Default::default()
                    };
                    if let Ok(v) = reference::eval(&e.cel, &reference::context(&scope)) {
                        other.insert(e.field.clone(), v);
                    }
                }
            }
            row.insert("other".into(), other.into());
        }
    }

    for (ci, caller) in callers.iter().enumerate() {
        let ctx = SessionContext::new();
        let params = param_values(cc, caller)?;
        let caller_value = reference::ctx_value_typed(caller, &cc.ctx_other);
        let who = format!("{}/{}", caller.tenant, caller.id);
        for item in &items {
            if item.caller_free && ci > 0 {
                continue;
            }
            let mem = MemTable::try_new(cc.row_schema.clone(), vec![vec![batch.clone()]])?;
            let run = async {
                let scan =
                    LogicalPlanBuilder::scan("sample", provider_as_source(Arc::new(mem)), None)?
                        .build()?;
                let enriched = enrich_plan(scan, cc)
                    .map_err(|e| datafusion::error::DataFusionError::External(Box::new(e)))?;
                let plan = LogicalPlanBuilder::from(enriched)
                    .project(vec![item.expr.clone().alias("r")])?
                    .build()?;
                let plan =
                    parcel_core::compile::resolve(plan)?.with_param_values(params.clone())?;
                ctx.execute_logical_plan(plan).await?.collect().await
            };
            let actual: Vec<Cell> = match run.await {
                Ok(batches) => batches
                    .iter()
                    .flat_map(|b| {
                        (0..b.num_rows()).map(move |i| {
                            ScalarValue::try_from_array(b.column(0), i)
                                .map(|s| reference::from_scalar(&s))
                                .map_err(|e| e.to_string())
                        })
                    })
                    .collect(),
                Err(e) => vec![Err(e.to_string()); rows],
            };
            for (i, row) in row_values.iter().enumerate() {
                report.evaluations += 1;
                let expected: Cell = if item.reads.iter().any(|r| !reads_present(row, r)) {
                    // parcel's null rule, applied by the reference side explicitly.
                    Ok(if item.predicate {
                        Some(Value::Bool(false))
                    } else {
                        None
                    })
                } else {
                    let scope = Scope {
                        ctx: Some(caller_value.clone()),
                        dataset: None,
                        row: Some(row.clone().into()),
                        pins: cc.functions.clone(),
                    };
                    reference::eval(&item.cel, &reference::context(&scope))
                        // A CEL `null` result is a SQL null.
                        .map(|v| (!matches!(v, Value::Null)).then_some(v))
                };
                let got = actual.get(i).cloned().unwrap_or(Err("missing row".into()));
                if expected.is_err() && got.is_err() {
                    report.both_errored += 1;
                }
                if !agree(&expected, &got) {
                    report.mismatches.push(Mismatch {
                        rule: item.id.clone(),
                        caller: who.clone(),
                        row: i,
                        reference: show(&expected),
                        datafusion: show(&got),
                    });
                }
            }
        }
    }
    Ok(report)
}

/// Whether a value read (`col` or `other.<field>`) is present, i.e. not null.
fn reads_present(row: &HashMap<String, Value>, read: &str) -> bool {
    match read.strip_prefix("other.") {
        Some(f) => match row.get("other") {
            Some(Value::Map(m)) => m
                .map
                .contains_key(&cel::objects::Key::String(Arc::new(f.to_owned()))),
            _ => false,
        },
        None => row.contains_key(read),
    }
}

fn row_maps(batch: &RecordBatch) -> Vec<HashMap<String, Value>> {
    let schema = batch.schema();
    (0..batch.num_rows())
        .map(|i| {
            let mut m = HashMap::new();
            for (ci, f) in schema.fields().iter().enumerate() {
                if let Ok(s) = ScalarValue::try_from_array(batch.column(ci), i)
                    && let Some(v) = reference::from_scalar(&s)
                {
                    m.insert(f.name().clone(), v);
                }
            }
            m
        })
        .collect()
}

fn agree(a: &Cell, b: &Cell) -> bool {
    match (a, b) {
        (Ok(Some(Value::Float(x))), Ok(Some(Value::Float(y)))) => {
            x == y || (x - y).abs() <= 1e-9 * x.abs().max(y.abs()).max(1.0)
        }
        (Ok(x), Ok(y)) => x == y,
        // Both engines refusing the same input (overflow, division by zero) is agreement.
        (Err(_), Err(_)) => true,
        _ => false,
    }
}

fn show(v: &Cell) -> String {
    match v {
        Ok(Some(v)) => format!("{v:?}"),
        Ok(None) => "null".into(),
        Err(e) => format!("error: {e}"),
    }
}
