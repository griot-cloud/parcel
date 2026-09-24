//! parcel's type system and its mapping to Arrow.
//!
//! Every CEL expression parcel accepts is typed with [`Type`]. Columns of the
//! bound data arrive as Arrow types and are mapped onto [`Type`]; a column whose
//! Arrow type has no v0 mapping can still be exposed as a pass-through, but no
//! rule may read it.

use std::fmt;

use datafusion_common::arrow::datatypes::{DataType, Field, TimeUnit};

/// The type of a CEL value inside a parcel rule.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Type {
    Bool,
    Int,
    Uint,
    Double,
    String,
    Bytes,
    Timestamp,
    Duration,
    List(Box<Type>),
    /// An exact decimal with this many digits after the point, at most 18 digits in all.
    /// The reference interpreter sees its unscaled integer (100.50 at scale 2 is 10050).
    Decimal(i8),
}

/// Decimals rules can read: 18 digits fit an i64 unscaled value exactly.
pub const DECIMAL_PRECISION: u8 = 18;

impl Type {
    pub fn list(elem: Type) -> Type {
        Type::List(Box::new(elem))
    }

    /// Types with a total order, which is what `<`, `min` and `max` need.
    pub fn is_ordered(&self) -> bool {
        matches!(
            self,
            Type::Int
                | Type::Uint
                | Type::Double
                | Type::String
                | Type::Bytes
                | Type::Timestamp
                | Type::Duration
                | Type::Decimal(_)
        )
    }

    pub fn is_numeric(&self) -> bool {
        matches!(self, Type::Int | Type::Uint | Type::Double)
    }

    /// The canonical Arrow type a value of this type is materialised as.
    pub fn to_arrow(&self) -> DataType {
        match self {
            Type::Bool => DataType::Boolean,
            Type::Int => DataType::Int64,
            Type::Uint => DataType::UInt64,
            Type::Double => DataType::Float64,
            Type::String => DataType::Utf8,
            Type::Bytes => DataType::Binary,
            Type::Timestamp => DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into())),
            Type::Duration => DataType::Duration(TimeUnit::Microsecond),
            Type::List(elem) => DataType::List(Field::new_list_field(elem.to_arrow(), true).into()),
            Type::Decimal(s) => DataType::Decimal128(DECIMAL_PRECISION, *s),
        }
    }

    /// Map an Arrow type onto a parcel type, or say why it cannot be read by rules.
    pub fn from_arrow(dt: &DataType) -> Result<Type, String> {
        Ok(match dt {
            DataType::Boolean => Type::Bool,
            DataType::Int8 | DataType::Int16 | DataType::Int32 | DataType::Int64 => Type::Int,
            DataType::UInt8 | DataType::UInt16 | DataType::UInt32 | DataType::UInt64 => Type::Uint,
            DataType::Float16 | DataType::Float32 | DataType::Float64 => Type::Double,
            DataType::Utf8 | DataType::LargeUtf8 | DataType::Utf8View => Type::String,
            DataType::Binary
            | DataType::LargeBinary
            | DataType::BinaryView
            | DataType::FixedSizeBinary(_) => Type::Bytes,
            DataType::Timestamp(_, _) => Type::Timestamp,
            DataType::Duration(_) => Type::Duration,
            DataType::List(f)
            | DataType::LargeList(f)
            | DataType::ListView(f)
            | DataType::LargeListView(f) => Type::list(Type::from_arrow(f.data_type())?),
            DataType::Decimal32(p, s)
            | DataType::Decimal64(p, s)
            | DataType::Decimal128(p, s)
            | DataType::Decimal256(p, s) => {
                if *p > DECIMAL_PRECISION || *s < 0 {
                    return Err(format!(
                        "decimal({p},{s}) is wider than rules can read exactly; rules read decimals of up to \
                         {DECIMAL_PRECISION} digits with a non-negative scale"
                    ));
                }
                Type::Decimal(*s)
            }
            DataType::Dictionary(_, value) => Type::from_arrow(value)?,
            other => {
                return Err(format!(
                    "Arrow type {other} is not supported by rules in parcel v0"
                ));
            }
        })
    }
}

/// Types serialise as their display form (`int`, `list<string>`), which keeps hashes readable and stable.
impl serde::Serialize for Type {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl fmt::Display for Type {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Type::Bool => f.write_str("bool"),
            Type::Int => f.write_str("int"),
            Type::Uint => f.write_str("uint"),
            Type::Double => f.write_str("double"),
            Type::String => f.write_str("string"),
            Type::Bytes => f.write_str("bytes"),
            Type::Timestamp => f.write_str("timestamp"),
            Type::Duration => f.write_str("duration"),
            Type::List(e) => write!(f, "list<{e}>"),
            Type::Decimal(sc) => write!(f, "decimal({DECIMAL_PRECISION},{sc})"),
        }
    }
}

/// Parse a column type as written in a contract's `expose` section.
///
/// Accepted: `bool`, `int8`..`int64`, `uint8`..`uint64`, `float32`, `float64`,
/// `utf8` (alias `string`), `large_utf8`, `binary` (alias `bytes`),
/// `timestamp` (microseconds, UTC) or `timestamp[s|ms|us|ns]`, `duration`
/// or `duration[s|ms|us|ns]`, `date32`, `decimal(p,s)`, and `list<T>`.
pub fn parse_type_name(s: &str) -> Result<DataType, String> {
    let t = s.trim();
    let unit = |u: &str| -> Result<TimeUnit, String> {
        match u {
            "s" => Ok(TimeUnit::Second),
            "ms" => Ok(TimeUnit::Millisecond),
            "us" => Ok(TimeUnit::Microsecond),
            "ns" => Ok(TimeUnit::Nanosecond),
            _ => Err(format!("unknown time unit '{u}' in type '{s}'")),
        }
    };
    if let Some(inner) = t.strip_prefix("list<").and_then(|r| r.strip_suffix('>')) {
        let elem = parse_type_name(inner)?;
        return Ok(DataType::List(Field::new_list_field(elem, true).into()));
    }
    if let Some(args) = t.strip_prefix("decimal(").and_then(|r| r.strip_suffix(')')) {
        let (p, sc) = args
            .split_once(',')
            .ok_or_else(|| format!("expected decimal(p,s), got '{s}'"))?;
        let p: u8 = p
            .trim()
            .parse()
            .map_err(|_| format!("bad decimal precision in '{s}'"))?;
        let sc: i8 = sc
            .trim()
            .parse()
            .map_err(|_| format!("bad decimal scale in '{s}'"))?;
        if p == 0 || p > 38 {
            return Err(format!("decimal precision must be 1..=38 in '{s}'"));
        }
        return Ok(DataType::Decimal128(p, sc));
    }
    if let Some(u) = t
        .strip_prefix("timestamp[")
        .and_then(|r| r.strip_suffix(']'))
    {
        return Ok(DataType::Timestamp(unit(u)?, Some("UTC".into())));
    }
    if let Some(u) = t
        .strip_prefix("duration[")
        .and_then(|r| r.strip_suffix(']'))
    {
        return Ok(DataType::Duration(unit(u)?));
    }
    Ok(match t {
        "bool" | "boolean" => DataType::Boolean,
        "int8" => DataType::Int8,
        "int16" => DataType::Int16,
        "int32" => DataType::Int32,
        "int64" => DataType::Int64,
        "uint8" => DataType::UInt8,
        "uint16" => DataType::UInt16,
        "uint32" => DataType::UInt32,
        "uint64" => DataType::UInt64,
        "float32" => DataType::Float32,
        "float64" | "double" => DataType::Float64,
        "utf8" | "string" => DataType::Utf8,
        "large_utf8" => DataType::LargeUtf8,
        "binary" | "bytes" => DataType::Binary,
        "timestamp" => DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into())),
        "duration" => DataType::Duration(TimeUnit::Microsecond),
        "date32" | "date" => DataType::Date32,
        _ => return Err(format!("unknown type '{s}'")),
    })
}

/// Whether a raw column may be exposed under a declared type without a transform.
///
/// Identical types always match. Otherwise both must map to the same parcel
/// type (e.g. `int32` exposed as `int64`); the view builder inserts the cast.
pub fn exposable_as(raw: &DataType, declared: &DataType) -> bool {
    if raw == declared {
        return true;
    }
    match (raw, declared) {
        (DataType::Decimal128(_, a), DataType::Decimal128(_, b)) => a == b,
        (DataType::Timestamp(..), DataType::Timestamp(..)) => true,
        _ => {
            matches!((Type::from_arrow(raw), Type::from_arrow(declared)), (Ok(a), Ok(b)) if a == b)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_type_names() {
        assert_eq!(parse_type_name("int64").unwrap(), DataType::Int64);
        assert_eq!(parse_type_name("utf8").unwrap(), DataType::Utf8);
        assert_eq!(
            parse_type_name("decimal(18,2)").unwrap(),
            DataType::Decimal128(18, 2)
        );
        assert_eq!(
            Type::from_arrow(&parse_type_name("list<utf8>").unwrap()).unwrap(),
            Type::list(Type::String)
        );
        assert!(parse_type_name("varchar").is_err());
    }

    #[test]
    fn decimals_are_passthrough_only() {
        assert_eq!(
            Type::from_arrow(&DataType::Decimal128(18, 2)).unwrap(),
            Type::Decimal(2)
        );
        assert!(Type::from_arrow(&DataType::Decimal128(38, 2)).is_err());
        assert!(exposable_as(
            &DataType::Decimal128(18, 2),
            &DataType::Decimal128(18, 2)
        ));
    }
}
