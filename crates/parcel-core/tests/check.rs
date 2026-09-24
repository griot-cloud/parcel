//! Milestone 1: the check and classify passes.

use datafusion_common::arrow::datatypes::{DataType, Field, Schema};
use parcel_core::ir::{ExprKind, MacroKind, NsSet};
use parcel_core::{CheckedRule, Code, ContractDoc, Registry, check_contract};

const SALES_ORDERS: &str = include_str!("fixtures/sales_orders.yaml");

fn orders_schema() -> Schema {
    Schema::new(vec![
        Field::new("order_id", DataType::Int64, true),
        Field::new("customer_id", DataType::Int64, true),
        Field::new("email", DataType::Utf8, true),
        Field::new("msisdn", DataType::Utf8, true),
        Field::new("tenant_id", DataType::Utf8, false),
        Field::new("unit_price_cents", DataType::Int64, true),
        Field::new("qty", DataType::Int32, true),
        Field::new("amount_cents", DataType::Int64, true),
        Field::new("amount", DataType::Decimal128(18, 2), true),
        Field::new(
            "tags",
            DataType::List(Field::new_list_field(DataType::Utf8, true).into()),
            true,
        ),
        Field::new("region", DataType::Utf8, false),
        Field::new("dt", DataType::Date32, false),
    ])
}

#[test]
fn sales_orders_checks() {
    let doc = ContractDoc::parse(SALES_ORDERS).unwrap();
    let c = check_contract(&doc, &orders_schema(), &Registry::builtin())
        .unwrap_or_else(|d| panic!("{d:#?}"));
    assert_eq!(c.rules.len(), 10);
    assert_eq!(c.exposed.len(), 5);
    assert_eq!(
        c.functions
            .iter()
            .map(|p| p.name.as_str())
            .collect::<Vec<_>>(),
        ["hash_sha256"]
    );

    let admit = c.rules.iter().find(|r| r.id() == "own_or_admin").unwrap();
    assert_eq!(admit.exprs()[0].expr.ns, NsSet::ROW.union(NsSet::CTX));
    let guarantee = c.rules.iter().find(|r| r.id() == "fresh_enough").unwrap();
    assert_eq!(
        guarantee.exprs()[0].expr.ns,
        NsSet::DATASET.union(NsSet::CTX)
    );
    assert!(matches!(c.rules.last().unwrap(), CheckedRule::Shape { .. }));
}

#[test]
fn contract_hash_is_stable() {
    let a = ContractDoc::parse(SALES_ORDERS).unwrap();
    let b = ContractDoc::parse(&SALES_ORDERS.replace("{k: 5}", "{ k: 5 }")).unwrap();
    let reg = Registry::builtin();
    let (a, b) = (
        check_contract(&a, &orders_schema(), &reg).unwrap(),
        check_contract(&b, &orders_schema(), &reg).unwrap(),
    );
    assert_eq!(a.contract_hash, b.contract_hash);
    assert_eq!(a.contract_hash.len(), 64);
}

#[test]
fn macros_are_recognised() {
    let src = r#"
contract: t
version: 1
binding: {parquet: x}
expose: [{name: region, type: utf8}]
rules:
  - {id: a, op: admit, expr: "row.tags.exists(t, t == ctx.tenant)"}
  - {id: b, op: admit, expr: "ctx.roles.all(r, r.startsWith('ea_'))"}
  - {id: c, op: admit, expr: "size(row.tags.filter(t, t.contains('x'))) > 0"}
  - {id: d, op: admit, expr: "'X' in row.tags.map(t, t + 'X')"}
"#;
    let c = check_contract(
        &ContractDoc::parse(src).unwrap(),
        &orders_schema(),
        &Registry::builtin(),
    )
    .unwrap_or_else(|d| panic!("{d:#?}"));
    let kinds: Vec<MacroKind> = c
        .rules
        .iter()
        .map(|r| {
            let mut k = None;
            r.exprs()[0].expr.walk(&mut |e| {
                if let ExprKind::Macro { kind, .. } = &e.kind {
                    k.get_or_insert(*kind);
                }
            });
            k.unwrap()
        })
        .collect();
    assert_eq!(
        kinds,
        [
            MacroKind::Exists,
            MacroKind::All,
            MacroKind::Filter,
            MacroKind::Map
        ]
    );
}

/// Wrap one rule (YAML flow mapping) in a minimal contract and return the diagnostic codes.
fn codes(rule: &str) -> Vec<Code> {
    let src = format!(
        "contract: t\nversion: 1\nbinding: {{parquet: x}}\nexpose:\n  - {{name: email, type: utf8}}\n  - {{name: amount, type: 'decimal(18,2)'}}\n  - {{name: qty, type: int64}}\nrules:\n  - {rule}\n"
    );
    let doc = ContractDoc::parse(&src).unwrap_or_else(|d| panic!("{d}"));
    match check_contract(&doc, &orders_schema(), &Registry::builtin()) {
        Ok(_) => vec![],
        Err(d) => d.into_iter().map(|d| d.code).collect(),
    }
}

#[test]
fn rejections() {
    use Code::*;
    let cases: &[(&str, Code)] = &[
        // Classify: namespaces must fit the operation.
        (r#"{id: a, op: decide, expr: "row.qty > 1"}"#, Namespace),
        (
            r#"{id: a, op: assert, expr: "row.tenant_id == ctx.tenant", on_fail: drop}"#,
            Namespace,
        ),
        (
            r#"{id: a, op: guarantee, expr: "dataset.row_count > 0 && ctx.tenant == 'x'", on_fail: deny}"#,
            Namespace,
        ),
        (
            r#"{id: a, op: guarantee, expr: "row.qty > 0", on_fail: deny}"#,
            Namespace,
        ),
        (
            r#"{id: a, op: admit, expr: "dataset.row_count > 0"}"#,
            Namespace,
        ),
        (
            r#"{id: a, op: shape, operator: suppress, params: {k: 5}, unless: "row.qty > 1"}"#,
            Namespace,
        ),
        // Check: identifiers and fields.
        (r#"{id: a, op: admit, expr: "row.nope > 1"}"#, UnknownColumn),
        (
            r#"{id: a, op: admit, expr: "ctx.team == 'x'"}"#,
            UnknownField,
        ),
        (r#"{id: a, op: admit, expr: "qty > 1"}"#, UnknownIdentifier),
        (
            r#"{id: a, op: guarantee, expr: "dataset.qty.median > 1", on_fail: deny}"#,
            UnknownField,
        ),
        (
            r#"{id: a, op: guarantee, expr: "dataset.assertions.nope.pass_rate > 0.5", on_fail: deny}"#,
            UnknownField,
        ),
        (
            r#"{id: a, op: admit, expr: "row.amount > 0"}"#,
            UnreadableColumn,
        ),
        (
            r#"{id: a, op: admit, expr: "row.other.risk == 'x'"}"#,
            Unsupported,
        ),
        // Check: types.
        (r#"{id: a, op: admit, expr: "row.qty > 1.5"}"#, TypeMismatch),
        (r#"{id: a, op: admit, expr: "row.qty + 1"}"#, TypeMismatch),
        (
            r#"{id: a, op: admit, expr: "row.email.startsWith(1)"}"#,
            NoMatchingOverload,
        ),
        (
            r#"{id: a, op: admit, expr: "hash_sha256(row.qty) == 'x'"}"#,
            NoMatchingOverload,
        ),
        (
            r#"{id: a, op: admit, expr: "row.email.matches('(')"}"#,
            TypeMismatch,
        ),
        (
            r#"{id: a, op: transform, column: email, expr: "row.qty"}"#,
            TypeMismatch,
        ),
        // Check: the profile.
        (
            r#"{id: a, op: admit, expr: "row.email == null"}"#,
            OutsideProfile,
        ),
        (
            r#"{id: a, op: admit, expr: "{'a': 1}['a'] == 1"}"#,
            OutsideProfile,
        ),
        (
            r#"{id: a, op: admit, expr: "row.email.matches(ctx.tenant)"}"#,
            OutsideProfile,
        ),
        (
            r#"{id: a, op: admit, expr: "row.tags.exists_one(t, t == 'x')"}"#,
            OutsideProfile,
        ),
        (
            r#"{id: a, op: admit, expr: "has(ctx.tenant)"}"#,
            OutsideProfile,
        ),
        (
            r#"{id: a, op: admit, expr: "now() > ctx.now"}"#,
            UnknownFunction,
        ),
        (
            r#"{id: a, op: admit, expr: "row.email.reverse() == ''"}"#,
            UnknownFunction,
        ),
        (r#"{id: a, op: admit, expr: "row.qty >"}"#, Parse),
        // Rule structure.
        (r#"{id: Bad-Id, op: admit, expr: "true"}"#, InvalidRuleId),
        (
            r#"{id: a, op: transform, column: tenant_id, expr: "row.tenant_id"}"#,
            TransformTarget,
        ),
        (
            r#"{id: a, op: shape, operator: blur, params: {}}"#,
            ShapeParams,
        ),
        (
            r#"{id: a, op: shape, operator: suppress, params: {k: 0}}"#,
            ShapeParams,
        ),
        (
            r#"{id: a, op: shape, operator: noise, column: qty, params: {sensitivity: 1, epsilon: 0.5}}"#,
            ShapeParams,
        ),
        (
            r#"{id: a, op: shape, operator: sample, params: {fraction: 0.1, key: nope}}"#,
            ShapeParams,
        ),
    ];
    let mut failures = Vec::new();
    for (rule, want) in cases {
        let got = codes(rule);
        if !got.contains(want) {
            failures.push(format!("{rule}\n    want {want:?}, got {got:?}"));
        }
    }
    assert!(
        failures.is_empty(),
        "{} cases failed:\n  {}",
        failures.len(),
        failures.join("\n  ")
    );
}

#[test]
fn document_level_rejections() {
    let reg = Registry::builtin();
    let run = |src: &str| -> Vec<Code> {
        let doc = ContractDoc::parse(src).unwrap();
        check_contract(&doc, &orders_schema(), &reg)
            .err()
            .unwrap_or_default()
            .into_iter()
            .map(|d| d.code)
            .collect()
    };
    let base = "contract: t\nversion: 1\nbinding: {parquet: x, partitioned_by: [region]}\n";
    assert!(
        run(&format!("{base}expose: [{{name: nope, type: utf8}}]")).contains(&Code::UnknownColumn)
    );
    assert!(
        run(&format!("{base}expose: [{{name: qty, type: utf8}}]"))
            .contains(&Code::ExposeTypeMismatch)
    );
    assert!(run(&format!("{base}expose: [{{name: qty, type: varchar}}]")).contains(&Code::BadType));
    assert!(
        run(&format!(
            "{base}expose: [{{name: qty, type: int64}}, {{name: qty, type: int64}}]"
        ))
        .contains(&Code::DuplicateExpose)
    );
    assert!(run(&format!("{base}inherits: sales/base\nexpose: []")).contains(&Code::Unsupported));
    assert!(
        run(&format!("{base}expose: []\nrules:\n  - {{id: a, op: admit, expr: 'true'}}\n  - {{id: a, op: admit, expr: 'true'}}"))
            .contains(&Code::DuplicateRuleId)
    );
    assert!(
        run("contract: t\nversion: 1\nbinding: {parquet: x, partitioned_by: [zone]}\nexpose: []")
            .contains(&Code::UnknownColumn)
    );
    // Decimals may pass through, just not be read by rules.
    assert!(
        run(&format!(
            "{base}expose: [{{name: amount, type: 'decimal(18,2)'}}]"
        ))
        .is_empty()
    );
}
