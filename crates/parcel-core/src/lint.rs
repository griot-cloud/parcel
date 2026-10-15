//! Lints: contracts that compile but probably do not do what their author meant.
//!
//! A diagnostic refuses a contract. A lint never does: it points at a rule that is legal but
//! suspicious (a warning) or at something worth adding (a note), and says what to write instead.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use crate::cel_print::{Style, print};
use crate::check::{CheckedContract, CheckedRule, ProducerKind};
use crate::document::ContractDoc;
use crate::ir::{BinOp, CtxField, DatasetField, ExprKind, Lit, TExpr, Var};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// Probably a mistake.
    Warning,
    /// A suggestion.
    Note,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Lint {
    /// Stable name, e.g. `admit-reads-only-ctx`.
    pub name: &'static str,
    pub severity: Severity,
    pub rule: Option<String>,
    pub message: String,
    pub help: String,
    /// The 1-based line of the document it is about, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,
}

impl Lint {
    fn new(
        name: &'static str,
        severity: Severity,
        rule: Option<&str>,
        message: String,
        help: impl Into<String>,
    ) -> Lint {
        Lint {
            name,
            severity,
            rule: rule.map(str::to_owned),
            message,
            help: help.into(),
            line: None,
        }
    }
}

/// Every lint for a contract that has passed the check. `doc` is the document as written,
/// before inheritance, so notes about it name what its author can change.
pub fn lint(doc: &ContractDoc, c: &CheckedContract) -> Vec<Lint> {
    let mut out = Vec::new();
    use Severity::*;

    for r in &c.rules {
        let id = r.id();
        if let CheckedRule::Admit { expr, .. } = r
            && !expr.expr.ns.row
        {
            out.push(Lint::new(
                "admit-reads-only-ctx",
                Warning,
                Some(id),
                format!("`{id}` reads only ctx: `{}`", expr.source),
                "use `op: decide`",
            ));
        }
        if let CheckedRule::Decide { expr, .. }
        | CheckedRule::Admit { expr, .. }
        | CheckedRule::Assert { expr, .. }
        | CheckedRule::Guarantee { expr, .. } = r
            && expr.expr.ns.is_empty()
            && expr.expr.pins().is_empty()
        {
            out.push(Lint::new(
                "constant-rule",
                Warning,
                Some(id),
                format!("`{id}` reads no field: `{}`", expr.source),
                "read `row.<column>`, `ctx.<field>` or `dataset.<field>`",
            ));
        }
    }

    // Two rules with the same operation and condition.
    let mut seen: BTreeMap<(String, String), &str> = BTreeMap::new();
    for r in &c.rules {
        let op = match r {
            CheckedRule::Decide { .. } => "decide",
            CheckedRule::Admit { .. } => "admit",
            CheckedRule::Assert { .. } => "assert",
            CheckedRule::Guarantee { .. } => "guarantee",
            _ => continue,
        };
        let cel = print(&r.exprs()[0].expr, Style::Canonical);
        if let Some(first) = seen.insert((op.to_owned(), cel), r.id()) {
            out.push(Lint::new(
                "duplicate-condition",
                Warning,
                Some(r.id()),
                format!("`{}` repeats `{first}`", r.id()),
                "remove one",
            ));
        }
    }

    // Caller conditions that no caller can meet together: `ctx.tenant == 'a'` and `== 'b'`.
    let mut required: BTreeMap<CtxField, (String, &str)> = BTreeMap::new();
    for r in &c.rules {
        let (CheckedRule::Decide { expr, .. } | CheckedRule::Admit { expr, .. }) = r else {
            continue;
        };
        if expr.expr.ns.row {
            continue;
        }
        for (field, value) in ctx_equalities(&expr.expr) {
            match required.get(&field) {
                Some((other, by)) if *other != value => out.push(Lint::new(
                    "unsatisfiable-callers",
                    Warning,
                    Some(r.id()),
                    format!(
                        "`{by}` needs ctx.{} == '{other}', `{}` needs ctx.{} == '{value}'",
                        field.name(),
                        r.id(),
                        field.name()
                    ),
                    format!(
                        "to allow either: `ctx.{0} == '{other}' || ctx.{0} == '{value}'`",
                        field.name()
                    ),
                )),
                Some(_) => {}
                None => {
                    required.insert(field, (value, r.id()));
                }
            }
        }
    }

    // Declared extension fields nothing reads.
    let mut read = BTreeSet::new();
    let mut note = |e: &TExpr| {
        e.walk(&mut |n| match &n.kind {
            ExprKind::Var(Var::RowOther(f)) | ExprKind::HasOther(f) => {
                read.insert(("row", f.clone()));
            }
            ExprKind::Var(Var::CtxOther(f)) => {
                read.insert(("ctx", f.clone()));
            }
            ExprKind::Var(Var::Dataset(DatasetField::Other(f))) => {
                read.insert(("dataset", f.clone()));
            }
            _ => {}
        })
    };
    for r in &c.rules {
        for x in r.exprs() {
            note(&x.expr);
        }
    }
    for (_, x) in &c.enrichers {
        note(&x.expr);
    }
    for p in &c.producers {
        if let ProducerKind::Aggregate { arg: Some(x), .. } = &p.kind {
            note(&x.expr);
        }
    }
    for (ns, fields) in [
        ("row", &c.other.row),
        ("ctx", &c.other.ctx),
        ("dataset", &c.other.dataset),
    ] {
        for f in fields.keys() {
            if !read.contains(&(ns, f.clone())) {
                out.push(Lint::new(
                    "unused-extension",
                    Warning,
                    None,
                    format!("`extensions.{ns}.{f}` is never read"),
                    "remove it",
                ));
            }
        }
    }

    let has_access = c
        .rules
        .iter()
        .any(|r| matches!(r, CheckedRule::Decide { .. } | CheckedRule::Admit { .. }));
    if !has_access && doc.inherits.is_none() {
        out.push(Lint::new(
            "open-access",
            Note,
            None,
            "no decide or admit rules: every caller reads every row".into(),
            "add `op: decide` or `op: admit`",
        ));
    }
    if doc.owner.is_none() && doc.inherits.is_none() {
        out.push(Lint::new(
            "no-owner",
            Note,
            None,
            "no `owner`".into(),
            "add `owner: <organisation>`",
        ));
    }
    if !doc.contract.contains('/') {
        out.push(Lint::new(
            "contract-name",
            Note,
            None,
            format!("`{}` has no `<area>/` prefix", doc.contract),
            format!("e.g. `sales/{}`", doc.contract),
        ));
    }
    out
}

/// `ctx.<field> == '<text>'` conditions that must all hold for `e` to be true.
fn ctx_equalities(e: &TExpr) -> Vec<(CtxField, String)> {
    match &e.kind {
        ExprKind::Binary(BinOp::And, a, b) => {
            let mut v = ctx_equalities(a);
            v.extend(ctx_equalities(b));
            v
        }
        ExprKind::Binary(BinOp::Eq, a, b) => match (&a.kind, &b.kind) {
            (ExprKind::Var(Var::Ctx(f)), ExprKind::Lit(Lit::String(s)))
            | (ExprKind::Lit(Lit::String(s)), ExprKind::Var(Var::Ctx(f))) => {
                vec![(*f, s.clone())]
            }
            _ => Vec::new(),
        },
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Registry, check_contract};
    use datafusion_common::arrow::datatypes::{DataType, Field, Schema};

    fn lints(rules: &str) -> Vec<Lint> {
        let src = format!(
            "contract: sales/orders\nversion: 1\nowner: acme\nbinding: {{parquet: x/}}\nexpose:\n  - {{name: amount, type: int64}}\nrules:\n{rules}"
        );
        let doc = ContractDoc::parse(&src).unwrap();
        let schema = Schema::new(vec![Field::new("amount", DataType::Int64, true)]);
        let c = check_contract(&doc, &schema, &Registry::builtin()).unwrap();
        lint(&doc, &c)
    }

    fn names(l: &[Lint]) -> Vec<&str> {
        l.iter().map(|l| l.name).collect()
    }

    #[test]
    fn a_ctx_only_admit_is_a_decide() {
        let l = lints("  - {id: mine, op: admit, expr: \"ctx.tenant == 'acme'\"}\n");
        assert_eq!(names(&l), ["admit-reads-only-ctx"]);
    }

    #[test]
    fn callers_no_one_can_be() {
        let l = lints(
            "  - {id: a, op: decide, expr: \"ctx.tenant == 'acme'\"}\n  - {id: b, op: decide, expr: \"ctx.tenant == 'globex' && ctx.purpose == 'x'\"}\n",
        );
        assert_eq!(names(&l), ["unsatisfiable-callers"]);
        assert_eq!(l[0].rule.as_deref(), Some("b"));
    }

    #[test]
    fn constants_duplicates_and_open_access() {
        let l = lints(
            "  - {id: a, op: assert, expr: \"row.amount > 0\", on_fail: drop}\n  - {id: b, op: assert, expr: \"row.amount > 0\", on_fail: report}\n  - {id: c, op: guarantee, expr: \"1 < 2\", on_fail: deny}\n",
        );
        assert_eq!(
            names(&l),
            ["constant-rule", "duplicate-condition", "open-access"]
        );
    }

    #[test]
    fn a_clean_contract_has_no_lints() {
        let l =
            lints("  - {id: mine, op: admit, expr: \"row.amount > 0 && ctx.tenant == 'acme'\"}\n");
        assert!(l.is_empty(), "{l:#?}");
    }
}
