//! Milestone 2: translation and assembly.

mod common;

use parcel_core::compile::Tier;
use parcel_core::{ContractDoc, Registry, compile};

#[test]
fn sales_orders_compiles() {
    let doc = ContractDoc::parse(common::SALES_ORDERS).unwrap();
    let c = compile(&doc, &common::orders_schema(), &Registry::builtin())
        .unwrap_or_else(|d| panic!("{d:#?}"));
    let cc = &c.contract;
    if std::env::var("PARCEL_SHOW").is_ok() {
        println!("{}", serde_json::to_string_pretty(cc).unwrap());
        println!("{}", c.validation.plan.display_indent());
    }
    assert_eq!(cc.exposed_schema.fields().len(), 5);
    assert_eq!(cc.admits.len(), 1);
    assert_eq!(cc.flags.len(), 3);
    assert_eq!(cc.decisions.len(), 1);
    // ctx subtrees become placeholders: the whole `'admin' in ctx.roles`, and `ctx.tenant` on its own.
    let cels: Vec<&str> = cc.params.iter().map(|p| p.cel.as_str()).collect();
    assert!(cels.contains(&"ctx.tenant"), "{cels:?}");
    assert!(cels.contains(&"(\"admin\" in ctx.roles)"), "{cels:?}");
    let admit = cc.report.iter().find(|r| r.rule == "own_or_admin").unwrap();
    assert_eq!(admit.tier, Tier::Prunes);
    assert_eq!(
        c.validation.data_guarantees,
        ["ids_present", "msisdns_mostly_valid"]
    );
    assert_eq!(c.validation.query_time_guarantees, ["fresh_enough"]);
    assert_eq!(c.write.layout.cluster_by, ["_c_amount_consistent"]);
    assert_eq!(c.write.layout.bloom, ["tenant_id"]);
}

#[test]
fn compilation_is_deterministic() {
    let doc = ContractDoc::parse(common::SALES_ORDERS).unwrap();
    let a = compile(&doc, &common::orders_schema(), &Registry::builtin()).unwrap();
    let b = compile(&doc, &common::orders_schema(), &Registry::builtin()).unwrap();
    assert_eq!(a.contract.compilation_hash, b.contract.compilation_hash);
    assert_eq!(
        serde_json::to_string(&a.contract).unwrap(),
        serde_json::to_string(&b.contract).unwrap()
    );
}
