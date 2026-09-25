//! Helpers an engine uses to run parcel artifacts, so every runtime binds them the same way:
//! caller parameters, the enrichment stage, the validation plan over any table, and the
//! `dataset` namespace from stored statistics. None of this stores data or plans queries.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use cel::Value;
use chrono::{DateTime, Utc};
use datafusion::arrow::array::RecordBatch;
use datafusion::arrow::datatypes::SchemaRef;
use datafusion::catalog::TableProvider;
use datafusion::common::tree_node::{Transformed, TreeNode, TreeNodeRecursion};
use datafusion::common::{ParamValues, ScalarValue};
use datafusion::datasource::provider_as_source;
use datafusion::logical_expr::{Expr, LogicalPlan, LogicalPlanBuilder};
use datafusion::prelude::SessionContext;
use parcel_core::compile::BINDING_TABLE;
use parcel_core::types::Type;
use parcel_core::{Compilation, CompiledContract};
use serde::Serialize;

use crate::Caller;
use crate::reference::{self, Scope};

#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    DataFusion(#[from] datafusion::error::DataFusionError),
}

impl From<String> for RuntimeError {
    fn from(s: String) -> Self {
        RuntimeError::Invalid(s)
    }
}

pub type Result<T> = std::result::Result<T, RuntimeError>;

/// The one-row validation verdict (parcel design 9.2), with the statistics it carried.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Verdict {
    pub contract: String,
    pub contract_hash: String,
    pub compilation_hash: String,
    pub valid: bool,
    pub breached: Vec<String>,
    pub row_count: i64,
    /// Failing rows per assert.
    pub failures: BTreeMap<String, i64>,
    /// Data-only guarantees and whether each held.
    pub guarantees: BTreeMap<String, bool>,
    /// Every statistic, keyed by its column name. Decimals are exact text.
    pub stats: BTreeMap<String, serde_json::Value>,
}

/// A session with parcel's own functions registered.
pub fn session() -> SessionContext {
    let ctx = SessionContext::new();
    for udf in parcel_core::udfs::parcel_udfs() {
        ctx.register_udf(udf);
    }
    ctx
}

/// Run a validation plan over `data` (any table with the contract's scan schema) and read
/// the verdict. A certificate verifier runs the same plan over the same data.
pub async fn validate(
    c: &Compilation,
    validation_plan: LogicalPlan,
    data: Arc<dyn TableProvider>,
) -> Result<Verdict> {
    let cc = &c.contract;
    let plan = bind_binding(validation_plan, cc, data)?;
    let batches = session()
        .execute_logical_plan(plan)
        .await?
        .collect()
        .await?;
    let batch = batches
        .iter()
        .find(|b| b.num_rows() > 0)
        .ok_or_else(|| RuntimeError::Invalid("validation returned no row".into()))?;
    let get = |name: &str| -> Result<ScalarValue> {
        let idx = batch
            .schema()
            .index_of(name)
            .map_err(|e| RuntimeError::Invalid(e.to_string()))?;
        Ok(ScalarValue::try_from_array(batch.column(idx), 0)?)
    };
    let breached = match get("breached")? {
        ScalarValue::Utf8(Some(s))
        | ScalarValue::Utf8View(Some(s))
        | ScalarValue::LargeUtf8(Some(s)) => s
            .split(',')
            .filter(|x| !x.is_empty())
            .map(str::to_owned)
            .collect(),
        _ => Vec::new(),
    };
    let mut failures = BTreeMap::new();
    for a in &c.validation.asserts {
        failures.insert(
            a.clone(),
            scalar_i64(&get(&format!("fail__{a}"))?).unwrap_or(0),
        );
    }
    let mut guarantees = BTreeMap::new();
    for g in &c.validation.data_guarantees {
        guarantees.insert(
            g.clone(),
            matches!(
                get(&format!("guarantee__{g}"))?,
                ScalarValue::Boolean(Some(true))
            ),
        );
    }
    let mut stats = BTreeMap::new();
    for s in &c.validation.stats {
        stats.insert(s.column.clone(), scalar_json(&get(&s.column)?));
    }
    Ok(Verdict {
        contract: cc.name.clone(),
        contract_hash: cc.contract_hash.clone(),
        compilation_hash: cc.compilation_hash.clone(),
        valid: matches!(get("valid")?, ScalarValue::Boolean(Some(true))),
        breached,
        row_count: scalar_i64(&get("row_count")?).unwrap_or(0),
        failures,
        guarantees,
        stats,
    })
}

/// Swap the validation plan's placeholder table for real data, projected to the scan schema.
pub fn bind_binding(
    plan: LogicalPlan,
    cc: &CompiledContract,
    data: Arc<dyn TableProvider>,
) -> Result<LogicalPlan> {
    let columns: Vec<Expr> = cc
        .scan_schema
        .fields()
        .iter()
        .map(|f| {
            datafusion::logical_expr::cast(col_ref(f.name()), f.data_type().clone()).alias(f.name())
        })
        .collect();
    // Raw data has no `_other` yet: run the enrichers first, as the write path would.
    let needs_enrich = !cc.enrich.is_empty()
        && data
            .schema()
            .field_with_name(parcel_core::ir::OTHER_COLUMN)
            .is_err();
    let scan =
        LogicalPlanBuilder::scan("__parcel_data", provider_as_source(data), None)?.build()?;
    let scan = if needs_enrich {
        enrich_plan(scan, cc)?
    } else {
        scan
    };
    let real = LogicalPlanBuilder::from(scan)
        .project(columns)?
        .alias(BINDING_TABLE)?
        .build()?;
    let out = plan.transform_up(|node| {
        if let LogicalPlan::TableScan(ts) = &node
            && ts.table_name.table() == BINDING_TABLE
        {
            return Ok(Transformed::new(
                real.clone(),
                true,
                TreeNodeRecursion::Jump,
            ));
        }
        Ok(Transformed::no(node))
    })?;
    Ok(out.data.recompute_schema()?)
}

/// Add the `_other` struct column the contract's enrichers produce (parcel design 10, stage 1).
pub fn enrich_plan(input: LogicalPlan, cc: &CompiledContract) -> Result<LogicalPlan> {
    if cc.enrich.is_empty() {
        return Ok(input);
    }
    let mut select: Vec<Expr> = input
        .schema()
        .columns()
        .into_iter()
        .map(Expr::Column)
        .collect();
    let mut args = Vec::new();
    for e in &cc.enrich {
        args.push(datafusion::logical_expr::lit(e.field.clone()));
        args.push(e.expr.clone());
    }
    select.push(
        datafusion::functions::core::expr_fn::named_struct(args)
            .alias(parcel_core::ir::OTHER_COLUMN),
    );
    let plan = LogicalPlanBuilder::from(input).project(select)?.build()?;
    Ok(parcel_core::compile::resolve(plan)?)
}

/// Values for every `ctx` placeholder in a compiled contract, from the reference interpreter.
pub fn param_values(cc: &CompiledContract, caller: &Caller) -> Result<ParamValues> {
    let ctx = reference::context(&Scope {
        ctx: Some(reference::ctx_value_typed(caller, &cc.ctx_other)),
        pins: cc.functions.clone(),
        ..Default::default()
    });
    let mut map = HashMap::new();
    for p in &cc.params {
        let v = reference::eval(&p.cel, &ctx)?;
        let s = reference::to_scalar(&v, &p.ty)?;
        map.insert(p.id.clone(), s.into());
    }
    Ok(ParamValues::Map(map))
}

/// The first `decide` rule that refuses this caller, if any. Decisions read only `ctx`, so an
/// engine evaluates them once per query, before any file is opened.
pub fn refusal(cc: &CompiledContract, caller: &Caller) -> Result<Option<String>> {
    let ctx = reference::context(&Scope {
        ctx: Some(reference::ctx_value_typed(caller, &cc.ctx_other)),
        pins: cc.functions.clone(),
        ..Default::default()
    });
    for d in &cc.decisions {
        if !reference::eval_bool(&d.cel, &ctx)? {
            return Ok(Some(d.id.clone()));
        }
    }
    Ok(None)
}

/// The shape rules that apply to this caller: those whose `unless` does not hold.
pub fn active_shapes<'a>(
    cc: &'a CompiledContract,
    caller: &Caller,
) -> Result<Vec<&'a parcel_core::compile::ShapeRule>> {
    let ctx = reference::context(&Scope {
        ctx: Some(reference::ctx_value_typed(caller, &cc.ctx_other)),
        pins: cc.functions.clone(),
        ..Default::default()
    });
    let mut out = Vec::new();
    for s in &cc.shapes {
        let skip = match &s.unless {
            Some(u) => reference::eval_bool(u, &ctx)?,
            None => false,
        };
        if !skip {
            out.push(s);
        }
    }
    Ok(out)
}

/// The `dataset` namespace from stored statistics (a verdict's `stats`) and write metadata.
pub fn dataset_value(
    c: &Compilation,
    stats: &BTreeMap<String, serde_json::Value>,
    written_at: DateTime<Utc>,
    contract_hash: &str,
) -> Value {
    let mut entries: Vec<(String, Value)> = vec![
        (
            "dataset.written_at".into(),
            reference::timestamp(written_at),
        ),
        (
            "dataset.contract_hash".into(),
            reference::string(contract_hash),
        ),
    ];
    for s in &c.validation.stats {
        if let Some(v) = stats.get(&s.column).and_then(|j| json_to_cel(j, &s.ty)) {
            entries.push((s.path.clone(), v));
        }
    }
    reference::dataset_value(&entries)
}

fn json_to_cel(j: &serde_json::Value, ty: &Type) -> Option<Value> {
    Some(match ty {
        Type::Decimal(s) => Value::Int(parse_decimal(j.as_str()?, *s)?),
        Type::Int => Value::Int(j.as_i64()?),
        Type::Uint => Value::UInt(j.as_u64()?),
        Type::Double => Value::Float(j.as_f64()?),
        Type::String => reference::string(j.as_str()?),
        Type::Bool => Value::Bool(j.as_bool()?),
        Type::Timestamp => reference::timestamp(
            DateTime::parse_from_rfc3339(j.as_str()?)
                .ok()?
                .with_timezone(&Utc),
        ),
        _ => return None,
    })
}

fn scalar_i64(s: &ScalarValue) -> Option<i64> {
    match s {
        ScalarValue::Int64(v) => *v,
        ScalarValue::UInt64(v) => v.map(|v| v as i64),
        _ => None,
    }
}

/// A statistic as JSON; decimals as exact text, never a float.
pub fn scalar_json(s: &ScalarValue) -> serde_json::Value {
    use serde_json::json;
    match s {
        ScalarValue::Decimal128(Some(v), _, scale) => {
            json!(parcel_core::cel_print::decimal_text(*v as i64, *scale))
        }
        ScalarValue::Int64(Some(v)) => json!(v),
        ScalarValue::UInt64(Some(v)) => json!(v),
        ScalarValue::Float64(Some(v)) => json!(v),
        ScalarValue::Boolean(Some(v)) => json!(v),
        ScalarValue::Utf8(Some(v))
        | ScalarValue::LargeUtf8(Some(v))
        | ScalarValue::Utf8View(Some(v)) => json!(v),
        ScalarValue::TimestampMicrosecond(Some(v), _) => {
            json!(DateTime::<Utc>::from_timestamp_micros(*v).map(|t| t.to_rfc3339()))
        }
        _ => serde_json::Value::Null,
    }
}

/// `"100.50"` at scale 2 → 10050, exactly.
pub fn parse_decimal(text: &str, scale: i8) -> Option<i64> {
    let (neg, t) = match text.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, text),
    };
    let (int, frac) = t.split_once('.').unwrap_or((t, ""));
    if frac.len() > scale as usize {
        return None;
    }
    let v: i64 = format!("{int}{frac}{}", "0".repeat(scale as usize - frac.len()))
        .parse()
        .ok()?;
    Some(if neg { -v } else { v })
}

/// An unqualified column reference that keeps its name verbatim.
pub fn col_ref(name: &str) -> Expr {
    Expr::Column(datafusion::common::Column::new_unqualified(name))
}

/// Cast a batch to the row schema a contract was compiled against.
pub fn conform(b: RecordBatch, schema: &SchemaRef) -> Result<RecordBatch> {
    use datafusion::arrow::compute::cast;
    let mut cols = Vec::with_capacity(schema.fields().len());
    for f in schema.fields() {
        let idx = b.schema().index_of(f.name()).map_err(|_| {
            RuntimeError::Invalid(format!(
                "the data has no column `{}` required by the contract",
                f.name()
            ))
        })?;
        let c = b.column(idx);
        cols.push(if c.data_type() == f.data_type() {
            c.clone()
        } else {
            cast(c, f.data_type()).map_err(|e| RuntimeError::Invalid(e.to_string()))?
        });
    }
    RecordBatch::try_new(schema.clone(), cols).map_err(|e| RuntimeError::Invalid(e.to_string()))
}

/// How many rows of `data` a caller would see: the contract's admits and drop-level asserts,
/// evaluated live with the caller's parameters. `parcel check` reports this per caller.
pub async fn selectivity(
    c: &Compilation,
    data: Arc<dyn TableProvider>,
    caller: &Caller,
) -> Result<usize> {
    use parcel_core::document::AssertOnFail;
    let cc = &c.contract;
    let scan =
        LogicalPlanBuilder::scan("__parcel_data", provider_as_source(data), None)?.build()?;
    let mut filters: Vec<Expr> = cc.admits.iter().map(|(_, e)| e.clone()).collect();
    filters.extend(
        cc.flags
            .iter()
            .filter(|f| f.on_fail == AssertOnFail::Drop)
            .map(|f| f.expr.clone()),
    );
    let mut b = LogicalPlanBuilder::from(enrich_plan(scan, cc)?);
    if let Some(f) = filters.into_iter().reduce(Expr::and) {
        b = b.filter(f)?;
    }
    let plan =
        parcel_core::compile::resolve(b.build()?)?.with_param_values(param_values(cc, caller)?)?;
    let batches = session()
        .execute_logical_plan(plan)
        .await?
        .collect()
        .await?;
    Ok(batches.iter().map(|b| b.num_rows()).sum())
}
