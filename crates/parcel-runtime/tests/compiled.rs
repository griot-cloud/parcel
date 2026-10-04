//! A compiled contract as bytes and back: the same bytes, the same compilation, and it runs.

use std::sync::Arc;

use datafusion::arrow::array::*;
use datafusion::arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use datafusion::datasource::MemTable;
use parcel_core::compile::{Compilation, PARCEL_VERSION};
use parcel_core::{ContractDoc, Registry, compile, compile_with};
use parcel_runtime::Caller;
use parcel_runtime::compiled::{CompiledBytes, FORMAT};
use parcel_runtime::differential::differential;
use parcel_runtime::plan::validate;

const LOANS: &str = r#"
contract: credit/loans
version: 3
owner: bank
binding: {parquet: loans/, partitioned_by: [region]}
residency: {jurisdictions: [KE], approved_output_may_leave: false}
expose:
  - {name: loan_id, type: int64}
  - {name: amount, type: float64}
  - {name: phone, type: utf8}
  - {name: score, type: utf8}
extensions:
  row: {risk_band: utf8, flagged: bool}
  ctx: {department: utf8, max_amount: float64}
  dataset: {source_system: utf8, median_amount: float64}
enrich:
  - {field: risk_band, expr: "row.score >= 700 ? 'low' : (row.score >= 500 ? 'medium' : 'high')"}
  - {field: flagged, expr: "row.other.risk_band == 'high' && row.amount > 5000.0"}
dataset_other:
  - {field: source_system, value: "mpesa-loans-v2"}
  - {field: median_amount, expr: "median(row.amount)"}
rules:
  - {id: analysts, op: decide, expr: "ctx.purpose in ['analytics', 'audit']"}
  - {id: risk_or_low, op: admit, expr: "ctx.other.department == 'risk' || row.other.risk_band == 'low'"}
  - {id: within_limit, op: admit, expr: "row.amount <= ctx.other.max_amount"}
  - {id: kenyan, op: admit, expr: "row.phone.matches('^254[17][0-9]{8}$') || ctx.clearance >= 3"}
  - {id: banded, op: assert, expr: "has(row.other.risk_band)", on_fail: report}
  - {id: positive, op: assert, expr: "row.amount > 0.0", on_fail: drop}
  - {id: known_source, op: guarantee, expr: "dataset.other.source_system == 'mpesa-loans-v2'", on_fail: deny}
  - {id: sane, op: guarantee, expr: "dataset.other.median_amount > 100.0 && dataset.row_count > 10", on_fail: annotate}
  - {id: masked_phone, op: transform, column: phone, expr: "ctx.clearance >= 2 ? row.phone : redact(row.phone)"}
  - {id: score_text, op: transform, column: score, expr: "string(row.score)"}
  - {id: small_cells, op: shape, operator: suppress, params: {k: 5}, unless: "ctx.purpose == 'audit'"}
"#;

fn schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("loan_id", DataType::Int64, false),
        Field::new("region", DataType::Utf8, false),
        Field::new("amount", DataType::Float64, true),
        Field::new("phone", DataType::Utf8, true),
        Field::new("score", DataType::Int64, true),
    ]))
}

fn batch() -> RecordBatch {
    let n = 60i64;
    RecordBatch::try_new(
        schema(),
        vec![
            Arc::new(Int64Array::from((0..n).collect::<Vec<_>>())),
            Arc::new(StringArray::from(
                (0..n)
                    .map(|i| ["ke-n", "ke-s"][(i % 2) as usize])
                    .collect::<Vec<_>>(),
            )),
            Arc::new(Float64Array::from(
                (0..n)
                    .map(|i| (i % 7 != 0).then_some(150.0 + (i * 97 % 90) as f64 * 100.0))
                    .collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                (0..n)
                    .map(|i| (i % 5 != 0).then(|| format!("2547{:08}", i * 1234567 % 100_000_000)))
                    .collect::<Vec<_>>(),
            )),
            Arc::new(Int64Array::from(
                (0..n)
                    .map(|i| (i % 11 != 0).then_some(300 + (i * 13 % 550)))
                    .collect::<Vec<_>>(),
            )),
        ],
    )
    .unwrap()
}

fn callers() -> Vec<Caller> {
    let caller = |department: &str, max: f64, clearance: i64| {
        let mut c = Caller::new("u", "bank", "analytics");
        c.other
            .insert("department".into(), serde_json::json!(department));
        c.other.insert("max_amount".into(), serde_json::json!(max));
        c.clearance = clearance;
        c
    };
    vec![
        caller("risk", 1e9, 3),
        caller("sales", 4000.0, 0),
        caller("sales", 9000.0, 2),
    ]
}

/// Bytes back to the same compilation, and the same compilation to the same bytes.
fn round_trip(c: &Compilation) -> Compilation {
    let bytes = c.to_bytes().unwrap();
    let back = Compilation::from_bytes(&bytes).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(back.to_bytes().unwrap(), bytes, "bytes are not stable");
    // Every readable field, expressions as text included.
    assert_eq!(
        serde_json::to_value(&back).unwrap(),
        serde_json::to_value(c).unwrap()
    );
    let (a, b) = (&c.contract, &back.contract);
    assert_eq!(a.admits, b.admits);
    assert_eq!(a.projection, b.projection);
    assert_eq!(a.admits_stored, b.admits_stored);
    assert_eq!(a.projection_stored, b.projection_stored);
    assert_eq!(a.flags, b.flags);
    assert_eq!(a.derived, b.derived);
    assert_eq!(a.enrich, b.enrich);
    assert_eq!(a.row_schema, b.row_schema);
    assert_eq!(a.exposed_schema, b.exposed_schema);
    assert_eq!(a.scan_schema, b.scan_schema);
    assert_eq!(a.residency, b.residency);
    assert_eq!(a.binding, b.binding);
    assert_eq!(c.write.flags, back.write.flags);
    assert_eq!(c.write.derived, back.write.derived);
    assert_eq!(c.write.enrich, back.write.enrich);
    assert_eq!(c.validation.plan, back.validation.plan);
    back
}

#[tokio::test]
async fn a_compiled_contract_round_trips_and_runs_as_compiled() {
    let doc = ContractDoc::parse(LOANS).unwrap();
    let c = compile(&doc, &schema(), &Registry::builtin()).unwrap_or_else(|d| panic!("{d:#?}"));
    assert!(!c.contract.derived.is_empty(), "the regex admit is stored");
    assert!(!c.contract.enrich.is_empty());
    let back = round_trip(&c);

    let table = |b| Arc::new(MemTable::try_new(schema(), vec![vec![b]]).unwrap());
    let want = validate(&c, c.validation.plan.clone(), table(batch()))
        .await
        .unwrap();
    let got = validate(&back, back.validation.plan.clone(), table(batch()))
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(&got.stats).unwrap(),
        serde_json::to_value(&want.stats).unwrap()
    );
    assert_eq!(got.failures, want.failures);
    assert_eq!(got.breached, want.breached);

    let diff = differential(&back, &batch(), &callers()).await.unwrap();
    assert!(diff.passed(), "{diff:?}");
    assert!(diff.evaluations > 0);
}

#[test]
fn inherited_contracts_round_trip() {
    let parent = ContractDoc::parse("contract: demo/parent\nversion: 1\nbinding: {iceberg: lake.sales.orders}\nexpose: [{name: id, type: int64}]\nresidency: {jurisdictions: [KE, EU], approved_output_may_leave: true}\nrules:\n  - {id: mine, op: admit, expr: 'row.id > 0'}").unwrap();
    let child = ContractDoc::parse("contract: demo/child\nversion: 2\ninherits: demo/parent\nresidency: {jurisdictions: [KE], approved_output_may_leave: false}").unwrap();
    let schema = Schema::new(vec![Field::new("id", DataType::Int64, false)]);
    let c = compile_with(&child, &schema, &Registry::builtin(), &|_| {
        Some(parent.clone())
    })
    .unwrap();
    let back = round_trip(&c);
    assert!(back.contract.residency.permits("KE"));
    assert!(!back.contract.residency.permits("EU"));
}

#[test]
fn another_parcel_version_or_format_is_refused_naming_both() {
    let doc = ContractDoc::parse(LOANS).unwrap();
    let c = compile(&doc, &schema(), &Registry::builtin()).unwrap();
    let mut v: serde_json::Value = serde_json::from_slice(&c.to_bytes().unwrap()).unwrap();
    v["parcel_version"] = "0.0.1".into();
    let err = Compilation::from_bytes(&serde_json::to_vec(&v).unwrap()).unwrap_err();
    assert!(
        err.contains("parcel 0.0.1") && err.contains(&format!("parcel {PARCEL_VERSION}")),
        "{err}"
    );
    v["parcel_version"] = PARCEL_VERSION.into();
    v["format"] = "parcel-compiled/0".into();
    let err = Compilation::from_bytes(&serde_json::to_vec(&v).unwrap()).unwrap_err();
    assert!(
        err.contains("parcel-compiled/0") && err.contains(FORMAT),
        "{err}"
    );
}

#[test]
fn a_broken_expression_is_refused_naming_it() {
    let doc = ContractDoc::parse(LOANS).unwrap();
    let c = compile(&doc, &schema(), &Registry::builtin()).unwrap();
    let mut v: serde_json::Value = serde_json::from_slice(&c.to_bytes().unwrap()).unwrap();
    v["contract"]["admits"][1]["expr"] = "00ff".into();
    let err = Compilation::from_bytes(&serde_json::to_vec(&v).unwrap()).unwrap_err();
    assert!(err.contains("admit/within_limit"), "{err}");
}

#[cfg(feature = "wasm")]
#[tokio::test]
async fn user_functions_travel_inside_the_bytes() {
    use parcel_core::registry::{Cost, FunctionManifest};
    use parcel_runtime::wasm::install;
    let module = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/meter_serial.wasm"
    ))
    .unwrap();
    let manifest = FunctionManifest {
        name: "is_meter_serial".into(),
        version: 1,
        signatures: vec!["(string) -> bool".into()],
        deterministic: true,
        cost: Cost::Moderate,
    };
    let mut registry = Registry::builtin();
    registry.insert(install(&module, &manifest, "utility").unwrap());
    let doc = ContractDoc::parse("contract: utility/meters\nversion: 1\nowner: utility\nbinding: {parquet: meters/}\nexpose: [{name: serial, type: utf8}]\nrules:\n  - {id: valid, op: admit, expr: 'is_meter_serial(row.serial)'}").unwrap();
    let schema = Schema::new(vec![Field::new("serial", DataType::Utf8, true)]);
    let c = compile(&doc, &schema, &registry).unwrap_or_else(|d| panic!("{d:#?}"));
    assert_eq!(c.contract.functions.len(), 1);
    round_trip(&c);
}
