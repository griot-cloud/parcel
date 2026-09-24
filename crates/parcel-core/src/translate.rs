//! The Translate pass: typed IR → DataFusion [`Expr`].
//!
//! - `row.<col>` becomes a column reference, cast to the canonical Arrow type
//!   of its parcel type so both engines see the same values.
//! - Every maximal subtree that reads only `ctx` becomes one placeholder
//!   (`$c0`, `$c1`, ...). peQL evaluates that subtree once per query with the
//!   reference interpreter and binds the result, after which DataFusion's
//!   simplifier folds away whatever the caller's context decides.
//! - Registry built-ins become their DataFusion expression trees, so the
//!   planner can see inside them.
//! - Predicates and transforms are wrapped for parcel's null rule (design 6):
//!   a null in any row field a rule reads makes an `admit`/`assert` false and a
//!   `transform` null. Fields tested only through `has()` are exempt.

use std::collections::BTreeSet;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use datafusion_common::arrow::datatypes::{DataType, Field, Schema, TimeUnit};
use datafusion_common::{Column, ScalarValue};
use datafusion_expr::expr::Placeholder;
use datafusion_expr::{
    Expr, Operator, binary_expr, cast, in_list, is_not_null, lambda, lambda_var, lit, not, when,
};
use datafusion_functions::expr_fn as f;
use datafusion_functions_nested::expr_fn as nested;
use serde::Serialize;

use crate::cel_print::{Style, print};
use crate::ir::*;
use crate::types::Type;

/// A `ctx`-only subtree lifted out of a row expression and bound per query.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CtxParam {
    /// Placeholder name without the `$`.
    pub id: String,
    /// The subtree, as CEL for the reference interpreter.
    pub cel: String,
    pub ty: Type,
}

/// Placeholders allocated while translating one contract, deduplicated by subtree.
#[derive(Clone, Debug, Default)]
pub struct Params {
    pub list: Vec<CtxParam>,
}

impl Params {
    fn intern(&mut self, e: &TExpr) -> Expr {
        let cel = print(e, Style::Reference);
        let id = match self.list.iter().find(|p| p.cel == cel) {
            Some(p) => p.id.clone(),
            None => {
                let id = format!("c{}", self.list.len());
                self.list.push(CtxParam {
                    id: id.clone(),
                    cel,
                    ty: e.ty.clone(),
                });
                id
            }
        };
        placeholder(&id, &e.ty)
    }
}

pub fn placeholder(id: &str, ty: &Type) -> Expr {
    Expr::Placeholder(Placeholder {
        id: format!("${id}"),
        field: Some(Arc::new(Field::new(id, ty.to_arrow(), true))),
    })
}

pub struct Translator<'a> {
    pub schema: &'a Schema,
    pub params: &'a mut Params,
    /// When set, `dataset.*` fields read the statistics columns of the validation aggregate.
    pub dataset_columns: bool,
}

pub type TResult = Result<Expr, String>;

impl Translator<'_> {
    /// An `admit`/`assert` predicate under parcel's null rule: never null, false when a read field is null.
    pub fn predicate(&mut self, e: &TExpr) -> TResult {
        let body = f::coalesce(vec![self.expr(e)?, lit(false)]);
        Ok(match self.guard(e) {
            Some(g) => g.and(body),
            None => body,
        })
    }

    /// A `transform` under parcel's null rule: null when a read field is null.
    pub fn transform(&mut self, e: &TExpr) -> TResult {
        let body = self.expr(e)?;
        Ok(match self.guard(e) {
            Some(g) => when(g, body)
                .otherwise(lit(
                    ScalarValue::try_from(&e.ty.to_arrow()).map_err(|x| x.to_string())?
                ))
                .map_err(|x| x.to_string())?,
            None => body,
        })
    }

    /// `col IS NOT NULL AND ...` for every row column the expression reads (not counting `has()`).
    fn guard(&self, e: &TExpr) -> Option<Expr> {
        let mut cols = BTreeSet::new();
        e.walk(&mut |n| {
            if let ExprKind::Var(Var::Row(c)) = &n.kind {
                cols.insert(c.clone());
            }
        });
        cols.into_iter()
            .map(|c| is_not_null(column(&c)))
            .reduce(Expr::and)
    }

    pub fn expr(&mut self, e: &TExpr) -> TResult {
        if e.ns == NsSet::CTX && !has_local(e) {
            return Ok(self.params.intern(e));
        }
        Ok(match &e.kind {
            ExprKind::Lit(l) => lit(lit_value(l)),
            ExprKind::Var(Var::Row(c)) => self.row(c, &e.ty)?,
            ExprKind::Var(Var::Dataset(d)) if self.dataset_columns => column(&stat_column(d)),
            ExprKind::Var(v) => {
                return Err(format!(
                    "`{}` cannot be read per row",
                    crate::cel_print::var_path(v)
                ));
            }
            ExprKind::Local(v) => lambda_var(v.clone()),
            ExprKind::List(xs) => {
                nested::make_array(xs.iter().map(|x| self.expr(x)).collect::<Result<_, _>>()?)
            }
            ExprKind::Not(x) => not(self.expr(x)?),
            ExprKind::Neg(x) => Expr::Negative(Box::new(self.expr(x)?)),
            ExprKind::Binary(op, a, b) => self.binary(*op, a, b)?,
            ExprKind::Cond(c, a, b) => when(self.expr(c)?, self.expr(a)?)
                .otherwise(self.expr(b)?)
                .map_err(|x| x.to_string())?,
            ExprKind::Has(c) => is_not_null(column(c)),
            ExprKind::Builtin(b, args) => self.builtin(*b, args)?,
            ExprKind::Call(pin, args) => {
                let args = args
                    .iter()
                    .map(|a| self.expr(a))
                    .collect::<Result<Vec<_>, _>>()?;
                builtin_function(&pin.name, args)?
            }
            ExprKind::Macro {
                kind,
                var,
                range,
                body,
            } => {
                let range = self.expr(range)?;
                let body_expr = self.expr(body)?;
                match kind {
                    MacroKind::Exists => {
                        nested::array_any_match(range, lambda([var.clone()], body_expr))
                    }
                    // all(v, p) == !exists(v, !p)
                    MacroKind::All => not(nested::array_any_match(
                        range,
                        lambda([var.clone()], not(body_expr)),
                    )),
                    MacroKind::Filter => {
                        nested::array_filter(range, lambda([var.clone()], body_expr))
                    }
                    MacroKind::Map => {
                        nested::array_transform(range, lambda([var.clone()], body_expr))
                    }
                }
            }
        })
    }

    fn row(&self, c: &str, ty: &Type) -> TResult {
        let field = self.schema.field_with_name(c).map_err(|e| e.to_string())?;
        let want = ty.to_arrow();
        Ok(if field.data_type() == &want {
            column(c)
        } else {
            cast(column(c), want)
        })
    }

    fn binary(&mut self, op: BinOp, a: &TExpr, b: &TExpr) -> TResult {
        if op == BinOp::In {
            let needle = self.expr(a)?;
            return Ok(match &b.kind {
                // A literal list whose elements are all constant or ctx: an IN list the planner prunes with.
                ExprKind::List(items) => in_list(
                    needle,
                    items
                        .iter()
                        .map(|i| self.expr(i))
                        .collect::<Result<_, _>>()?,
                    false,
                ),
                _ => nested::array_has(self.expr(b)?, needle),
            });
        }
        let (l, r) = (self.expr(a)?, self.expr(b)?);
        let o = match op {
            BinOp::And => return Ok(l.and(r)),
            BinOp::Or => return Ok(l.or(r)),
            BinOp::Eq => Operator::Eq,
            BinOp::Ne => Operator::NotEq,
            BinOp::Lt => Operator::Lt,
            BinOp::Le => Operator::LtEq,
            BinOp::Gt => Operator::Gt,
            BinOp::Ge => Operator::GtEq,
            BinOp::Add if a.ty == Type::String => Operator::StringConcat,
            BinOp::Add => Operator::Plus,
            BinOp::Sub => Operator::Minus,
            BinOp::Mul => Operator::Multiply,
            BinOp::Div => Operator::Divide,
            BinOp::Mod => Operator::Modulo,
            BinOp::In => unreachable!(),
        };
        Ok(binary_expr(l, o, r))
    }

    fn builtin(&mut self, b: Builtin, args: &[TExpr]) -> TResult {
        // Constant-fold timestamp('...') and duration('...') so the plan carries typed literals.
        match (b, &args[0].kind) {
            (Builtin::ToTimestamp, ExprKind::Lit(Lit::String(s))) => {
                return Ok(lit(parse_timestamp(s)?));
            }
            (Builtin::ToDuration, ExprKind::Lit(Lit::String(s))) => {
                return Ok(lit(parse_duration(s)?));
            }
            (Builtin::ToTimestamp | Builtin::ToDuration, _) => return self.expr(&args[0]),
            _ => {}
        }
        let a: Vec<Expr> = args
            .iter()
            .map(|x| self.expr(x))
            .collect::<Result<_, _>>()?;
        let mut a = a.into_iter();
        let mut next = || a.next().expect("checker guarantees arity");
        let part = |p: &str, x: Expr| cast(f::date_part(lit(p), x), DataType::Int64);
        Ok(match b {
            Builtin::Size => match &args[0].ty {
                Type::String => cast(f::character_length(next()), DataType::Int64),
                Type::Bytes => bytes_len_udf().call(vec![next()]),
                _ => cast(nested::cardinality(next()), DataType::Int64),
            },
            Builtin::StartsWith => f::starts_with(next(), next()),
            Builtin::EndsWith => f::ends_with(next(), next()),
            Builtin::Contains => binary_expr(f::strpos(next(), next()), Operator::Gt, lit(0i64)),
            Builtin::Matches => f::regexp_like(next(), next(), None),
            Builtin::ToInt => cast(next(), DataType::Int64),
            Builtin::ToUint => cast(next(), DataType::UInt64),
            Builtin::ToDouble => cast(next(), DataType::Float64),
            Builtin::ToString => cast(next(), DataType::Utf8),
            Builtin::GetFullYear => part("year", next()),
            // CEL's month, day of month and day of year count from zero.
            Builtin::GetMonth => binary_expr(part("month", next()), Operator::Minus, lit(1i64)),
            Builtin::GetDayOfMonth => binary_expr(part("day", next()), Operator::Minus, lit(1i64)),
            Builtin::GetDayOfYear => binary_expr(part("doy", next()), Operator::Minus, lit(1i64)),
            Builtin::GetDayOfWeek => part("dow", next()),
            Builtin::GetHours => part("hour", next()),
            Builtin::GetMinutes => part("minute", next()),
            Builtin::GetSeconds => cast(
                f::floor(cast(f::date_part(lit("second"), next()), DataType::Float64)),
                DataType::Int64,
            ),
            Builtin::ToTimestamp | Builtin::ToDuration => unreachable!("handled above"),
        })
    }
}

/// The DataFusion expression for each registry built-in (design 7.3).
pub fn builtin_function(name: &str, mut args: Vec<Expr>) -> TResult {
    let x = args
        .pop()
        .ok_or_else(|| format!("`{name}` needs an argument"))?;
    Ok(match name {
        "hash_sha256" => f::encode(f::sha256(x), lit("hex")),
        "redact" => f::repeat(lit("*"), cast(f::character_length(x), DataType::Int64)),
        "is_msisdn" => f::regexp_like(x, lit(MSISDN_PATTERN), None),
        "is_email" => f::regexp_like(x, lit(EMAIL_PATTERN), None),
        other => {
            return Err(format!(
                "`{other}` has no DataFusion implementation in parcel v0"
            ));
        }
    })
}

/// Kenyan mobile numbers in international form without `+`: 254 then 7xx or 1xx.
pub const MSISDN_PATTERN: &str = "^254[17][0-9]{8}$";
/// A deliberately simple address shape: local@domain.tld, no spaces.
pub const EMAIL_PATTERN: &str = "^[^@\\s]+@[^@\\s]+\\.[^@\\s]+$";

/// The column name the validation aggregate gives a `dataset` field.
pub fn stat_column(d: &DatasetField) -> String {
    match d {
        DatasetField::RowCount => "row_count".into(),
        DatasetField::WrittenAt => "written_at".into(),
        DatasetField::ContractHash => "contract_hash".into(),
        DatasetField::Column { column, stat } => {
            format!("{column}__{}", crate::cel_print::stat_name(*stat))
        }
        DatasetField::AssertionPassRate { assertion } => {
            format!("assertions__{assertion}__pass_rate")
        }
    }
}

/// `parcel_bytes_len(binary) -> int64`: DataFusion's `octet_length` takes strings only.
pub fn bytes_len_udf() -> datafusion_expr::ScalarUDF {
    datafusion_expr::ScalarUDF::new_from_impl(BytesLen {
        signature: datafusion_expr::Signature::exact(
            vec![DataType::Binary],
            datafusion_expr::Volatility::Immutable,
        ),
    })
}

#[derive(Debug, PartialEq, Eq, Hash)]
struct BytesLen {
    signature: datafusion_expr::Signature,
}

impl datafusion_expr::ScalarUDFImpl for BytesLen {
    fn name(&self) -> &str {
        "parcel_bytes_len"
    }
    fn signature(&self) -> &datafusion_expr::Signature {
        &self.signature
    }
    fn return_type(&self, _: &[DataType]) -> datafusion_common::Result<DataType> {
        Ok(DataType::Int64)
    }
    fn invoke_with_args(
        &self,
        args: datafusion_expr::ScalarFunctionArgs,
    ) -> datafusion_common::Result<datafusion_expr::ColumnarValue> {
        use datafusion_common::arrow::array::{Array, BinaryArray, Int64Array};
        let arr = args.args[0].to_array(args.number_rows)?;
        let bin = arr.as_any().downcast_ref::<BinaryArray>().ok_or_else(|| {
            datafusion_common::DataFusionError::Internal("parcel_bytes_len expects binary".into())
        })?;
        let out: Int64Array = (0..bin.len())
            .map(|i| (!bin.is_null(i)).then(|| bin.value(i).len() as i64))
            .collect();
        Ok(datafusion_expr::ColumnarValue::Array(Arc::new(out)))
    }
}

pub fn column(name: &str) -> Expr {
    Expr::Column(Column::new_unqualified(name))
}

fn has_local(e: &TExpr) -> bool {
    let mut found = false;
    e.walk(&mut |n| found |= matches!(n.kind, ExprKind::Local(_)));
    found
}

pub fn lit_value(l: &Lit) -> ScalarValue {
    match l {
        Lit::Bool(b) => ScalarValue::Boolean(Some(*b)),
        Lit::Int(i) => ScalarValue::Int64(Some(*i)),
        Lit::Uint(u) => ScalarValue::UInt64(Some(*u)),
        Lit::Double(d) => ScalarValue::Float64(Some(*d)),
        Lit::String(s) => ScalarValue::Utf8(Some(s.clone())),
        Lit::Bytes(b) => ScalarValue::Binary(Some(b.clone())),
    }
}

pub fn timestamp_scalar(t: DateTime<Utc>) -> ScalarValue {
    ScalarValue::TimestampMicrosecond(Some(t.timestamp_micros()), Some("UTC".into()))
}

pub fn parse_timestamp(s: &str) -> Result<ScalarValue, String> {
    let t = DateTime::parse_from_rfc3339(s).map_err(|e| format!("timestamp('{s}'): {e}"))?;
    Ok(timestamp_scalar(t.with_timezone(&Utc)))
}

/// Parse a CEL duration literal (`72h`, `1h30m`, `1.5s`, `-250ms`) to microseconds.
pub fn parse_duration_micros(s: &str) -> Result<i64, String> {
    let bad = || format!("duration('{s}'): expected a sequence like 1h30m, 90s or 250ms");
    let (neg, mut rest) = match s.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, s.strip_prefix('+').unwrap_or(s)),
    };
    if rest.is_empty() {
        return Err(bad());
    }
    let mut total: f64 = 0.0;
    while !rest.is_empty() {
        let n = rest
            .find(|c: char| !(c.is_ascii_digit() || c == '.'))
            .ok_or_else(bad)?;
        let value: f64 = rest[..n].parse().map_err(|_| bad())?;
        rest = &rest[n..];
        let (unit, len) = [
            ("ns", 1e-3),
            ("us", 1.0),
            ("µs", 1.0),
            ("ms", 1e3),
            ("h", 3.6e9),
            ("m", 6e7),
            ("s", 1e6),
        ]
        .into_iter()
        .find(|(u, _)| rest.starts_with(u))
        .map(|(u, m)| (m, u.len()))
        .ok_or_else(bad)?;
        total += value * unit;
        rest = &rest[len..];
    }
    let micros = total.round() as i64;
    Ok(if neg { -micros } else { micros })
}

pub fn parse_duration(s: &str) -> Result<ScalarValue, String> {
    Ok(ScalarValue::DurationMicrosecond(Some(
        parse_duration_micros(s)?,
    )))
}

/// The Arrow type a parcel type is materialised as; re-exported for callers building schemas.
pub fn arrow_type(t: &Type) -> DataType {
    t.to_arrow()
}

/// Microsecond timestamps with UTC are parcel's canonical timestamp type.
pub fn canonical_timestamp() -> DataType {
    DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations() {
        assert_eq!(parse_duration_micros("72h").unwrap(), 72 * 3_600_000_000);
        assert_eq!(parse_duration_micros("1h30m").unwrap(), 5_400_000_000);
        assert_eq!(parse_duration_micros("1.5s").unwrap(), 1_500_000);
        assert_eq!(parse_duration_micros("-250ms").unwrap(), -250_000);
        assert!(parse_duration_micros("3 days").is_err());
    }
}
