//! What a newcomer sees: whole-number literals that just work, and refusals that say what to do.

use datafusion_common::arrow::datatypes::{DataType, Field, Schema};
use parcel_core::{Code, ContractDoc, Diagnostic, Registry, check_contract};

fn schema() -> Schema {
    Schema::new(vec![
        Field::new("amount", DataType::Int64, true),
        Field::new("cost", DataType::Float64, true),
        Field::new("count", DataType::UInt32, true),
        Field::new("supplier_id", DataType::Utf8, true),
    ])
}

fn contract(expose: &str, rules: &str) -> String {
    format!(
        "contract: sales/orders\nversion: 1\nbinding: {{parquet: x/}}\nexpose:\n{expose}rules:\n{rules}"
    )
}

const EXPOSE: &str = "  - {name: amount, type: int64}\n  - {name: cost, type: float64}\n";

fn check(rules: &str) -> Result<(), Vec<Diagnostic>> {
    let doc = ContractDoc::parse(&contract(EXPOSE, rules)).unwrap();
    check_contract(&doc, &schema(), &Registry::builtin()).map(|_| ())
}

fn refused(rules: &str) -> Diagnostic {
    check(rules).expect_err("expected a refusal").remove(0)
}

#[test]
fn whole_number_literals_meet_doubles_and_uints() {
    for expr in [
        "row.cost > 0",
        "row.cost >= -1",
        "0 < row.cost",
        "row.cost * 2 < 100",
        "row.cost in [1, 2.5]",
        "row.cost in [1, 2]",
        "row.count > 5",
        "row.count in [1, 2]",
    ] {
        check(&format!(
            "  - {{id: r, op: assert, expr: \"{expr}\", on_fail: drop}}\n"
        ))
        .unwrap_or_else(|d| panic!("{expr}: {d:#?}"));
    }
}

#[test]
fn only_literals_are_promoted() {
    let d = refused("  - {id: r, op: assert, expr: \"row.amount < row.cost\", on_fail: drop}\n");
    assert_eq!(d.code, Code::TypeMismatch);
    assert_eq!(d.help.as_deref(), Some("use `double(row.amount)`"));
    // Not exact as a double.
    let d =
        refused("  - {id: r, op: assert, expr: \"row.cost < 9007199254740993\", on_fail: drop}\n");
    assert_eq!(d.code, Code::TypeMismatch);
    // A double literal never narrows an int column.
    let d = refused("  - {id: r, op: assert, expr: \"row.amount > 2.5\", on_fail: drop}\n");
    assert!(d.help.unwrap().contains("double(row.amount)"));
}

#[test]
fn namespace_refusals_say_which_operation_fits() {
    let d = refused("  - {id: r, op: decide, expr: \"row.amount > 0\"}\n");
    assert_eq!(d.code, Code::Namespace);
    let help = d.help.unwrap();
    assert!(
        help.contains("`op: admit`") && help.contains("decide: ctx"),
        "{help}"
    );

    let d = refused("  - {id: r, op: assert, expr: \"ctx.tenant == 'a'\", on_fail: drop}\n");
    assert!(d.help.unwrap().contains("`op: admit`"));

    let d = refused("  - {id: r, op: guarantee, expr: \"row.amount > 0\", on_fail: deny}\n");
    assert!(d.help.unwrap().contains("`op: assert`"));
}

#[test]
fn names_are_suggested() {
    let d = refused("  - {id: r, op: admit, expr: \"row.ammount > 0\"}\n");
    assert_eq!(d.code, Code::UnknownColumn);
    assert_eq!(d.help.as_deref(), Some("did you mean `row.amount`?"));

    let d = refused("  - {id: r, op: admit, expr: \"amount > 0\"}\n");
    assert_eq!(d.help.as_deref(), Some("use `row.amount`"));

    let d = refused("  - {id: r, op: decide, expr: \"ctx.tennant == 'a'\"}\n");
    assert_eq!(d.help.as_deref(), Some("did you mean `ctx.tenant`?"));

    let d = refused("  - {id: Positive-Amount, op: admit, expr: \"row.amount > 0\"}\n");
    assert_eq!(d.code, Code::InvalidRuleId);
    assert_eq!(d.help.as_deref(), Some("try `positive_amount`"));
}

#[test]
fn sql_habits_get_a_hint() {
    for (expr, hint) in [
        ("row.amount > 0 and row.cost > 0", "`&&`"),
        ("row.supplier_id = 'a'", "`==`"),
        ("row.supplier_id is null", "has(row.<column>)"),
    ] {
        let d = refused(&format!("  - {{id: r, op: admit, expr: \"{expr}\"}}\n"));
        assert_eq!(d.code, Code::Parse, "{expr}");
        assert!(d.help.unwrap().contains(hint), "{expr}");
    }
}

#[test]
fn type_names_from_other_languages_are_translated() {
    let doc = ContractDoc::parse(&contract(
        "  - {name: amount, type: integer}\n  - {name: cost, type: float64}\n",
        "  - {id: r, op: admit, expr: \"row.cost > 0\"}\n",
    ))
    .unwrap();
    let d = check_contract(&doc, &schema(), &Registry::builtin()).unwrap_err();
    assert_eq!(d[0].code, Code::BadType);
    assert!(
        d[0].help
            .as_deref()
            .unwrap()
            .starts_with("did you mean `int64`?")
    );
}

#[test]
fn an_empty_section_is_named() {
    let d = ContractDoc::parse_all(&contract(EXPOSE, "")).unwrap_err();
    assert_eq!(d[0].message, "`rules:` is empty");
}

#[test]
fn every_document_problem_at_once() {
    let src = contract(
        "  - {name:amount, type: int64}\n  - {name:cost, type: float64}\n",
        "  - id: a\n    op: assert\n    expr: \"row.amount > 0\"\n  - id: b\n    op: decide\n    expr: \"true\"\n    on_fail: deny\n",
    );
    let d = ContractDoc::parse_all(&src).unwrap_err();
    assert_eq!(d.len(), 3, "{d:#?}");
    assert!(d[0].help.as_deref().unwrap().contains("line 6"), "{d:#?}");
    assert_eq!((d[1].rule.as_deref(), d[1].line), (Some("a"), Some(8)));
    assert_eq!((d[2].rule.as_deref(), d[2].line), (Some("b"), Some(14)));
}
