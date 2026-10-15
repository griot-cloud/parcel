//! The customers contract as a Postgres script.

#[path = "../../parcel-sql/tests/common.rs"]
mod common;

#[test]
fn caller_fields_are_settings_and_the_script_registers_the_query() {
    let c = common::compiled(common::CUSTOMERS);
    let a = parcel_postgres::artifact(&c, None).unwrap();
    let q = &a.query;
    assert_eq!(a.source, "\"customers\"");
    assert!(
        q.contains("NULLIF(current_setting('parcel.tenant', TRUE), '')"),
        "{q}"
    );
    assert!(
        q.contains("current_setting('parcel.other_region', TRUE)"),
        "{q}"
    );
    assert!(q.contains("parcel.refuse('refused by known_tenant:"), "{q}");
    assert!(q.contains("encode(sha256(convert_to("), "{q}");
    assert!(q.contains("DOUBLE PRECISION"), "{q}");
    assert!(!q.contains("AS DOUBLE)"), "{q}");

    let script = parcel_postgres::script(&c, None).unwrap();
    assert!(script.contains("CREATE SCHEMA IF NOT EXISTS parcel;"));
    assert!(script.contains("CREATE OR REPLACE FUNCTION parcel.use(contract_name text)"));
    assert!(script.contains("INSERT INTO parcel.contracts"));
    assert!(script.contains("ON CONFLICT (name) DO UPDATE"));
}
