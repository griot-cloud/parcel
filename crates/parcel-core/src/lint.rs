//! Lint: whether a contract is written well, from the document alone.
//!
//! Linting needs no data, no column types and no functions. It reports problems the file
//! itself shows (a malformed rule id, a type name that does not exist, an expression that does
//! not parse, an operation reading a namespace it may not) and lints: rules that are legal but
//! probably not what their author meant (warnings), or something worth adding (notes).
//! Whether the rules type-check against the data's columns is `validate`'s job
//! ([`crate::check_contract`]), which a clean lint lets run.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use crate::check::{ExposedColumn, namespace_problem, rule_id_problems, shape_op, type_help};
use crate::diag::{Code, Diagnostic, did_you_mean};
use crate::document::{ContractDoc, Rule};
use crate::ir::CtxField;
use crate::syntax::{Reads, reads};
use crate::types::parse_type_name;

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

/// What linting found: problems, which `validate` would also refuse, and lints, which it
/// would not.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Report {
    pub problems: Vec<Diagnostic>,
    pub lints: Vec<Lint>,
}

/// Lint a contract as written. `doc` is not resolved against its parents, so a child
/// contract's checks that need the parent's `expose` or `extensions` are left to `validate`.
pub fn lint(doc: &ContractDoc) -> Report {
    let mut problems = Vec::new();
    let child = doc.inherits.is_some();

    if let Some(terms) = &doc.residency
        && let Err(message) = terms.validate()
    {
        problems.push(Diagnostic::new(Code::Residency, None, message));
    }
    problems.extend(rule_id_problems(&doc.rules));

    // Type names, in expose and extensions.
    let mut exposed = Vec::new();
    let mut names = BTreeSet::new();
    for col in doc.expose.iter().flatten() {
        if !names.insert(col.name.as_str()) {
            problems.push(
                Diagnostic::new(
                    Code::DuplicateExpose,
                    None,
                    format!("column `{}` is exposed twice", col.name),
                )
                .with_help("remove one"),
            );
            continue;
        }
        match parse_type_name(&col.type_name) {
            Ok(t) => exposed.push(ExposedColumn {
                name: col.name.clone(),
                type_name: col.type_name.clone(),
                data_type: t,
            }),
            Err(msg) => problems.push(
                Diagnostic::new(Code::BadType, None, format!("expose `{}`: {msg}", col.name))
                    .with_help(type_help(&col.type_name)),
            ),
        }
    }
    let declared = |ns: &str| -> BTreeSet<String> {
        let x = doc.extensions.as_ref();
        let m = match ns {
            "row" => x.map(|x| &x.row),
            "ctx" => x.map(|x| &x.ctx),
            _ => x.map(|x| &x.dataset),
        };
        m.map(|m| m.keys().cloned().collect()).unwrap_or_default()
    };
    if let Some(x) = &doc.extensions {
        for (ns, decl) in [("row", &x.row), ("ctx", &x.ctx), ("dataset", &x.dataset)] {
            for (field, type_name) in decl {
                if let Err(msg) =
                    parse_type_name(type_name).and_then(|t| crate::types::Type::from_arrow(&t))
                {
                    problems.push(
                        Diagnostic::new(
                            Code::BadType,
                            None,
                            format!("extensions.{ns}.{field}: {msg}"),
                        )
                        .with_help(type_help(type_name)),
                    );
                }
            }
        }
    }

    // Each rule: it parses, reads only what its operation may, names real caller fields,
    // and targets exposed columns.
    let mut transformed: BTreeMap<&str, &str> = BTreeMap::new();
    let mut read: Vec<(&Rule, Reads)> = Vec::new();
    for rule in &doc.rules {
        let id = rule.id();
        let source = match rule {
            Rule::Decide(r) => Some(&r.expr),
            Rule::Admit(r) => Some(&r.expr),
            Rule::Assert(r) => Some(&r.expr),
            Rule::Transform(r) => Some(&r.expr),
            Rule::Guarantee(r) => Some(&r.expr),
            Rule::Shape(r) => r.unless.as_ref(),
        };
        if let Rule::Transform(t) = rule {
            if let Some(prev) = transformed.insert(&t.column, id) {
                problems.push(Diagnostic::new(
                    Code::DuplicateTransform,
                    Some(id),
                    format!("column `{}` already has a transform, `{prev}`", t.column),
                ));
            }
            if doc.expose.is_some() && !exposed.iter().any(|c| c.name == t.column) {
                let help = match did_you_mean(&t.column, exposed.iter().map(|c| c.name.as_str())) {
                    Some(c) => format!("did you mean `{c}`?"),
                    None => format!("expose `{}` first", t.column),
                };
                problems.push(
                    Diagnostic::new(
                        Code::TransformTarget,
                        Some(id),
                        format!("transform target `{}` is not an exposed column", t.column),
                    )
                    .with_help(help),
                );
            }
        }
        if let Rule::Shape(s) = rule
            && doc.expose.is_some()
            && let Err(msg) = shape_op(s, &exposed)
        {
            problems.push(Diagnostic::new(Code::ShapeParams, Some(id), msg));
        }
        let Some(source) = source else { continue };
        let r = match reads(id, source) {
            Ok(r) => r,
            Err(d) => {
                problems.push(d);
                continue;
            }
        };
        if let Some(d) = namespace_problem(rule, source, r.ns, r.ctx_beyond_now()) {
            problems.push(d);
        }
        for field in &r.ctx {
            if let Some(other) = field.strip_prefix("other.") {
                if !child && !declared("ctx").contains(other) {
                    problems.push(
                        Diagnostic::new(
                            Code::UnknownField,
                            Some(id),
                            format!("`ctx.other.{other}` is not declared"),
                        )
                        .with_help(format!("add `{other}: <type>` under `extensions.ctx`")),
                    );
                }
            } else if CtxField::from_name(field).is_none() {
                let names = CtxField::ALL.map(CtxField::name);
                let help = match did_you_mean(field, names) {
                    Some(n) => format!("did you mean `ctx.{n}`?"),
                    None => format!(
                        "fields: {}; declare others under `extensions.ctx`, read as `ctx.other.<field>`",
                        names.join(", ")
                    ),
                };
                problems.push(
                    Diagnostic::new(
                        Code::UnknownField,
                        Some(id),
                        format!("`ctx.{field}` is not a caller field"),
                    )
                    .with_help(help),
                );
            }
        }
        for field in &r.row {
            if let Some(other) = field.strip_prefix("other.")
                && !child
                && !declared("row").contains(other)
            {
                problems.push(
                    Diagnostic::new(
                        Code::UnknownField,
                        Some(id),
                        format!("`row.other.{other}` is not declared"),
                    )
                    .with_help(format!("add `{other}: <type>` under `extensions.row`")),
                );
            }
        }
        read.push((rule, r));
    }

    // Enrichers compute stored values from the row alone.
    let mut produced = Vec::new();
    for e in doc.enrich.iter().flatten() {
        match reads(&e.field, &e.expr) {
            Ok(r) if r.ns.ctx || r.ns.dataset => problems.push(Diagnostic::new(
                Code::Namespace,
                None,
                format!(
                    "enrich `{}`: enrichers compute stored values and may read only row",
                    e.field
                ),
            )),
            Ok(r) => produced.push(r),
            Err(d) => problems.push(d),
        }
    }
    for p in doc.dataset_other.iter().flatten() {
        if let Some(src) = &p.expr {
            match reads(&p.field, src) {
                Ok(r) => produced.push(r),
                Err(d) => problems.push(d),
            }
        }
    }

    let lints = lints(doc, &read, &produced, &declared);
    Report { problems, lints }
}

fn lints(
    doc: &ContractDoc,
    read: &[(&Rule, Reads)],
    produced: &[Reads],
    declared: &dyn Fn(&str) -> BTreeSet<String>,
) -> Vec<Lint> {
    use Severity::*;
    let mut out = Vec::new();
    let source = |rule: &Rule| match rule {
        Rule::Decide(r) => r.expr.clone(),
        Rule::Admit(r) => r.expr.clone(),
        Rule::Assert(r) => r.expr.clone(),
        Rule::Transform(r) => r.expr.clone(),
        Rule::Guarantee(r) => r.expr.clone(),
        Rule::Shape(r) => r.unless.clone().unwrap_or_default(),
    };

    for (rule, r) in read {
        let id = rule.id();
        if matches!(rule, Rule::Admit(_)) && !r.ns.row {
            out.push(Lint::new(
                "admit-reads-only-ctx",
                Warning,
                Some(id),
                format!("`{id}` reads only ctx: `{}`", source(rule)),
                "use `op: decide`",
            ));
        }
        if matches!(
            rule,
            Rule::Decide(_) | Rule::Admit(_) | Rule::Assert(_) | Rule::Guarantee(_)
        ) && r.ns.is_empty()
            && !r.calls
        {
            out.push(Lint::new(
                "constant-rule",
                Warning,
                Some(id),
                format!("`{id}` reads no field: `{}`", source(rule)),
                "read `row.<column>`, `ctx.<field>` or `dataset.<field>`",
            ));
        }
    }

    // Two rules with the same operation and condition, ignoring spacing.
    let mut seen: BTreeMap<(&str, String), &str> = BTreeMap::new();
    for (rule, _) in read {
        if matches!(rule, Rule::Transform(_) | Rule::Shape(_)) {
            continue;
        }
        let condition: String = source(rule).split_whitespace().collect();
        if let Some(first) = seen.insert((rule.op_name(), condition), rule.id()) {
            out.push(Lint::new(
                "duplicate-condition",
                Warning,
                Some(rule.id()),
                format!("`{}` repeats `{first}`", rule.id()),
                "remove one",
            ));
        }
    }

    // Caller conditions that no caller can meet together: `ctx.tenant == 'a'` and `== 'b'`.
    let mut required: BTreeMap<&str, (&str, &str)> = BTreeMap::new();
    for (rule, r) in read {
        if !matches!(rule, Rule::Decide(_) | Rule::Admit(_)) || r.ns.row {
            continue;
        }
        for (field, value) in &r.ctx_equalities {
            match required.get(field.as_str()) {
                Some((other, by)) if other != value => out.push(Lint::new(
                    "unsatisfiable-callers",
                    Warning,
                    Some(rule.id()),
                    format!(
                        "`{by}` needs ctx.{field} == '{other}', `{}` needs ctx.{field} == '{value}'",
                        rule.id()
                    ),
                    format!("to allow either: `ctx.{field} == '{other}' || ctx.{field} == '{value}'`"),
                )),
                Some(_) => {}
                None => {
                    required.insert(field, (value, rule.id()));
                }
            }
        }
    }

    // Declared extension fields nothing reads.
    let all = read.iter().map(|(_, r)| r).chain(produced);
    let mut used: BTreeSet<(&str, String)> = BTreeSet::new();
    for r in all {
        for f in &r.row {
            if let Some(o) = f.strip_prefix("other.") {
                used.insert(("row", o.to_owned()));
            }
        }
        for f in &r.ctx {
            if let Some(o) = f.strip_prefix("other.") {
                used.insert(("ctx", o.to_owned()));
            }
        }
        for f in &r.dataset_other {
            used.insert(("dataset", f.clone()));
        }
    }
    for ns in ["row", "ctx", "dataset"] {
        for f in declared(ns) {
            if !used.contains(&(ns, f.clone())) {
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

    let child = doc.inherits.is_some();
    let has_access = doc
        .rules
        .iter()
        .any(|r| matches!(r, Rule::Decide(_) | Rule::Admit(_)));
    if !has_access && !child {
        out.push(Lint::new(
            "open-access",
            Note,
            None,
            "no decide or admit rules: every caller reads every row".into(),
            "add `op: decide` or `op: admit`",
        ));
    }
    if doc.owner.is_none() && !child {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn report(head: &str, rules: &str) -> Report {
        let src = format!(
            "contract: sales/orders\nversion: 1.0.0\nowner: acme\nbinding: {{parquet: x/}}\n{head}expose:\n  - {{name: amount, type: int64}}\n  - {{name: email, type: utf8}}\nrules:\n{rules}"
        );
        lint(&ContractDoc::parse(&src).unwrap())
    }

    fn names(r: &Report) -> Vec<&str> {
        r.lints.iter().map(|l| l.name).collect()
    }

    fn codes(r: &Report) -> Vec<Code> {
        r.problems.iter().map(|d| d.code).collect()
    }

    #[test]
    fn a_ctx_only_admit_is_a_decide() {
        let r = report(
            "",
            "  - {id: mine, op: admit, expr: \"ctx.tenant == 'acme'\"}\n",
        );
        assert!(r.problems.is_empty(), "{r:#?}");
        assert_eq!(names(&r), ["admit-reads-only-ctx"]);
    }

    #[test]
    fn callers_no_one_can_be() {
        let r = report(
            "",
            "  - {id: a, op: decide, expr: \"ctx.tenant == 'acme'\"}\n  - {id: b, op: decide, expr: \"ctx.tenant == 'globex' && ctx.purpose == 'x'\"}\n",
        );
        assert_eq!(names(&r), ["unsatisfiable-callers"]);
        assert_eq!(r.lints[0].rule.as_deref(), Some("b"));
    }

    #[test]
    fn constants_duplicates_and_open_access() {
        let r = report(
            "",
            "  - {id: a, op: assert, expr: \"row.amount > 0\", on_fail: drop}\n  - {id: b, op: assert, expr: \"row.amount>0\", on_fail: report}\n  - {id: c, op: guarantee, expr: \"1 < 2\", on_fail: deny}\n",
        );
        assert_eq!(
            names(&r),
            ["constant-rule", "duplicate-condition", "open-access"]
        );
    }

    #[test]
    fn a_clean_contract_has_nothing_to_report() {
        let r = report(
            "",
            "  - {id: mine, op: admit, expr: \"row.amount > 0 && ctx.tenant == 'acme'\"}\n",
        );
        assert_eq!(r, Report::default());
    }

    #[test]
    fn problems_the_file_shows_without_data() {
        let r = report(
            "",
            "  - {id: who, op: decide, expr: \"row.amount > 0\"}\n  - {id: Bad-Id, op: admit, expr: \"ctx.tennant == 'a'\"}\n  - {id: p, op: assert, expr: \"row.amount > 0 and true\", on_fail: drop}\n  - {id: m, op: transform, column: emial, expr: \"redact(row.email)\"}\n",
        );
        assert_eq!(
            codes(&r),
            [
                Code::InvalidRuleId,
                Code::Namespace,
                Code::UnknownField,
                Code::Parse,
                Code::TransformTarget,
            ]
        );
    }

    #[test]
    fn columns_are_not_lints_business() {
        // Whether `row.nope` exists is a question for the data's schema: `validate`.
        let r = report(
            "",
            "  - {id: a, op: assert, expr: \"row.nope > 0\", on_fail: drop}\n",
        );
        assert!(r.problems.is_empty(), "{r:#?}");
    }

    #[test]
    fn extensions_are_checked_and_used() {
        let r = report(
            "extensions:\n  row: {amount_major: double, unused: int64}\n  ctx: {region: string}\nenrich:\n  - {field: amount_major, expr: \"double(row.amount) / 100.0\"}\n",
            "  - {id: big, op: admit, expr: \"row.other.amount_major > 10.0 && ctx.other.region == 'EA'\"}\n  - {id: x, op: admit, expr: \"ctx.other.zone == 'z'\"}\n",
        );
        assert_eq!(codes(&r), [Code::UnknownField]);
        assert_eq!(names(&r), ["admit-reads-only-ctx", "unused-extension"]);
    }
}
