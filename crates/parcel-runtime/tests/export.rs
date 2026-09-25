//! The validation plan exported as SQL reproduces the verdict, and parses in other dialects.

use std::sync::Arc;

use datafusion::arrow::array::*;
use datafusion::arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use datafusion::datasource::MemTable;
use datafusion::prelude::SessionContext;
use datafusion::sql::sqlparser::dialect::{DuckDbDialect, PostgreSqlDialect};
use datafusion::sql::sqlparser::parser::Parser;
use parcel_core::{Compilation, ContractDoc, Registry, compile};
use parcel_runtime::export::validation_sql;
use parcel_runtime::plan::{Verdict, validate};

async fn verdict_of(c: &Compilation) -> Verdict {
    let data = Arc::new(MemTable::try_new(schema(), vec![vec![batch()]]).unwrap());
    validate(c, c.validation.plan.clone(), data).await.unwrap()
}

const CONTRACT: &str = r#"
contract: pay/transfers
version: 1
binding: {parquet: transfers/}
expose:
  - {name: id, type: int64}
  - {name: amount, type: int64}
rules:
  - {id: id_present, op: assert, expr: "has(row.id)", on_fail: deny}
  - {id: positive, op: assert, expr: "row.amount > 0", on_fail: drop}
  - {id: phone_ok, op: assert, expr: "row.phone.matches('^2547[0-9]{8}$')", on_fail: report}
  - {id: few_nulls, op: guarantee, expr: "dataset.amount.null_rate < 0.1", on_fail: deny}
  - {id: enough, op: guarantee, expr: "dataset.row_count > 10 && dataset.assertions.phone_ok.pass_rate > 0.5", on_fail: annotate}
"#;

fn schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int64, true),
        Field::new("amount", DataType::Int64, true),
        Field::new("phone", DataType::Utf8, true),
    ]))
}

fn batch() -> RecordBatch {
    RecordBatch::try_new(
        schema(),
        vec![
            Arc::new(Int64Array::from((0..100).map(Some).collect::<Vec<_>>())),
            Arc::new(Int64Array::from(
                (0..100)
                    .map(|i| if i % 20 == 0 { None } else { Some(i * 10 - 50) })
                    .collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                (0..100)
                    .map(|i| {
                        if i % 7 == 0 {
                            format!("07{i:08}")
                        } else {
                            format!("2547{i:08}")
                        }
                    })
                    .collect::<Vec<_>>(),
            )),
        ],
    )
    .unwrap()
}

#[tokio::test]
async fn exported_sql_reproduces_the_verdict() {
    let comp = compile(
        &ContractDoc::parse(CONTRACT).unwrap(),
        &schema(),
        &Registry::builtin(),
    )
    .unwrap();
    let verdict = verdict_of(&comp).await;

    let export = validation_sql(&comp, "datafusion", "transfers").unwrap();
    assert!(export.warnings.is_empty());
    let ctx = SessionContext::new();
    ctx.register_table(
        "transfers",
        Arc::new(MemTable::try_new(schema(), vec![vec![batch()]]).unwrap()),
    )
    .unwrap();
    let rows = ctx
        .sql(&export.sql)
        .await
        .unwrap_or_else(|e| panic!("{e}\n{}", export.sql))
        .collect()
        .await
        .unwrap();
    let b = &rows[0];
    let get = |name: &str| {
        let i = b.schema().index_of(name).unwrap();
        datafusion::arrow::util::display::array_value_to_string(b.column(i), 0).unwrap()
    };
    assert_eq!(get("valid"), verdict.valid.to_string());
    assert_eq!(get("row_count"), verdict.row_count.to_string());
    for (a, fails) in &verdict.failures {
        assert_eq!(get(&format!("fail__{a}")), fails.to_string(), "{a}");
    }

    for (dialect, parse) in [
        (
            "duckdb",
            Box::new(|s: &str| Parser::parse_sql(&DuckDbDialect {}, s).map(|_| ()))
                as Box<dyn Fn(&str) -> _>,
        ),
        (
            "postgres",
            Box::new(|s: &str| Parser::parse_sql(&PostgreSqlDialect {}, s).map(|_| ())),
        ),
    ] {
        let e = validation_sql(&comp, dialect, "public.transfers").unwrap();
        parse(&e.sql).unwrap_or_else(|err| panic!("{dialect}: {err}\n{}", e.sql));
        assert!(e.sql.contains("transfers"));
        if dialect == "postgres" {
            assert!(
                e.warnings.iter().any(|w| w.starts_with("regexp_like")),
                "{:?}",
                e.warnings
            );
        } else {
            assert!(e.sql.contains("regexp_matches"), "{}", e.sql);
        }
    }
    assert!(validation_sql(&comp, "cobol", "t").is_err());

    // DuckDB: rates divide doubles with `/`, never DuckDB's integer `//`.
    let duck = validation_sql(&comp, "duckdb", "t").unwrap();
    assert!(!duck.sql.contains("//"), "{}", duck.sql);
}

#[test]
fn duckdb_integer_division_truncates_explicitly() {
    let src = r#"
contract: t/div
version: 1
binding: {parquet: x/}
expose: [{name: id, type: int64}]
rules:
  - {id: halves, op: assert, expr: "row.amount / 2 >= 0", on_fail: report}
"#;
    let doc = ContractDoc::parse(src).unwrap();
    let c = compile(&doc, &schema(), &Registry::builtin()).unwrap();
    let duck = validation_sql(&c, "duckdb", "t").unwrap();
    assert!(duck.sql.contains("trunc("), "{}", duck.sql);
    assert!(duck.warnings.iter().any(|w| w.contains("integer division")));
    Parser::parse_sql(&DuckDbDialect {}, &duck.sql).unwrap();
}

#[cfg(feature = "substrait")]
#[tokio::test]
async fn substrait_round_trip_reproduces_the_verdict() {
    use datafusion_substrait::logical_plan::consumer::from_substrait_plan;
    use datafusion_substrait::substrait::proto::Plan;
    use prost::Message;

    let comp = compile(
        &ContractDoc::parse(CONTRACT).unwrap(),
        &schema(),
        &Registry::builtin(),
    )
    .unwrap();
    let verdict = verdict_of(&comp).await;

    let (bytes, _warnings) =
        parcel_runtime::export::validation_substrait(&comp, "transfers").unwrap();
    let plan = Plan::decode(bytes.as_slice()).unwrap();
    let ctx = SessionContext::new();
    ctx.register_table(
        "transfers",
        Arc::new(MemTable::try_new(schema(), vec![vec![batch()]]).unwrap()),
    )
    .unwrap();
    let logical = from_substrait_plan(&ctx.state(), &plan).await.unwrap();
    let rows = ctx
        .execute_logical_plan(logical)
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    let b = &rows[0];
    let get = |name: &str| {
        let i = b
            .schema()
            .index_of(name)
            .unwrap_or_else(|_| panic!("{name} in {:?}", b.schema()));
        datafusion::arrow::util::display::array_value_to_string(b.column(i), 0).unwrap()
    };
    assert_eq!(get("valid"), verdict.valid.to_string());
    for (a, fails) in &verdict.failures {
        assert_eq!(get(&format!("fail__{a}")), fails.to_string(), "{a}");
    }
}
