//! Caller-only CEL as engine SQL.
//!
//! A compiled contract keeps every expression that reads only `ctx` as CEL: the `decide`
//! rules, and the placeholders (`$c0`) in its row expressions. These are small: comparisons of
//! caller fields with literals, `in` lists, string tests. This writes them as SQL in which each
//! caller field is read the engine's way. Anything outside that subset is refused with the
//! expression named, never guessed.

use std::collections::BTreeMap;

use cel::common::ast::{Expr, IdedExpr, LiteralValue, operators};
use parcel_core::ir::CtxField;
use parcel_core::types::Type;

use crate::{Engine, Query};

/// Every caller field the query reads, with its parcel type: `tenant`, `other.region`.
pub fn caller_fields(q: &Query) -> BTreeMap<String, Type> {
    let mut out = BTreeMap::new();
    let sources = q
        .params
        .iter()
        .map(|p| p.cel.as_str())
        .chain(q.decisions.iter().map(|d| d.cel.as_str()));
    for src in sources {
        if let Ok(ast) = cel::parser::Parser::new().parse(src) {
            collect(&ast, &q.ctx_other, &mut out);
        }
    }
    out
}

fn collect(e: &IdedExpr, other: &BTreeMap<String, Type>, out: &mut BTreeMap<String, Type>) {
    if let Some((field, ty)) = field(e, other) {
        out.insert(field, ty);
        return;
    }
    match &e.expr {
        Expr::Call(c) => {
            if let Some(t) = &c.target {
                collect(t, other, out);
            }
            c.args.iter().for_each(|a| collect(a, other, out));
        }
        Expr::List(l) => l.elements.iter().for_each(|x| collect(x, other, out)),
        Expr::Select(s) => collect(&s.operand, other, out),
        Expr::Comprehension(c) => {
            for x in [
                &c.iter_range,
                &c.accu_init,
                &c.loop_cond,
                &c.loop_step,
                &c.result,
            ] {
                collect(x, other, out);
            }
        }
        _ => {}
    }
}

/// `ctx.<field>` or `ctx.other.<field>`, with its type.
fn field(e: &IdedExpr, other: &BTreeMap<String, Type>) -> Option<(String, Type)> {
    let Expr::Select(s) = &e.expr else {
        return None;
    };
    if s.test {
        return None;
    }
    match &s.operand.expr {
        Expr::Ident(root) if root == "ctx" => {
            let f = CtxField::from_name(&s.field)?;
            Some((s.field.clone(), f.ty()))
        }
        Expr::Select(inner) if inner.field == "other" && !inner.test => match &inner.operand.expr {
            Expr::Ident(root) if root == "ctx" => {
                let ty = other.get(&s.field)?.clone();
                Some((format!("other.{}", s.field), ty))
            }
            _ => None,
        },
        _ => None,
    }
}

/// `src`, a CEL expression that reads only `ctx`, as SQL for `engine`.
pub fn to_sql(
    src: &str,
    engine: &dyn Engine,
    other: &BTreeMap<String, Type>,
) -> Result<String, String> {
    let ast = cel::parser::Parser::new()
        .parse(src)
        .map_err(|e| format!("cannot parse: {e}"))?;
    Writer { engine, other }.expr(&ast)
}

struct Writer<'a> {
    engine: &'a dyn Engine,
    other: &'a BTreeMap<String, Type>,
}

impl Writer<'_> {
    fn expr(&self, e: &IdedExpr) -> Result<String, String> {
        if let Some((f, ty)) = field(e, self.other) {
            return Ok(self.engine.caller(&f, &ty));
        }
        match &e.expr {
            Expr::Literal(l) => self.literal(l),
            Expr::Call(c) => self.call(c),
            Expr::Select(s) => Err(format!("`{}` is not a caller field", s.field)),
            Expr::Ident(n) => Err(format!("`{n}` cannot be read here")),
            Expr::List(_) => Err("a list here is not supported".into()),
            Expr::Comprehension(_) => {
                Err("list macros over caller fields are not supported".into())
            }
            Expr::Map(_) | Expr::Struct(_) | Expr::Unspecified => {
                Err("this expression is not supported".into())
            }
        }
    }

    fn literal(&self, l: &LiteralValue) -> Result<String, String> {
        Ok(match l {
            LiteralValue::Boolean(b) => if *b.inner() { "TRUE" } else { "FALSE" }.into(),
            LiteralValue::Int(i) => i.inner().to_string(),
            LiteralValue::UInt(u) => u.inner().to_string(),
            LiteralValue::Double(d) => {
                let s = format!("{:?}", d.inner());
                if s.contains(['.', 'e', 'E']) {
                    s
                } else {
                    format!("{s}.0")
                }
            }
            LiteralValue::String(s) => self.engine.string(s.inner()),
            LiteralValue::Null => "NULL".into(),
            LiteralValue::Bytes(_) => return Err("bytes literals are not supported".into()),
        })
    }

    /// The parcel type of `e`, where it can be told from the expression alone.
    fn ty(&self, e: &IdedExpr) -> Option<Type> {
        if let Some((_, t)) = field(e, self.other) {
            return Some(t);
        }
        match &e.expr {
            Expr::Literal(LiteralValue::String(_)) => Some(Type::String),
            Expr::Literal(LiteralValue::Int(_)) => Some(Type::Int),
            Expr::Literal(LiteralValue::Double(_)) => Some(Type::Double),
            Expr::List(l) => l.elements.first().and_then(|x| self.ty(x)).map(Type::list),
            Expr::Call(c) if c.func_name == "string" => Some(Type::String),
            Expr::Call(c) if c.func_name == operators::ADD => {
                c.args.first().and_then(|a| self.ty(a))
            }
            _ => None,
        }
    }

    fn call(&self, c: &cel::common::ast::CallExpr) -> Result<String, String> {
        let name = c.func_name.as_str();
        let args = &c.args;
        let two = |op: &str| -> Result<String, String> {
            let [a, b] = args.as_slice() else {
                return Err(format!("`{name}` takes two operands"));
            };
            Ok(format!("({} {op} {})", self.expr(a)?, self.expr(b)?))
        };
        if c.target.is_none() {
            match name {
                operators::EQUALS => return two("="),
                operators::NOT_EQUALS => return two("<>"),
                operators::LESS => return two("<"),
                operators::LESS_EQUALS => return two("<="),
                operators::GREATER => return two(">"),
                operators::GREATER_EQUALS => return two(">="),
                operators::LOGICAL_AND => return two("AND"),
                operators::LOGICAL_OR => return two("OR"),
                operators::SUBSTRACT => return two("-"),
                operators::MULTIPLY => return two("*"),
                operators::DIVIDE => return two("/"),
                operators::MODULO => return two("%"),
                operators::ADD => {
                    let [a, b] = args.as_slice() else {
                        return Err("`+` takes two operands".into());
                    };
                    return if self.ty(a) == Some(Type::String) || self.ty(b) == Some(Type::String) {
                        Ok(self.engine.concat(&self.expr(a)?, &self.expr(b)?))
                    } else {
                        two("+")
                    };
                }
                operators::LOGICAL_NOT => return Ok(format!("(NOT {})", self.expr(&args[0])?)),
                operators::NEGATE => return Ok(format!("(-{})", self.expr(&args[0])?)),
                operators::CONDITIONAL => {
                    let [cond, a, b] = args.as_slice() else {
                        return Err("`?:` takes three operands".into());
                    };
                    return Ok(format!(
                        "(CASE WHEN {} THEN {} ELSE {} END)",
                        self.expr(cond)?,
                        self.expr(a)?,
                        self.expr(b)?
                    ));
                }
                operators::IN => {
                    let [item, list] = args.as_slice() else {
                        return Err("`in` takes two operands".into());
                    };
                    let item_sql = self.expr(item)?;
                    return match &list.expr {
                        Expr::List(l) => {
                            let items = l
                                .elements
                                .iter()
                                .map(|x| self.expr(x))
                                .collect::<Result<Vec<_>, _>>()?;
                            Ok(format!("({item_sql} IN ({}))", items.join(", ")))
                        }
                        _ => Ok(self.engine.list_contains(&self.expr(list)?, &item_sql)),
                    };
                }
                "size" | parcel_core::cel_print::REF_STRLEN => {
                    let x = &args[0];
                    let sql = self.expr(x)?;
                    return Ok(match self.ty(x) {
                        Some(Type::List(_)) => self.engine.list_length(&sql),
                        _ => self.engine.string_length(&sql),
                    });
                }
                "int" => return Ok(self.engine.cast(&self.expr(&args[0])?, &Type::Int)),
                "uint" => return Ok(self.engine.cast(&self.expr(&args[0])?, &Type::Uint)),
                "double" => return Ok(self.engine.cast(&self.expr(&args[0])?, &Type::Double)),
                "string" => return Ok(self.engine.cast(&self.expr(&args[0])?, &Type::String)),
                "timestamp" => {
                    return match &args[0].expr {
                        Expr::Literal(LiteralValue::String(s)) => {
                            Ok(self.engine.timestamp(s.inner()))
                        }
                        _ => Ok(self.engine.cast(&self.expr(&args[0])?, &Type::Timestamp)),
                    };
                }
                "matches" if args.len() == 2 => {
                    return Ok(self
                        .engine
                        .matches(&self.expr(&args[0])?, &self.expr(&args[1])?));
                }
                _ => {}
            }
        }
        if let Some(t) = &c.target {
            let a = self.expr(t)?;
            let arg = |i: usize| -> Result<String, String> {
                args.get(i)
                    .ok_or_else(|| format!("`{name}` needs an argument"))
                    .and_then(|x| self.expr(x))
            };
            return match name {
                "startsWith" => Ok(self.engine.starts_with(&a, &arg(0)?)),
                "endsWith" => Ok(self.engine.ends_with(&a, &arg(0)?)),
                "contains" => Ok(self.engine.contains(&a, &arg(0)?)),
                "matches" => Ok(self.engine.matches(&a, &arg(0)?)),
                "size" => Ok(match self.ty(t) {
                    Some(Type::List(_)) => self.engine.list_length(&a),
                    _ => self.engine.string_length(&a),
                }),
                other => Err(format!("`.{other}()` on a caller field is not supported")),
            };
        }
        Err(format!("`{name}` on caller fields is not supported"))
    }
}
