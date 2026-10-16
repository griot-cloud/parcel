//! parcel contracts in Spark.
//!
//! A compiled contract (`.parcel`) becomes a JSON file carrying one governed Spark SQL query.
//! The `parcel` Spark extension (Java, in `jvm/`) reads these files and resolves a contract's
//! name while Spark analyses a query, replacing it with the governed query: nothing is created
//! per contract, and Spark optimises the query like any other.
//!
//! ```text
//! spark-sql --jars parcel-spark.jar \
//!   --conf spark.sql.extensions=cloud.griot.parcel.spark.ParcelExtensions \
//!   --conf spark.parcel.contracts=contracts/ \
//!   --conf spark.parcel.caller.tenant=our_company \
//!   --conf spark.parcel.caller.purpose=analytics
//! > SELECT region, count(*) FROM `sales/customers@1.0.0` GROUP BY region;
//! ```
//!
//! Caller fields are written as `__parcel_caller(<field>)__` tokens; the extension replaces
//! each with a typed literal from `spark.parcel.caller.<field>` when it resolves the name.

use datafusion::logical_expr::Expr;
use datafusion::sql::sqlparser::ast;
use datafusion::sql::unparser::Unparser;
use datafusion::sql::unparser::dialect::{CharacterLengthStyle, Dialect};
use parcel_core::Compilation;
use parcel_core::document::Source;
use parcel_core::types::Type;
use parcel_sql::{Artifact, Engine, named_call};

/// The Spark target.
#[derive(Default)]
pub struct Spark {
    dialect: SparkDialect,
}

/// A compiled contract as a governed Spark SQL query, reading `source` (or the binding).
pub fn artifact(c: &Compilation, source: Option<&str>) -> Result<Artifact, String> {
    let mut a = parcel_sql::governed(c)?.artifact(&Spark::default(), source)?;
    // Spark reads a backslash in a string literal as an escape; keep each one literal.
    a.query = a.query.replace('\\', "\\\\");
    Ok(a)
}

/// The JSON file the Spark extension loads.
pub fn json(c: &Compilation, source: Option<&str>) -> Result<String, String> {
    serde_json::to_string_pretty(&artifact(c, source)?).map_err(|e| e.to_string())
}

/// The token caller field `field` is written as, for the extension to replace.
pub fn token(field: &str) -> String {
    format!("__parcel_caller({field})__")
}

fn type_name(ty: &Type) -> String {
    match ty {
        Type::Bool => "BOOLEAN".into(),
        Type::Int => "BIGINT".into(),
        Type::Uint => "DECIMAL(20,0)".into(),
        Type::Double => "DOUBLE".into(),
        Type::String => "STRING".into(),
        Type::Bytes => "BINARY".into(),
        Type::Timestamp => "TIMESTAMP".into(),
        Type::Duration => "INTERVAL DAY TO SECOND".into(),
        Type::List(t) => format!("ARRAY<{}>", type_name(t)),
        Type::Embedding(_) => "ARRAY<FLOAT>".into(),
        Type::Decimal(s) => format!("DECIMAL(18,{s})"),
    }
}

impl Engine for Spark {
    fn name(&self) -> &'static str {
        "spark"
    }

    fn dialect(&self) -> &dyn Dialect {
        &self.dialect
    }

    fn quote(&self, ident: &str) -> String {
        format!("`{}`", ident.replace('`', "``"))
    }

    fn source(&self, binding: &Source) -> String {
        match binding {
            Source::Parquet(p) => {
                let path = p.strip_prefix("file://").unwrap_or(p);
                format!("parquet.{}", self.quote(path.trim_end_matches('/')))
            }
            Source::Iceberg(t) => t
                .namespace
                .iter()
                .chain(std::iter::once(&t.table))
                .map(|p| self.quote(p))
                .collect::<Vec<_>>()
                .join("."),
        }
    }

    fn guard(&self, cond: &str, message: &str) -> String {
        format!(
            "CASE WHEN coalesce({cond}, FALSE) THEN TRUE ELSE CAST(raise_error({}) AS BOOLEAN) END",
            self.string(message)
        )
    }

    fn caller(&self, field: &str, _: &Type) -> String {
        token(field)
    }

    fn string(&self, s: &str) -> String {
        format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'"))
    }

    fn cast(&self, x: &str, ty: &Type) -> String {
        format!("CAST({x} AS {})", type_name(ty))
    }

    fn list_length(&self, x: &str) -> String {
        format!("size({x})")
    }

    fn list_contains(&self, list: &str, item: &str) -> String {
        format!("array_contains({list}, {item})")
    }

    fn starts_with(&self, a: &str, b: &str) -> String {
        format!("startswith({a}, {b})")
    }

    fn ends_with(&self, a: &str, b: &str) -> String {
        format!("endswith({a}, {b})")
    }

    fn contains(&self, a: &str, b: &str) -> String {
        format!("contains({a}, {b})")
    }

    fn matches(&self, a: &str, pattern: &str) -> String {
        format!("regexp_like({a}, {pattern})")
    }

    fn concat(&self, a: &str, b: &str) -> String {
        format!("concat({a}, {b})")
    }
}

/// Spark SQL as DataFusion's unparser writes it: backtick identifiers, `STRING`, and the
/// functions Spark names differently.
#[derive(Default)]
struct SparkDialect;

impl Dialect for SparkDialect {
    fn identifier_quote_style(&self, _: &str) -> Option<char> {
        Some('`')
    }

    fn utf8_cast_dtype(&self) -> ast::DataType {
        ast::DataType::String(None)
    }

    fn large_utf8_cast_dtype(&self) -> ast::DataType {
        ast::DataType::String(None)
    }

    fn character_length_style(&self) -> CharacterLengthStyle {
        CharacterLengthStyle::CharacterLength
    }

    fn timestamp_cast_dtype(
        &self,
        _: &datafusion::arrow::datatypes::TimeUnit,
        _: &Option<std::sync::Arc<str>>,
    ) -> ast::DataType {
        ast::DataType::Timestamp(None, ast::TimezoneInfo::None)
    }

    fn scalar_function_to_sql_overrides(
        &self,
        unparser: &Unparser,
        func_name: &str,
        args: &[Expr],
    ) -> datafusion::error::Result<Option<ast::Expr>> {
        let renamed = match func_name {
            "starts_with" => Some("startswith"),
            "ends_with" => Some("endswith"),
            "strpos" => Some("instr"),
            _ => None,
        };
        if let Some(name) = renamed {
            return Ok(Some(
                unparser.expr_to_sql(&named_call(name, args.to_vec()))?,
            ));
        }
        // encode(sha256(x), 'hex') is Spark's sha2(x, 256), which returns lowercase hex.
        if func_name == "encode"
            && let [Expr::ScalarFunction(inner), _] = args
            && inner.name() == "sha256"
        {
            let mut a = inner.args.clone();
            a.push(datafusion::logical_expr::lit(256i64));
            return Ok(Some(unparser.expr_to_sql(&named_call("sha2", a))?));
        }
        Ok(None)
    }
}
