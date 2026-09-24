//! Shape operators (peQL design 4.7): suppress, sample and noise.

use std::collections::BTreeMap;
use std::sync::Arc;

use datafusion::arrow::array::{Array, ArrayRef, Float64Array, UInt64Array};
use datafusion::arrow::datatypes::DataType;
use datafusion::common::Result as DFResult;
use datafusion::common::tree_node::{Transformed, TreeNode, TreeNodeRecursion};
use datafusion::functions_aggregate::expr_fn::count;
use datafusion::logical_expr::{
    ColumnarValue, Expr, LogicalPlan, LogicalPlanBuilder, Operator, ScalarFunctionArgs, ScalarUDF,
    ScalarUDFImpl, Signature, Volatility, binary_expr, cast, col, lit,
};
use parcel_core::ShapeOp;
use parcel_runtime::Caller;

use crate::budget::BudgetStore;
use crate::engine::col_ref;
use crate::error::{EngineError, Result};

pub const SAMPLE_BUCKETS: u64 = 1_000_000;
const GROUP_SIZE: &str = "__parcel_group_size";

/// `parcel_sample_bucket(x)`: a stable bucket in [0, 1e6) from the value's text, FNV-1a.
#[derive(Debug, PartialEq, Eq, Hash)]
struct SampleBucket {
    signature: Signature,
}

impl ScalarUDFImpl for SampleBucket {
    fn name(&self) -> &str {
        "parcel_sample_bucket"
    }
    fn signature(&self) -> &Signature {
        &self.signature
    }
    fn return_type(&self, _: &[DataType]) -> DFResult<DataType> {
        Ok(DataType::UInt64)
    }
    fn invoke_with_args(&self, args: ScalarFunctionArgs) -> DFResult<ColumnarValue> {
        let arr = args.args[0].to_array(args.number_rows)?;
        let text = datafusion::arrow::compute::cast(&arr, &DataType::Utf8)?;
        let text = text
            .as_any()
            .downcast_ref::<datafusion::arrow::array::StringArray>()
            .expect("utf8");
        let out: UInt64Array = (0..text.len())
            .map(|i| {
                (!text.is_null(i)).then(|| {
                    let mut h: u64 = 0xcbf29ce484222325;
                    for b in text.value(i).bytes() {
                        h ^= b as u64;
                        h = h.wrapping_mul(0x100000001b3);
                    }
                    h % SAMPLE_BUCKETS
                })
            })
            .collect();
        Ok(ColumnarValue::Array(Arc::new(out) as ArrayRef))
    }
}

pub fn sample_bucket_udf() -> ScalarUDF {
    ScalarUDF::new_from_impl(SampleBucket {
        signature: Signature::any(1, Volatility::Immutable),
    })
}

/// Keep a deterministic `fraction` of rows keyed on `key`: repeated queries see the same sample.
pub fn sample_predicate(key: &str, fraction: f64) -> Expr {
    let bucket = sample_bucket_udf().call(vec![col_ref(key)]);
    binary_expr(
        bucket,
        Operator::Lt,
        lit((fraction * SAMPLE_BUCKETS as f64).round() as u64),
    )
}

pub fn has_aggregate(plan: &LogicalPlan) -> bool {
    plan.exists(|p| Ok(matches!(p, LogicalPlan::Aggregate(_))))
        .unwrap_or(false)
}

/// Drop every group of the topmost aggregate with fewer than `k` rows, invisibly to the parents.
pub fn suppress_groups(plan: LogicalPlan, k: u64) -> Result<LogicalPlan> {
    let mut done = false;
    let out = plan.transform_down(|node| {
        if done {
            return Ok(Transformed::new(node, false, TreeNodeRecursion::Jump));
        }
        let LogicalPlan::Aggregate(a) = &node else {
            return Ok(Transformed::no(node));
        };
        done = true;
        let original: Vec<Expr> = a.schema.columns().into_iter().map(Expr::Column).collect();
        let mut aggr = a.aggr_expr.clone();
        aggr.push(count(lit(1i64)).alias(GROUP_SIZE));
        let rebuilt = LogicalPlanBuilder::from(a.input.as_ref().clone())
            .aggregate(a.group_expr.clone(), aggr)?
            .filter(binary_expr(col(GROUP_SIZE), Operator::GtEq, lit(k as i64)))?
            .project(original)?
            .build()?;
        Ok(Transformed::new(rebuilt, true, TreeNodeRecursion::Jump))
    })?;
    Ok(out.data)
}

/// `parcel_laplace(value, scale)`: value plus Laplace(0, scale) noise.
#[derive(Debug, PartialEq, Eq, Hash)]
struct Laplace {
    signature: Signature,
}

impl ScalarUDFImpl for Laplace {
    fn name(&self) -> &str {
        "parcel_laplace"
    }
    fn signature(&self) -> &Signature {
        &self.signature
    }
    fn return_type(&self, _: &[DataType]) -> DFResult<DataType> {
        Ok(DataType::Float64)
    }
    fn invoke_with_args(&self, args: ScalarFunctionArgs) -> DFResult<ColumnarValue> {
        let v = args.args[0].to_array(args.number_rows)?;
        let v = datafusion::arrow::compute::cast(&v, &DataType::Float64)?;
        let v = v.as_any().downcast_ref::<Float64Array>().expect("f64");
        let scale = match &args.args[1] {
            ColumnarValue::Scalar(datafusion::common::ScalarValue::Float64(Some(x))) => *x,
            _ => 1.0,
        };
        let out: Float64Array = v
            .iter()
            .map(|x| x.map(|x| x + laplace_sample(scale)))
            .collect();
        Ok(ColumnarValue::Array(Arc::new(out)))
    }
}

/// Inverse-CDF Laplace sample from a uniform draw.
fn laplace_sample(scale: f64) -> f64 {
    let u: f64 = uniform() - 0.5;
    -scale * u.signum() * (1.0 - 2.0 * u.abs()).ln()
}

/// A uniform draw in (0, 1) from the OS-seeded hasher; noise needs unpredictability, not speed.
fn uniform() -> f64 {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u64(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0),
    );
    ((h.finish() >> 11) as f64 + 0.5) / (1u64 << 53) as f64
}

fn laplace_udf() -> ScalarUDF {
    ScalarUDF::new_from_impl(Laplace {
        signature: Signature::any(2, Volatility::Volatile),
    })
}

/// Add Laplace noise to every aggregate over a noised column, charging each budget once per query.
/// Refuses queries that could reveal the column other than through an aggregate.
pub fn apply_noise(
    plan: LogicalPlan,
    optimized: &LogicalPlan,
    shapes: &[&ShapeOp],
    caller: &Caller,
    store: &BudgetStore,
    remaining: &mut BTreeMap<String, f64>,
) -> Result<LogicalPlan> {
    let noised: Vec<(&str, f64, f64, &str)> = shapes
        .iter()
        .filter_map(|s| match s {
            ShapeOp::Noise {
                column,
                sensitivity,
                epsilon,
                budget,
            } => Some((column.as_str(), *sensitivity, *epsilon, budget.as_str())),
            _ => None,
        })
        .collect();
    let reads = |e: &Expr, c: &str| e.column_refs().iter().any(|r| r.name == c);
    if !has_aggregate(&plan) {
        // Without an aggregate, the query may run only if it never reads a noised column.
        // The optimised plan's scans list exactly the columns read.
        let mut read = Vec::new();
        optimized
            .apply(|p| {
                if let LogicalPlan::TableScan(ts) = p {
                    for (c, ..) in &noised {
                        if ts.projected_schema.fields().iter().any(|f| f.name() == c) {
                            read.push(c.to_string());
                        }
                    }
                }
                Ok(TreeNodeRecursion::Continue)
            })
            .ok();
        if read.is_empty() {
            return Ok(plan);
        }
        return Err(EngineError::Invalid(format!(
            "`{}` is released to this caller only through aggregates (SUM, AVG, COUNT, ...)",
            read.join("`, `")
        )));
    }
    let mut touched: Vec<(String, f64)> = Vec::new();
    let out = plan.transform_up(|node| {
        let LogicalPlan::Aggregate(a) = &node else {
            return Ok(Transformed::no(node));
        };
        for (c, ..) in &noised {
            if a.group_expr.iter().any(|g| reads(g, c)) {
                return Err(datafusion::error::DataFusionError::Plan(format!(
                    "`{c}` cannot be a grouping key for this caller"
                )));
            }
        }
        let mut changed = false;
        let out_cols = a.schema.columns();
        let n_group = a.group_expr.len();
        let mut proj: Vec<Expr> = Vec::new();
        for (i, c) in out_cols.iter().enumerate() {
            let base = Expr::Column(c.clone());
            let agg = (i >= n_group).then(|| &a.aggr_expr[i - n_group]);
            let hit = agg.and_then(|e| noised.iter().find(|(nc, ..)| reads(e, nc)));
            proj.push(match hit {
                Some((_, sensitivity, epsilon, budget)) => {
                    changed = true;
                    touched.push((budget.to_string(), *epsilon));
                    let noisy = laplace_udf().call(vec![base, lit(sensitivity / epsilon)]);
                    cast(noisy, DataType::Float64).alias(c.name.clone())
                }
                None => base.alias_qualified(c.relation.clone(), c.name.clone()),
            });
        }
        if !changed {
            return Ok(Transformed::no(node));
        }
        let rebuilt = LogicalPlanBuilder::from(node).project(proj)?.build()?;
        Ok(Transformed::new(rebuilt, true, TreeNodeRecursion::Jump))
    })?;
    // Charge each budget once per query, at the largest epsilon it was used with.
    let mut charges: BTreeMap<String, f64> = BTreeMap::new();
    for (b, e) in touched {
        let entry = charges.entry(b).or_insert(0.0);
        *entry = entry.max(e);
    }
    let who = format!("{}/{}", caller.tenant, caller.id);
    for (b, e) in charges {
        let left = store
            .charge(&b, &who, e)
            .ok_or_else(|| EngineError::BudgetExhausted { budget: b.clone() })?;
        remaining.insert(b, left);
    }
    Ok(out.data)
}
