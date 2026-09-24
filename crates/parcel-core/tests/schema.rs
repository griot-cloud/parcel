//! The JSON Schema accepts every contract the parser accepts, and rejects what it rejects.

use parcel_core::document::json_schema;

fn validator() -> jsonschema::Validator {
    jsonschema::validator_for(&json_schema()).expect("the schema is valid JSON Schema")
}

fn as_json(yaml: &str) -> serde_json::Value {
    yaml_serde::from_str(yaml).unwrap()
}

#[test]
fn every_example_contract_validates() {
    let v = validator();
    let mut sources = vec![include_str!("fixtures/sales_orders.yaml").to_owned()];
    for dir in ["../../examples/quickstart/contracts"] {
        for e in
            std::fs::read_dir(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(dir)).unwrap()
        {
            sources.push(std::fs::read_to_string(e.unwrap().path()).unwrap());
        }
    }
    for src in sources {
        let doc = as_json(&src);
        let errors: Vec<String> = v.iter_errors(&doc).map(|e| e.to_string()).collect();
        assert!(errors.is_empty(), "{errors:?}\n{src}");
        parcel_core::ContractDoc::parse(&src).unwrap();
    }
}

#[test]
fn the_schema_rejects_what_the_parser_rejects() {
    let v = validator();
    for bad in [
        "contract: t\nversion: 1\nexpose: []\nsurprise: true\n",
        "contract: t\nversion: 1\nrules:\n  - {id: a, op: decide, expr: 'true', on_fail: deny}\n",
        "contract: t\nversion: 1\nrules:\n  - {id: a, op: frobnicate, expr: 'true'}\n",
        "contract: t\nversion: -1\n",
    ] {
        assert!(!v.is_valid(&as_json(bad)), "schema accepted:\n{bad}");
        assert!(
            parcel_core::ContractDoc::parse(bad).is_err(),
            "parser accepted:\n{bad}"
        );
    }
}

#[test]
fn the_checked_in_schema_is_current() {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../schema/contract.schema.json");
    let want = serde_json::to_string_pretty(&json_schema()).unwrap() + "\n";
    if std::env::var("PARCEL_BLESS").is_ok() {
        std::fs::write(&path, &want).unwrap();
    }
    let got = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        got == want,
        "schema/contract.schema.json is stale; rerun with PARCEL_BLESS=1"
    );
}
