//! The split pass: row-only subtrees of mixed rules are stored at write and read at query time.

use std::sync::Arc;

use datafusion::arrow::array::*;
use datafusion::arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use datafusion::arrow::util::pretty::pretty_format_batches;
use datafusion::catalog::TableProvider;
use datafusion::datasource::{MemTable, provider_as_source};
use datafusion::logical_expr::{Expr, LogicalPlanBuilder};
use parcel_core::{Compilation, ContractDoc, Registry, compile};
use parcel_runtime::Caller;
use parcel_runtime::differential::differential;
use parcel_runtime::plan::{param_values, session};

const CONTRACT: &str = r#"
contract: telco/subscribers
version: 1
binding: {parquet: subs/}
expose:
  - {name: id, type: int64}
  - {name: name, type: utf8}
  - {name: phone, type: utf8}
rules:
  - id: valid_or_cleared
    op: admit
    expr: "is_msisdn(row.phone) || ctx.clearance > 3"
  - id: region_tags
    op: admit
    expr: "row.tags.exists(t, t.startsWith('ke-')) || 'admin' in ctx.roles"
  - id: mask_name
    op: transform
    column: name
    expr: "ctx.tenant == 'safcom' ? row.name : redact(row.name)"
"#;

fn schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        Field::new("name", DataType::Utf8, true),
        Field::new("phone", DataType::Utf8, true),
        Field::new(
            "tags",
            DataType::List(Field::new_list_field(DataType::Utf8, true).into()),
            true,
        ),
    ]))
}

fn batch() -> RecordBatch {
    let n = 400;
    let mut tags = ListBuilder::new(StringBuilder::new());
    for i in 0..n {
        tags.values()
            .append_value(if i % 3 == 0 { "ke-nbo" } else { "ug-kla" });
        tags.append(true);
    }
    RecordBatch::try_new(
        schema(),
        vec![
            Arc::new(Int64Array::from((0..n).collect::<Vec<i64>>())),
            Arc::new(StringArray::from(
                (0..n)
                    .map(|i| {
                        if i % 11 == 0 {
                            None
                        } else {
                            Some(format!("Subscriber {i}"))
                        }
                    })
                    .collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                (0..n)
                    .map(|i| {
                        if i % 4 == 0 {
                            format!("0{}", 700000000 + i)
                        } else {
                            format!("2547{:08}", i)
                        }
                    })
                    .collect::<Vec<_>>(),
            )),
            Arc::new(tags.finish()),
        ],
    )
    .unwrap()
}

fn scan(data: Arc<dyn TableProvider>) -> LogicalPlanBuilder {
    LogicalPlanBuilder::scan("t", provider_as_source(data), None).unwrap()
}

/// The data as a writer stores it: raw columns plus every derived `_d_` column.
async fn stored_data(c: &Compilation) -> Arc<dyn TableProvider> {
    let raw = Arc::new(MemTable::try_new(schema(), vec![vec![batch()]]).unwrap());
    let mut cols: Vec<Expr> = schema()
        .fields()
        .iter()
        .map(|f| parcel_runtime::plan::col_ref(f.name()))
        .collect();
    cols.extend(
        c.contract
            .derived
            .iter()
            .map(|d| d.expr.clone().alias(&d.column)),
    );
    let plan =
        parcel_core::compile::resolve(scan(raw).project(cols).unwrap().build().unwrap()).unwrap();
    let df = session().execute_logical_plan(plan).await.unwrap();
    let schema = Arc::new(df.schema().as_arrow().clone());
    let batches = df.collect().await.unwrap();
    Arc::new(MemTable::try_new(schema, vec![batches]).unwrap())
}

/// What a caller sees: admits, then the projection, ordered by id.
async fn view(
    c: &Compilation,
    data: Arc<dyn TableProvider>,
    caller: &Caller,
    stored: bool,
) -> String {
    let cc = &c.contract;
    let (admits, projection) = if stored {
        (&cc.admits_stored, &cc.projection_stored)
    } else {
        (&cc.admits, &cc.projection)
    };
    let mut b = scan(data);
    if let Some(f) = admits.iter().map(|(_, e)| e.clone()).reduce(Expr::and) {
        b = b.filter(f).unwrap();
    }
    let b = b
        .project(projection.iter().map(|(n, e)| e.clone().alias(n)))
        .unwrap()
        .sort(vec![parcel_runtime::plan::col_ref("id").sort(true, false)])
        .unwrap();
    let plan = parcel_core::compile::resolve(b.build().unwrap())
        .unwrap()
        .with_param_values(param_values(cc, caller).unwrap())
        .unwrap();
    let batches = session()
        .execute_logical_plan(plan)
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    assert!(batches.iter().map(|b| b.num_rows()).sum::<usize>() > 0);
    pretty_format_batches(&batches).unwrap().to_string()
}

#[tokio::test]
async fn stored_and_live_agree() {
    let doc = ContractDoc::parse(CONTRACT).unwrap();
    let c = compile(&doc, &schema(), &Registry::builtin()).unwrap_or_else(|d| panic!("{d:#?}"));
    let derived: Vec<&str> = c.contract.derived.iter().map(|d| d.cel.as_str()).collect();
    assert_eq!(derived.len(), 3, "{derived:?}");
    assert!(derived.contains(&"is_msisdn(row.phone)"));
    assert!(derived.contains(&"redact(row.name)"));
    let report = &c.contract.report;
    assert!(
        report
            .iter()
            .find(|r| r.rule == "valid_or_cleared")
            .unwrap()
            .reason
            .contains("computed at write")
    );
    let stored_reads_d = c
        .contract
        .admits_stored
        .iter()
        .chain(&c.contract.projection_stored)
        .any(|(_, e)| e.to_string().contains("_d_"));
    assert!(stored_reads_d, "the stored variants read derived columns");

    let data = stored_data(&c).await;
    let callers = [
        Caller::new("a", "safcom", "analytics"),
        Caller::new("b", "airtel", "analytics").with_roles(&["admin"]),
        {
            let mut c = Caller::new("c", "airtel", "analytics");
            c.clearance = 5;
            c
        },
    ];
    for caller in &callers {
        let stored = view(&c, data.clone(), caller, true).await;
        let live = view(&c, data.clone(), caller, false).await;
        assert_eq!(stored, live, "caller {}", caller.tenant);
    }

    let diff = differential(&c, &batch(), &callers).await.unwrap();
    assert!(
        diff.passed(),
        "{:?}",
        &diff.mismatches[..diff.mismatches.len().min(5)]
    );
}
