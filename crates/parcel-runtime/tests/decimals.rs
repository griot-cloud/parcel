//! Decimals in rules: exact on both engines, checked by the differential test.

use std::sync::Arc;

use datafusion::arrow::array::*;
use datafusion::arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use datafusion::datasource::MemTable;
use parcel_core::{Code, ContractDoc, Registry, compile};
use parcel_runtime::Caller;
use parcel_runtime::differential::differential;
use parcel_runtime::plan::validate;

const PAYMENTS: &str = r#"
contract: pay/mpesa
version: 1
binding: {parquet: mpesa/}
expose:
  - {name: id, type: int64}
  - {name: amount, type: 'decimal(12,2)'}
  - {name: net, type: 'decimal(12,2)'}
extensions:
  dataset: {total: 'decimal(18,2)', mean: float64}
dataset_other:
  - {field: total, expr: "sum(row.amount)"}
  - {field: mean, expr: "avg(row.amount)"}
rules:
  - {id: consistent, op: assert, expr: "row.amount == row.unit_price * row.qty", on_fail: report}
  - {id: above_min, op: assert, expr: "row.amount > 100.50", on_fail: report}
  - {id: with_fee, op: assert, expr: "row.amount + row.fee <= 70000.00", on_fail: report}
  - {id: vat, op: assert, expr: "row.amount * 1.16 < 50000", on_fail: report}
  - {id: as_double, op: assert, expr: "double(row.amount) / 2.0 > 1000.0", on_fail: report}
  - {id: truncated, op: assert, expr: "int(row.amount) % 2 == 0", on_fail: report}
  - {id: listed, op: assert, expr: "row.amount in [150.00, 250.50, -12.34]", on_fail: report}
  - {id: negated, op: assert, expr: "-row.amount < 0.0", on_fail: report}
  - {id: squares, op: assert, expr: "row.fee * row.fee > 1.0000", on_fail: report}
  - {id: positive_or_admin, op: admit, expr: "row.amount >= 0 || 'admin' in ctx.roles"}
  - {id: net_of_fee, op: transform, column: net, expr: "ctx.tenant == 'safcom' ? row.net : row.net - row.fee"}
  - {id: bounded, op: guarantee, expr: "dataset.amount.max < 1000000.00 && dataset.amount.min > -100.00", on_fail: deny}
  - {id: big_total, op: guarantee, expr: "dataset.other.total > 1000.00 && dataset.other.mean > 1.0", on_fail: annotate}
"#;

fn schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        Field::new("amount", DataType::Decimal128(12, 2), true),
        Field::new("unit_price", DataType::Decimal128(12, 2), true),
        Field::new("qty", DataType::Int32, true),
        Field::new("fee", DataType::Decimal128(10, 2), true),
        Field::new("net", DataType::Decimal128(12, 2), true),
    ]))
}

fn batch() -> RecordBatch {
    let n = 200i64;
    let dec = |v: Vec<Option<i128>>, p: u8| {
        Decimal128Array::from(v)
            .with_precision_and_scale(p, 2)
            .unwrap()
    };
    let unit: Vec<Option<i128>> = (0..n)
        .map(|i| Some(1_000 + (i * 7919 % 50_000) as i128))
        .collect();
    let qty: Vec<Option<i32>> = (0..n)
        .map(|i| {
            if i % 17 == 0 {
                None
            } else {
                Some(1 + (i % 5) as i32)
            }
        })
        .collect();
    let amount: Vec<Option<i128>> = (0..n)
        .map(|i| match i % 23 {
            0 => None,
            1 => Some(-1234),
            2 => Some(15_000),
            3 => Some(25_050),
            _ => Some(
                unit[i as usize].unwrap() * qty[i as usize].unwrap_or(1) as i128
                    + if i % 7 == 0 { 1 } else { 0 },
            ),
        })
        .collect();
    let fee: Vec<Option<i128>> = (0..n)
        .map(|i| {
            if i % 13 == 0 {
                None
            } else {
                Some((i % 9) as i128 * 55)
            }
        })
        .collect();
    RecordBatch::try_new(
        schema(),
        vec![
            Arc::new(Int64Array::from((0..n).collect::<Vec<_>>())),
            Arc::new(dec(amount.clone(), 12)),
            Arc::new(dec(unit, 12)),
            Arc::new(Int32Array::from(qty)),
            Arc::new(dec(fee, 10)),
            Arc::new(dec(amount, 12)),
        ],
    )
    .unwrap()
}

#[tokio::test]
async fn decimals_agree_between_engines() {
    let doc = ContractDoc::parse(PAYMENTS).unwrap();
    let c = compile(&doc, &schema(), &Registry::builtin()).unwrap_or_else(|d| panic!("{d:#?}"));
    let callers = [
        Caller::new("a", "safcom", "analytics"),
        Caller::new("b", "bank", "analytics").with_roles(&["admin"]),
    ];
    let diff = differential(&c, &batch(), &callers).await.unwrap();
    let shown: Vec<String> = diff
        .mismatches
        .iter()
        .take(12)
        .map(|m| {
            format!(
                "{} row {}: cel={} df={}",
                m.rule, m.row, m.reference, m.datafusion
            )
        })
        .collect();
    assert!(
        diff.passed(),
        "{} mismatches:\n{}",
        diff.mismatches.len(),
        shown.join("\n")
    );
    assert_eq!(diff.both_errored, 0);

    let data = Arc::new(MemTable::try_new(schema(), vec![vec![batch()]]).unwrap());
    let v = validate(&c, c.validation.plan.clone(), data).await.unwrap();
    assert!(v.valid, "{:?}", v.breached);
    // Every assert splits the rows: the agreement above is not all-true or all-false.
    for (rule, fails) in &v.failures {
        assert!(
            *fails > 0 && *fails < v.row_count,
            "{rule}: {fails} of {} fail",
            v.row_count
        );
    }
    assert!(
        v.stats["amount__max"].is_string(),
        "decimals are stored as exact text: {}",
        v.stats["amount__max"]
    );
    assert!(v.stats["other__total"].as_str().unwrap().contains('.'));
}

#[test]
fn inexact_decimal_rules_are_refused() {
    let base = "contract: t\nversion: 1\nbinding: {parquet: x}\nexpose: [{name: id, type: int64}]\nrules:\n";
    let codes = |rule: &str| -> Vec<Code> {
        let doc = ContractDoc::parse(&format!("{base}  - {rule}\n")).unwrap();
        compile(&doc, &schema(), &Registry::builtin())
            .err()
            .unwrap_or_default()
            .into_iter()
            .map(|d| d.code)
            .collect()
    };
    // A literal with more places than the column's scale.
    assert!(
        codes(r#"{id: a, op: assert, expr: "row.amount > 1.005", on_fail: report}"#)
            .contains(&Code::TypeMismatch)
    );
    // Division.
    assert!(
        codes(r#"{id: a, op: assert, expr: "row.amount / 2 > 1.00", on_fail: report}"#)
            .contains(&Code::OutsideProfile)
    );
    // Mixed scales.
    assert!(
        codes(r#"{id: a, op: assert, expr: "row.amount * row.fee > row.amount", on_fail: report}"#)
            .contains(&Code::TypeMismatch)
    );
    // A decimal against a non-literal double.
    assert!(
        codes(r#"{id: a, op: assert, expr: "row.amount > double(row.qty)", on_fail: report}"#)
            .contains(&Code::TypeMismatch)
    );
}
