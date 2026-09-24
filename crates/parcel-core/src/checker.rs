//! The Check pass for one expression: CEL AST → typed [`TExpr`].
//!
//! Types every node bottom-up, resolves identifiers against the three
//! namespaces, pins registry calls, recognises macro expansions, and rejects
//! anything outside the parcel profile (design section 6). It does not know
//! which operation the expression belongs to; that is the classify pass.

use std::collections::{BTreeMap, BTreeSet};

use cel::common::ast::{CallExpr, ComprehensionExpr, Expr, IdedExpr, LiteralValue, operators};
use cel::parser::Parser;

use crate::diag::{Code, Diagnostic};
use crate::ir::*;
use crate::registry::Registry;
use crate::types::Type;

/// Everything an expression may refer to.
pub struct Env<'a> {
    /// The bound schema: every column, and either its parcel type or why rules cannot read it.
    pub columns: &'a BTreeMap<String, Result<Type, String>>,
    /// Ids of `assert` rules, which `dataset.assertions.<id>.pass_rate` may name.
    pub assertions: &'a BTreeSet<String>,
    pub registry: &'a Registry,
}

/// Parse and check one CEL expression.
pub fn check_expr(env: &Env, rule: &str, source: &str) -> Result<TExpr, Vec<Diagnostic>> {
    let ast = Parser::new().parse(source).map_err(|errs| {
        vec![Diagnostic::new(
            Code::Parse,
            Some(rule),
            format!("cannot parse `{source}`: {errs}"),
        )]
    })?;
    let mut c = Checker {
        env,
        rule,
        diags: Vec::new(),
        scope: Vec::new(),
    };
    match c.expr(&ast) {
        Some(e) if c.diags.is_empty() => Ok(e),
        _ => {
            if c.diags.is_empty() {
                c.err(
                    Code::OutsideProfile,
                    format!("`{source}` could not be checked"),
                );
            }
            Err(c.diags)
        }
    }
}

struct Checker<'a, 'e> {
    env: &'a Env<'e>,
    rule: &'a str,
    diags: Vec<Diagnostic>,
    /// Comprehension variables in scope, innermost last.
    scope: Vec<(String, Type)>,
}

type R = Option<TExpr>;

impl Checker<'_, '_> {
    fn err(&mut self, code: Code, msg: impl Into<String>) -> R {
        self.diags.push(Diagnostic::new(code, Some(self.rule), msg));
        None
    }

    fn expr(&mut self, e: &IdedExpr) -> R {
        match &e.expr {
            Expr::Literal(l) => self.literal(l),
            Expr::Ident(name) => self.ident(name),
            Expr::Select(s) => {
                if s.test {
                    return self.has(e);
                }
                self.path(e)
            }
            Expr::List(l) => self.list(&l.elements),
            Expr::Call(c) => self.call(c),
            Expr::Comprehension(c) => self.comprehension(c),
            Expr::Map(_) => self.err(
                Code::OutsideProfile,
                "map literals are not in the parcel profile",
            ),
            Expr::Struct(_) => self.err(
                Code::OutsideProfile,
                "message literals are not in the parcel profile",
            ),
            Expr::Unspecified => self.err(Code::Parse, "empty expression"),
        }
    }

    fn literal(&mut self, l: &LiteralValue) -> R {
        let (lit, ty) = match l {
            LiteralValue::Boolean(b) => (Lit::Bool(*b.inner()), Type::Bool),
            LiteralValue::Int(i) => (Lit::Int(*i.inner()), Type::Int),
            LiteralValue::UInt(u) => (Lit::Uint(*u.inner()), Type::Uint),
            LiteralValue::Double(d) => (Lit::Double(*d.inner()), Type::Double),
            LiteralValue::String(s) => (Lit::String(s.inner().to_owned()), Type::String),
            LiteralValue::Bytes(b) => (Lit::Bytes(b.inner().to_vec()), Type::Bytes),
            LiteralValue::Null => {
                return self.err(
                    Code::OutsideProfile,
                    "`null` is not in the parcel profile; test presence with has(row.<column>)",
                );
            }
        };
        Some(TExpr::new(ExprKind::Lit(lit), ty))
    }

    fn ident(&mut self, name: &str) -> R {
        if let Some((_, ty)) = self.scope.iter().rev().find(|(n, _)| n == name) {
            return Some(TExpr::new(ExprKind::Local(name.to_owned()), ty.clone()));
        }
        match name {
            "row" | "ctx" | "dataset" => self.err(
                Code::OutsideProfile,
                format!(
                    "`{name}` is a namespace; refer to one of its fields, e.g. `{name}.<field>`"
                ),
            ),
            _ => self.err(
                Code::UnknownIdentifier,
                format!(
                    "unknown identifier `{name}`; variables live under `row.`, `ctx.` or `dataset.`"
                ),
            ),
        }
    }

    /// Flatten a chain of field selections rooted at an identifier: `dataset.a.b` → ["dataset","a","b"].
    fn select_path(e: &IdedExpr) -> Option<Vec<String>> {
        match &e.expr {
            Expr::Ident(n) => Some(vec![n.clone()]),
            Expr::Select(s) if !s.test => {
                let mut p = Self::select_path(&s.operand)?;
                p.push(s.field.clone());
                Some(p)
            }
            _ => None,
        }
    }

    fn path(&mut self, e: &IdedExpr) -> R {
        let Some(path) = Self::select_path(e) else {
            return self.err(
                Code::OutsideProfile,
                "field selection is only allowed on `row`, `ctx` and `dataset`",
            );
        };
        let p: Vec<&str> = path.iter().map(String::as_str).collect();
        if self.scope.iter().any(|(n, _)| n == p[0]) {
            return self.err(
                Code::OutsideProfile,
                format!(
                    "`{}` is a list element; struct fields are not in parcel v0",
                    p[0]
                ),
            );
        }
        if p.get(1) == Some(&"other") {
            return self.err(
                Code::Unsupported,
                format!(
                    "`{}.other` extension fields are not supported in parcel v0",
                    p[0]
                ),
            );
        }
        match p.as_slice() {
            ["row", col] => self.row_var(col),
            ["row", col, ..] => self.err(
                Code::OutsideProfile,
                format!(
                    "`row.{col}` is not a struct; nested fields are not supported in parcel v0"
                ),
            ),
            ["ctx", field] => match CtxField::from_name(field) {
                Some(f) => Some(TExpr::new(ExprKind::Var(Var::Ctx(f)), f.ty())),
                None => self.err(
                    Code::UnknownField,
                    format!(
                        "`ctx.{field}` is not a caller field; known fields: {}",
                        CtxField::ALL.map(CtxField::name).join(", ")
                    ),
                ),
            },
            ["dataset", rest @ ..] => self.dataset_var(rest),
            [root, ..] => self.ident(root),
            [] => unreachable!("a select path always has a root"),
        }
    }

    fn row_var(&mut self, col: &str) -> R {
        match self.env.columns.get(col) {
            Some(Ok(ty)) => Some(TExpr::new(
                ExprKind::Var(Var::Row(col.to_owned())),
                ty.clone(),
            )),
            Some(Err(why)) => self.err(Code::UnreadableColumn, format!("`row.{col}`: {why}")),
            None => self.err(
                Code::UnknownColumn,
                format!("`row.{col}`: the bound schema has no column `{col}`"),
            ),
        }
    }

    fn dataset_var(&mut self, rest: &[&str]) -> R {
        let (field, ty) = match rest {
            ["row_count"] => (DatasetField::RowCount, Type::Int),
            ["written_at"] => (DatasetField::WrittenAt, Type::Timestamp),
            ["contract_hash"] => (DatasetField::ContractHash, Type::String),
            ["assertions", id, "pass_rate"] => {
                if !self.env.assertions.contains(*id) {
                    return self.err(
                        Code::UnknownField,
                        format!(
                            "`dataset.assertions.{id}`: this contract has no assert rule `{id}`"
                        ),
                    );
                }
                (
                    DatasetField::AssertionPassRate {
                        assertion: (*id).to_owned(),
                    },
                    Type::Double,
                )
            }
            [col, stat_name] => {
                let Some(stat) = ColumnStat::from_name(stat_name) else {
                    return self.err(
                        Code::UnknownField,
                        format!("`dataset.{col}.{stat_name}`: statistics are null_count, null_rate, distinct_count, min, max"),
                    );
                };
                let ty = match stat {
                    ColumnStat::NullCount | ColumnStat::DistinctCount => Type::Int,
                    ColumnStat::NullRate => Type::Double,
                    ColumnStat::Min | ColumnStat::Max => match self.env.columns.get(*col) {
                        Some(Ok(t)) if t.is_ordered() => t.clone(),
                        Some(Ok(t)) => {
                            return self.err(
                                Code::TypeMismatch,
                                format!("`dataset.{col}.{stat_name}`: {t} has no order"),
                            );
                        }
                        Some(Err(why)) => {
                            return self
                                .err(Code::UnreadableColumn, format!("`dataset.{col}`: {why}"));
                        }
                        None => {
                            return self.err(
                                Code::UnknownColumn,
                                format!("`dataset.{col}`: no column `{col}`"),
                            );
                        }
                    },
                };
                if !self.env.columns.contains_key(*col) {
                    return self.err(
                        Code::UnknownColumn,
                        format!("`dataset.{col}`: the bound schema has no column `{col}`"),
                    );
                }
                (
                    DatasetField::Column {
                        column: (*col).to_owned(),
                        stat,
                    },
                    ty,
                )
            }
            _ => {
                return self.err(
                    Code::UnknownField,
                    format!(
                        "`dataset.{}` is not a dataset field; use row_count, written_at, contract_hash, \
                         <column>.<stat> or assertions.<id>.pass_rate",
                        rest.join(".")
                    ),
                );
            }
        };
        Some(TExpr::new(ExprKind::Var(Var::Dataset(field)), ty))
    }

    fn has(&mut self, e: &IdedExpr) -> R {
        let Expr::Select(s) = &e.expr else {
            unreachable!()
        };
        match Self::select_path(&s.operand).as_deref() {
            Some([root]) if root == "row" => {
                self.row_var(&s.field)?;
                Some(TExpr::new(ExprKind::Has(s.field.clone()), Type::Bool))
            }
            _ => self.err(
                Code::OutsideProfile,
                "has() is only defined on row fields, e.g. has(row.order_id)",
            ),
        }
    }

    fn list(&mut self, elements: &[IdedExpr]) -> R {
        let items: Vec<TExpr> = elements
            .iter()
            .map(|x| self.expr(x))
            .collect::<Option<_>>()?;
        let Some(first) = items.first() else {
            return self.err(
                Code::OutsideProfile,
                "empty list literals have no type; parcel needs at least one element",
            );
        };
        let elem = first.ty.clone();
        if let Some(bad) = items.iter().find(|i| i.ty != elem) {
            return self.err(
                Code::TypeMismatch,
                format!("list mixes {elem} and {}", bad.ty),
            );
        }
        Some(TExpr::new(ExprKind::List(items), Type::list(elem)))
    }

    fn call(&mut self, c: &CallExpr) -> R {
        let name = c.func_name.as_str();
        let binary = |op| Some(op);
        let bin = match name {
            operators::LOGICAL_AND => binary(BinOp::And),
            operators::LOGICAL_OR => binary(BinOp::Or),
            operators::EQUALS => binary(BinOp::Eq),
            operators::NOT_EQUALS => binary(BinOp::Ne),
            operators::LESS => binary(BinOp::Lt),
            operators::LESS_EQUALS => binary(BinOp::Le),
            operators::GREATER => binary(BinOp::Gt),
            operators::GREATER_EQUALS => binary(BinOp::Ge),
            operators::ADD => binary(BinOp::Add),
            operators::SUBSTRACT => binary(BinOp::Sub),
            operators::MULTIPLY => binary(BinOp::Mul),
            operators::DIVIDE => binary(BinOp::Div),
            operators::MODULO => binary(BinOp::Mod),
            operators::IN => binary(BinOp::In),
            _ => None,
        };
        if let Some(op) = bin {
            let [a, b] = c.args.as_slice() else {
                return self.err(Code::Parse, format!("{name} takes two operands"));
            };
            let (a, b) = (self.expr(a), self.expr(b));
            return self.binary(op, a?, b?);
        }
        match name {
            operators::LOGICAL_NOT => {
                let x = self.expr(&c.args[0])?;
                self.expect(&x, &Type::Bool, "operand of `!`")?;
                Some(TExpr::new(ExprKind::Not(Box::new(x)), Type::Bool))
            }
            operators::NEGATE => {
                let x = self.expr(&c.args[0])?;
                if !matches!(x.ty, Type::Int | Type::Double | Type::Duration) {
                    return self.err(Code::TypeMismatch, format!("cannot negate {}", x.ty));
                }
                let ty = x.ty.clone();
                Some(TExpr::new(ExprKind::Neg(Box::new(x)), ty))
            }
            operators::CONDITIONAL => {
                let (a, b, d) = (
                    self.expr(&c.args[0]),
                    self.expr(&c.args[1]),
                    self.expr(&c.args[2]),
                );
                let (a, b, d) = (a?, b?, d?);
                self.expect(&a, &Type::Bool, "condition of `?:`")?;
                if b.ty != d.ty {
                    return self.err(
                        Code::TypeMismatch,
                        format!("`?:` branches have different types: {} and {}", b.ty, d.ty),
                    );
                }
                let ty = b.ty.clone();
                Some(TExpr::new(
                    ExprKind::Cond(Box::new(a), Box::new(b), Box::new(d)),
                    ty,
                ))
            }
            operators::INDEX | operators::OPT_INDEX | operators::OPT_SELECT => self.err(
                Code::OutsideProfile,
                "indexing is not in the parcel profile",
            ),
            _ => self.function(c),
        }
    }

    fn expect(&mut self, e: &TExpr, ty: &Type, what: &str) -> Option<()> {
        if &e.ty == ty {
            Some(())
        } else {
            self.err(
                Code::TypeMismatch,
                format!("{what} must be {ty}, found {}", e.ty),
            );
            None
        }
    }

    fn binary(&mut self, op: BinOp, a: TExpr, b: TExpr) -> R {
        use Type::*;
        let ty = match op {
            BinOp::And | BinOp::Or => {
                self.expect(&a, &Bool, "operand of a logical operator")?;
                self.expect(&b, &Bool, "operand of a logical operator")?;
                Bool
            }
            BinOp::Eq | BinOp::Ne => {
                if a.ty != b.ty {
                    return self.err(Code::TypeMismatch, mismatch("compare", &a.ty, &b.ty));
                }
                if matches!(a.ty, List(_)) {
                    return self.err(
                        Code::OutsideProfile,
                        "list equality is not in the parcel profile",
                    );
                }
                Bool
            }
            BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                if a.ty != b.ty {
                    return self.err(Code::TypeMismatch, mismatch("compare", &a.ty, &b.ty));
                }
                if !a.ty.is_ordered() && a.ty != Bool {
                    return self.err(
                        Code::TypeMismatch,
                        format!("{} values cannot be ordered", a.ty),
                    );
                }
                Bool
            }
            BinOp::In => {
                let List(elem) = &b.ty else {
                    return self.err(
                        Code::TypeMismatch,
                        format!("right side of `in` must be a list, found {}", b.ty),
                    );
                };
                if **elem != a.ty {
                    return self.err(
                        Code::TypeMismatch,
                        format!("`in` tests a {} against a list<{elem}>", a.ty),
                    );
                }
                Bool
            }
            BinOp::Add | BinOp::Sub => match (&a.ty, &b.ty) {
                (Int, Int) | (Uint, Uint) | (Double, Double) | (Duration, Duration) => a.ty.clone(),
                (String, String) if op == BinOp::Add => String,
                (Timestamp, Duration) => Timestamp,
                (Duration, Timestamp) if op == BinOp::Add => Timestamp,
                (Timestamp, Timestamp) if op == BinOp::Sub => Duration,
                (List(_), List(_)) => {
                    return self.err(
                        Code::OutsideProfile,
                        "list concatenation is not in the parcel profile",
                    );
                }
                _ => {
                    return self.err(
                        Code::TypeMismatch,
                        mismatch(
                            if op == BinOp::Add { "add" } else { "subtract" },
                            &a.ty,
                            &b.ty,
                        ),
                    );
                }
            },
            BinOp::Mul | BinOp::Div => match (&a.ty, &b.ty) {
                (Int, Int) | (Uint, Uint) | (Double, Double) => a.ty.clone(),
                _ => {
                    return self.err(
                        Code::TypeMismatch,
                        mismatch("multiply or divide", &a.ty, &b.ty),
                    );
                }
            },
            BinOp::Mod => match (&a.ty, &b.ty) {
                (Int, Int) | (Uint, Uint) => a.ty.clone(),
                _ => {
                    return self.err(
                        Code::TypeMismatch,
                        mismatch("take the modulus of", &a.ty, &b.ty),
                    );
                }
            },
        };
        Some(TExpr::new(
            ExprKind::Binary(op, Box::new(a), Box::new(b)),
            ty,
        ))
    }

    /// Profile functions (size, string methods, conversions, timestamp accessors) and registry calls.
    fn function(&mut self, c: &CallExpr) -> R {
        let name = c.func_name.as_str();
        let mut args: Vec<TExpr> = Vec::new();
        if let Some(t) = &c.target {
            args.push(self.expr(t)?);
        }
        for a in &c.args {
            args.push(self.expr(a)?);
        }
        let method = c.target.is_some();
        let tys: Vec<&Type> = args.iter().map(|a| &a.ty).collect();
        use Type::*;
        let found: Option<(Builtin, Type)> = match (name, method, tys.as_slice()) {
            ("size", _, [String | Bytes | List(_)]) => Some((Builtin::Size, Int)),
            ("startsWith", true, [String, String]) => Some((Builtin::StartsWith, Bool)),
            ("endsWith", true, [String, String]) => Some((Builtin::EndsWith, Bool)),
            ("contains", true, [String, String]) => Some((Builtin::Contains, Bool)),
            ("matches", _, [String, String]) => {
                // The pattern must be a literal: it is validated now and becomes a literal regex in the plan.
                let ExprKind::Lit(Lit::String(pat)) = &args[1].kind else {
                    return self.err(
                        Code::OutsideProfile,
                        "the pattern given to matches() must be a string literal",
                    );
                };
                if let Err(e) = regex_syntax::parse(pat) {
                    return self.err(
                        Code::TypeMismatch,
                        format!("invalid regular expression `{pat}`: {e}"),
                    );
                }
                Some((Builtin::Matches, Bool))
            }
            ("int", false, [Int | Uint | Double | String | Timestamp]) => {
                Some((Builtin::ToInt, Int))
            }
            ("uint", false, [Int | Uint | Double | String]) => Some((Builtin::ToUint, Uint)),
            ("double", false, [Int | Uint | Double | String]) => Some((Builtin::ToDouble, Double)),
            ("string", false, [Int | Uint | Double | String | Bool | Timestamp | Duration]) => {
                Some((Builtin::ToString, String))
            }
            ("timestamp", false, [String | Timestamp]) => Some((Builtin::ToTimestamp, Timestamp)),
            ("duration", false, [String | Duration]) => Some((Builtin::ToDuration, Duration)),
            ("getFullYear", true, [Timestamp]) => Some((Builtin::GetFullYear, Int)),
            ("getMonth", true, [Timestamp]) => Some((Builtin::GetMonth, Int)),
            ("getDayOfMonth", true, [Timestamp]) => Some((Builtin::GetDayOfMonth, Int)),
            ("getDayOfWeek", true, [Timestamp]) => Some((Builtin::GetDayOfWeek, Int)),
            ("getDayOfYear", true, [Timestamp]) => Some((Builtin::GetDayOfYear, Int)),
            ("getHours", true, [Timestamp | Duration]) => Some((Builtin::GetHours, Int)),
            ("getMinutes", true, [Timestamp | Duration]) => Some((Builtin::GetMinutes, Int)),
            ("getSeconds", true, [Timestamp | Duration]) => Some((Builtin::GetSeconds, Int)),
            _ => None,
        };
        if let Some((b, ty)) = found {
            if matches!(b, Builtin::ToDuration | Builtin::ToTimestamp)
                && args[0].ty == String
                && !matches!(args[0].kind, ExprKind::Lit(_))
            {
                // Parsing strings per row has no pushdown-safe equivalent; keep it to literals.
                return self.err(
                    Code::OutsideProfile,
                    format!("{name}() of a string requires a string literal"),
                );
            }
            return Some(TExpr::new(ExprKind::Builtin(b, args), ty));
        }
        if is_profile_name(name) {
            let shown: Vec<std::string::String> = tys.iter().map(|t| t.to_string()).collect();
            let how = if method { "method" } else { "function" };
            return self.err(
                Code::NoMatchingOverload,
                format!("no {how} `{name}` takes ({})", shown.join(", ")),
            );
        }
        if method {
            return self.err(
                Code::UnknownFunction,
                format!("`.{name}()` is not a method in the parcel profile"),
            );
        }
        let Some(entry) = self.env.registry.get(name) else {
            return self.err(
                Code::UnknownFunction,
                format!("`{name}` is not in the function registry"),
            );
        };
        let Some(sig) = entry
            .signatures
            .iter()
            .find(|s| s.args.iter().eq(tys.iter().copied()))
        else {
            let shown: Vec<std::string::String> = tys.iter().map(|t| t.to_string()).collect();
            let have: Vec<std::string::String> = entry
                .signatures
                .iter()
                .map(|s| {
                    format!(
                        "({})",
                        s.args
                            .iter()
                            .map(|t| t.to_string())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                })
                .collect();
            return self.err(
                Code::NoMatchingOverload,
                format!(
                    "`{name}` does not take ({}); it takes {}",
                    shown.join(", "),
                    have.join(" or ")
                ),
            );
        };
        let ty = sig.ret.clone();
        Some(TExpr::new(ExprKind::Call(entry.pin(), args), ty))
    }

    /// Recognise the comprehensions the parser's macro expanders produce.
    fn comprehension(&mut self, c: &ComprehensionExpr) -> R {
        if c.iter_var2.is_some() {
            return self.err(
                Code::OutsideProfile,
                "two-variable comprehensions are not in the parcel profile",
            );
        }
        let kind_body: Option<(MacroKind, &IdedExpr)> = match (&c.accu_init.expr, &c.loop_step.expr)
        {
            (Expr::Literal(LiteralValue::Boolean(b)), Expr::Call(step)) if step.args.len() == 2 => {
                match (*b.inner(), step.func_name.as_str()) {
                    (false, operators::LOGICAL_OR) => Some((MacroKind::Exists, &step.args[1])),
                    (true, operators::LOGICAL_AND) => Some((MacroKind::All, &step.args[1])),
                    _ => None,
                }
            }
            (Expr::List(init), Expr::Call(step)) if init.elements.is_empty() => {
                match (step.func_name.as_str(), step.args.as_slice()) {
                    (
                        operators::ADD,
                        [
                            _,
                            IdedExpr {
                                expr: Expr::List(l),
                                ..
                            },
                        ],
                    ) if l.elements.len() == 1 => Some((MacroKind::Map, &l.elements[0])),
                    (
                        operators::CONDITIONAL,
                        [
                            cond,
                            IdedExpr {
                                expr: Expr::Call(add),
                                ..
                            },
                            _,
                        ],
                    ) => match add.args.as_slice() {
                        [
                            _,
                            IdedExpr {
                                expr: Expr::List(l),
                                ..
                            },
                        ] if l.elements.len() == 1
                            && matches!(&l.elements[0].expr, Expr::Ident(v) if *v == c.iter_var) =>
                        {
                            Some((MacroKind::Filter, cond))
                        }
                        _ => {
                            return self.err(
                                Code::OutsideProfile,
                                "map() with a filter argument is not in the parcel profile",
                            );
                        }
                    },
                    _ => None,
                }
            }
            _ => None,
        };
        let Some((kind, body)) = kind_body else {
            return self.err(
                Code::OutsideProfile,
                "only exists, all, filter and map comprehensions are in the parcel profile",
            );
        };
        let range = self.expr(&c.iter_range)?;
        let Type::List(elem) = &range.ty else {
            return self.err(
                Code::OutsideProfile,
                format!("comprehensions range over lists only, found {}", range.ty),
            );
        };
        if matches!(c.iter_var.as_str(), "row" | "ctx" | "dataset") {
            return self.err(
                Code::OutsideProfile,
                format!(
                    "`{}` cannot be used as a comprehension variable",
                    c.iter_var
                ),
            );
        }
        self.scope.push((c.iter_var.clone(), (**elem).clone()));
        let body = self.expr(body);
        self.scope.pop();
        let body = body?;
        let ty = match kind {
            MacroKind::Exists | MacroKind::All | MacroKind::Filter => {
                self.expect(&body, &Type::Bool, "the predicate of a comprehension")?;
                if kind == MacroKind::Filter {
                    range.ty.clone()
                } else {
                    Type::Bool
                }
            }
            MacroKind::Map => Type::list(body.ty.clone()),
        };
        Some(TExpr::new(
            ExprKind::Macro {
                kind,
                var: c.iter_var.clone(),
                range: Box::new(range),
                body: Box::new(body),
            },
            ty,
        ))
    }
}

fn is_profile_name(name: &str) -> bool {
    matches!(
        name,
        "size"
            | "startsWith"
            | "endsWith"
            | "contains"
            | "matches"
            | "int"
            | "uint"
            | "double"
            | "string"
            | "timestamp"
            | "duration"
            | "getFullYear"
            | "getMonth"
            | "getDayOfMonth"
            | "getDayOfWeek"
            | "getDayOfYear"
            | "getHours"
            | "getMinutes"
            | "getSeconds"
    )
}

fn mismatch(verb: &str, a: &Type, b: &Type) -> String {
    let hint = match (a, b) {
        (Type::Int, Type::Double) | (Type::Double, Type::Int) => {
            "; convert one side with double() or int()"
        }
        (Type::Int, Type::Uint) | (Type::Uint, Type::Int) => {
            "; convert one side with int() or uint()"
        }
        _ => "",
    };
    format!("cannot {verb} {a} and {b}{hint}")
}
