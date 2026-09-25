//! The reference CEL interpreter.
//!
//! Evaluates CEL printed from parcel's IR in [`Style::Reference`] with the
//! `cel` crate, extended with parcel's registry built-ins. The built-ins here
//! and the DataFusion expressions in `parcel_core::translate` are two halves
//! of one registry entry; the differential test holds them to each other.
//!
//! [`Style::Reference`]: parcel_core::cel_print::Style::Reference

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, OnceLock};

use cel::{Context, Program, Value};
use chrono::{DateTime, Duration, FixedOffset, TimeZone, Utc};
use datafusion_common::ScalarValue;
use datafusion_common::arrow::array::Array;
use datafusion_common::arrow::datatypes::{DataType, Field, TimeUnit};
use parcel_core::cel_print::REF_STRLEN;
use parcel_core::translate::{EMAIL_PATTERN, MSISDN_PATTERN};
use parcel_core::types::Type;
use sha2::{Digest, Sha256};

use crate::Caller;

/// The `ctx` namespace for a caller, with `ctx.other` values typed as the contract declares.
/// A declared field the caller does not supply is absent; reading it is an error, so rules
/// that depend on it fail closed.
pub fn ctx_value_typed(c: &Caller, declared: &BTreeMap<String, Type>) -> Value {
    let Value::Map(base) = ctx_value(c) else {
        unreachable!("ctx is a map")
    };
    let mut m: HashMap<String, Value> = HashMap::new();
    for (k, v) in base.map.iter() {
        if let cel::objects::Key::String(k) = k {
            m.insert(k.to_string(), v.clone());
        }
    }
    let mut other: HashMap<String, Value> = HashMap::new();
    for (field, ty) in declared {
        if let Some(v) = c.other.get(field).and_then(|j| json_value(j, ty)) {
            other.insert(field.clone(), v);
        }
    }
    m.insert("other".into(), other.into());
    m.into()
}

/// A JSON value as the CEL value of a declared type, or `None` if it does not fit.
pub fn json_value(j: &serde_json::Value, ty: &Type) -> Option<Value> {
    Some(match ty {
        Type::Bool => Value::Bool(j.as_bool()?),
        Type::Int => Value::Int(j.as_i64()?),
        Type::Uint => Value::UInt(j.as_u64()?),
        Type::Double => Value::Float(j.as_f64()?),
        Type::String => string(j.as_str()?),
        Type::Timestamp => timestamp(
            DateTime::parse_from_rfc3339(j.as_str()?)
                .ok()?
                .with_timezone(&Utc),
        ),
        Type::List(elem) => Value::List(Arc::new(
            j.as_array()?
                .iter()
                .map(|x| json_value(x, elem))
                .collect::<Option<Vec<_>>>()?,
        )),
        Type::Decimal(s) => Value::Int((j.as_f64()? * 10f64.powi(*s as i32)).round() as i64),
        Type::Bytes | Type::Duration => return None,
    })
}

/// The `ctx` namespace for a caller.
pub fn ctx_value(c: &Caller) -> Value {
    let mut m: HashMap<String, Value> = HashMap::new();
    m.insert("id".into(), string(&c.id));
    m.insert("tenant".into(), string(&c.tenant));
    m.insert("purpose".into(), string(&c.purpose));
    m.insert("tier".into(), string(&c.tier));
    m.insert("clearance".into(), Value::Int(c.clearance));
    m.insert("classification".into(), string(&c.classification));
    m.insert(
        "roles".into(),
        Value::List(Arc::new(c.roles.iter().map(|r| string(r)).collect())),
    );
    m.insert("now".into(), timestamp(c.now));
    m.into()
}

/// The `dataset` namespace from `(path, value)` pairs such as `("dataset.customer_id.null_rate", 0.01)`.
pub fn dataset_value(entries: &[(String, Value)]) -> Value {
    #[derive(Default)]
    struct Node(BTreeMap<String, Node>, Option<Value>);
    let mut root = Node::default();
    for (path, v) in entries {
        let mut node = &mut root;
        for part in path.trim_start_matches("dataset.").split('.') {
            node = node.0.entry(part.to_owned()).or_default();
        }
        node.1 = Some(v.clone());
    }
    fn build(n: Node) -> Value {
        match n.1 {
            Some(v) => v,
            None => {
                n.0.into_iter()
                    .map(|(k, v)| (k, build(v)))
                    .collect::<HashMap<String, Value>>()
                    .into()
            }
        }
    }
    build(root)
}

/// The variables one evaluation may read. Absent row keys are how nulls appear to CEL.
#[derive(Clone, Debug, Default)]
pub struct Scope {
    pub ctx: Option<Value>,
    pub dataset: Option<Value>,
    pub row: Option<Value>,
    /// User functions the expression may call: exactly the ones its contract is pinned to.
    pub pins: std::collections::BTreeSet<parcel_core::registry::FunctionPin>,
}

/// A CEL context holding the standard library, parcel's built-ins and the scope's variables.
pub fn context(scope: &Scope) -> Context<'static> {
    let mut c = Context::default();
    c.add_function(REF_STRLEN, |s: Arc<String>| -> i64 {
        s.chars().count() as i64
    });
    c.add_function("hash_sha256", |v: Value| -> String {
        match v {
            Value::String(s) => hex::encode(Sha256::digest(s.as_bytes())),
            Value::Bytes(b) => hex::encode(Sha256::digest(b.as_slice())),
            _ => String::new(),
        }
    });
    c.add_function("redact", |_s: Arc<String>| -> String {
        parcel_core::translate::REDACTED.to_owned()
    });
    c.add_function("partial", |s: Arc<String>, n: i64| -> String {
        let chars: Vec<char> = s.chars().collect();
        let redacted = parcel_core::translate::REDACTED;
        if n < 0 || chars.len() as i64 <= n {
            return redacted.to_owned();
        }
        let tail: String = chars[chars.len() - n as usize..].iter().collect();
        format!("{redacted}{tail}")
    });
    c.add_function("is_msisdn", |s: Arc<String>| -> bool {
        msisdn_re().is_match(&s)
    });
    c.add_function("is_email", |s: Arc<String>| -> bool {
        email_re().is_match(&s)
    });
    // Tenants' WebAssembly functions: exactly the ones this expression's contract is pinned to.
    #[cfg(feature = "wasm")]
    for pin in &scope.pins {
        let Some(f) = crate::wasm::loaded()
            .read()
            .expect("lock")
            .get(&pin.hash)
            .cloned()
        else {
            continue;
        };
        let fname = pin.name.clone();
        c.add_function(
            &pin.name,
            move |cel::extractors::Arguments(args): cel::extractors::Arguments| -> Result<Value, cel::ExecutionError> {
                f.call_values(&args).map_err(|e| cel::ExecutionError::function_error(&fname, e))
            },
        );
    }
    for (name, v) in [
        ("ctx", &scope.ctx),
        ("dataset", &scope.dataset),
        ("row", &scope.row),
    ] {
        if let Some(v) = v {
            c.add_variable_from_value(name, v.clone());
        }
    }
    c
}

fn msisdn_re() -> &'static regex::Regex {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(MSISDN_PATTERN).expect("valid pattern"))
}

fn email_re() -> &'static regex::Regex {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(EMAIL_PATTERN).expect("valid pattern"))
}

/// Compiled programs are cached by source; contracts evaluate the same few expressions constantly.
fn program(src: &str) -> Result<Arc<Program>, String> {
    static CACHE: OnceLock<std::sync::Mutex<HashMap<String, Arc<Program>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(p) = cache.lock().expect("cache lock").get(src) {
        return Ok(p.clone());
    }
    let p = Arc::new(Program::compile(src).map_err(|e| format!("cannot compile `{src}`: {e}"))?);
    cache
        .lock()
        .expect("cache lock")
        .insert(src.to_owned(), p.clone());
    Ok(p)
}

pub fn eval(src: &str, ctx: &Context) -> Result<Value, String> {
    program(src)?
        .execute(ctx)
        .map_err(|e| format!("`{src}`: {e}"))
}

pub fn eval_bool(src: &str, ctx: &Context) -> Result<bool, String> {
    match eval(src, ctx)? {
        Value::Bool(b) => Ok(b),
        other => Err(format!("`{src}` gave {other:?}, not a bool")),
    }
}

pub fn string(s: &str) -> Value {
    Value::String(Arc::new(s.to_owned()))
}

pub fn timestamp(t: DateTime<Utc>) -> Value {
    Value::Timestamp(t.with_timezone(&FixedOffset::east_opt(0).expect("UTC offset")))
}

/// A CEL value as the Arrow scalar of the parcel type it was checked as.
pub fn to_scalar(v: &Value, ty: &Type) -> Result<ScalarValue, String> {
    Ok(match (ty, v) {
        (Type::Bool, Value::Bool(b)) => ScalarValue::Boolean(Some(*b)),
        (Type::Int, Value::Int(i)) => ScalarValue::Int64(Some(*i)),
        (Type::Uint, Value::UInt(u)) => ScalarValue::UInt64(Some(*u)),
        (Type::Double, Value::Float(f)) => ScalarValue::Float64(Some(*f)),
        (Type::String, Value::String(s)) => ScalarValue::Utf8(Some(s.to_string())),
        (Type::Bytes, Value::Bytes(b)) => ScalarValue::Binary(Some(b.to_vec())),
        (Type::Timestamp, Value::Timestamp(t)) => {
            ScalarValue::TimestampMicrosecond(Some(t.timestamp_micros()), Some("UTC".into()))
        }
        (Type::Decimal(s), Value::Int(v)) => {
            ScalarValue::Decimal128(Some(*v as i128), parcel_core::types::DECIMAL_PRECISION, *s)
        }
        (Type::Duration, Value::Duration(d)) => {
            ScalarValue::DurationMicrosecond(d.num_microseconds())
        }
        (Type::List(elem), Value::List(items)) => {
            let scalars = items
                .iter()
                .map(|i| to_scalar(i, elem))
                .collect::<Result<Vec<_>, _>>()?;
            ScalarValue::List(ScalarValue::new_list(&scalars, &elem.to_arrow(), true))
        }
        _ => return Err(format!("expected {ty}, got {v:?}")),
    })
}

/// An Arrow scalar as a CEL value; `None` for null.
pub fn from_scalar(s: &ScalarValue) -> Option<Value> {
    use ScalarValue as S;
    Some(match s {
        S::Boolean(Some(b)) => Value::Bool(*b),
        S::Int8(Some(i)) => Value::Int(*i as i64),
        S::Int16(Some(i)) => Value::Int(*i as i64),
        S::Int32(Some(i)) => Value::Int(*i as i64),
        S::Int64(Some(i)) => Value::Int(*i),
        S::UInt8(Some(u)) => Value::UInt(*u as u64),
        S::UInt16(Some(u)) => Value::UInt(*u as u64),
        S::UInt32(Some(u)) => Value::UInt(*u as u64),
        S::UInt64(Some(u)) => Value::UInt(*u),
        S::Float32(Some(f)) => Value::Float(*f as f64),
        S::Float64(Some(f)) => Value::Float(*f),
        S::Utf8(Some(s)) | S::LargeUtf8(Some(s)) | S::Utf8View(Some(s)) => string(s),
        S::Binary(Some(b))
        | S::LargeBinary(Some(b))
        | S::BinaryView(Some(b))
        | S::FixedSizeBinary(_, Some(b)) => Value::Bytes(Arc::new(b.clone())),
        S::TimestampSecond(Some(v), _) => timestamp(Utc.timestamp_opt(*v, 0).single()?),
        S::TimestampMillisecond(Some(v), _) => timestamp(Utc.timestamp_millis_opt(*v).single()?),
        S::TimestampMicrosecond(Some(v), _) => timestamp(Utc.timestamp_micros(*v).single()?),
        S::TimestampNanosecond(Some(v), _) => timestamp(Utc.timestamp_nanos(*v)),
        S::DurationSecond(Some(v)) => Value::Duration(Duration::seconds(*v)),
        S::DurationMillisecond(Some(v)) => Value::Duration(Duration::milliseconds(*v)),
        S::DurationMicrosecond(Some(v)) => Value::Duration(Duration::microseconds(*v)),
        S::DurationNanosecond(Some(v)) => Value::Duration(Duration::nanoseconds(*v)),
        S::List(arr) => {
            if arr.is_null(0) {
                return None;
            }
            let values = arr.value(0);
            let mut out = Vec::with_capacity(values.len());
            for i in 0..values.len() {
                let item = ScalarValue::try_from_array(&values, i).ok()?;
                // CEL lists cannot hold SQL-style nulls; parcel treats a list with a null element as null.
                out.push(from_scalar(&item)?);
            }
            Value::List(Arc::new(out))
        }
        S::Dictionary(_, inner) => return from_scalar(inner),
        // Decimals reach the interpreter as their exact unscaled integer.
        S::Decimal128(Some(v), _, _) => Value::Int(i64::try_from(*v).ok()?),
        S::Decimal64(Some(v), _, _) => Value::Int(*v),
        S::Decimal32(Some(v), _, _) => Value::Int(*v as i64),
        _ => return None,
    })
}

/// The Arrow list type parcel uses for `list<T>` parameters.
pub fn list_type(elem: &Type) -> DataType {
    DataType::List(Arc::new(Field::new_list_field(elem.to_arrow(), true)))
}

/// Parcel's canonical timestamp type.
pub fn timestamp_type() -> DataType {
    DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_and_ctx() {
        let caller = Caller::new("u1", "acme", "analytics").with_roles(&["admin"]);
        let ctx = context(&Scope {
            ctx: Some(ctx_value(&caller)),
            ..Default::default()
        });
        assert!(eval_bool("'admin' in ctx.roles && ctx.tenant == 'acme'", &ctx).unwrap());
        assert_eq!(eval("parcel_strlen('héllo')", &ctx).unwrap(), Value::Int(5));
        assert_eq!(eval("redact('abc')", &ctx).unwrap(), string("***"));
        assert_eq!(eval("redact('abcdefgh')", &ctx).unwrap(), string("***"));
        assert_eq!(
            eval("partial('0712345678', 4)", &ctx).unwrap(),
            string("***5678")
        );
        assert_eq!(eval("partial('5678', 4)", &ctx).unwrap(), string("***"));
        assert!(eval_bool("is_msisdn('254712345678')", &ctx).unwrap());
        assert!(!eval_bool("is_email('nope')", &ctx).unwrap());
        assert_eq!(
            eval("hash_sha256('abc')", &ctx).unwrap(),
            string("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad")
        );
    }

    #[test]
    fn dataset_paths_nest() {
        let ds = dataset_value(&[
            ("dataset.row_count".into(), Value::Int(10)),
            ("dataset.customer_id.null_rate".into(), Value::Float(0.01)),
            ("dataset.assertions.pk.pass_rate".into(), Value::Float(1.0)),
        ]);
        let ctx = context(&Scope {
            dataset: Some(ds),
            ..Default::default()
        });
        assert!(
            eval_bool(
                "dataset.customer_id.null_rate < 0.02 && dataset.row_count == 10",
                &ctx
            )
            .unwrap()
        );
        assert!(eval_bool("dataset.assertions.pk.pass_rate == 1.0", &ctx).unwrap());
    }

    #[test]
    fn scalar_round_trip() {
        let v = to_scalar(
            &Value::List(Arc::new(vec![string("a")])),
            &Type::list(Type::String),
        )
        .unwrap();
        assert_eq!(
            from_scalar(&v),
            Some(Value::List(Arc::new(vec![string("a")])))
        );
    }
}
