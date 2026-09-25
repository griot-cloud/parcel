//! Shapes compiled into the view (sample, row noise) and applied to the caller's plan
//! (suppress, aggregate noise).

use std::sync::Arc;

use datafusion::arrow::array::*;
use datafusion::arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use datafusion::catalog::view::ViewTable;
use datafusion::datasource::{MemTable, provider_as_source};
use datafusion::logical_expr::{Expr, LogicalPlan, LogicalPlanBuilder};
use datafusion::prelude::SessionContext;
use parcel_core::{Compilation, ContractDoc, Registry, compile};
use parcel_runtime::Caller;
use parcel_runtime::plan::{active_shapes, param_values, session};
use parcel_runtime::shape::{Charge, apply, suppress_ungrouped};

const PAYROLL: &str = r#"
contract: hr/payroll
version: 1
binding: {parquet: payroll/}
expose:
  - {name: id, type: int64}
  - {name: dept, type: utf8}
  - {name: salary, type: int64}
  - {name: bonus, type: float64}
rules:
  - {id: sampled, op: shape, operator: sample, params: {fraction: 0.25, key: id}, unless: "ctx.tenant == 'hr'"}
  - {id: noisy_salary, op: shape, operator: noise, column: salary, params: {sensitivity: 1000, epsilon: 1.0, budget: payroll, at: row}, unless: "ctx.tenant == 'hr'"}
  - {id: noisy_bonus, op: shape, operator: noise, column: bonus, params: {sensitivity: 10, epsilon: 0.5, budget: bonus}, unless: "ctx.tenant == 'hr'"}
  - {id: small_cells, op: shape, operator: suppress, params: {k: 30}, unless: "ctx.tenant == 'hr'"}
"#;

fn schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        Field::new("dept", DataType::Utf8, false),
        Field::new("salary", DataType::Int64, false),
        Field::new("bonus", DataType::Float64, false),
    ]))
}

fn batch() -> RecordBatch {
    let n = 2000i64;
    let depts = ["eng", "ops", "sales", "legal"];
    RecordBatch::try_new(
        schema(),
        vec![
            Arc::new(Int64Array::from((0..n).collect::<Vec<_>>())),
            Arc::new(StringArray::from(
                // legal is small: 20 of 2000 rows.
                (0..n)
                    .map(|i| {
                        if i % 100 == 0 {
                            depts[3]
                        } else {
                            depts[(i % 3) as usize]
                        }
                    })
                    .collect::<Vec<_>>(),
            )),
            Arc::new(Int64Array::from(
                (0..n).map(|i| 50_000 + i * 10).collect::<Vec<_>>(),
            )),
            Arc::new(Float64Array::from(
                (0..n).map(|i| (i % 7) as f64).collect::<Vec<_>>(),
            )),
        ],
    )
    .unwrap()
}

fn compiled() -> Compilation {
    let doc = ContractDoc::parse(PAYROLL).unwrap();
    compile(&doc, &schema(), &Registry::builtin()).unwrap_or_else(|d| panic!("{d:#?}"))
}

/// The view a caller sees, registered as `payroll` on a session with parcel's functions.
fn session_for(c: &Compilation, caller: &Caller) -> SessionContext {
    let cc = &c.contract;
    let data = Arc::new(MemTable::try_new(schema(), vec![vec![batch()]]).unwrap());
    let mut b = LogicalPlanBuilder::scan("raw", provider_as_source(data), None).unwrap();
    if let Some(f) = cc.admits.iter().map(|(_, e)| e.clone()).reduce(Expr::and) {
        b = b.filter(f).unwrap();
    }
    let view = b
        .project(cc.projection.iter().map(|(n, e)| e.clone().alias(n)))
        .unwrap()
        .build()
        .unwrap();
    let view = parcel_core::compile::resolve(view)
        .unwrap()
        .with_param_values(param_values(cc, caller).unwrap())
        .unwrap();
    let ctx = session();
    ctx.register_table("payroll", Arc::new(ViewTable::new(view, None)))
        .unwrap();
    ctx
}

async fn run(
    c: &Compilation,
    caller: &Caller,
    sql: &str,
) -> Result<(Vec<RecordBatch>, Vec<Charge>), String> {
    let ctx = session_for(c, caller);
    let plan: LogicalPlan = ctx
        .state()
        .create_logical_plan(sql)
        .await
        .map_err(|e| e.to_string())?;
    let optimized = ctx.state().optimize(&plan).map_err(|e| e.to_string())?;
    let active = active_shapes(&c.contract, caller).unwrap();
    let shaped = apply(plan, &optimized, &active).map_err(|e| e.to_string())?;
    let mut out = ctx
        .execute_logical_plan(shaped.plan)
        .await
        .map_err(|e| e.to_string())?
        .collect()
        .await
        .map_err(|e| e.to_string())?;
    if let (Some(k), false) = (shaped.suppress_k, shaped.has_aggregate) {
        out = suppress_ungrouped(out, k);
    }
    Ok((out, shaped.charges))
}

fn rows(b: &[RecordBatch]) -> usize {
    b.iter().map(|b| b.num_rows()).sum()
}

fn i64s(b: &[RecordBatch], col: usize) -> Vec<i64> {
    b.iter()
        .flat_map(|b| {
            let a = b
                .column(col)
                .as_any()
                .downcast_ref::<Int64Array>()
                .unwrap()
                .clone();
            a.values().to_vec()
        })
        .collect()
}

#[tokio::test]
async fn sample_is_stable_and_waived_by_unless() {
    let c = compiled();
    let analyst = Caller::new("a", "globex", "analytics");
    let (first, _) = run(&c, &analyst, "SELECT id FROM payroll ORDER BY id")
        .await
        .unwrap();
    let (second, _) = run(&c, &analyst, "SELECT id FROM payroll ORDER BY id")
        .await
        .unwrap();
    let n = rows(&first);
    assert!((350..650).contains(&n), "about a quarter of 2000, got {n}");
    assert_eq!(
        i64s(&first, 0),
        i64s(&second, 0),
        "the same sample each time"
    );
    let (all, _) = run(
        &c,
        &Caller::new("h", "hr", "analytics"),
        "SELECT id FROM payroll",
    )
    .await
    .unwrap();
    assert_eq!(rows(&all), 2000);
}

#[tokio::test]
async fn row_noise_changes_values_and_charges_only_readers() {
    let c = compiled();
    let analyst = Caller::new("a", "globex", "analytics");
    let (noisy, charges) = run(&c, &analyst, "SELECT id, salary FROM payroll ORDER BY id")
        .await
        .unwrap();
    let ids = i64s(&noisy, 0);
    let salaries = i64s(&noisy, 1);
    let changed = ids
        .iter()
        .zip(&salaries)
        .filter(|(id, s)| **s != 50_000 + **id * 10)
        .count();
    assert!(
        changed as f64 > 0.9 * ids.len() as f64,
        "{changed} of {} changed",
        ids.len()
    );
    assert_eq!(
        charges,
        vec![Charge {
            budget: "payroll".into(),
            epsilon: 1.0
        }]
    );

    let (_, charges) = run(&c, &analyst, "SELECT id FROM payroll").await.unwrap();
    assert!(
        charges.is_empty(),
        "a query that does not read salary spends nothing"
    );

    let (exact, charges) = run(
        &c,
        &Caller::new("h", "hr", "analytics"),
        "SELECT id, salary FROM payroll ORDER BY id",
    )
    .await
    .unwrap();
    assert!(
        i64s(&exact, 0)
            .iter()
            .zip(i64s(&exact, 1))
            .all(|(id, s)| s == 50_000 + id * 10)
    );
    assert!(charges.is_empty());
}

#[tokio::test]
async fn aggregate_noise_and_suppression() {
    let c = compiled();
    let analyst = Caller::new("a", "globex", "analytics");
    let sql =
        "SELECT dept, SUM(bonus) AS b, COUNT(*) AS n FROM payroll GROUP BY dept ORDER BY dept";
    let (out, charges) = run(&c, &analyst, sql).await.unwrap();
    let depts: Vec<String> = out
        .iter()
        .flat_map(|b| {
            let a = b
                .column(0)
                .as_any()
                .downcast_ref::<StringArray>()
                .unwrap()
                .clone();
            (0..a.len())
                .map(|i| a.value(i).to_owned())
                .collect::<Vec<_>>()
        })
        .collect();
    assert!(
        !depts.contains(&"legal".to_owned()),
        "legal has 5 sampled rows, under k: {depts:?}"
    );
    assert!(charges.contains(&Charge {
        budget: "bonus".into(),
        epsilon: 0.5
    }));

    let refused = run(&c, &analyst, "SELECT bonus FROM payroll")
        .await
        .unwrap_err();
    assert!(refused.contains("only through aggregates"), "{refused}");
    let refused = run(
        &c,
        &analyst,
        "SELECT bonus, COUNT(*) FROM payroll GROUP BY bonus",
    )
    .await
    .unwrap_err();
    assert!(refused.contains("grouping key"), "{refused}");

    // One row is one group: under k, nothing comes back.
    let (one, _) = run(&c, &analyst, "SELECT id FROM payroll WHERE id < 40")
        .await
        .unwrap();
    assert_eq!(rows(&one), 0);
}
