//! The customers contract as a Spark artifact.

#[path = "../../parcel-sql/tests/common.rs"]
mod common;

#[test]
fn caller_fields_are_tokens_the_extension_fills() {
    let a = parcel_spark::artifact(&common::compiled(common::CUSTOMERS), None).unwrap();
    let q = &a.query;
    assert_eq!(a.source, "parquet.`data/customers`");
    assert!(q.contains("__parcel_caller(tenant)__"), "{q}");
    assert!(q.contains("__parcel_caller(other.region)__"), "{q}");
    assert!(
        q.contains("CAST(raise_error('refused by known_tenant:"),
        "{q}"
    );
    assert!(q.contains("sha2(") && q.contains("`customer_id`"), "{q}");
    assert!(q.contains("AS STRING"), "{q}");
    let json: serde_json::Value = serde_json::from_str(
        &parcel_spark::json(&common::compiled(common::CUSTOMERS), None).unwrap(),
    )
    .unwrap();
    assert_eq!(json["format"], "parcel-target/1");
    assert_eq!(json["name"], "sales/customers@1.0.0");
}
