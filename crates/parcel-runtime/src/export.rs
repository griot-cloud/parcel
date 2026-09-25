//! Export the validation plan as SQL, so a contract's checks run in engines parcel does not
//! embed: DuckDB, Postgres, Snowflake, BigQuery and others (see `design/parcel-ecosystem.md`).
//!
//! The SQL is produced by DataFusion's unparser from the same plan parcel executes. What a
//! dialect cannot express faithfully is listed as a warning rather than silently emitted.

use datafusion::common::TableReference;
use datafusion::common::tree_node::{Transformed, TreeNode, TreeNodeRecursion};
use datafusion::logical_expr::{Expr, LogicalPlan};
use datafusion::sql::unparser::Unparser;
use datafusion::sql::unparser::dialect::{
    BigQueryDialect, CustomDialectBuilder, DefaultDialect, Dialect, DuckDBDialect, MySqlDialect,
    PostgreSqlDialect, SnowflakeDialect, SqliteDialect,
};
use parcel_core::Compilation;
use parcel_core::compile::BINDING_TABLE;
use serde::Serialize;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SqlExport {
    pub dialect: String,
    pub sql: String,
    /// Constructs whose meaning in this dialect parcel has not verified.
    pub warnings: Vec<String>,
}

pub const DIALECTS: &[&str] = &[
    "datafusion",
    "duckdb",
    "postgres",
    "mysql",
    "sqlite",
    "bigquery",
    "snowflake",
];

/// The validation plan as SQL in `dialect`, scanning `table` in place of the binding.
pub fn validation_sql(c: &Compilation, dialect: &str, table: &str) -> Result<SqlExport, String> {
    let custom;
    let duck;
    let d: &dyn Dialect = match dialect {
        "datafusion" => {
            custom = CustomDialectBuilder::new().build();
            &custom
        }
        "duckdb" => {
            duck = DuckDb(DuckDBDialect::new());
            &duck
        }
        "postgres" => &PostgreSqlDialect {},
        "mysql" => &MySqlDialect {},
        "sqlite" => &SqliteDialect {},
        "bigquery" => &BigQueryDialect {},
        "snowflake" => &SnowflakeDialect::new(),
        "default" => &DefaultDialect {},
        other => {
            return Err(format!(
                "unknown dialect `{other}`; known: {}",
                DIALECTS.join(", ")
            ));
        }
    };
    let mut plan = rename_binding(c.validation.plan.clone(), table).map_err(|e| e.to_string())?;
    let mut warnings_extra = Vec::new();
    if dialect == "duckdb" {
        let (p, rewrote) = duckdb_integer_division(plan).map_err(|e| e.to_string())?;
        plan = p;
        if rewrote {
            warnings_extra.push("integer division is written as trunc over doubles for DuckDB: exact while |values| < 2^53".to_owned());
        }
    }
    let statement = Unparser::new(d)
        .plan_to_sql(&plan)
        .map_err(|e| format!("{dialect}: {e}"))?;
    let mut w = warnings(&plan, dialect);
    w.extend(warnings_extra);
    Ok(SqlExport {
        dialect: dialect.to_owned(),
        sql: statement.to_string(),
        warnings: w,
    })
}

/// DataFusion's DuckDB dialect writes every division as `//`, DuckDB's integer division, which
/// is wrong for the rates a validation plan computes. This one keeps `/`; integer divisions
/// are rewritten explicitly by [`duckdb_integer_division`].
struct DuckDb(DuckDBDialect);

impl Dialect for DuckDb {
    fn identifier_quote_style(&self, identifier: &str) -> Option<char> {
        self.0.identifier_quote_style(identifier)
    }
    fn character_length_style(&self) -> datafusion::sql::unparser::dialect::CharacterLengthStyle {
        self.0.character_length_style()
    }
    fn scalar_function_to_sql_overrides(
        &self,
        unparser: &Unparser,
        func_name: &str,
        args: &[Expr],
    ) -> datafusion::error::Result<Option<datafusion::sql::sqlparser::ast::Expr>> {
        // DuckDB spells these differently; the semantics match (search, lowercase hex).
        let renamed = match func_name {
            "regexp_like" => Some("regexp_matches"),
            "character_length" => Some("length"),
            _ => None,
        };
        if let Some(name) = renamed {
            return Ok(Some(
                unparser.expr_to_sql(&named_call(name, args.to_vec()))?,
            ));
        }
        // encode(sha256(x), 'hex') is DuckDB's sha256(x), which already returns hex.
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

/// A call to a function by name, for unparsing only.
fn named_call(name: &str, args: Vec<Expr>) -> Expr {
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
            datafusion::common::not_impl_err!("{} exists for SQL export only", self.name)
        }
    }
    let udf = datafusion::logical_expr::ScalarUDF::new_from_impl(Named {
        name: name.to_owned(),
        signature: datafusion::logical_expr::Signature::variadic_any(
            datafusion::logical_expr::Volatility::Immutable,
        ),
    });
    udf.call(args)
}

/// Integer `a / b` → `CAST(trunc(CAST(a AS DOUBLE) / CAST(b AS DOUBLE)) AS BIGINT)`: DuckDB's `/`
/// on integers returns a double, while DataFusion and CEL truncate.
fn duckdb_integer_division(plan: LogicalPlan) -> datafusion::error::Result<(LogicalPlan, bool)> {
    use datafusion::arrow::datatypes::DataType;
    use datafusion::logical_expr::{BinaryExpr, ExprSchemable, Operator, cast};
    let mut rewrote = false;
    let out = plan.transform_up(|node| {
        let inputs = node.inputs();
        if inputs.is_empty() {
            return Ok(Transformed::no(node));
        }
        let mut schema = inputs[0].schema().as_ref().clone();
        for other in &inputs[1..] {
            schema.merge(other.schema());
        }
        let r = node.map_expressions(|e| {
            e.transform_up(|x| match &x {
                Expr::BinaryExpr(BinaryExpr {
                    left,
                    op: Operator::Divide,
                    right,
                }) => {
                    let lt = left.get_type(&schema)?;
                    let rt = right.get_type(&schema)?;
                    if lt.is_integer() && rt.is_integer() {
                        rewrote = true;
                        let quotient = datafusion::logical_expr::binary_expr(
                            cast(left.as_ref().clone(), DataType::Float64),
                            Operator::Divide,
                            cast(right.as_ref().clone(), DataType::Float64),
                        );
                        Ok(Transformed::yes(cast(
                            datafusion::functions::math::expr_fn::trunc(vec![quotient]),
                            DataType::Int64,
                        )))
                    } else {
                        Ok(Transformed::no(x))
                    }
                }
                _ => Ok(Transformed::no(x)),
            })
        })?;
        Ok(r)
    })?;
    Ok((out.data.recompute_schema()?, rewrote))
}

fn rename_binding(plan: LogicalPlan, table: &str) -> datafusion::error::Result<LogicalPlan> {
    let reference = TableReference::parse_str(table);
    let out = plan.transform_up(|node| match node {
        LogicalPlan::TableScan(ts) if ts.table_name.table() == BINDING_TABLE => {
            let scan = datafusion::logical_expr::LogicalPlanBuilder::scan(
                reference.clone(),
                ts.source.clone(),
                None,
            )?
            .build()?;
            Ok(Transformed::yes(scan))
        }
        other => Ok(Transformed::no(other)),
    })?;
    // Column qualifiers still name the binding; requalify them.
    let out = out.data.transform_up(|node| {
        node.map_expressions(|e| {
            e.transform_up(|x| match x {
                Expr::Column(mut c)
                    if c.relation
                        .as_ref()
                        .is_some_and(|r| r.table() == BINDING_TABLE) =>
                {
                    c.relation = Some(reference.clone());
                    Ok(Transformed::yes(Expr::Column(c)))
                }
                other => Ok(Transformed::no(other)),
            })
        })
    })?;
    out.data.recompute_schema()
}

/// Functions whose semantics parcel verified only in DataFusion.
fn warnings(plan: &LogicalPlan, dialect: &str) -> Vec<String> {
    if dialect == "datafusion" {
        return Vec::new();
    }
    let mut names = std::collections::BTreeSet::new();
    let _ = plan.apply(|node| {
        for e in node.expressions() {
            let _ = e.apply(|x| {
                match x {
                    Expr::ScalarFunction(f) => {
                        names.insert(f.name().to_owned());
                    }
                    Expr::HigherOrderFunction(f) => {
                        names.insert(f.name().to_owned());
                    }
                    _ => {}
                }
                Ok(TreeNodeRecursion::Continue)
            });
        }
        Ok(TreeNodeRecursion::Continue)
    });
    // Checked against DuckDB by `examples/verify-duckdb.py`.
    let duckdb_verified = [
        "regexp_like",
        "character_length",
        "sha256",
        "encode",
        "starts_with",
        "ends_with",
        "strpos",
        "date_part",
        "trunc",
    ];
    names
        .into_iter()
        .filter(|n| !matches!(n.as_str(), "coalesce" | "concat_ws"))
        .filter(|n| !(dialect == "duckdb" && duckdb_verified.contains(&n.as_str())))
        .map(|n| match n.as_str() {
            "regexp_like" => format!("regexp_like: regular expression syntax and search semantics differ between engines; check `{dialect}`"),
            "sha256" | "encode" => format!("{n}: hashing and encoding functions differ between engines; check `{dialect}`"),
            "parcel_bytes_len" => "parcel_bytes_len: a parcel function; replace with the dialect's byte length".to_owned(),
            n if n.starts_with("array_") => format!("{n}: list functions and lambdas are not portable; check `{dialect}`"),
            other => format!("{other}: verified in DataFusion only; check `{dialect}`"),
        })
        .collect()
}

#[cfg(feature = "substrait")]
/// The validation plan as a Substrait plan (protobuf bytes), scanning `table` in place of the
/// binding, for engines that consume Substrait. Functions are carried as Substrait extension
/// functions by name; those whose meaning parcel verified only in DataFusion are warned about.
pub fn validation_substrait(
    c: &Compilation,
    table: &str,
) -> Result<(Vec<u8>, Vec<String>), String> {
    use prost::Message;
    let plan = rename_binding(c.validation.plan.clone(), table).map_err(|e| e.to_string())?;
    let state = datafusion::prelude::SessionContext::new().state();
    let substrait = datafusion_substrait::logical_plan::producer::to_substrait_plan(&plan, &state)
        .map_err(|e| format!("substrait: {e}"))?;
    Ok((substrait.encode_to_vec(), warnings(&plan, "substrait")))
}
