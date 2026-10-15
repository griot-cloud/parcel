//! A compiled contract as a governed query: its parts, and what is refused.

mod common;

#[test]
fn parts_of_the_customers_contract() {
    let q = parcel_sql::governed(&common::compiled(common::CUSTOMERS)).unwrap();
    assert_eq!(q.name, "sales/customers@1.0.0");
    let ids = |v: Vec<&str>| v.join(",");
    assert_eq!(
        ids(q.decisions.iter().map(|d| d.id.as_str()).collect()),
        "known_tenant,approved_purpose,partners_market_only"
    );
    assert_eq!(
        ids(q.filters.iter().map(|f| f.id.as_str()).collect()),
        "home_region,partner_markets,adults_for_marketing,plausible_age,valid_customer_id,non_negative_cost"
    );
    assert_eq!(q.columns.len(), 11);
    assert_eq!(
        q.decisions[0].message.as_deref(),
        Some("Our company and approved partners only")
    );
    let caller: Vec<String> = parcel_sql::caller_fields(&q).into_keys().collect();
    assert_eq!(caller, ["other.region", "purpose", "tenant", "tier"]);
}

#[test]
fn what_one_query_cannot_express_is_refused() {
    let shaped = format!(
        "{}\n  - id: small_groups\n    op: shape\n    operator: suppress\n    params: {{k: 5}}\n",
        common::CUSTOMERS
    );
    let err = parcel_sql::governed(&common::compiled(&shaped)).unwrap_err();
    assert!(
        err.contains("shape `small_groups` is not supported"),
        "{err}"
    );
}
