//! The engine: register contracts, write under them, validate them, query through them.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;

use cel::Value;
use chrono::Utc;
use datafusion::arrow::array::RecordBatch;
use datafusion::arrow::datatypes::SchemaRef;
use datafusion::catalog::view::ViewTable;
use datafusion::common::tree_node::{Transformed, TreeNode, TreeNodeRecursion};
use datafusion::common::{ParamValues, ScalarValue};
use datafusion::config::TableParquetOptions;
use datafusion::dataframe::DataFrameWriteOptions;
use datafusion::datasource::{MemTable, provider_as_source};
use datafusion::logical_expr::{Expr, LogicalPlan, LogicalPlanBuilder, SortExpr};
use datafusion::prelude::{SessionConfig, SessionContext};
use parcel_core::compile::{BINDING_TABLE, Flag};
use parcel_core::document::{AssertOnFail, GuaranteeOnFail};
use parcel_core::{Compilation, CompiledContract, ContractDoc, Registry, ShapeOp, compile};
use parcel_runtime::Caller;
use parcel_runtime::reference::{self, Scope};
use serde::Serialize;

use crate::binding::{self, CONTRACT_HASH_KEY};
use crate::budget::BudgetStore;
use crate::error::{EngineError, Result};
use crate::manifest::Manifest;
use crate::shape;

/// A registered contract: its document and the three compiled artifacts.
#[derive(Clone, Debug)]
pub struct Registered {
    pub doc: ContractDoc,
    pub compilation: Arc<Compilation>,
}

/// The one-row validation verdict (design 9.2), with the statistics it carried.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Verdict {
    pub contract: String,
    pub contract_hash: String,
    pub compilation_hash: String,
    pub valid: bool,
    pub breached: Vec<String>,
    pub row_count: i64,
    /// Failing rows per assert.
    pub failures: BTreeMap<String, i64>,
    /// Data-only guarantees and whether each held.
    pub guarantees: BTreeMap<String, bool>,
    /// Every statistic, keyed by its column name.
    pub stats: BTreeMap<String, serde_json::Value>,
    pub data_hash: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct WriteReport {
    pub rows_written: usize,
    pub files: usize,
    pub verdict: Verdict,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WriteMode {
    Append,
    Overwrite,
}

/// What peQL's resolver decides for one contract and one caller (peQL design 4.3).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Resolution {
    pub contract: String,
    pub version: u32,
    pub contract_hash: String,
    pub compilation_hash: String,
    /// Every decide rule that ran, and that it allowed.
    pub decisions: Vec<String>,
    /// Guarantees that failed with `on_fail: annotate`.
    pub annotations: Vec<String>,
    /// Shape operators applied to this caller.
    pub shapes: Vec<String>,
    /// Whether flags were read from storage (true) or evaluated live.
    pub flags_materialised: bool,
    #[serde(skip)]
    active_shapes: Vec<ShapeOp>,
}

/// Everything a caller learns about a query besides the rows (peQL design 4.10).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Envelope {
    pub contracts: Vec<Resolution>,
    pub rows: usize,
    /// The `suppress` threshold applied, if any: groups smaller than this were removed.
    pub suppress_k: Option<u64>,
    /// Privacy budget left per budget name after this query.
    pub budgets: BTreeMap<String, f64>,
}

pub struct QueryResult {
    pub batches: Vec<RecordBatch>,
    pub envelope: Envelope,
}

/// The reference executor. peQL is the production engine; this one exists so parcel's
/// artifacts can be exercised end to end, from the CLI and in tests.
pub struct Engine {
    base: PathBuf,
    registry: Registry,
    contracts: BTreeMap<String, Registered>,
    budgets: BudgetStore,
}

impl Engine {
    /// An engine whose relative bindings resolve under `base`.
    pub fn new(base: impl Into<PathBuf>) -> Engine {
        Engine {
            base: base.into(),
            registry: Registry::builtin(),
            contracts: BTreeMap::new(),
            budgets: BudgetStore::default(),
        }
    }

    pub fn contracts(&self) -> impl Iterator<Item = &Registered> {
        self.contracts.values()
    }

    pub fn get(&self, name: &str) -> Result<&Registered> {
        self.contracts
            .get(name)
            .ok_or_else(|| EngineError::UnknownContract(name.to_owned()))
    }

    /// Compile a contract against the schema of the data it binds and register it.
    pub fn register_contract(
        &mut self,
        source: &str,
        schema: &datafusion::arrow::datatypes::Schema,
    ) -> Result<&Registered> {
        let doc = ContractDoc::parse(source).map_err(|d| EngineError::Compile(vec![d]))?;
        let compilation = compile(&doc, schema, &self.registry).map_err(EngineError::Compile)?;
        parcel_runtime::verify_pins(&compilation.contract.functions)?;
        let name = doc.contract.clone();
        self.contracts.insert(
            name.clone(),
            Registered {
                doc,
                compilation: Arc::new(compilation),
            },
        );
        Ok(&self.contracts[&name])
    }

    pub fn root(&self, contract: &CompiledContract) -> PathBuf {
        binding::root(contract, &self.base)
    }

    pub fn manifest(&self, name: &str) -> Result<Option<Manifest>> {
        let r = self.get(name)?;
        Ok(Manifest::load(&self.root(&r.compilation.contract))?)
    }

    fn session() -> SessionContext {
        let config = SessionConfig::new()
            .with_information_schema(false)
            .set_bool("datafusion.execution.parquet.pushdown_filters", true)
            .set_bool("datafusion.execution.parquet.reorder_filters", true)
            .set_bool("datafusion.sql_parser.enable_ident_normalization", false);
        let ctx = SessionContext::new_with_config(config);
        ctx.register_udf(shape::sample_bucket_udf());
        ctx
    }

    /// Write batches under a contract: evaluate flags, lay out, stamp, validate, write the manifest.
    pub async fn write(
        &self,
        name: &str,
        batches: Vec<RecordBatch>,
        mode: WriteMode,
    ) -> Result<WriteReport> {
        let r = self.get(name)?;
        let cc = &r.compilation.contract;
        let root = self.root(cc);
        std::fs::create_dir_all(&root)?;
        let batches = conform(batches, &cc.row_schema)?;
        let rows_written: usize = batches.iter().map(|b| b.num_rows()).sum();

        let ctx = Self::session();
        let mem = MemTable::try_new(cc.row_schema.clone(), vec![batches])?;
        let mut select: Vec<Expr> = cc
            .row_schema
            .fields()
            .iter()
            .map(|f| col_ref(f.name()))
            .collect();
        for flag in &r.compilation.write.flags {
            select.push(flag.expr.clone().alias(&flag.column));
        }
        let plan = LogicalPlanBuilder::scan("incoming", provider_as_source(Arc::new(mem)), None)?
            .project(select)?
            .build()?;
        let plan = parcel_core::compile::resolve(plan)?;
        let df = ctx.execute_logical_plan(plan).await?;

        if mode == WriteMode::Overwrite {
            for f in binding::list_files(&root)? {
                std::fs::remove_file(f)?;
            }
        }
        let layout = &r.compilation.write.layout;
        let sort: Vec<SortExpr> = layout
            .cluster_by
            .iter()
            .map(|c| col_ref(c).sort(false, false))
            .collect();
        let mut options =
            DataFrameWriteOptions::new().with_partition_by(layout.partition_by.clone());
        if !sort.is_empty() {
            options = options.with_sort_by(sort);
        }
        let mut parquet = TableParquetOptions::default();
        parquet
            .key_value_metadata
            .insert(CONTRACT_HASH_KEY.into(), Some(cc.contract_hash.clone()));
        parquet
            .key_value_metadata
            .insert("parcel.contract".into(), Some(cc.name.clone()));
        parquet.global.statistics_enabled = Some("page".into());
        for c in &layout.bloom {
            let opts = parquet
                .column_specific_options
                .entry(c.clone())
                .or_default();
            opts.bloom_filter_enabled = Some(true);
        }
        let target = format!("{}/", root.canonicalize()?.display());
        df.write_parquet(&target, options, Some(parquet)).await?;

        let verdict = self.validate(name).await?;
        let files = binding::list_files(&root)?;
        let flag_columns: Vec<String> = cc.flags.iter().map(|f| f.column.clone()).collect();
        let entries = files
            .iter()
            .map(|f| binding::file_entry(&root, f, &flag_columns))
            .collect::<Result<Vec<_>>>()?;
        Manifest {
            contract: cc.name.clone(),
            contract_hash: cc.contract_hash.clone(),
            compilation_hash: cc.compilation_hash.clone(),
            written_at: Utc::now(),
            row_count: verdict.row_count,
            valid: verdict.valid,
            breached: verdict.breached.clone(),
            stats: verdict.stats.clone(),
            data_hash: verdict.data_hash.clone(),
            files: entries,
        }
        .save(&root)?;
        Ok(WriteReport {
            rows_written,
            files: files.len(),
            verdict,
        })
    }

    /// Run the contract's validation plan over its binding and return the verdict (design 9.2).
    pub async fn validate(&self, name: &str) -> Result<Verdict> {
        let r = self.get(name)?;
        let cc = &r.compilation.contract;
        let root = self.root(cc);
        let ctx = Self::session();
        let provider = binding::provider(cc, &root)?;
        let plan = bind_binding(r.compilation.validation.plan.clone(), cc, provider)?;
        let batches = ctx.execute_logical_plan(plan).await?.collect().await?;
        let batch = batches
            .iter()
            .find(|b| b.num_rows() > 0)
            .ok_or_else(|| EngineError::Invalid("validation returned no row".into()))?;
        let get = |name: &str| -> Result<ScalarValue> {
            let idx = batch
                .schema()
                .index_of(name)
                .map_err(|e| EngineError::Invalid(e.to_string()))?;
            Ok(ScalarValue::try_from_array(batch.column(idx), 0)?)
        };
        let valid = matches!(get("valid")?, ScalarValue::Boolean(Some(true)));
        let breached = match get("breached")? {
            ScalarValue::Utf8(Some(s))
            | ScalarValue::Utf8View(Some(s))
            | ScalarValue::LargeUtf8(Some(s)) => s
                .split(',')
                .filter(|x| !x.is_empty())
                .map(str::to_owned)
                .collect(),
            _ => Vec::new(),
        };
        let row_count = scalar_i64(&get("row_count")?).unwrap_or(0);
        let mut failures = BTreeMap::new();
        for a in &r.compilation.validation.asserts {
            failures.insert(
                a.clone(),
                scalar_i64(&get(&format!("fail__{a}"))?).unwrap_or(0),
            );
        }
        let mut guarantees = BTreeMap::new();
        for g in &r.compilation.validation.data_guarantees {
            guarantees.insert(
                g.clone(),
                matches!(
                    get(&format!("guarantee__{g}"))?,
                    ScalarValue::Boolean(Some(true))
                ),
            );
        }
        let mut stats = BTreeMap::new();
        for s in &r.compilation.validation.stats {
            stats.insert(s.column.clone(), scalar_json(&get(&s.column)?));
        }
        let data_hash = binding::data_hash(&root, &binding::list_files(&root)?)?;
        Ok(Verdict {
            contract: cc.name.clone(),
            contract_hash: cc.contract_hash.clone(),
            compilation_hash: cc.compilation_hash.clone(),
            valid,
            breached,
            row_count,
            failures,
            guarantees,
            stats,
            data_hash,
        })
    }

    /// Decide, check guarantees and choose shapes for one caller (peQL design 4.3).
    pub fn resolve(&self, name: &str, caller: &Caller) -> Result<(Resolution, Manifest)> {
        let r = self.get(name)?;
        let cc = &r.compilation.contract;
        parcel_runtime::verify_pins(&cc.functions)?;
        let ctx_scope = Scope {
            ctx: Some(reference::ctx_value(caller)),
            ..Default::default()
        };
        let ctx = reference::context(&ctx_scope);
        let mut decisions = Vec::new();
        for d in &cc.decisions {
            if !reference::eval_bool(&d.cel, &ctx)? {
                return Err(EngineError::Denied {
                    contract: cc.name.clone(),
                    rule: d.id.clone(),
                });
            }
            decisions.push(d.id.clone());
        }
        let manifest = Manifest::load(&self.root(cc))?.ok_or_else(|| EngineError::NotWritten {
            contract: cc.name.clone(),
        })?;
        if !manifest.valid {
            return Err(EngineError::NotServable {
                contract: cc.name.clone(),
                breached: manifest.breached.clone(),
            });
        }
        let dataset = dataset_value(&r.compilation, &manifest);
        let gctx = reference::context(&Scope {
            ctx: ctx_scope.ctx.clone(),
            dataset: Some(dataset),
            row: None,
        });
        let mut annotations = Vec::new();
        for g in &cc.guarantees {
            if !reference::eval_bool(&g.cel, &gctx)? {
                match g.on_fail {
                    GuaranteeOnFail::Deny => {
                        return Err(EngineError::GuaranteeFailed {
                            contract: cc.name.clone(),
                            rule: g.id.clone(),
                        });
                    }
                    GuaranteeOnFail::Annotate => annotations.push(g.id.clone()),
                }
            }
        }
        let mut shapes = Vec::new();
        let mut active_shapes = Vec::new();
        for s in &cc.shapes {
            let skip = match &s.unless {
                Some(u) => reference::eval_bool(u, &ctx)?,
                None => false,
            };
            if !skip {
                shapes.push(s.id.clone());
                active_shapes.push(s.shape.clone());
            }
        }
        Ok((
            Resolution {
                contract: cc.name.clone(),
                version: cc.version,
                contract_hash: cc.contract_hash.clone(),
                compilation_hash: cc.compilation_hash.clone(),
                decisions,
                annotations,
                shapes,
                flags_materialised: manifest.flags_current(&cc.contract_hash),
                active_shapes,
            },
            manifest,
        ))
    }

    /// The view that stands in for a contract, for one caller (peQL design 4.5).
    pub fn view(
        &self,
        name: &str,
        caller: &Caller,
        resolution: &Resolution,
    ) -> Result<LogicalPlan> {
        let r = self.get(name)?;
        let cc = &r.compilation.contract;
        let provider = binding::provider(cc, &self.root(cc))?;
        let mut filters: Vec<Expr> = cc.admits.iter().map(|(_, e)| e.clone()).collect();
        for Flag {
            column,
            expr,
            on_fail,
            ..
        } in &cc.flags
        {
            if *on_fail == AssertOnFail::Drop {
                filters.push(if resolution.flags_materialised {
                    col_ref(column)
                } else {
                    expr.clone()
                });
            }
        }
        for s in &resolution.active_shapes {
            if let ShapeOp::Sample { fraction, key } = s {
                filters.push(shape::sample_predicate(key, *fraction));
            }
        }
        let mut b = LogicalPlanBuilder::scan(
            format!("__parcel_{}", cc.name.replace('/', "_")),
            provider_as_source(provider),
            None,
        )?;
        if let Some(f) = filters.into_iter().reduce(Expr::and) {
            b = b.filter(f)?;
        }
        let projection: Vec<Expr> = cc
            .projection
            .iter()
            .map(|(n, e)| e.clone().alias(n))
            .collect();
        let plan = parcel_core::compile::resolve(b.project(projection)?.build()?)?;
        Ok(plan.with_param_values(param_values(cc, caller)?)?)
    }

    /// What a caller would see: the exposed schema, if the contract admits them at all.
    pub fn describe(&self, name: &str, caller: &Caller) -> Result<SchemaRef> {
        let r = self.get(name)?;
        let ctx = reference::context(&Scope {
            ctx: Some(reference::ctx_value(caller)),
            ..Default::default()
        });
        for d in &r.compilation.contract.decisions {
            if !reference::eval_bool(&d.cel, &ctx)? {
                return Err(EngineError::Denied {
                    contract: name.to_owned(),
                    rule: d.id.clone(),
                });
            }
        }
        Ok(r.compilation.contract.exposed_schema.clone())
    }

    /// Run SQL in which every table is a contract.
    pub async fn query(&self, sql: &str, caller: &Caller) -> Result<QueryResult> {
        let ctx = Self::session();
        let state = ctx.state();
        let statement = state.sql_to_statement(sql, &datafusion::config::Dialect::Generic)?;
        let refs = state.resolve_table_references(&statement)?;
        let mut resolutions = Vec::new();
        for t in refs {
            let name = t.table().to_owned();
            if resolutions.iter().any(|r: &Resolution| r.contract == name) {
                continue;
            }
            let (resolution, _) = self.resolve(&name, caller)?;
            let view = self.view(&name, caller, &resolution)?;
            ctx.register_table(t.clone(), Arc::new(ViewTable::new(view, None)))?;
            resolutions.push(resolution);
        }

        let mut plan = ctx.state().create_logical_plan(sql).await?;
        let shapes: Vec<&ShapeOp> = resolutions
            .iter()
            .flat_map(|r| r.active_shapes.iter())
            .collect();
        let k = shapes
            .iter()
            .filter_map(|s| {
                if let ShapeOp::Suppress { k } = s {
                    Some(*k)
                } else {
                    None
                }
            })
            .max();
        let mut budgets = BTreeMap::new();
        let noise: Vec<&ShapeOp> = shapes
            .iter()
            .copied()
            .filter(|s| matches!(s, ShapeOp::Noise { .. }))
            .collect();
        if !noise.is_empty() {
            plan = shape::apply_noise(plan, &noise, caller, &self.budgets, &mut budgets)?;
        }
        let has_aggregate = shape::has_aggregate(&plan);
        if let (Some(k), true) = (k, has_aggregate) {
            plan = shape::suppress_groups(plan, k)?;
        }
        let df = ctx.execute_logical_plan(plan).await?;
        let mut batches = df.collect().await?;
        if let (Some(k), false) = (k, has_aggregate)
            && batches.iter().map(|b| b.num_rows()).sum::<usize>() < k as usize
        {
            // No GROUP BY: the whole result is one group (peQL design 4.7).
            batches = batches.into_iter().map(|b| b.slice(0, 0)).collect();
        }
        let rows = batches.iter().map(|b| b.num_rows()).sum();
        Ok(QueryResult {
            batches,
            envelope: Envelope {
                contracts: resolutions,
                rows,
                suppress_k: k,
                budgets,
            },
        })
    }
}

/// Swap the validation plan's placeholder table for the real binding, projected to the row schema.
pub fn bind_binding(
    plan: LogicalPlan,
    cc: &CompiledContract,
    provider: Arc<dyn datafusion::datasource::TableProvider>,
) -> Result<LogicalPlan> {
    let columns: Vec<Expr> = cc
        .row_schema
        .fields()
        .iter()
        .map(|f| {
            datafusion::logical_expr::cast(col_ref(f.name()), f.data_type().clone()).alias(f.name())
        })
        .collect();
    let real = LogicalPlanBuilder::scan("__parcel_files", provider_as_source(provider), None)?
        .project(columns)?
        .alias(BINDING_TABLE)?
        .build()?;
    let out = plan.transform_up(|node| {
        if let LogicalPlan::TableScan(ts) = &node
            && ts.table_name.table() == BINDING_TABLE
        {
            return Ok(Transformed::new(
                real.clone(),
                true,
                TreeNodeRecursion::Jump,
            ));
        }
        Ok(Transformed::no(node))
    })?;
    Ok(out.data.recompute_schema()?)
}

/// Values for every `ctx` placeholder, from the reference interpreter.
pub fn param_values(cc: &CompiledContract, caller: &Caller) -> Result<ParamValues> {
    let ctx = reference::context(&Scope {
        ctx: Some(reference::ctx_value(caller)),
        ..Default::default()
    });
    let mut map = HashMap::new();
    for p in &cc.params {
        let v = reference::eval(&p.cel, &ctx)?;
        let s = reference::to_scalar(&v, &p.ty)?;
        map.insert(p.id.clone(), s.into());
    }
    Ok(ParamValues::Map(map))
}

/// The `dataset` namespace from a manifest.
fn dataset_value(c: &Compilation, m: &Manifest) -> Value {
    let mut entries: Vec<(String, Value)> = vec![
        (
            "dataset.written_at".into(),
            reference::timestamp(m.written_at),
        ),
        (
            "dataset.contract_hash".into(),
            reference::string(&m.contract_hash),
        ),
    ];
    for s in &c.validation.stats {
        if let Some(v) = m.stats.get(&s.column).and_then(|j| json_to_cel(j, &s.ty)) {
            entries.push((s.path.clone(), v));
        }
    }
    reference::dataset_value(&entries)
}

fn json_to_cel(j: &serde_json::Value, ty: &parcel_core::types::Type) -> Option<Value> {
    use parcel_core::types::Type;
    Some(match ty {
        Type::Int => Value::Int(j.as_i64()?),
        Type::Uint => Value::UInt(j.as_u64()?),
        Type::Double => Value::Float(j.as_f64()?),
        Type::String => reference::string(j.as_str()?),
        Type::Bool => Value::Bool(j.as_bool()?),
        Type::Timestamp => reference::timestamp(
            chrono::DateTime::parse_from_rfc3339(j.as_str()?)
                .ok()?
                .with_timezone(&Utc),
        ),
        _ => return None,
    })
}

fn scalar_i64(s: &ScalarValue) -> Option<i64> {
    match s {
        ScalarValue::Int64(v) => *v,
        ScalarValue::UInt64(v) => v.map(|v| v as i64),
        _ => None,
    }
}

fn scalar_json(s: &ScalarValue) -> serde_json::Value {
    use serde_json::json;
    match s {
        ScalarValue::Int64(Some(v)) => json!(v),
        ScalarValue::UInt64(Some(v)) => json!(v),
        ScalarValue::Float64(Some(v)) => json!(v),
        ScalarValue::Boolean(Some(v)) => json!(v),
        ScalarValue::Utf8(Some(v))
        | ScalarValue::LargeUtf8(Some(v))
        | ScalarValue::Utf8View(Some(v)) => json!(v),
        ScalarValue::TimestampMicrosecond(Some(v), _) => {
            json!(chrono::DateTime::<Utc>::from_timestamp_micros(*v).map(|t| t.to_rfc3339()))
        }
        _ => serde_json::Value::Null,
    }
}

/// An unqualified column reference that keeps its name verbatim.
pub fn col_ref(name: &str) -> Expr {
    Expr::Column(datafusion::common::Column::new_unqualified(name))
}

/// Cast incoming batches to the row schema the contract was compiled against.
fn conform(batches: Vec<RecordBatch>, schema: &SchemaRef) -> Result<Vec<RecordBatch>> {
    batches
        .into_iter()
        .map(|b| conform_one(b, schema))
        .collect()
}

pub(crate) fn conform_one(b: RecordBatch, schema: &SchemaRef) -> Result<RecordBatch> {
    use datafusion::arrow::compute::cast;
    {
        {
            let mut cols = Vec::with_capacity(schema.fields().len());
            for f in schema.fields() {
                let idx = b.schema().index_of(f.name()).map_err(|_| {
                    EngineError::Invalid(format!(
                        "incoming data has no column `{}` required by the contract",
                        f.name()
                    ))
                })?;
                let c = b.column(idx);
                cols.push(if c.data_type() == f.data_type() {
                    c.clone()
                } else {
                    cast(c, f.data_type()).map_err(|e| EngineError::Invalid(e.to_string()))?
                });
            }
            RecordBatch::try_new(schema.clone(), cols)
                .map_err(|e| EngineError::Invalid(e.to_string()))
        }
    }
}
