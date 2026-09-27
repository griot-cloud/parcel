use datafusion_common::arrow::datatypes::{DataType, Field, Schema};
use parcel_core::{Code, ContractDoc, Registry, check_contract, compile_with, contract_hash};

fn schema() -> Schema {
    Schema::new(vec![Field::new("id", DataType::Int64, false)])
}
fn doc(terms: &str) -> ContractDoc {
    ContractDoc::parse(&format!("contract: demo/parent\nversion: 1\nbinding: {{parquet: data/}}\nexpose: [{{name: id, type: int64}}]\n{terms}\n")).unwrap()
}
fn child(terms: &str) -> ContractDoc {
    ContractDoc::parse(&format!(
        "contract: demo/child\nversion: 1\ninherits: demo/parent\n{terms}\n"
    ))
    .unwrap()
}
fn terms(codes: &str, output: bool) -> String {
    format!("residency: {{jurisdictions: [{codes}], approved_output_may_leave: {output}}}")
}

#[test]
fn checker_refuses_unknown_empty_and_wildcard_jurisdictions() {
    for codes in ["", "ZZ", "ke", "KEN", "'*'", "' KE'"] {
        let d = doc(&terms(codes, false));
        let errors = check_contract(&d, &schema(), &Registry::builtin()).unwrap_err();
        assert!(
            errors.iter().any(|e| e.code == Code::Residency),
            "{codes}: {errors:?}"
        );
    }
    let d = doc(&terms("KE, EU, US", false));
    let checked = check_contract(&d, &schema(), &Registry::builtin()).unwrap();
    assert!(checked.residency.permits("KE"));
    assert!(checked.residency.permits("EU"));
    assert!(!checked.residency.permits("ke"));
    assert!(!checked.residency.permits("ZZ"));
    assert!(!checked.residency.approved_output_may_leave);
}

#[test]
fn terms_are_hashed_as_a_set_and_output_permission_is_hashed() {
    let a = doc(&terms("KE, EU", false));
    let reordered = doc(&terms("EU, KE, KE", false));
    assert_eq!(contract_hash(&a), contract_hash(&reordered));
    assert_ne!(contract_hash(&a), contract_hash(&doc(&terms("KE", false))));
    assert_ne!(
        contract_hash(&a),
        contract_hash(&doc(&terms("KE, EU", true)))
    );
}

#[test]
fn inheritance_keeps_or_narrows_both_permissions() {
    let p = doc(&terms("KE, EU", true));
    for (c, wanted) in [(child(""), true), (child(&terms("KE", false)), false)] {
        let compiled =
            compile_with(&c, &schema(), &Registry::builtin(), &|_| Some(p.clone())).unwrap();
        assert!(compiled.contract.residency.permits("KE"));
        assert_eq!(compiled.contract.residency.permits("EU"), wanted);
        assert_eq!(
            compiled.contract.residency.approved_output_may_leave,
            wanted
        );
    }
}

#[test]
fn inheritance_refuses_widening_and_invalid_ancestors() {
    for (p, c) in [
        (doc(&terms("KE", false)), child(&terms("KE, EU", false))),
        (doc(&terms("KE", false)), child(&terms("KE", true))),
        (doc(&terms("", false)), child(&terms("KE", false))),
        (doc(&terms("ZZ", false)), child(&terms("KE", false))),
        (doc(""), child(&terms("KE", false))),
    ] {
        assert!(compile_with(&c, &schema(), &Registry::builtin(), &|_| Some(p.clone())).is_err());
    }
    assert!(
        !check_contract(&doc(""), &schema(), &Registry::builtin())
            .unwrap()
            .residency
            .permits("KE")
    );
}
