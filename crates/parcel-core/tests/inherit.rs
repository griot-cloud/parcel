//! Inheritance: a child contract can only restrict its parent.

mod common;

use std::collections::BTreeMap;

use parcel_core::{Code, ContractDoc, Registry, compile_with};

const BASE: &str = r#"
contract: sales/base
version: 1
binding: {parquet: data/orders/, partitioned_by: [region]}
expose:
  - {name: order_id, type: int64}
  - {name: email, type: utf8}
  - {name: region, type: utf8}
  - {name: amount_cents, type: int64}
rules:
  - {id: analytics_only, op: decide, expr: "ctx.purpose in ['analytics', 'reporting']"}
  - {id: own_rows, op: admit, expr: "row.tenant_id == ctx.tenant"}
  - {id: pk_present, op: assert, expr: "has(row.order_id)", on_fail: deny}
  - {id: mask_email, op: transform, column: email, expr: "hash_sha256(row.email)"}
"#;

fn store(extra: &[&str]) -> BTreeMap<String, ContractDoc> {
    let mut m = BTreeMap::new();
    for src in std::iter::once(BASE).chain(extra.iter().copied()) {
        let d = ContractDoc::parse(src).unwrap();
        m.insert(d.contract.clone(), d);
    }
    m
}

fn compile_child(src: &str) -> Result<parcel_core::Compilation, Vec<parcel_core::Diagnostic>> {
    let s = store(&[src]);
    let doc = ContractDoc::parse(src).unwrap();
    compile_with(&doc, &common::orders_schema(), &Registry::builtin(), &|n| {
        s.get(n).cloned()
    })
}

#[test]
fn a_child_narrows_and_adds_rules() {
    let child = r#"
contract: sales/ea
version: 1
inherits: sales/base
expose:
  - {name: order_id, type: int64}
  - {name: email, type: utf8}
  - {name: amount_cents, type: int64}
rules:
  - {id: ea_only, op: admit, expr: "row.region == 'EA'"}
  - {id: short_email, op: transform, column: email, expr: "row.email.startsWith('0') ? 'zero' : row.email"}
"#;
    let c = compile_child(child).unwrap_or_else(|d| panic!("{d:#?}"));
    let cc = &c.contract;
    assert_eq!(cc.name, "sales/ea");
    assert_eq!(cc.exposed_schema.fields().len(), 3);
    assert_eq!(
        cc.decisions.len(),
        1,
        "the parent's decide rule is inherited"
    );
    assert_eq!(cc.admits.len(), 2);
    assert_eq!(cc.flags.len(), 1);
    // The child's transform sees the parent's hashed email: the composed transform hashes first.
    let email = cc.row_rules.iter().find(|r| r.id == "short_email").unwrap();
    assert!(
        email.cel.contains("hash_sha256(row.email)"),
        "{}",
        email.cel
    );
    assert!(
        cc.row_rules.iter().all(|r| r.id != "mask_email"),
        "the parent transform is composed into the child's"
    );
}

#[test]
fn composed_transforms_never_reveal_the_raw_value() {
    let child = r#"
contract: sales/leaky
version: 1
inherits: sales/base
rules:
  - {id: unmask, op: transform, column: email, expr: "row.email"}
"#;
    let c = compile_child(child).unwrap();
    // `row.email` in the child means the parent's hashed email.
    let t = c
        .contract
        .row_rules
        .iter()
        .find(|r| r.id == "unmask")
        .unwrap();
    assert_eq!(t.cel, "hash_sha256(row.email)");
}

#[test]
fn a_child_cannot_widen() {
    let cases = [
        // Exposing a column the parent hides.
        (
            "contract: x\nversion: 1\ninherits: sales/base\nexpose:\n  - {name: tenant_id, type: utf8}\n",
            Code::Inheritance,
        ),
        // Changing a column's type.
        (
            "contract: x\nversion: 1\ninherits: sales/base\nexpose:\n  - {name: order_id, type: utf8}\n",
            Code::Inheritance,
        ),
        // Reading a hidden column into an exposed one.
        (
            "contract: x\nversion: 1\ninherits: sales/base\nrules:\n  - {id: leak, op: transform, column: email, expr: \"row.tenant_id\"}\n",
            Code::Inheritance,
        ),
        // Redefining a parent rule.
        (
            "contract: x\nversion: 1\ninherits: sales/base\nrules:\n  - {id: own_rows, op: admit, expr: \"true\"}\n",
            Code::Inheritance,
        ),
        // Binding other data.
        (
            "contract: x\nversion: 1\ninherits: sales/base\nbinding: {parquet: elsewhere/}\n",
            Code::Inheritance,
        ),
        // An unknown parent.
        (
            "contract: x\nversion: 1\ninherits: sales/nope\n",
            Code::Inheritance,
        ),
    ];
    for (src, code) in cases {
        let errs = compile_child(src)
            .err()
            .unwrap_or_else(|| panic!("accepted:\n{src}"));
        assert!(errs.iter().any(|d| d.code == code), "{src}\n{errs:#?}");
    }
}

#[test]
fn a_child_admit_can_filter_on_hidden_columns() {
    let child = "contract: x\nversion: 1\ninherits: sales/base\nrules:\n  - {id: big, op: admit, expr: \"row.qty > 2\"}\n";
    let c = compile_child(child).unwrap();
    assert_eq!(c.contract.admits.len(), 2);
}

#[test]
fn cycles_are_reported() {
    let a = "contract: a\nversion: 1\ninherits: b\n";
    let b = "contract: b\nversion: 1\ninherits: a\n";
    let s = store(&[a, b]);
    let doc = ContractDoc::parse(a).unwrap();
    let errs = compile_with(&doc, &common::orders_schema(), &Registry::builtin(), &|n| {
        s.get(n).cloned()
    })
    .unwrap_err();
    assert!(errs[0].message.contains("cycle"), "{errs:?}");
}

#[test]
fn grandchildren_compose_through_every_layer() {
    let mid = "contract: sales/mid\nversion: 1\ninherits: sales/base\nrules:\n  - {id: upper, op: transform, column: email, expr: \"row.email + '!'\"}\n";
    let leaf = "contract: sales/leaf\nversion: 1\ninherits: sales/mid\nrules:\n  - {id: again, op: transform, column: email, expr: \"row.email + '?'\"}\n";
    let s = store(&[mid, leaf]);
    let doc = ContractDoc::parse(leaf).unwrap();
    let c = compile_with(&doc, &common::orders_schema(), &Registry::builtin(), &|n| {
        s.get(n).cloned()
    })
    .unwrap();
    let t = c
        .contract
        .row_rules
        .iter()
        .find(|r| r.id == "again")
        .unwrap();
    assert_eq!(t.cel, "((hash_sha256(row.email) + \"!\") + \"?\")");
}

#[test]
fn hiding_a_transformed_column_drops_its_transform() {
    let child = "contract: x\nversion: 1\ninherits: sales/base\nexpose:\n  - {name: order_id, type: int64}\n";
    let c = compile_child(child).unwrap_or_else(|d| panic!("{d:#?}"));
    assert_eq!(c.contract.projection.len(), 1);
    assert!(c.contract.row_rules.iter().all(|r| r.id != "mask_email"));
}
