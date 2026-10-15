//! A compiled contract (`.parcel`) as one governed SQL query, for engines parcel does not
//! embed.
//!
//! An engine target reads the compiled contract, the same artifact peQL loads, and writes it
//! as one query over the data its binding names:
//!
//! ```text
//! SELECT <exposed columns, transforms applied>
//! FROM <source>
//! WHERE <decide guards> AND <admit rules> AND <assert rules with on_fail: drop>
//! ```
//!
//! Row expressions are the compiled ones, written in the engine's dialect by DataFusion's
//! unparser, so the engine evaluates exactly what peQL evaluates. Where a compiled expression
//! has a caller placeholder (`$c0`), the placeholder's CEL, which reads only `ctx`, is written
//! as SQL in which each caller field is read the engine's way: a DuckDB variable, a Postgres
//! setting, a Spark configuration value. Nothing is copied and nothing is created per
//! contract; the engine resolves the contract's name to this query when it is queried.
//!
//! What one query cannot express is refused, not approximated: shapes (they act on the
//! caller's aggregates or spend a privacy budget), enrichers, and WebAssembly functions.

mod ctx;

use std::collections::BTreeMap;

use datafusion::logical_expr::Expr;
use datafusion::sql::unparser::Unparser;
use datafusion::sql::unparser::dialect::Dialect;
use parcel_core::Compilation;
use parcel_core::document::{AssertOnFail, Source};
use parcel_core::translate::CtxParam;
use parcel_core::types::Type;
use serde::{Deserialize, Serialize};

pub use ctx::caller_fields;

/// The version of the artifacts engine targets write.
pub const FORMAT: &str = "parcel-target/1";

/// A rule that filters rows: an `admit`, or an `assert` with `on_fail: drop`.
#[derive(Clone, Debug)]
pub struct Filter {
    pub id: String,
    pub op: &'static str,
    pub message: Option<String>,
    pub expr: Expr,
}

/// A `decide` rule: refuses the caller when its CEL, which reads only `ctx`, is false.
#[derive(Clone, Debug)]
pub struct Decision {
    pub id: String,
    pub message: Option<String>,
    pub cel: String,
}

/// A compiled contract as the parts of one query.
#[derive(Clone, Debug)]
pub struct Query {
    /// `<contract>@<version>`: the name an engine resolves.
    pub name: String,
    pub contract: String,
    pub version: String,
    pub compilation_hash: String,
    /// Each exposed column and its value: the raw column, or its transform.
    pub columns: Vec<(String, Expr)>,
    pub filters: Vec<Filter>,
    pub decisions: Vec<Decision>,
    /// The caller placeholders the row expressions use, with their CEL.
    pub params: Vec<CtxParam>,
    /// Declared `ctx.other` fields and their types.
    pub ctx_other: BTreeMap<String, Type>,
    /// Where the data is, as the contract's binding names it.
    pub binding: Source,
}

/// The name an engine resolves for a contract: `sales/customers@1.0.0`.
pub fn target_name(contract: &str, version: &str) -> String {
    format!("{contract}@{version}")
}

/// The governed query of a compiled contract.
pub fn governed(c: &Compilation) -> Result<Query, String> {
    let cc = &c.contract;
    if let Some(s) = cc.report.iter().find(|r| r.op == "shape") {
        return Err(format!(
            "shape `{}` is not supported by engine targets: shapes act on the caller's aggregates or spend a privacy budget, which one query cannot do",
            s.rule
        ));
    }
    if !cc.enrich.is_empty() {
        return Err("enrichers (`enrich`) are not supported by engine targets".into());
    }
    let builtin = parcel_core::Registry::builtin();
    let custom: Vec<&str> = cc
        .functions
        .iter()
        .filter(|p| builtin.get(&p.name).is_none_or(|b| b.hash != p.hash))
        .map(|p| p.name.as_str())
        .collect();
    if !custom.is_empty() {
        return Err(format!(
            "WebAssembly functions are not supported by engine targets: {}",
            custom.join(", ")
        ));
    }
    let message = |id: &str| {
        cc.report
            .iter()
            .find(|r| r.rule == id)
            .and_then(|r| r.message.clone())
    };
    let mut filters: Vec<Filter> = cc
        .admits
        .iter()
        .map(|(id, e)| Filter {
            id: id.clone(),
            op: "admit",
            message: message(id),
            expr: e.clone(),
        })
        .collect();
    filters.extend(
        cc.flags
            .iter()
            .filter(|f| f.on_fail == AssertOnFail::Drop)
            .map(|f| Filter {
                id: f.assert_id.clone(),
                op: "assert",
                message: message(&f.assert_id),
                expr: f.expr.clone(),
            }),
    );
    Ok(Query {
        name: target_name(&cc.name, &cc.version.to_string()),
        contract: cc.name.clone(),
        version: cc.version.to_string(),
        compilation_hash: cc.compilation_hash.clone(),
        columns: cc.projection.clone(),
        filters,
        decisions: cc
            .decisions
            .iter()
            .map(|d| Decision {
                id: d.id.clone(),
                message: message(&d.id),
                cel: d.cel.clone(),
            })
            .collect(),
        params: cc.params.clone(),
        ctx_other: cc.ctx_other.clone(),
        binding: cc.binding.source.clone(),
    })
}

/// One part of a query written in an engine's SQL.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Part {
    pub id: String,
    pub op: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    pub sql: String,
}

/// A column of the governed query.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Column {
    pub name: String,
    pub sql: String,
}

/// A caller field the query reads: its name (`tenant`, `other.region`) and parcel type.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CallerField {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: String,
}

/// A compiled contract written for one engine: its parts in the engine's SQL, and the query.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Artifact {
    pub format: String,
    pub engine: String,
    pub name: String,
    pub contract: String,
    pub version: String,
    pub compilation_hash: String,
    pub source: String,
    pub caller: Vec<CallerField>,
    pub columns: Vec<Column>,
    pub decisions: Vec<Part>,
    pub filters: Vec<Part>,
    /// The governed query, caller fields read the engine's way.
    pub query: String,
}

/// What an engine needs written its own way, beyond its dialect.
pub trait Engine {
    /// The engine's name, as `parcel compile --target` takes it.
    fn name(&self) -> &'static str;
    /// The dialect compiled expressions are unparsed in.
    fn dialect(&self) -> &dyn Dialect;
    /// A quoted identifier.
    fn quote(&self, ident: &str) -> String;
    /// The relation the contract's binding names, when no source is given.
    fn source(&self, binding: &Source) -> String;
    /// A boolean that is `cond`, or raises `message` when `cond` is false.
    fn guard(&self, cond: &str, message: &str) -> String;
    /// Caller field `field` (`tenant`, `other.region`) of parcel type `ty`.
    fn caller(&self, field: &str, ty: &Type) -> String;
    /// A string literal.
    fn string(&self, s: &str) -> String {
        string_literal(s)
    }
    /// A timestamp literal from RFC 3339 text.
    fn timestamp(&self, s: &str) -> String {
        format!("TIMESTAMP {}", string_literal(s))
    }
    /// `x` cast to parcel type `ty`.
    fn cast(&self, x: &str, ty: &Type) -> String;
    /// Characters in a string.
    fn string_length(&self, x: &str) -> String {
        format!("char_length({x})")
    }
    /// Elements in a list.
    fn list_length(&self, x: &str) -> String;
    /// Whether list `list` holds `item`.
    fn list_contains(&self, list: &str, item: &str) -> String;
    /// `a` starts with `b`.
    fn starts_with(&self, a: &str, b: &str) -> String;
    /// `a` ends with `b`.
    fn ends_with(&self, a: &str, b: &str) -> String;
    /// `a` contains `b`.
    fn contains(&self, a: &str, b: &str) -> String;
    /// `a` matches regular expression `pattern` anywhere.
    fn matches(&self, a: &str, pattern: &str) -> String;
    /// String concatenation.
    fn concat(&self, a: &str, b: &str) -> String {
        format!("({a} || {b})")
    }
}

impl Query {
    /// The query for `engine`, reading `source` (or the binding) as its data.
    pub fn artifact(&self, engine: &dyn Engine, source: Option<&str>) -> Result<Artifact, String> {
        let unparser = Unparser::new(engine.dialect());
        // Each caller placeholder, as SQL reading the caller the engine's way.
        let mut params = BTreeMap::new();
        for p in &self.params {
            let sql = ctx::to_sql(&p.cel, engine, &self.ctx_other)
                .map_err(|e| format!("caller expression `{}`: {e}", p.cel))?;
            params.insert(p.id.clone(), sql);
        }
        let sql = |e: &Expr| -> Result<String, String> {
            let ast = unparser
                .expr_to_sql(e)
                .map_err(|x| format!("{}: {x}", engine.name()))?;
            bind_params(&ast.to_string(), &params)
        };
        let source = match source {
            Some(s) => s.to_owned(),
            None => engine.source(&self.binding),
        };
        let columns = self
            .columns
            .iter()
            .map(|(name, e)| {
                Ok(Column {
                    name: name.clone(),
                    sql: sql(e)?,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        let decisions = self
            .decisions
            .iter()
            .map(|d| {
                Ok(Part {
                    id: d.id.clone(),
                    op: "decide".into(),
                    message: d.message.clone(),
                    sql: ctx::to_sql(&d.cel, engine, &self.ctx_other)
                        .map_err(|e| format!("decide `{}`: {e}", d.id))?,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        let filters = self
            .filters
            .iter()
            .map(|f| {
                Ok(Part {
                    id: f.id.clone(),
                    op: f.op.into(),
                    message: f.message.clone(),
                    sql: sql(&f.expr)?,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;

        let select = columns
            .iter()
            .map(|c| format!("{} AS {}", c.sql, engine.quote(&c.name)))
            .collect::<Vec<_>>()
            .join(",\n       ");
        let mut conditions: Vec<String> = decisions
            .iter()
            .map(|d| {
                let refusal = match &d.message {
                    Some(m) => format!("refused by {}: {m}", d.id),
                    None => format!("refused by {}", d.id),
                };
                format!("{} /* decide {} */", engine.guard(&d.sql, &refusal), d.id)
            })
            .collect();
        conditions.extend(
            filters
                .iter()
                .map(|f| format!("{} /* {} {} */", f.sql, f.op, f.id)),
        );
        let query = if conditions.is_empty() {
            format!("SELECT {select}\nFROM {source}")
        } else {
            format!(
                "SELECT {select}\nFROM {source}\nWHERE {}",
                conditions.join("\n  AND ")
            )
        };
        let caller = caller_fields(self)
            .into_iter()
            .map(|(name, ty)| CallerField {
                name,
                ty: ty.to_string(),
            })
            .collect();
        Ok(Artifact {
            format: FORMAT.into(),
            engine: engine.name().into(),
            name: self.name.clone(),
            contract: self.contract.clone(),
            version: self.version.clone(),
            compilation_hash: self.compilation_hash.clone(),
            source,
            caller,
            columns,
            decisions,
            filters,
            query,
        })
    }
}

/// Each `$cN` placeholder in `sql` replaced by its SQL, parenthesised.
fn bind_params(sql: &str, params: &BTreeMap<String, String>) -> Result<String, String> {
    let mut out = String::with_capacity(sql.len());
    let mut rest = sql;
    while let Some(i) = rest.find("$c") {
        out.push_str(&rest[..i]);
        let after = &rest[i + 2..];
        let digits = after
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(after.len());
        if digits == 0 {
            out.push_str("$c");
            rest = after;
            continue;
        }
        let id = format!("c{}", &after[..digits]);
        let value = params
            .get(&id)
            .ok_or_else(|| format!("no caller expression for ${id}"))?;
        out.push('(');
        out.push_str(value);
        out.push(')');
        rest = &after[digits..];
    }
    out.push_str(rest);
    Ok(out)
}

/// A call to function `name` with `args`, for writing SQL only: an engine renames a function
/// DataFusion spells differently, then the unparser writes the call.
pub fn named_call(name: &str, args: Vec<Expr>) -> Expr {
    #[derive(Debug, PartialEq, Eq, Hash)]
    struct Named {
        name: String,
        signature: datafusion::logical_expr::Signature,
    }
    impl datafusion::logical_expr::ScalarUDFImpl for Named {
        fn name(&self) -> &str {
            &self.name
        }
        fn signature(&self) -> &datafusion::logical_expr::Signature {
            &self.signature
        }
        fn return_type(
            &self,
            _: &[datafusion::arrow::datatypes::DataType],
        ) -> datafusion::error::Result<datafusion::arrow::datatypes::DataType> {
            Ok(datafusion::arrow::datatypes::DataType::Null)
        }
        fn invoke_with_args(
            &self,
            _: datafusion::logical_expr::ScalarFunctionArgs,
        ) -> datafusion::error::Result<datafusion::logical_expr::ColumnarValue> {
            datafusion::common::not_impl_err!("{} is for writing SQL only", self.name)
        }
    }
    datafusion::logical_expr::ScalarUDF::new_from_impl(Named {
        name: name.to_owned(),
        signature: datafusion::logical_expr::Signature::variadic_any(
            datafusion::logical_expr::Volatility::Immutable,
        ),
    })
    .call(args)
}

/// A string literal in standard SQL: single quotes, doubled inside.
pub fn string_literal(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// A Parquet binding's files: a directory reads every `.parquet` file under it.
pub fn parquet_glob(path: &str) -> String {
    let p = path.strip_prefix("file://").unwrap_or(path);
    if p.ends_with(".parquet") {
        p.to_owned()
    } else {
        format!("{}/**/*.parquet", p.trim_end_matches('/'))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholders_are_bound() {
        let params = BTreeMap::from([
            ("c0".to_owned(), "a = 1".to_owned()),
            ("c10".to_owned(), "b".to_owned()),
        ]);
        assert_eq!(
            bind_params("$c0 OR (x = $c10) OR y = '$cash'", &params).unwrap(),
            "(a = 1) OR (x = (b)) OR y = '$cash'"
        );
        assert!(bind_params("$c3", &params).is_err());
    }

    #[test]
    fn parquet_paths() {
        assert_eq!(parquet_glob("data/orders/"), "data/orders/**/*.parquet");
        assert_eq!(parquet_glob("file:///d/o.parquet"), "/d/o.parquet");
        assert_eq!(string_literal("o'brien"), "'o''brien'");
    }
}
