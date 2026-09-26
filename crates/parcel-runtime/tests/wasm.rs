//! Tenants' WebAssembly functions: registered, pinned, sandboxed, used by contracts.
#![cfg(feature = "wasm")]

use std::sync::Arc;

use datafusion::arrow::array::*;
use datafusion::arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use datafusion::datasource::MemTable;
use parcel_core::registry::FunctionEntry;
use parcel_core::registry::FunctionManifest;
use parcel_core::{ContractDoc, Registry, compile};
use parcel_runtime::Caller;
use parcel_runtime::bundle::{Bundle, BundledFunction};
use parcel_runtime::differential::differential;
use parcel_runtime::plan::{selectivity, validate};
use parcel_runtime::wasm::install;

const MODULE: &[u8] = include_bytes!("fixtures/meter_serial.wasm");

fn manifest(name: &str, sig: &str) -> FunctionManifest {
    yaml_serde::from_str(&format!(
        "name: {name}\nversion: 1\nsignatures: [\"{sig}\"]\n"
    ))
    .unwrap()
}

const FUNCTIONS: [(&str, &str); 3] = [
    ("is_meter_serial", "(string) -> bool"),
    ("units", "(int, int) -> int"),
    ("county", "(string) -> string"),
];

/// Load kplc's functions; the registry a kplc contract compiles against.
fn kplc_registry() -> Registry {
    let mut r = Registry::builtin();
    for (name, sig) in FUNCTIONS {
        let e: FunctionEntry = install(MODULE, &manifest(name, sig), "kplc").unwrap();
        r.insert(e);
    }
    r
}

const TOKENS: &str = r#"
contract: kplc/tokens
version: 1
owner: kplc
binding: {parquet: tokens/}
expose:
  - {name: token_id, type: int64}
  - {name: meter, type: utf8}
  - {name: amount_cents, type: int64}
extensions:
  row: {units: int64}
enrich:
  - {field: units, expr: "units(row.amount_cents, row.tariff_cents)"}
rules:
  - {id: valid_meter, op: assert, expr: "is_meter_serial(row.meter)", on_fail: drop}
  - {id: some_units, op: assert, expr: "row.other.units > 0", on_fail: report}
  - {id: own_county, op: admit, expr: "county(row.meter) == ctx.tier || 'admin' in ctx.roles"}
  - {id: mask_meter, op: transform, column: meter, expr: "'admin' in ctx.roles ? row.meter : county(row.meter)"}
"#;

fn schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("token_id", DataType::Int64, false),
        Field::new("meter", DataType::Utf8, true),
        Field::new("amount_cents", DataType::Int64, true),
        Field::new("tariff_cents", DataType::Int64, true),
    ]))
}

fn batch() -> RecordBatch {
    let n = 240i64;
    RecordBatch::try_new(
        schema(),
        vec![
            Arc::new(Int64Array::from((0..n).collect::<Vec<_>>())),
            Arc::new(StringArray::from(
                (0..n)
                    .map(|i| match i % 9 {
                        0 => None,
                        1 => Some("MK47ABC".to_owned()),
                        _ => Some(format!(
                            "MK{}{:08}",
                            ["47", "01", "30"][(i % 3) as usize],
                            i
                        )),
                    })
                    .collect::<Vec<_>>(),
            )),
            Arc::new(Int64Array::from(
                (0..n).map(|i| Some(5_000 + i * 250)).collect::<Vec<_>>(),
            )),
            Arc::new(Int64Array::from(
                (0..n)
                    .map(|i| if i % 11 == 0 { Some(0) } else { Some(2_300) })
                    .collect::<Vec<_>>(),
            )),
        ],
    )
    .unwrap()
}

fn caller(county: &str) -> Caller {
    let mut c = Caller::new("u", "kplc", "analytics");
    c.tier = county.into();
    c
}

#[tokio::test]
async fn user_functions_end_to_end() {
    let registry = kplc_registry();
    let doc = ContractDoc::parse(TOKENS).unwrap();
    let comp = compile(&doc, &schema(), &registry.visible_to(Some("kplc")))
        .unwrap_or_else(|d| panic!("{d:#?}"));
    assert_eq!(comp.contract.functions.len(), 3);
    assert!(comp.contract.functions.iter().all(|p| p.hash.len() == 64));

    let data = || Arc::new(MemTable::try_new(schema(), vec![vec![batch()]]).unwrap());
    let v = validate(&comp, comp.validation.plan.clone(), data())
        .await
        .unwrap();
    assert_eq!(
        v.failures["valid_meter"],
        (0..240).filter(|i| i % 9 <= 1).count() as i64
    );
    assert_eq!(
        v.failures["some_units"],
        (0..240).filter(|i| i % 11 == 0).count() as i64
    );

    // A county analyst sees that county's valid meters; an admin sees every valid meter.
    let county47 = selectivity(&comp, data(), &caller("47")).await.unwrap();
    let admin = Caller::new("a", "kplc", "analytics").with_roles(&["admin"]);
    let all = selectivity(&comp, data(), &admin).await.unwrap();
    assert_eq!(all, 240 - (0..240).filter(|i| i % 9 <= 1).count());
    assert!(county47 > 0 && county47 < all);

    // Interpreter and DataFusion call the same module and agree, row by row.
    let diff = differential(&comp, &batch(), &[caller("47"), admin])
        .await
        .unwrap();
    assert!(
        diff.passed(),
        "{:#?}",
        &diff.mismatches[..diff.mismatches.len().min(5)]
    );
    assert!(diff.evaluations > 1500);

    // A bundle carries the modules; a verifier recompiles and checks it with nothing else.
    let functions = FUNCTIONS
        .iter()
        .map(|(n, s)| BundledFunction {
            owner: "kplc".into(),
            manifest: manifest(n, s),
            module: hex::encode(MODULE),
        })
        .collect();
    let bundle = Bundle::with_functions(&doc, &[], &schema(), &comp, functions).unwrap();
    Bundle::from_json(&bundle.to_json().unwrap())
        .unwrap()
        .verify()
        .unwrap();
}

#[test]
fn another_tenants_contract_cannot_call_them() {
    let registry = kplc_registry();
    let other = TOKENS
        .replace("owner: kplc", "owner: rival")
        .replace("contract: kplc/tokens", "contract: rival/tokens");
    let doc = ContractDoc::parse(&other).unwrap();
    let errs = compile(&doc, &schema(), &registry.visible_to(Some("rival")))
        .err()
        .unwrap();
    assert!(
        errs.iter()
            .any(|d| d.message.contains("not in the function registry")),
        "{errs:?}"
    );
}

#[test]
fn bad_modules_and_manifests_are_refused() {
    let err = |bytes: &[u8], name: &str, sig: &str| {
        install(bytes, &manifest(name, sig), "t")
            .err()
            .unwrap_or_default()
    };
    assert!(err(b"nope", "x", "(int) -> int").contains("not a WebAssembly module"));
    assert!(err(MODULE, "absent", "(int) -> int").contains("parcel_fn_absent"));
    assert!(err(MODULE, "is_meter_serial", "(int) -> bool").contains("smoke batch"));
    let importing: &[u8] = &[
        0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, 0x01, 0x04, 0x01, 0x60, 0x00, 0x00, 0x02,
        0x0d, 0x01, 0x03, b'e', b'n', b'v', 0x05, b'c', b'l', b'o', b'c', b'k', 0x00, 0x00,
    ];
    assert!(err(importing, "x", "(int) -> int").contains("imports `env::clock`"));
}
