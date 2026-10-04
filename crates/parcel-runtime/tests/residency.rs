use datafusion::arrow::datatypes::{DataType, Field, Schema};
use parcel_core::compile::Compilation;
use parcel_core::{ContractDoc, Registry, compile_with};
use parcel_runtime::compiled::CompiledBytes;

fn parent() -> ContractDoc {
    ContractDoc::parse("contract: demo/parent\nversion: 1\nbinding: {parquet: data/}\nexpose: [{name: id, type: int64}]\nresidency: {jurisdictions: [KE, EU], approved_output_may_leave: true}").unwrap()
}

fn compiled(doc: &ContractDoc) -> Compilation {
    let schema = Schema::new(vec![Field::new("id", DataType::Int64, false)]);
    let c = compile_with(doc, &schema, &Registry::builtin(), &|_| Some(parent())).unwrap();
    Compilation::from_bytes(&c.to_bytes().unwrap()).unwrap()
}

#[test]
fn compiled_bytes_carry_inherited_terms() {
    for inherit in [false, true] {
        let doc = if inherit {
            ContractDoc::parse("contract: demo/child\nversion: 1\ninherits: demo/parent\nresidency: {jurisdictions: [KE], approved_output_may_leave: false}").unwrap()
        } else {
            parent()
        };
        let residency = compiled(&doc).contract.residency;
        assert!(residency.permits("KE"));
        assert_eq!(residency.permits("EU"), !inherit);
        assert_eq!(residency.approved_output_may_leave, !inherit);
        assert!(!residency.permits("US"));
    }
}

#[test]
fn missing_terms_deny_placement() {
    let mut doc = parent();
    doc.residency = None;
    let residency = compiled(&doc).contract.residency;
    assert!(!residency.permits("KE"));
    assert!(!residency.approved_output_may_leave);
}
