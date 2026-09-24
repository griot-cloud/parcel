//! Print the typed IR back to CEL source.
//!
//! Two uses. [`Style::Canonical`] gives one spelling per expression, which is
//! what hashing and the report show. [`Style::Reference`] is what the reference
//! interpreter executes: identical, except that it calls parcel's own helpers
//! where the `cel` crate departs from the CEL specification (it counts string
//! size in bytes, not code points).

use std::fmt::Write;

use crate::ir::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    Canonical,
    Reference,
}

/// The name of the reference interpreter's code-point string length helper.
pub const REF_STRLEN: &str = "parcel_strlen";

pub fn print(e: &TExpr, style: Style) -> String {
    let mut out = String::new();
    write_expr(e, style, &mut out);
    out
}

fn write_expr(e: &TExpr, style: Style, o: &mut String) {
    let p = |x: &TExpr, o: &mut String| write_expr(x, style, o);
    match &e.kind {
        ExprKind::Lit(l) => write_lit(l, o),
        ExprKind::Var(v) => o.push_str(&var_path(v)),
        ExprKind::Local(v) => o.push_str(v),
        ExprKind::List(xs) => {
            o.push('[');
            for (i, x) in xs.iter().enumerate() {
                if i > 0 {
                    o.push_str(", ");
                }
                p(x, o);
            }
            o.push(']');
        }
        ExprKind::Not(x) => {
            o.push_str("!(");
            p(x, o);
            o.push(')');
        }
        ExprKind::Neg(x) => {
            o.push_str("-(");
            p(x, o);
            o.push(')');
        }
        ExprKind::Binary(op, a, b) => {
            o.push('(');
            p(a, o);
            o.push_str(match op {
                BinOp::And => " && ",
                BinOp::Or => " || ",
                BinOp::Eq => " == ",
                BinOp::Ne => " != ",
                BinOp::Lt => " < ",
                BinOp::Le => " <= ",
                BinOp::Gt => " > ",
                BinOp::Ge => " >= ",
                BinOp::Add => " + ",
                BinOp::Sub => " - ",
                BinOp::Mul => " * ",
                BinOp::Div => " / ",
                BinOp::Mod => " % ",
                BinOp::In => " in ",
            });
            p(b, o);
            o.push(')');
        }
        ExprKind::Cond(c, a, b) => {
            o.push('(');
            p(c, o);
            o.push_str(" ? ");
            p(a, o);
            o.push_str(" : ");
            p(b, o);
            o.push(')');
        }
        ExprKind::Has(c) => {
            let _ = write!(o, "has(row.{c})");
        }
        ExprKind::HasOther(f) => {
            let _ = write!(o, "has(row.other.{f})");
        }
        ExprKind::Builtin(b, args) => {
            let global = |name: &str, o: &mut String| {
                o.push_str(name);
                o.push('(');
                for (i, a) in args.iter().enumerate() {
                    if i > 0 {
                        o.push_str(", ");
                    }
                    p(a, o);
                }
                o.push(')');
            };
            let method = |name: &str, o: &mut String| {
                o.push('(');
                p(&args[0], o);
                let _ = write!(o, ").{name}(");
                for (i, a) in args[1..].iter().enumerate() {
                    if i > 0 {
                        o.push_str(", ");
                    }
                    p(a, o);
                }
                o.push(')');
            };
            match b {
                Builtin::Size
                    if style == Style::Reference && args[0].ty == crate::types::Type::String =>
                {
                    global(REF_STRLEN, o)
                }
                Builtin::Size => global("size", o),
                Builtin::StartsWith => method("startsWith", o),
                Builtin::EndsWith => method("endsWith", o),
                Builtin::Contains => method("contains", o),
                Builtin::Matches => method("matches", o),
                Builtin::ToInt => global("int", o),
                Builtin::ToUint => global("uint", o),
                Builtin::ToDouble => global("double", o),
                Builtin::ToString => global("string", o),
                Builtin::ToTimestamp => global("timestamp", o),
                Builtin::ToDuration => global("duration", o),
                Builtin::GetFullYear => method("getFullYear", o),
                Builtin::GetMonth => method("getMonth", o),
                Builtin::GetDayOfMonth => method("getDayOfMonth", o),
                Builtin::GetDayOfWeek => method("getDayOfWeek", o),
                Builtin::GetDayOfYear => method("getDayOfYear", o),
                Builtin::GetHours => method("getHours", o),
                Builtin::GetMinutes => method("getMinutes", o),
                Builtin::GetSeconds => method("getSeconds", o),
            }
        }
        ExprKind::Call(pin, args) => {
            o.push_str(&pin.name);
            o.push('(');
            for (i, a) in args.iter().enumerate() {
                if i > 0 {
                    o.push_str(", ");
                }
                p(a, o);
            }
            o.push(')');
        }
        ExprKind::Macro {
            kind,
            var,
            range,
            body,
        } => {
            o.push('(');
            p(range, o);
            let name = match kind {
                MacroKind::Exists => "exists",
                MacroKind::All => "all",
                MacroKind::Filter => "filter",
                MacroKind::Map => "map",
            };
            let _ = write!(o, ").{name}({var}, ");
            p(body, o);
            o.push(')');
        }
    }
}

pub fn var_path(v: &Var) -> String {
    match v {
        Var::Row(c) => format!("row.{c}"),
        Var::RowOther(f) => format!("row.other.{f}"),
        Var::Ctx(f) => format!("ctx.{}", f.name()),
        Var::CtxOther(f) => format!("ctx.other.{f}"),
        Var::Dataset(d) => match d {
            DatasetField::RowCount => "dataset.row_count".into(),
            DatasetField::WrittenAt => "dataset.written_at".into(),
            DatasetField::ContractHash => "dataset.contract_hash".into(),
            DatasetField::Column { column, stat } => {
                format!("dataset.{column}.{}", stat_name(*stat))
            }
            DatasetField::AssertionPassRate { assertion } => {
                format!("dataset.assertions.{assertion}.pass_rate")
            }
            DatasetField::Other(f) => format!("dataset.other.{f}"),
        },
    }
}

pub fn stat_name(s: ColumnStat) -> &'static str {
    match s {
        ColumnStat::NullCount => "null_count",
        ColumnStat::NullRate => "null_rate",
        ColumnStat::DistinctCount => "distinct_count",
        ColumnStat::Min => "min",
        ColumnStat::Max => "max",
    }
}

fn write_lit(l: &Lit, o: &mut String) {
    match l {
        Lit::Bool(b) => o.push_str(if *b { "true" } else { "false" }),
        Lit::Int(i) => {
            // i64::MIN has no positive literal; CEL parses `-9223372036854775808` as one token.
            let _ = write!(o, "{i}");
        }
        Lit::Uint(u) => {
            let _ = write!(o, "{u}u");
        }
        Lit::Double(d) => {
            let s = format!("{d:?}");
            o.push_str(&s);
            if !s.contains(['.', 'e', 'E']) {
                o.push_str(".0");
            }
        }
        Lit::String(s) => {
            o.push('"');
            for c in s.chars() {
                match c {
                    '"' => o.push_str("\\\""),
                    '\\' => o.push_str("\\\\"),
                    '\n' => o.push_str("\\n"),
                    '\r' => o.push_str("\\r"),
                    '\t' => o.push_str("\\t"),
                    c if (c as u32) < 0x20 => {
                        let _ = write!(o, "\\u{:04x}", c as u32);
                    }
                    c => o.push(c),
                }
            }
            o.push('"');
        }
        Lit::Bytes(b) => {
            o.push_str("b\"");
            for byte in b {
                let _ = write!(o, "\\x{byte:02x}");
            }
            o.push('"');
        }
    }
}
