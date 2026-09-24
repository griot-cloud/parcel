//! The differential test (design 11): every per-row rule is evaluated twice over a
//! sample, once by the reference CEL interpreter and once by the translated
//! DataFusion expression, and any disagreement is a hard failure.

use std::collections::HashMap;
use std::sync::Arc;

use cel::Value;
use datafusion::arrow::array::RecordBatch;
use datafusion::common::ScalarValue;
use datafusion::datasource::{MemTable, provider_as_source};
use datafusion::logical_expr::{Expr, LogicalPlanBuilder};
use datafusion::prelude::SessionContext;
use parcel_core::Compilation;
use parcel_core::compile::RowRuleKind;
use parcel_runtime::Caller;
use parcel_runtime::reference::{self, Scope};
use serde::Serialize;

use crate::engine::param_values;
use crate::error::{EngineError, Result};

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

/// Compare interpreter and DataFusion over `batch` for each caller.
pub async fn differential(
    c: &Compilation,
    batch: &RecordBatch,
    callers: &[Caller],
) -> Result<DiffReport> {
    let cc = &c.contract;
    let batch = crate::engine::conform_one(batch.clone(), &cc.row_schema)?;
    let rows = batch.num_rows();
    let row_values = row_maps(&batch);
    let mut report = DiffReport {
        rows,
        callers: callers.len(),
        evaluations: 0,
        both_errored: 0,
        mismatches: Vec::new(),
    };

    let exprs: Vec<(&parcel_core::compile::RowRule, Expr)> = cc
        .row_rules
        .iter()
        .map(|r| {
            let e = match &r.kind {
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
            };
            e.map(|e| (r, e))
                .ok_or_else(|| EngineError::Invalid(format!("rule `{}` has no expression", r.id)))
        })
        .collect::<Result<_>>()?;
    if exprs.is_empty() {
        return Ok(report);
    }

    for (ci, caller) in callers.iter().enumerate() {
        let ctx = SessionContext::new();
        let params = param_values(cc, caller)?;
        // One plan per rule, so a rule that fails to plan cannot hide the others.
        let mut per_rule: Vec<std::result::Result<Vec<RecordBatch>, String>> = Vec::new();
        for (_, e) in &exprs {
            let mem = MemTable::try_new(cc.row_schema.clone(), vec![vec![batch.clone()]])?;
            let run = async {
                let plan =
                    LogicalPlanBuilder::scan("sample", provider_as_source(Arc::new(mem)), None)?
                        .project(vec![e.clone().alias("r")])?
                        .build()?;
                let plan =
                    parcel_core::compile::resolve(plan)?.with_param_values(params.clone())?;
                ctx.execute_logical_plan(plan).await?.collect().await
            };
            per_rule.push(run.await.map_err(|e| e.to_string()));
        }
        let caller_value = reference::ctx_value(caller);
        let who = format!("{}/{}", caller.tenant, caller.id);
        for (ri, (rule, _)) in exprs.iter().enumerate() {
            // Asserts do not read ctx: check them once.
            if matches!(rule.kind, RowRuleKind::Assert) && ci > 0 {
                continue;
            }
            let actual: Vec<std::result::Result<Option<Value>, String>> = match &per_rule[ri] {
                Ok(batches) => {
                    let mut out = Vec::with_capacity(rows);
                    for b in batches {
                        for i in 0..b.num_rows() {
                            out.push(
                                ScalarValue::try_from_array(b.column(0), i)
                                    .map(|s| reference::from_scalar(&s))
                                    .map_err(|e| e.to_string()),
                            );
                        }
                    }
                    out
                }
                Err(e) => vec![Err(e.clone()); rows],
            };
            let predicate = !matches!(rule.kind, RowRuleKind::Transform { .. });
            for (i, row) in row_values.iter().enumerate() {
                report.evaluations += 1;
                let expected: std::result::Result<Option<Value>, String> =
                    if rule.reads.iter().any(|c| !row.contains_key(c)) {
                        // parcel's null rule, applied by the reference side explicitly.
                        Ok(if predicate {
                            Some(Value::Bool(false))
                        } else {
                            None
                        })
                    } else {
                        let scope = Scope {
                            ctx: Some(caller_value.clone()),
                            dataset: None,
                            row: Some(row.clone().into()),
                        };
                        reference::eval(&rule.cel, &reference::context(&scope)).map(Some)
                    };
                let got = actual.get(i).cloned().unwrap_or(Err("missing row".into()));
                if expected.is_err() && got.is_err() {
                    report.both_errored += 1;
                }
                if !agree(&expected, &got) {
                    report.mismatches.push(Mismatch {
                        rule: rule.id.clone(),
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

fn agree(
    a: &std::result::Result<Option<Value>, String>,
    b: &std::result::Result<Option<Value>, String>,
) -> bool {
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

fn show(v: &std::result::Result<Option<Value>, String>) -> String {
    match v {
        Ok(Some(v)) => format!("{v:?}"),
        Ok(None) => "null".into(),
        Err(e) => format!("error: {e}"),
    }
}
