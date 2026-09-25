//! The shape rules that act on a caller's whole query rather than on the view: `suppress`
//! and `noise` at aggregates (parcel design 6.7). Each is a rewrite of the caller's logical
//! plan, so any engine applies them the same way. `sample` and `noise` at rows are compiled
//! into the view itself. Nothing here keeps state: privacy budgets are charged by the engine,
//! from the [`Charge`]s a rewrite returns.

use std::collections::BTreeMap;

use datafusion::arrow::array::RecordBatch;
use datafusion::arrow::datatypes::DataType;
use datafusion::common::tree_node::{Transformed, TreeNodeRecursion};
use datafusion::functions_aggregate::expr_fn::count;
use datafusion::logical_expr::{
    Expr, LogicalPlan, LogicalPlanBuilder, Operator, binary_expr, cast, col, lit,
};
use parcel_core::check::{NoiseAt, ShapeOp};
use parcel_core::compile::ShapeRule;
use parcel_core::udfs::laplace_udf;
use serde::Serialize;

use crate::plan::{Result, RuntimeError};

const GROUP_SIZE: &str = "__parcel_group_size";

/// Epsilon a query spends from a named budget.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Charge {
    pub budget: String,
    pub epsilon: f64,
}

/// A caller's query after the shape rules that apply to it.
#[derive(Debug)]
pub struct Shaped {
    pub plan: LogicalPlan,
    /// What to charge before running it: once per budget per query, at the largest epsilon.
    pub charges: Vec<Charge>,
    /// The `suppress` threshold. Without any aggregate, the whole result is one group: see
    /// [`suppress_ungrouped`].
    pub suppress_k: Option<u64>,
    pub has_aggregate: bool,
}

/// Apply the active shape rules of every contract in a query to the caller's plan.
/// `optimized` is the same plan after optimisation, whose scans list exactly the columns read.
pub fn apply(plan: LogicalPlan, optimized: &LogicalPlan, active: &[&ShapeRule]) -> Result<Shaped> {
    let mut charges: BTreeMap<String, f64> = BTreeMap::new();
    let mut charge = |budget: &str, epsilon: f64| {
        let e = charges.entry(budget.to_owned()).or_insert(0.0);
        *e = e.max(epsilon);
    };
    let read = columns_read(optimized);
    let mut plan = plan;
    let mut aggregate_noise = Vec::new();
    for s in active {
        if let ShapeOp::Noise {
            column,
            sensitivity,
            epsilon,
            budget,
            at,
        } = &s.shape
        {
            match at {
                // Compiled into the view: charge when the query reads the column.
                NoiseAt::Row => {
                    if read.iter().any(|c| c == column) {
                        charge(budget, *epsilon);
                    }
                }
                NoiseAt::Aggregate => aggregate_noise.push((
                    column.as_str(),
                    sensitivity / epsilon,
                    budget.as_str(),
                    *epsilon,
                )),
            }
        }
    }
    if !aggregate_noise.is_empty() {
        let (noised, touched) = noise_aggregates(plan, &read, &aggregate_noise)?;
        plan = noised;
        for (b, e) in touched {
            charge(&b, e);
        }
    }
    let has_aggregate = has_aggregate(&plan);
    let suppress_k = active
        .iter()
        .filter_map(|s| match s.shape {
            ShapeOp::Suppress { k } => Some(k),
            _ => None,
        })
        .max();
    if let (Some(k), true) = (suppress_k, has_aggregate) {
        plan = suppress_groups(plan, k)?;
    }
    Ok(Shaped {
        plan,
        charges: charges
            .into_iter()
            .map(|(budget, epsilon)| Charge { budget, epsilon })
            .collect(),
        suppress_k,
        has_aggregate,
    })
}

/// Without a `GROUP BY` the whole result is one group: fewer than `k` rows returns nothing.
pub fn suppress_ungrouped(batches: Vec<RecordBatch>, k: u64) -> Vec<RecordBatch> {
    if batches.iter().map(|b| b.num_rows()).sum::<usize>() < k as usize {
        batches.into_iter().map(|b| b.slice(0, 0)).collect()
    } else {
        batches
    }
}

/// Whether the query aggregates anywhere, subqueries included.
pub fn has_aggregate(plan: &LogicalPlan) -> bool {
    let mut found = false;
    let _ = plan.apply_with_subqueries(|p| {
        found |= matches!(p, LogicalPlan::Aggregate(_));
        Ok(if found {
            TreeNodeRecursion::Stop
        } else {
            TreeNodeRecursion::Continue
        })
    });
    found
}

/// Every column any scan in an optimised plan reads.
pub fn columns_read(optimized: &LogicalPlan) -> Vec<String> {
    let mut read = Vec::new();
    let _ = optimized.apply_with_subqueries(|p| {
        if let LogicalPlan::TableScan(ts) = p {
            read.extend(
                ts.projected_schema
                    .fields()
                    .iter()
                    .map(|f| f.name().clone()),
            );
        }
        Ok(TreeNodeRecursion::Continue)
    });
    read
}

/// Drop every group smaller than `k` from every aggregate, invisibly to its parents. Every
/// aggregate, not only the outermost: a `UNION` of two must not leak through its second branch.
pub fn suppress_groups(plan: LogicalPlan, k: u64) -> Result<LogicalPlan> {
    let out = plan.transform_up_with_subqueries(|node| {
        let LogicalPlan::Aggregate(a) = &node else {
            return Ok(Transformed::no(node));
        };
        let original: Vec<Expr> = a.schema.columns().into_iter().map(Expr::Column).collect();
        let mut aggr = a.aggr_expr.clone();
        aggr.push(count(lit(1i64)).alias(GROUP_SIZE));
        let rebuilt = LogicalPlanBuilder::from(a.input.as_ref().clone())
            .aggregate(a.group_expr.clone(), aggr)?
            .filter(binary_expr(col(GROUP_SIZE), Operator::GtEq, lit(k as i64)))?
            .project(original)?
            .build()?;
        Ok(Transformed::yes(rebuilt))
    })?;
    Ok(out.data)
}

/// Add Laplace noise to every aggregate over a noised column. Refuses a query that could
/// reveal the column other than through an aggregate. Returns the budgets touched.
fn noise_aggregates(
    plan: LogicalPlan,
    read: &[String],
    noised: &[(&str, f64, &str, f64)],
) -> Result<(LogicalPlan, Vec<(String, f64)>)> {
    let reads = |e: &Expr, c: &str| e.column_refs().iter().any(|r| r.name == c);
    if !has_aggregate(&plan) {
        let leaked: Vec<&str> = noised
            .iter()
            .map(|(c, ..)| *c)
            .filter(|c| read.iter().any(|r| r == c))
            .collect();
        if leaked.is_empty() {
            return Ok((plan, Vec::new()));
        }
        return Err(RuntimeError::Invalid(format!(
            "`{}` is released to this caller only through aggregates (SUM, AVG, COUNT, ...)",
            leaked.join("`, `")
        )));
    }
    let mut touched = Vec::new();
    let out = plan.transform_up_with_subqueries(|node| {
        let LogicalPlan::Aggregate(a) = &node else {
            return Ok(Transformed::no(node));
        };
        for (c, ..) in noised {
            if a.group_expr.iter().any(|g| reads(g, c)) {
                return Err(datafusion::error::DataFusionError::Plan(format!(
                    "`{c}` cannot be a grouping key for this caller"
                )));
            }
        }
        let n_group = a.group_expr.len();
        let mut changed = false;
        let mut proj = Vec::new();
        for (i, c) in a.schema.columns().iter().enumerate() {
            let base = Expr::Column(c.clone());
            let hit = (i >= n_group)
                .then(|| &a.aggr_expr[i - n_group])
                .and_then(|e| noised.iter().find(|(nc, ..)| reads(e, nc)));
            proj.push(match hit {
                Some((_, scale, budget, epsilon)) => {
                    changed = true;
                    touched.push((budget.to_string(), *epsilon));
                    laplace_udf()
                        .call(vec![cast(base, DataType::Float64), lit(*scale)])
                        .alias(c.name.clone())
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
    Ok((out.data, touched))
}
