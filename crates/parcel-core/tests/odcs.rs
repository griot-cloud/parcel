//! ODCS v3 import.

use datafusion_common::arrow::datatypes::{DataType, Field, Schema};
use parcel_core::document::Rule;
use parcel_core::{Registry, compile, odcs};

const PAYMENTS: &str = include_str!("fixtures/odcs_payments.yaml");

#[test]
fn imports_and_compiles() {
    let imported = odcs::import(PAYMENTS, None).unwrap();
    let doc = &imported.doc;
    assert_eq!(doc.contract, "seller/seller_payments_v1");
    assert_eq!(doc.version, 1);
    assert_eq!(doc.owner.as_deref(), Some("climatequantuminc"));
    let expose: Vec<(&str, &str)> = doc
        .expose
        .as_ref()
        .unwrap()
        .iter()
        .map(|c| (c.name.as_str(), c.type_name.as_str()))
        .collect();
    assert_eq!(
        expose,
        [
            ("txn_ref_dt", "date32"),
            ("rcvr_id", "utf8"),
            ("rcvr_cntry_code", "utf8"),
            ("amount", "decimal(18,2)"),
            ("tags", "list<utf8>")
        ]
    );
    let ids: Vec<&str> = doc.rules.iter().map(Rule::id).collect();
    assert_eq!(
        ids,
        [
            "rcvr_id_present",
            "rcvr_id_unique",
            "rcvr_cntry_code_present",
            "cntry_two_letters"
        ]
    );
    // What did not carry over is said, not dropped silently.
    let notes = imported.notes.join("\n");
    for expected in [
        "rowCount",
        "(sql)",
        "servers",
        "slaProperties",
        "rcvr_id.classification",
    ] {
        assert!(
            notes.contains(expected),
            "missing note about {expected}:\n{notes}"
        );
    }

    // The imported contract compiles against data with those columns.
    let schema = Schema::new(vec![
        Field::new("txn_ref_dt", DataType::Date32, true),
        Field::new("rcvr_id", DataType::Utf8, true),
        Field::new("rcvr_cntry_code", DataType::Utf8, true),
        Field::new("amount", DataType::Decimal128(18, 2), true),
        Field::new(
            "tags",
            DataType::List(Field::new_list_field(DataType::Utf8, true).into()),
            true,
        ),
    ]);
    compile(doc, &schema, &Registry::builtin()).unwrap_or_else(|d| panic!("{d:#?}"));
}

#[test]
fn picks_an_object_by_name() {
    assert!(odcs::import(PAYMENTS, Some("nope")).is_err());
    assert!(odcs::import(PAYMENTS, Some("tbl")).is_ok());
    assert!(odcs::import("kind: DataContract\n", None).is_err());
}
