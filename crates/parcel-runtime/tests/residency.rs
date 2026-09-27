use datafusion::arrow::datatypes::{DataType, Field, Schema};
use parcel_core::{ContractDoc, Registry, compile_with};
use parcel_runtime::bundle::{Bundle, FORMAT};

fn bundle(inherit: bool) -> Bundle {
    let schema = Schema::new(vec![Field::new("id", DataType::Int64, false)]);
    let parent = ContractDoc::parse("contract: demo/parent\nversion: 1\nbinding: {parquet: data/}\nexpose: [{name: id, type: int64}]\nresidency: {jurisdictions: [KE, EU], approved_output_may_leave: true}").unwrap();
    let doc = if inherit {
        ContractDoc::parse("contract: demo/child\nversion: 1\ninherits: demo/parent\nresidency: {jurisdictions: [KE], approved_output_may_leave: false}").unwrap()
    } else {
        parent.clone()
    };
    let ancestors = if inherit {
        vec![parent.clone()]
    } else {
        vec![]
    };
    let c = compile_with(&doc, &schema, &Registry::builtin(), &|_| {
        Some(parent.clone())
    })
    .unwrap();
    Bundle::new(&doc, &ancestors, &schema, &c).unwrap()
}

#[test]
fn bundle_round_trip_verifies_and_exposes_inherited_terms() {
    for inherit in [false, true] {
        let b = bundle(inherit);
        let b = Bundle::from_json(&b.to_json().unwrap()).unwrap();
        assert_eq!(b.format, FORMAT);
        assert_eq!(b.parcel_version, "0.0.2");
        let c = b.verify().unwrap();
        assert_eq!(b.contract_hash, c.contract.contract_hash);
        assert!(b.residency().permits("KE"));
        assert_eq!(b.residency().permits("EU"), !inherit);
        assert_eq!(b.residency().approved_output_may_leave, !inherit);
        assert!(!b.residency().permits("US"));
    }
}

#[test]
fn changed_document_or_hash_cannot_grant_key_release() {
    let mut b = bundle(true);
    b.document
        .residency
        .as_mut()
        .unwrap()
        .jurisdictions
        .insert("US".into());
    assert!(b.verify().is_err());
    assert!(!b.residency().permits("US"));
    let mut b = bundle(false);
    b.document
        .residency
        .as_mut()
        .unwrap()
        .approved_output_may_leave = false;
    assert!(b.verify().is_err());
    assert!(!b.residency().permits("KE"));
    let mut b = bundle(false);
    b.contract_hash = "unapproved hash".into();
    assert!(b.verify().is_err());
    assert!(!b.residency().permits("KE"));
}

#[test]
fn missing_terms_deny_placement_after_valid_recompile() {
    let mut b = bundle(false);
    b.document.residency = None;
    let schema = b.schema().unwrap();
    let c = compile_with(&b.document, &schema, &Registry::builtin(), &|_| None).unwrap();
    let b = Bundle::new(&b.document, &[], &schema, &c).unwrap();
    b.verify().unwrap();
    assert!(!b.residency().permits("KE"));
    assert!(!b.residency().approved_output_may_leave);
}
