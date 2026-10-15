//! parcel contracts in Postgres.
//!
//! A compiled contract (`.parcel`) becomes one SQL file: it installs the `parcel` schema once
//! (a registry of contracts and three functions) and registers the contract's governed query.
//! Postgres has no hook for unknown names, so a session asks for a contract by name and gets
//! it as a temporary view, gone when the session ends:
//!
//! ```sql
//! \i customers.postgres.sql
//! SELECT parcel.caller(id => 'hq-analyst', tenant => 'our_company', purpose => 'analytics');
//! SELECT parcel.use('sales/customers@1.0.0');
//! SELECT region, count(*) FROM "sales/customers@1.0.0" GROUP BY region;
//! ```
//!
//! The view reads the caller from session settings (`parcel.tenant`, ...) every time it is
//! queried, and Postgres plans it like any query: filters reach the table and its indexes.
//! Nothing permanent is created per contract; the base table is never altered.

use datafusion::logical_expr::Expr;
use datafusion::sql::sqlparser::ast;
use datafusion::sql::unparser::Unparser;
use datafusion::sql::unparser::dialect::{Dialect, PostgreSqlDialect};
use parcel_core::Compilation;
use parcel_core::document::Source;
use parcel_core::types::Type;
use parcel_sql::{Artifact, Engine, named_call, string_literal};

/// What `parcel compile --target postgres` writes before each contract: the `parcel` schema,
/// created when missing, and its functions, replaced.
pub const INSTALL: &str = include_str!("../sql/install.sql");

/// The Postgres target.
pub struct Postgres {
    dialect: PostgresDialect,
}

impl Default for Postgres {
    fn default() -> Self {
        Postgres {
            dialect: PostgresDialect(PostgreSqlDialect {}),
        }
    }
}

/// A compiled contract as a governed Postgres query, reading `source` (or the table its
/// binding names) as its data.
pub fn artifact(c: &Compilation, source: Option<&str>) -> Result<Artifact, String> {
    parcel_sql::governed(c)?.artifact(&Postgres::default(), source)
}

/// The SQL file for a compiled contract: the install script, then the contract registered.
pub fn script(c: &Compilation, source: Option<&str>) -> Result<String, String> {
    let a = artifact(c, source)?;
    let json = serde_json::to_string_pretty(&a).map_err(|e| e.to_string())?;
    let quote = dollar_quote(&[&a.query, &json]);
    Ok(format!(
        "-- {name}: a parcel contract for Postgres, compiled {hash}.\n\
         -- Apply with psql, then: SELECT parcel.use('{name}');\n\n\
         {INSTALL}\n\
         INSERT INTO parcel.contracts (name, contract, version, compilation_hash, source, query, artifact)\n\
         VALUES ({n}, {contract}, {version}, {h}, {src},\n\
         {quote}{query}{quote},\n\
         {quote}{json}{quote}::jsonb)\n\
         ON CONFLICT (name) DO UPDATE SET\n\
         \x20 contract = EXCLUDED.contract, version = EXCLUDED.version,\n\
         \x20 compilation_hash = EXCLUDED.compilation_hash, source = EXCLUDED.source,\n\
         \x20 query = EXCLUDED.query, artifact = EXCLUDED.artifact, registered_at = now();\n",
        name = a.name,
        hash = &a.compilation_hash[..16],
        n = string_literal(&a.name),
        contract = string_literal(&a.contract),
        version = string_literal(&a.version),
        h = string_literal(&a.compilation_hash),
        src = string_literal(&a.source),
        query = a.query,
    ))
}

/// A dollar quote, `$parcel$` or a longer one, that none of `texts` contains.
fn dollar_quote(texts: &[&str]) -> String {
    let mut tag = "parcel".to_owned();
    while texts.iter().any(|t| t.contains(&format!("${tag}$"))) {
        tag.push('_');
    }
    format!("${tag}$")
}

/// The session setting caller field `field` is read from: `parcel.tenant`,
/// `parcel.other_region`.
pub fn setting(field: &str) -> String {
    format!("parcel.{}", field.replace('.', "_"))
}

fn type_name(ty: &Type) -> String {
    match ty {
        Type::Bool => "BOOLEAN".into(),
        Type::Int => "BIGINT".into(),
        Type::Uint => "NUMERIC(20,0)".into(),
        Type::Double => "DOUBLE PRECISION".into(),
        Type::String => "TEXT".into(),
        Type::Bytes => "BYTEA".into(),
        Type::Timestamp => "TIMESTAMPTZ".into(),
        Type::Duration => "INTERVAL".into(),
        Type::List(t) => format!("{}[]", type_name(t)),
        Type::Embedding(_) => "REAL[]".into(),
        Type::Decimal(s) => format!("NUMERIC(18,{s})"),
    }
}

impl Engine for Postgres {
    fn name(&self) -> &'static str {
        "postgres"
    }

    fn dialect(&self) -> &dyn Dialect {
        &self.dialect
    }

    fn quote(&self, ident: &str) -> String {
        format!("\"{}\"", ident.replace('"', "\"\""))
    }

    fn source(&self, binding: &Source) -> String {
        match binding {
            // A Parquet binding names files; in Postgres the data is the table of that name.
            Source::Parquet(p) => {
                let last = p
                    .trim_end_matches('/')
                    .rsplit('/')
                    .next()
                    .unwrap_or(p)
                    .trim_end_matches(".parquet");
                self.quote(last)
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
            "CASE WHEN coalesce({cond}, FALSE) THEN TRUE ELSE parcel.refuse({}) END",
            string_literal(message)
        )
    }

    fn caller(&self, field: &str, ty: &Type) -> String {
        let text = format!(
            "NULLIF(current_setting({}, TRUE), '')",
            string_literal(&setting(field))
        );
        match ty {
            Type::String => text,
            // A list is written comma-separated: `SET parcel.roles = 'admin,auditor'`.
            Type::List(t) => format!("CAST(string_to_array({text}, ',') AS {}[])", type_name(t)),
            Type::Timestamp if field == "now" => {
                format!("coalesce(CAST({text} AS TIMESTAMPTZ), now())")
            }
            other => format!("CAST({text} AS {})", type_name(other)),
        }
    }

    fn timestamp(&self, s: &str) -> String {
        format!("TIMESTAMPTZ {}", string_literal(s))
    }

    fn cast(&self, x: &str, ty: &Type) -> String {
        format!("CAST({x} AS {})", type_name(ty))
    }

    fn list_length(&self, x: &str) -> String {
        format!("cardinality({x})")
    }

    fn list_contains(&self, list: &str, item: &str) -> String {
        format!("({item} = ANY({list}))")
    }

    fn starts_with(&self, a: &str, b: &str) -> String {
        format!("starts_with({a}, {b})")
    }

    fn ends_with(&self, a: &str, b: &str) -> String {
        format!("(right({a}, char_length({b})) = {b})")
    }

    fn contains(&self, a: &str, b: &str) -> String {
        format!("(strpos({a}, {b}) > 0)")
    }

    fn matches(&self, a: &str, pattern: &str) -> String {
        format!("regexp_like({a}, {pattern})")
    }
}

/// DataFusion's Postgres dialect, with SHA-256 of text written the Postgres way.
struct PostgresDialect(PostgreSqlDialect);

impl Dialect for PostgresDialect {
    fn identifier_quote_style(&self, _: &str) -> Option<char> {
        Some('"')
    }

    fn character_length_style(&self) -> datafusion::sql::unparser::dialect::CharacterLengthStyle {
        self.0.character_length_style()
    }

    fn float64_ast_dtype(&self) -> ast::DataType {
        ast::DataType::DoublePrecision
    }

    fn scalar_function_to_sql_overrides(
        &self,
        unparser: &Unparser,
        func_name: &str,
        args: &[Expr],
    ) -> datafusion::error::Result<Option<ast::Expr>> {
        // Postgres hashes bytes: sha256(text) is sha256(convert_to(text, 'UTF8')).
        if func_name == "sha256"
            && let [x] = args
            && !matches!(x, Expr::ScalarFunction(f) if f.name() == "convert_to")
        {
            let bytes = named_call(
                "convert_to",
                vec![x.clone(), datafusion::logical_expr::lit("UTF8")],
            );
            return Ok(Some(
                unparser.expr_to_sql(&named_call("sha256", vec![bytes]))?,
            ));
        }
        self.0
            .scalar_function_to_sql_overrides(unparser, func_name, args)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dollar_quotes_avoid_the_text() {
        assert_eq!(dollar_quote(&["select 1"]), "$parcel$");
        assert_eq!(dollar_quote(&["a $parcel$ b"]), "$parcel_$");
    }
}
