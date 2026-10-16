//! The customers contract as a DuckDB query.

#[path = "../../parcel-sql/tests/common.rs"]
mod common;

#[test]
fn caller_fields_are_variables_and_refusals_raise() {
    let a = parcel_duckdb::artifact(&common::compiled(common::CUSTOMERS), None).unwrap();
    let q = &a.query;
    assert_eq!(a.engine, "duckdb");
    assert_eq!(a.source, "read_parquet('data/customers/**/*.parquet')");
    assert!(
        q.contains("CAST(getvariable('parcel_tenant') AS VARCHAR)"),
        "{q}"
    );
    assert!(q.contains("getvariable('parcel_other_region')"), "{q}");
    assert!(
        q.contains("error('refused by known_tenant: Our company and approved partners only')"),
        "{q}"
    );
    // Functions as DuckDB spells them.
    assert!(
        q.contains("regexp_matches(") && q.contains("sha256("),
        "{q}"
    );
    assert!(!q.contains("encode("), "{q}");
    assert!(!q.contains("$c"), "every caller placeholder is bound: {q}");
}

#[test]
fn a_source_replaces_the_binding() {
    let a = parcel_duckdb::artifact(&common::compiled(common::CUSTOMERS), Some("main.customers"))
        .unwrap();
    assert!(a.query.contains("FROM main.customers"));
}
