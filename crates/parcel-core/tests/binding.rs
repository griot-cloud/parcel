//! A binding names exactly one source: a Parquet location or an Iceberg table.

mod common;

use parcel_core::document::{IcebergTable, Source, json_schema};
use parcel_core::{Code, ContractDoc, Registry, check_contract, compile};

fn contract(binding: &str) -> String {
    format!(
        "contract: sales/orders\nversion: 1\nbinding: {binding}\nexpose:\n  - {{name: order_id, type: int64}}\n  - {{name: region, type: utf8}}\nrules:\n  - {{id: east, op: admit, expr: \"row.region == 'EA'\"}}\n"
    )
}

fn refused(binding: &str) -> String {
    let err = ContractDoc::parse(&contract(binding))
        .err()
        .unwrap_or_else(|| panic!("accepted binding {binding}"));
    assert_eq!(err.code, Code::Document, "{err:?}");
    err.message
}

#[test]
fn an_iceberg_binding_parses_checks_and_compiles() {
    let doc = ContractDoc::parse(&contract("{iceberg: sales.orders}")).unwrap();
    let table = IcebergTable {
        namespace: vec!["sales".into()],
        table: "orders".into(),
    };
    let binding = doc.binding.clone().unwrap();
    assert_eq!(binding.source, Source::Iceberg(table.clone()));
    assert!(binding.partitioned_by.is_empty());

    let checked = check_contract(&doc, &common::orders_schema(), &Registry::builtin())
        .unwrap_or_else(|d| panic!("{d:#?}"));
    assert_eq!(checked.binding, binding);

    let c = compile(&doc, &common::orders_schema(), &Registry::builtin())
        .unwrap_or_else(|d| panic!("{d:#?}"));
    assert_eq!(c.contract.binding.source, Source::Iceberg(table));
    let json = serde_json::to_value(&c.contract).unwrap();
    assert_eq!(
        json["binding"],
        serde_json::json!({"iceberg": "sales.orders"})
    );
}

#[test]
fn a_nested_namespace_keeps_every_part() {
    let doc = ContractDoc::parse(&contract("{iceberg: lake.sales_2026.orders}")).unwrap();
    let Source::Iceberg(t) = doc.binding.unwrap().source else {
        panic!("not an Iceberg binding")
    };
    assert_eq!(t.namespace, ["lake", "sales_2026"]);
    assert_eq!(t.table, "orders");
    assert_eq!(t.to_string(), "lake.sales_2026.orders");
}

#[test]
fn an_iceberg_partition_column_is_checked_like_a_parquet_one() {
    let ok = ContractDoc::parse(&contract(
        "{iceberg: sales.orders, partitioned_by: [region]}",
    ))
    .unwrap();
    let c = compile(&ok, &common::orders_schema(), &Registry::builtin())
        .unwrap_or_else(|d| panic!("{d:#?}"));
    assert_eq!(c.contract.binding.partitioned_by, ["region"]);

    let bad =
        ContractDoc::parse(&contract("{iceberg: sales.orders, partitioned_by: [zone]}")).unwrap();
    let errs = check_contract(&bad, &common::orders_schema(), &Registry::builtin()).unwrap_err();
    assert!(
        errs.iter()
            .any(|d| d.code == Code::UnknownColumn && d.message.contains("`zone`")),
        "{errs:#?}"
    );
}

#[test]
fn a_parquet_binding_round_trips_unchanged() {
    let doc = ContractDoc::parse(&contract(
        "{parquet: data/orders/, partitioned_by: [region]}",
    ))
    .unwrap();
    let binding = doc.binding.clone().unwrap();
    assert_eq!(binding.source, Source::Parquet("data/orders/".into()));
    assert_eq!(
        serde_json::to_value(&binding).unwrap(),
        serde_json::json!({"parquet": "data/orders/", "partitioned_by": ["region"]})
    );
}

#[test]
fn both_forms_together_are_refused() {
    let msg = refused("{parquet: data/orders/, iceberg: sales.orders}");
    assert!(
        msg.contains("`parquet`") && msg.contains("`iceberg`") && msg.contains("both"),
        "{msg}"
    );
}

#[test]
fn neither_form_is_refused() {
    let msg = refused("{partitioned_by: [region]}");
    assert!(
        msg.contains("`parquet`") && msg.contains("`iceberg`") && msg.contains("neither"),
        "{msg}"
    );
}

#[test]
fn unknown_binding_fields_are_refused() {
    let msg = refused("{iceberg: sales.orders, catalog: main}");
    assert!(msg.contains("catalog"), "{msg}");
}

#[test]
fn a_bad_iceberg_identifier_is_refused_naming_it() {
    for bad in [
        "orders",
        "a..b",
        "a.b c",
        ".orders",
        "sales.",
        "sales.1orders",
    ] {
        let msg = refused(&format!("{{iceberg: {bad:?}}}"));
        assert!(msg.contains(&format!("{bad:?}")), "{bad}: {msg}");
        assert!(msg.contains("namespace.table"), "{bad}: {msg}");
    }
}

#[test]
fn the_schema_accepts_one_form_and_refuses_the_rest() {
    let v = jsonschema::validator_for(&json_schema()).unwrap();
    let as_json =
        |binding: &str| -> serde_json::Value { yaml_serde::from_str(&contract(binding)).unwrap() };
    for good in [
        "{iceberg: sales.orders}",
        "{iceberg: lake.sales.orders, partitioned_by: [region]}",
        "{parquet: data/orders/}",
    ] {
        let doc = as_json(good);
        let errors: Vec<String> = v.iter_errors(&doc).map(|e| e.to_string()).collect();
        assert!(errors.is_empty(), "{good}: {errors:?}");
        ContractDoc::parse(&contract(good)).unwrap();
    }
    for bad in [
        "{parquet: data/orders/, iceberg: sales.orders}",
        "{partitioned_by: [region]}",
        "{iceberg: orders}",
        "{iceberg: 'a..b'}",
        "{iceberg: 'a.b c'}",
        "{iceberg: sales.orders, catalog: main}",
    ] {
        assert!(!v.is_valid(&as_json(bad)), "schema accepted {bad}");
        assert!(
            ContractDoc::parse(&contract(bad)).is_err(),
            "parser accepted {bad}"
        );
    }
}
