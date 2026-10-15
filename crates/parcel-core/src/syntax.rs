//! What a rule's expression reads, from its source alone: no schema, no types.
//!
//! `lint` uses this to check a contract without its data. The type checker
//! ([`crate::checker`]) does the same walk with types once column types are known.

use std::collections::BTreeSet;

use cel::common::ast::{EntryExpr, Expr, IdedExpr, LiteralValue, operators};

use crate::diag::Diagnostic;
use crate::ir::NsSet;

/// What one expression reads.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Reads {
    /// The namespaces it reads.
    pub ns: NsSet,
    /// `ctx` fields, with extension fields as `other.<field>`.
    pub ctx: BTreeSet<String>,
    /// `row` columns, with extension fields as `other.<field>`.
    pub row: BTreeSet<String>,
    /// `dataset.other` fields.
    pub dataset_other: BTreeSet<String>,
    /// Whether it calls a registry function (a built-in such as `redact`, or a custom one).
    pub calls: bool,
    /// `ctx.<field> == "<text>"` conditions that must all hold for it to be true.
    pub ctx_equalities: Vec<(String, String)>,
}

impl Reads {
    /// Whether it reads a caller field other than `ctx.now`, which a guarantee may not.
    pub fn ctx_beyond_now(&self) -> bool {
        self.ctx.iter().any(|f| f != "now")
    }
}

/// Parse `source` and say what it reads; a parse error names `rule`.
pub fn reads(rule: &str, source: &str) -> Result<Reads, Diagnostic> {
    let ast = crate::checker::parse(rule, source)?;
    let mut r = Reads::default();
    let mut w = Walk {
        out: &mut r,
        locals: Vec::new(),
    };
    w.expr(&ast);
    r.ctx_equalities = equalities(&ast);
    Ok(r)
}

struct Walk<'a> {
    out: &'a mut Reads,
    /// Comprehension variables in scope.
    locals: Vec<String>,
}

impl Walk<'_> {
    fn expr(&mut self, e: &IdedExpr) {
        match &e.expr {
            Expr::Ident(name) => self.root(name),
            Expr::Select(s) => match path(e) {
                Some(p) if !self.locals.contains(&p[0]) => self.path(&p),
                _ => self.expr(&s.operand),
            },
            Expr::Call(c) => {
                let name = c.func_name.as_str();
                let operator = !name.starts_with(|ch: char| ch.is_ascii_alphabetic());
                if !operator && !crate::checker::PROFILE_NAMES.contains(&name) {
                    self.out.calls = true;
                }
                if let Some(t) = &c.target {
                    self.expr(t);
                }
                for a in &c.args {
                    self.expr(a);
                }
            }
            Expr::List(l) => l.elements.iter().for_each(|x| self.expr(x)),
            Expr::Map(m) => {
                for entry in &m.entries {
                    if let EntryExpr::MapEntry(me) = &entry.expr {
                        self.expr(&me.key);
                        self.expr(&me.value);
                    }
                }
            }
            Expr::Struct(s) => {
                for entry in &s.entries {
                    if let EntryExpr::StructField(f) = &entry.expr {
                        self.expr(&f.value);
                    }
                }
            }
            Expr::Comprehension(c) => {
                self.expr(&c.iter_range);
                self.expr(&c.accu_init);
                let before = self.locals.len();
                self.locals.push(c.iter_var.clone());
                if let Some(v) = &c.iter_var2 {
                    self.locals.push(v.clone());
                }
                self.locals.push(c.accu_var.clone());
                self.expr(&c.loop_cond);
                self.expr(&c.loop_step);
                self.expr(&c.result);
                self.locals.truncate(before);
            }
            Expr::Literal(_) | Expr::Unspecified => {}
        }
    }

    fn root(&mut self, name: &str) {
        if self.locals.iter().any(|l| l == name) {
            return;
        }
        match name {
            "row" => self.out.ns.row = true,
            "ctx" => self.out.ns.ctx = true,
            "dataset" => self.out.ns.dataset = true,
            _ => {}
        }
    }

    /// `ns.field…`, including `has(row.x)`, whose select is a presence test.
    fn path(&mut self, p: &[String]) {
        self.root(&p[0]);
        let field = |i: usize| p.get(i).cloned().unwrap_or_default();
        let named = if p.get(1).map(String::as_str) == Some("other") {
            format!("other.{}", field(2))
        } else {
            field(1)
        };
        if named.is_empty() {
            return;
        }
        match p[0].as_str() {
            "row" => {
                self.out.row.insert(named);
            }
            "ctx" => {
                self.out.ctx.insert(named);
            }
            "dataset" if named.starts_with("other.") => {
                self.out
                    .dataset_other
                    .insert(named.trim_start_matches("other.").to_owned());
            }
            _ => {}
        }
    }
}

/// A chain of field selections rooted at an identifier: `dataset.a.b` → ["dataset","a","b"].
fn path(e: &IdedExpr) -> Option<Vec<String>> {
    match &e.expr {
        Expr::Ident(n) => Some(vec![n.clone()]),
        Expr::Select(s) => {
            let mut p = path(&s.operand)?;
            p.push(s.field.clone());
            Some(p)
        }
        _ => None,
    }
}

/// `ctx.<field> == "<text>"` conditions joined by `&&` at the top of the expression.
fn equalities(e: &IdedExpr) -> Vec<(String, String)> {
    let Expr::Call(c) = &e.expr else {
        return Vec::new();
    };
    match (c.func_name.as_str(), c.args.as_slice()) {
        (operators::LOGICAL_AND, [a, b]) => {
            let mut v = equalities(a);
            v.extend(equalities(b));
            v
        }
        (operators::EQUALS, [a, b]) => {
            let field = |x: &IdedExpr| match path(x).as_deref() {
                Some([ns, f]) if ns == "ctx" => Some(f.clone()),
                _ => None,
            };
            let text = |x: &IdedExpr| match &x.expr {
                Expr::Literal(LiteralValue::String(s)) => Some(s.inner().to_owned()),
                _ => None,
            };
            match (field(a), text(b), field(b), text(a)) {
                (Some(f), Some(t), _, _) | (_, _, Some(f), Some(t)) => vec![(f, t)],
                _ => Vec::new(),
            }
        }
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(src: &str) -> Reads {
        reads("t", src).unwrap()
    }

    #[test]
    fn namespaces_fields_and_calls() {
        let x = r("row.supplier_id == ctx.tenant && has(row.email)");
        assert!(x.ns.row && x.ns.ctx && !x.ns.dataset);
        assert_eq!(x.row, ["email", "supplier_id"].map(String::from).into());
        assert_eq!(x.ctx, ["tenant"].map(String::from).into());
        assert!(!x.calls);
        assert!(r("redact(row.email) == 'x'").calls);
        assert!(!r("row.name.startsWith('A')").calls);
        let x = r("row.other.amount_major > 1.0 && ctx.other.region == 'EA'");
        assert!(x.row.contains("other.amount_major") && x.ctx.contains("other.region"));
        assert!(
            r("dataset.other.avg_amount > 2.0")
                .dataset_other
                .contains("avg_amount")
        );
    }

    #[test]
    fn comprehension_variables_are_not_namespaces() {
        let x = r("row.tags.exists(t, t == 'vip')");
        assert!(x.ns.row && !x.ns.ctx);
        assert_eq!(x.row, ["tags"].map(String::from).into());
    }

    #[test]
    fn constants_and_equalities() {
        let x = r("1 < 2");
        assert!(x.ns.is_empty() && !x.calls);
        let x = r("ctx.tenant == 'acme' && 'x' == ctx.purpose");
        assert_eq!(
            x.ctx_equalities,
            vec![
                ("tenant".into(), "acme".into()),
                ("purpose".into(), "x".into())
            ]
        );
        assert!(!r("ctx.now > timestamp('2026-01-01T00:00:00Z')").ctx_beyond_now());
    }

    #[test]
    fn parse_errors_carry_hints() {
        let d = reads("t", "row.a > 0 and row.b > 0").unwrap_err();
        assert_eq!(d.help.as_deref(), Some("use `&&` and `||`"));
    }
}
