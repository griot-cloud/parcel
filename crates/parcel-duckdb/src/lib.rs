//! parcel contracts in DuckDB.
//!
//! A compiled contract (`.parcel`) becomes one DuckDB query over the data its binding names.
//! DuckDB reads the caller from variables, so the same query serves every caller:
//!
//! ```sql
//! SET VARIABLE parcel_tenant = 'our_company';
//! SET VARIABLE parcel_purpose = 'analytics';
//! SELECT * FROM "sales/customers@1.0.0";
//! ```
//!
//! The `parcel` DuckDB extension (crate `parcel-duckdb-extension`) resolves a contract's name
//! to this query on the fly, through a replacement scan: nothing is created per contract, and
//! DuckDB plans the query like any other, pushing filters into the scan.

use datafusion::logical_expr::Expr;
use datafusion::sql::sqlparser::ast;
use datafusion::sql::unparser::Unparser;
use datafusion::sql::unparser::dialect::{CharacterLengthStyle, Dialect, DuckDBDialect};
use parcel_core::Compilation;
use parcel_core::document::Source;
use parcel_core::types::Type;
use parcel_sql::{Artifact, Engine, named_call, parquet_glob, string_literal};

/// The DuckDB target.
pub struct DuckDb {
    dialect: DuckDbDialect,
}

impl Default for DuckDb {
    fn default() -> Self {
        DuckDb {
            dialect: DuckDbDialect(DuckDBDialect::new()),
        }
    }
}

/// A compiled contract as a DuckDB query, reading `source` (or the binding) as its data.
pub fn artifact(c: &Compilation, source: Option<&str>) -> Result<Artifact, String> {
    parcel_sql::governed(c)?.artifact(&DuckDb::default(), source)
}

/// The DuckDB variable caller field `field` is read from: `parcel_tenant`,
/// `parcel_other_region`.
pub fn variable(field: &str) -> String {
    format!("parcel_{}", field.replace('.', "_"))
}

fn type_name(ty: &Type) -> String {
    match ty {
        Type::Bool => "BOOLEAN".into(),
        Type::Int => "BIGINT".into(),
        Type::Uint => "UBIGINT".into(),
        Type::Double => "DOUBLE".into(),
        Type::String => "VARCHAR".into(),
        Type::Bytes => "BLOB".into(),
        Type::Timestamp => "TIMESTAMPTZ".into(),
        Type::Duration => "INTERVAL".into(),
        Type::List(t) => format!("{}[]", type_name(t)),
        Type::Embedding(n) => format!("FLOAT[{n}]"),
        Type::Decimal(s) => format!("DECIMAL(18,{s})"),
    }
}

impl Engine for DuckDb {
    fn name(&self) -> &'static str {
        "duckdb"
    }

    fn dialect(&self) -> &dyn Dialect {
        &self.dialect
    }

    fn quote(&self, ident: &str) -> String {
        format!("\"{}\"", ident.replace('"', "\"\""))
    }

    fn source(&self, binding: &Source) -> String {
        match binding {
            Source::Parquet(p) => format!("read_parquet({})", string_literal(&parquet_glob(p))),
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
            "CASE WHEN coalesce({cond}, FALSE) THEN TRUE ELSE error({}) END",
            string_literal(message)
        )
    }

    fn caller(&self, field: &str, ty: &Type) -> String {
        let read = format!(
            "CAST(getvariable({}) AS {})",
            string_literal(&variable(field)),
            type_name(ty)
        );
        // An unset `ctx.now` is the time of the query, as for every other caller.
        if field == "now" {
            format!("coalesce({read}, now())")
        } else {
            read
        }
    }

    fn cast(&self, x: &str, ty: &Type) -> String {
        format!("CAST({x} AS {})", type_name(ty))
    }

    fn string_length(&self, x: &str) -> String {
        format!("length({x})")
    }

    fn list_length(&self, x: &str) -> String {
        format!("len({x})")
    }

    fn list_contains(&self, list: &str, item: &str) -> String {
        format!("list_contains({list}, {item})")
    }

    fn starts_with(&self, a: &str, b: &str) -> String {
        format!("starts_with({a}, {b})")
    }

    fn ends_with(&self, a: &str, b: &str) -> String {
        format!("ends_with({a}, {b})")
    }

    fn contains(&self, a: &str, b: &str) -> String {
        format!("contains({a}, {b})")
    }

    fn matches(&self, a: &str, pattern: &str) -> String {
        format!("regexp_matches({a}, {pattern})")
    }
}

/// DataFusion's DuckDB dialect, with the functions DuckDB spells differently renamed. Every
/// rename keeps DataFusion's meaning.
struct DuckDbDialect(DuckDBDialect);

impl Dialect for DuckDbDialect {
    fn identifier_quote_style(&self, _: &str) -> Option<char> {
        Some('"')
    }

    fn character_length_style(&self) -> CharacterLengthStyle {
        self.0.character_length_style()
    }

    fn scalar_function_to_sql_overrides(
        &self,
        unparser: &Unparser,
        func_name: &str,
        args: &[Expr],
    ) -> datafusion::error::Result<Option<ast::Expr>> {
        let renamed = match func_name {
            // DuckDB's regexp_matches searches, as DataFusion's regexp_like does.
            "regexp_like" => Some("regexp_matches"),
            "character_length" => Some("length"),
            _ => None,
        };
        if let Some(name) = renamed {
            return Ok(Some(
                unparser.expr_to_sql(&named_call(name, args.to_vec()))?,
            ));
        }
        // encode(sha256(x), 'hex') is DuckDB's sha256(x), which returns lowercase hex.
        if func_name == "encode"
            && let [Expr::ScalarFunction(inner), _] = args
            && inner.name() == "sha256"
        {
            return Ok(Some(
                unparser.expr_to_sql(&named_call("sha256", inner.args.clone()))?,
            ));
        }
        self.0
            .scalar_function_to_sql_overrides(unparser, func_name, args)
    }
}
