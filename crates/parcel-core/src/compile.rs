//! The Assemble pass: one translation, three artifacts (design section 9).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use datafusion_common::arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use datafusion_expr::logical_plan::builder::LogicalTableSource;
use datafusion_expr::{
    Expr, LogicalPlan, LogicalPlanBuilder, Operator, binary_expr, cast, lit, when,
};
use datafusion_functions_aggregate::expr_fn as agg;
use serde::Serialize;
use serde_json::json;

use crate::cel_print::{Style, print};
use crate::check::{CheckedContract, CheckedRule, NoiseAt, ShapeOp};
use crate::diag::{Code, Diagnostic};
use crate::document::{AssertOnFail, Binding, ContractDoc, GuaranteeOnFail};
use crate::hash;
use crate::ir::{ColumnStat, CtxField, DatasetField, ExprKind, TExpr, Var};
use crate::registry::{FunctionPin, Registry};
use crate::translate::{CtxParam, Params, Translator, column, stat_column};
use crate::types::Type;

/// The table name the validation plan scans. Executors swap in the real binding.
pub const BINDING_TABLE: &str = "__parcel_binding";

/// The version of parcel that produced an artifact; part of the compilation hash.
pub const PARCEL_VERSION: &str = env!("CARGO_PKG_VERSION");

/// A rule the reference interpreter evaluates once per query.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CelRule {
    pub id: String,
    /// Reference-style CEL (see [`crate::cel_print`]).
    pub cel: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GuaranteeRule {
    pub id: String,
    pub cel: String,
    pub on_fail: GuaranteeOnFail,
    /// Every `dataset` field it reads, as the statistics column that carries it.
    pub reads: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ShapeRule {
    pub id: String,
    pub shape: ShapeOp,
    pub unless: Option<String>,
}

/// One `assert`, as the flag column the write path stores and the view reads.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Flag {
    pub assert_id: String,
    /// `_c_<id>`
    pub column: String,
    #[serde(serialize_with = "ser_expr")]
    pub expr: Expr,
    pub on_fail: AssertOnFail,
}

/// A rule evaluated per row, in reference CEL: what the differential test compares against.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RowRule {
    pub id: String,
    pub kind: RowRuleKind,
    pub cel: String,
    /// Row columns read by value; a null in any of them decides the result (design 6, "Nulls").
    pub reads: Vec<String>,
    pub ty: Type,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RowRuleKind {
    Admit,
    Assert,
    Transform { column: String },
}

/// A row-only subtree of an admit or transform, computed once at write and stored (design 8, "Split").
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Derived {
    /// `_d_<hash>` of the subtree's canonical CEL.
    pub column: String,
    pub cel: String,
    #[serde(serialize_with = "ser_expr")]
    pub expr: Expr,
    pub ty: Type,
}

/// One `row.other` field, computed at write from raw columns (design 10, stage 1).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct EnrichSpec {
    pub field: String,
    /// Reference CEL over raw columns (earlier enrichers inlined).
    pub cel: String,
    pub reads: Vec<String>,
    #[serde(serialize_with = "ser_expr")]
    pub expr: Expr,
    pub ty: Type,
}

/// How a rule will execute, shown to the author (design 9.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    /// Evaluated once per query; reads no data.
    PlanTime,
    /// A predicate the scan can use with statistics to skip files and row groups.
    Prunes,
    /// A predicate evaluated at the scan, row by row.
    ScanFilter,
    /// Materialised at write time; queries read the stored result.
    WriteTime,
    /// A projection; free when the column is not selected.
    Projection,
    /// A physical operator the optimiser cannot see through.
    Operator,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ReportEntry {
    pub rule: String,
    pub op: &'static str,
    pub tier: Tier,
    pub reason: String,
    /// The rule in canonical CEL, when it has an expression.
    pub cel: Option<String>,
}

/// The query-time artifact: fragments peQL splices into a caller's query (design 9.1).
#[derive(Clone, Debug, Serialize)]
pub struct CompiledContract {
    pub name: String,
    pub version: u32,
    pub contract_hash: String,
    pub compilation_hash: String,
    /// The tenant the contract belongs to. Engines use it for visibility and to decide
    /// whose functions the contract may call.
    pub owner: Option<String>,
    pub binding: Binding,
    #[serde(serialize_with = "ser_schema")]
    pub row_schema: SchemaRef,
    #[serde(serialize_with = "ser_schema")]
    pub exposed_schema: SchemaRef,
    /// `ctx`-only subtrees, bound per query to placeholders `$c0`, `$c1`, ...
    pub params: Vec<CtxParam>,
    pub decisions: Vec<CelRule>,
    #[serde(serialize_with = "ser_named_exprs")]
    pub admits: Vec<(String, Expr)>,
    /// Every assert. Those with `on_fail: drop` also filter the view.
    pub flags: Vec<Flag>,
    /// One expression per exposed column, in order: its raw column or its transform.
    #[serde(serialize_with = "ser_named_exprs")]
    pub projection: Vec<(String, Expr)>,
    pub guarantees: Vec<GuaranteeRule>,
    pub shapes: Vec<ShapeRule>,
    pub functions: BTreeSet<FunctionPin>,
    pub report: Vec<ReportEntry>,
    /// Every admit, assert and transform in reference CEL, for the differential test.
    pub row_rules: Vec<RowRule>,
    /// Row-only subtrees stored at write time.
    pub derived: Vec<Derived>,
    /// The binding's scan schema: the data's columns plus `_other` when enrichers produce it.
    #[serde(serialize_with = "ser_schema")]
    pub scan_schema: SchemaRef,
    /// `row.other` producers, run at write before anything else.
    pub enrich: Vec<EnrichSpec>,
    /// Declared `ctx.other` fields: how the embedding application's values are typed.
    pub ctx_other: BTreeMap<String, Type>,
    /// The admits and projection with derived subtrees read from storage. Equal to the live
    /// versions when nothing is derived; valid only over files written under this contract.
    #[serde(serialize_with = "ser_named_exprs")]
    pub admits_stored: Vec<(String, Expr)>,
    #[serde(serialize_with = "ser_named_exprs")]
    pub projection_stored: Vec<(String, Expr)>,
}

/// A statistic the validation plan computes: one `dataset` field.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct StatSpec {
    pub field: DatasetField,
    /// Output column name in the verdict row.
    pub column: String,
    /// CEL path, e.g. `dataset.customer_id.null_rate`.
    pub path: String,
    pub ty: Type,
}

/// A query that proves the contract holds (design 9.2). Returns exactly one row.
#[derive(Clone, Debug, Serialize)]
pub struct ValidationPlan {
    #[serde(serialize_with = "ser_plan")]
    pub plan: LogicalPlan,
    /// The data statistics the verdict carries, which the manifest stores.
    pub stats: Vec<StatSpec>,
    /// Asserts whose failure count is `fail__<id>` in the verdict.
    pub asserts: Vec<String>,
    /// Guarantees decided by the data alone; each is a bool column `guarantee__<id>`.
    pub data_guarantees: Vec<String>,
    /// Guarantees that also need the manifest's metadata or the query time; decided by peQL per query.
    pub query_time_guarantees: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Layout {
    /// Flag columns the view filters on, so fragments are homogeneous (design 10, stage 5).
    pub cluster_by: Vec<String>,
    pub partition_by: Vec<String>,
    /// Columns worth a bloom filter: those compared for equality with caller context.
    pub bloom: Vec<String>,
}

/// What must exist on disk for every rule to be cheap at query time (design 10).
#[derive(Clone, Debug, Serialize)]
pub struct WritePlan {
    pub enrich: Vec<EnrichSpec>,
    pub flags: Vec<Flag>,
    pub derived: Vec<Derived>,
    pub layout: Layout,
    /// Keys the manifest carries besides the statistics.
    pub manifest_fields: Vec<&'static str>,
}

/// The three artifacts, produced together and sharing a hash.
#[derive(Clone, Debug, Serialize)]
pub struct Compilation {
    pub contract: CompiledContract,
    pub validation: ValidationPlan,
    pub write: WritePlan,
}

/// Compile a contract document against the Arrow schema of its binding.
pub fn compile(
    doc: &ContractDoc,
    schema: &Schema,
    registry: &Registry,
) -> Result<Compilation, Vec<Diagnostic>> {
    let resolved = crate::inherit::Resolved {
        doc: doc.clone(),
        layers: Vec::new(),
    };
    compile_resolved(&resolved, schema, registry)
}

/// Compile a contract that may inherit, looking parents up by contract name.
pub fn compile_with(
    doc: &ContractDoc,
    schema: &Schema,
    registry: &Registry,
    lookup: &dyn Fn(&str) -> Option<ContractDoc>,
) -> Result<Compilation, Vec<Diagnostic>> {
    compile_resolved(&crate::inherit::resolve(doc, lookup)?, schema, registry)
}

/// Compile a flattened inheritance chain.
pub fn compile_resolved(
    resolved: &crate::inherit::Resolved,
    schema: &Schema,
    registry: &Registry,
) -> Result<Compilation, Vec<Diagnostic>> {
    let checked = crate::check::check_layers(&resolved.doc, schema, registry, &resolved.layers)?;
    assemble(&checked, schema)
        .map_err(|(rule, msg)| vec![Diagnostic::new(Code::Untranslatable, rule.as_deref(), msg)])
}

type AResult<T> = Result<T, (Option<String>, String)>;

fn assemble(c: &CheckedContract, schema: &Schema) -> AResult<Compilation> {
    let mut params = Params::default();
    let mut decisions = Vec::new();
    let mut admits = Vec::new();
    let mut flags = Vec::new();
    let mut transforms = Vec::new();
    let mut guarantees = Vec::new();
    let mut shapes = Vec::new();
    // `noise at: row`: (column, scale, unless), applied to the projection below.
    let mut row_noise: Vec<(String, f64, Option<Expr>)> = Vec::new();
    let mut report = Vec::new();
    let mut row_rules = Vec::new();

    // The split pass: row-only subtrees of admits and transforms that are worth storing.
    let mut derived: Vec<Derived> = Vec::new();
    let mut derived_map: BTreeMap<String, String> = BTreeMap::new();
    for rule in &c.rules {
        if !matches!(
            rule,
            CheckedRule::Admit { .. } | CheckedRule::Transform { .. }
        ) {
            continue;
        }
        let mut found = Vec::new();
        for x in rule.exprs() {
            split_candidates(&x.expr, &mut found);
        }
        for sub in found {
            let cel = print(sub, Style::Canonical);
            if derived_map.contains_key(&cel) {
                continue;
            }
            let column = format!("_d_{}", &hash::sha256_hex(cel.as_bytes())[..12]);
            let mut scratch = Params::default();
            let mut t = Translator {
                schema,
                params: &mut scratch,
                dataset_columns: false,
                derived: None,
            };
            let expr = t.expr(sub).map_err(|e| (Some(rule.id().to_owned()), e))?;
            derived_map.insert(cel.clone(), column.clone());
            derived.push(Derived {
                column,
                cel,
                expr,
                ty: sub.ty.clone(),
            });
        }
    }
    let mut admits_stored = Vec::new();
    let mut transforms_stored = Vec::new();

    for rule in &c.rules {
        let row_kind = match rule {
            CheckedRule::Admit { .. } => Some(RowRuleKind::Admit),
            CheckedRule::Assert { .. } => Some(RowRuleKind::Assert),
            CheckedRule::Transform { column, .. } => Some(RowRuleKind::Transform {
                column: column.clone(),
            }),
            _ => None,
        };
        if let (Some(kind), Some(x)) = (row_kind, rule.exprs().first()) {
            let reads = x.expr.value_reads();
            row_rules.push(RowRule {
                id: rule.id().to_owned(),
                kind,
                cel: print(&x.expr, Style::Reference),
                reads: reads.into_iter().collect(),
                ty: x.expr.ty.clone(),
            });
        }
        let id = rule.id().to_owned();
        let at = |e: String| (Some(id.clone()), e);
        let canonical = rule
            .exprs()
            .first()
            .map(|x| print(&x.expr, Style::Canonical));
        let mut t = Translator {
            schema,
            params: &mut params,
            dataset_columns: false,
            derived: None,
        };
        let (op, tier, reason) = match rule {
            CheckedRule::Decide { expr, .. } => {
                decisions.push(CelRule {
                    id: id.clone(),
                    cel: print(&expr.expr, Style::Reference),
                });
                (
                    "decide",
                    Tier::PlanTime,
                    "evaluated once per query from ctx; no data is read".to_owned(),
                )
            }
            CheckedRule::Admit { expr, .. } => {
                admits.push((id.clone(), t.predicate(&expr.expr).map_err(at)?));
                let mut ts = Translator {
                    schema,
                    params: t.params,
                    dataset_columns: false,
                    derived: Some(&derived_map),
                };
                admits_stored.push((id.clone(), ts.predicate(&expr.expr).map_err(at)?));
                let cols = expr
                    .expr
                    .row_columns()
                    .into_iter()
                    .collect::<Vec<_>>()
                    .join(", ");
                if prunable(&expr.expr) {
                    (
                        "admit",
                        Tier::Prunes,
                        format!(
                            "compares {cols} with constants or caller context; files and row groups are skipped by statistics"
                        ),
                    )
                } else {
                    (
                        "admit",
                        Tier::ScanFilter,
                        format!(
                            "evaluated at the scan over {cols}{}",
                            cost_note(&expr.expr, registry_names(&expr.expr))
                        ),
                    )
                }
            }
            CheckedRule::Assert { expr, on_fail, .. } => {
                let pred = t.predicate(&expr.expr).map_err(at)?;
                flags.push(Flag {
                    assert_id: id.clone(),
                    column: format!("_c_{id}"),
                    expr: pred,
                    on_fail: *on_fail,
                });
                let what = match on_fail {
                    AssertOnFail::Drop => {
                        "stored as a flag column at write; queries filter on the flag"
                    }
                    AssertOnFail::Deny => {
                        "evaluated at write; any failing row makes the dataset unservable"
                    }
                    AssertOnFail::Report => "evaluated at write; only its pass rate is recorded",
                };
                (
                    "assert",
                    Tier::WriteTime,
                    format!(
                        "{what}{}",
                        cost_note(&expr.expr, registry_names(&expr.expr))
                    ),
                )
            }
            CheckedRule::Transform {
                column: col, expr, ..
            } => {
                transforms.push((col.clone(), t.transform(&expr.expr).map_err(at)?));
                let mut ts = Translator {
                    schema,
                    params: t.params,
                    dataset_columns: false,
                    derived: Some(&derived_map),
                };
                transforms_stored.push((col.clone(), ts.transform(&expr.expr).map_err(at)?));
                (
                    "transform",
                    Tier::Projection,
                    format!("replaces `{col}`; pruned when `{col}` is not selected"),
                )
            }
            CheckedRule::Guarantee { expr, on_fail, .. } => {
                let reads = dataset_fields(&expr.expr).iter().map(stat_column).collect();
                guarantees.push(GuaranteeRule {
                    id: id.clone(),
                    cel: print(&expr.expr, Style::Reference),
                    on_fail: *on_fail,
                    reads,
                });
                (
                    "guarantee",
                    Tier::PlanTime,
                    "one manifest read per query; no data is read".to_owned(),
                )
            }
            CheckedRule::Shape { shape, unless, .. } => {
                shapes.push(ShapeRule {
                    id: id.clone(),
                    shape: shape.clone(),
                    unless: unless.as_ref().map(|u| print(&u.expr, Style::Reference)),
                });
                // `unless` reads only ctx, so it becomes a parameter the optimiser folds.
                let unless_expr = match unless {
                    Some(u) => Some(t.predicate(&u.expr).map_err(at)?),
                    None => None,
                };
                match shape {
                    ShapeOp::Sample { fraction, key } => {
                        let keep = crate::udfs::sample_predicate(column(key), *fraction);
                        let keep = match unless_expr {
                            Some(u) => u.or(keep),
                            None => keep,
                        };
                        admits.push((id.clone(), keep.clone()));
                        admits_stored.push((id.clone(), keep));
                        (
                            "shape",
                            Tier::ScanFilter,
                            format!(
                                "keeps a stable {}% of rows keyed on `{key}`; a filter in the view",
                                fraction * 100.0
                            ),
                        )
                    }
                    ShapeOp::Noise {
                        column: col,
                        sensitivity,
                        epsilon,
                        at: NoiseAt::Row,
                        ..
                    } => {
                        row_noise.push((col.clone(), sensitivity / epsilon, unless_expr));
                        (
                            "shape",
                            Tier::Projection,
                            format!(
                                "adds Laplace noise to each `{col}` value in the view; the budget is charged once per query that reads `{col}`"
                            ),
                        )
                    }
                    ShapeOp::Noise { column: col, .. } => (
                        "shape",
                        Tier::Operator,
                        format!(
                            "adds Laplace noise to aggregates over `{col}`; `{col}` cannot be read outside an aggregate"
                        ),
                    ),
                    ShapeOp::Suppress { k } => (
                        "shape",
                        Tier::Operator,
                        format!("removes groups of fewer than {k} rows from every aggregate"),
                    ),
                }
            }
        };
        // Name the stored subtrees this rule reads instead of evaluating.
        let mut reason = reason;
        if matches!(
            rule,
            CheckedRule::Admit { .. } | CheckedRule::Transform { .. }
        ) {
            let mut found = Vec::new();
            for x in rule.exprs() {
                split_candidates(&x.expr, &mut found);
            }
            for sub in found {
                let cel = print(sub, Style::Canonical);
                reason.push_str(&format!(
                    "; `{cel}` is computed at write and read from `{}`",
                    derived_map[&cel]
                ));
            }
        }
        report.push(ReportEntry {
            rule: id,
            op,
            tier,
            reason,
            cel: canonical,
        });
    }

    // The view's projection: every exposed column, raw (cast to its declared type) or transformed.
    let mut projection = Vec::new();
    let mut projection_stored = Vec::new();
    for e in &c.exposed {
        let raw = || -> AResult<Expr> {
            let field = schema
                .field_with_name(&e.name)
                .map_err(|x| (None, x.to_string()))?;
            Ok(if field.data_type() == &e.data_type {
                column(&e.name)
            } else {
                cast(column(&e.name), e.data_type.clone())
            })
        };
        // A transform yields the parcel type; the view exposes the declared Arrow type.
        let live = match transforms.iter().find(|(col, _)| *col == e.name) {
            Some((_, t)) => cast(t.clone(), e.data_type.clone()),
            None => raw()?,
        };
        let stored = match transforms_stored.iter().find(|(col, _)| *col == e.name) {
            Some((_, t)) => cast(t.clone(), e.data_type.clone()),
            None => live.clone(),
        };
        let (live, stored) = match row_noise.iter().find(|(c, ..)| *c == e.name) {
            Some((_, scale, unless)) => {
                let wrap = |p: Expr| {
                    let noisy = crate::udfs::noised(p.clone(), *scale, &e.data_type);
                    match unless {
                        Some(u) => datafusion_expr::when(u.clone(), p)
                            .otherwise(noisy)
                            .expect("a single-branch CASE is well formed"),
                        None => noisy,
                    }
                };
                (wrap(live), wrap(stored))
            }
            None => (live, stored),
        };
        projection.push((e.name.clone(), live));
        projection_stored.push((e.name.clone(), stored));
    }
    let exposed_schema = Arc::new(Schema::new(
        c.exposed
            .iter()
            .map(|e| Field::new(&e.name, e.data_type.clone(), true))
            .collect::<Vec<_>>(),
    ));

    let scan_schema = scan_schema(c, schema);
    let validation = validation_plan(c, &scan_schema, &flags)?;
    let mut enrich = Vec::new();
    for (field, x) in &c.enrichers {
        let mut scratch = Params::default();
        let mut t = Translator {
            schema,
            params: &mut scratch,
            dataset_columns: false,
            derived: None,
        };
        let expr = t
            .transform(&x.expr)
            .map_err(|e| (Some(format!("enrich.{field}")), e))?;
        enrich.push(EnrichSpec {
            field: field.clone(),
            cel: print(&x.expr, Style::Reference),
            reads: x.expr.value_reads().into_iter().collect(),
            expr,
            ty: x.expr.ty.clone(),
        });
    }
    let write = WritePlan {
        layout: Layout {
            cluster_by: flags
                .iter()
                .filter(|f| f.on_fail == AssertOnFail::Drop)
                .map(|f| f.column.clone())
                .collect(),
            partition_by: c.binding.partitioned_by.clone(),
            bloom: bloom_columns(c),
        },
        enrich: enrich.clone(),
        flags: flags.clone(),
        derived: derived.clone(),
        manifest_fields: vec![
            "contract_hash",
            "compilation_hash",
            "written_at",
            "row_count",
            "stats",
            "files",
        ],
    };

    let compilation_hash = compilation_hash(c, schema);
    Ok(Compilation {
        contract: CompiledContract {
            name: c.name.clone(),
            version: c.version,
            contract_hash: c.contract_hash.clone(),
            compilation_hash,
            owner: c.owner.clone(),
            binding: c.binding.clone(),
            row_schema: Arc::new(schema.clone()),
            exposed_schema,
            params: params.list,
            decisions,
            admits,
            flags,
            projection,
            guarantees,
            shapes,
            functions: c.functions.clone(),
            report,
            row_rules,
            derived,
            scan_schema: Arc::new(scan_schema),
            enrich,
            ctx_other: c.other.ctx.clone(),
            admits_stored,
            projection_stored,
        },
        validation,
        write,
    })
}

/// The hash every artifact carries: contract, bound schema, parcel version and pinned functions.
fn compilation_hash(c: &CheckedContract, schema: &Schema) -> String {
    let fields: Vec<_> = schema
        .fields()
        .iter()
        .map(|f| json!({"name": f.name(), "type": f.data_type().to_string(), "nullable": f.is_nullable()}))
        .collect();
    let v = json!({
        "contract_hash": c.contract_hash,
        "schema": fields,
        "parcel": PARCEL_VERSION,
        "functions": c.functions,
    });
    hash::sha256_hex(hash::canonical_json(&v).as_bytes())
}

/// One aggregation over the binding, then a projection applying each rule's threshold.
fn validation_plan(
    c: &CheckedContract,
    schema: &Schema,
    flags: &[Flag],
) -> AResult<ValidationPlan> {
    let fail = |e: datafusion_common::DataFusionError| (None, e.to_string());

    // Which dataset fields do guarantees read? Row count and every pass rate are always carried.
    let mut fields: Vec<DatasetField> = vec![DatasetField::RowCount];
    for f in flags {
        fields.push(DatasetField::AssertionPassRate {
            assertion: f.assert_id.clone(),
        });
    }
    let mut data_guarantees = Vec::new();
    let mut query_time_guarantees = Vec::new();
    for r in &c.rules {
        if let CheckedRule::Guarantee { id, expr, .. } = r {
            let reads = dataset_fields(&expr.expr);
            let metadata = reads
                .iter()
                .any(|d| matches!(d, DatasetField::WrittenAt | DatasetField::ContractHash));
            if metadata || expr.expr.ctx_fields().contains(&CtxField::Now) {
                query_time_guarantees.push(id.clone());
            } else {
                data_guarantees.push(id.clone());
            }
            fields.extend(
                reads
                    .into_iter()
                    .filter(|d| !matches!(d, DatasetField::WrittenAt | DatasetField::ContractHash)),
            );
        }
    }
    // Every dataset.other producer is carried, read or not: the manifest records them.
    for p in &c.producers {
        fields.push(DatasetField::Other(p.field.clone()));
    }
    let mut seen = BTreeSet::new();
    fields.retain(|f| seen.insert(f.clone()));

    // Stage 1: the aggregate. Everything is integer counts, sums and extrema, so it is exact and mergeable.
    let mut aggs: Vec<Expr> = vec![agg::count(lit(1i64)).alias("row_count")];
    for f in flags {
        let failed = cast(datafusion_expr::not(f.expr.clone()), DataType::Int64);
        aggs.push(agg::sum(failed).alias(format!("fail__{}", f.assert_id)));
    }
    let mut stats = Vec::new();
    for d in &fields {
        let name = stat_column(d);
        let (ty, agg_expr) = match d {
            DatasetField::RowCount | DatasetField::AssertionPassRate { .. } => {
                let ty = if matches!(d, DatasetField::RowCount) {
                    Type::Int
                } else {
                    Type::Double
                };
                stats.push(StatSpec {
                    field: d.clone(),
                    column: name.clone(),
                    path: crate::cel_print::var_path(&Var::Dataset(d.clone())),
                    ty,
                });
                continue; // computed in stage 2 from row_count and fail__*
            }
            DatasetField::Column { column: col, stat } => {
                let c = column(col);
                match stat {
                    ColumnStat::NullCount | ColumnStat::NullRate => (
                        if *stat == ColumnStat::NullCount {
                            Type::Int
                        } else {
                            Type::Double
                        },
                        agg::sum(cast(c.is_null(), DataType::Int64)).alias(format!("{col}__nulls")),
                    ),
                    ColumnStat::DistinctCount => {
                        (Type::Int, agg::count_distinct(c).alias(name.clone()))
                    }
                    ColumnStat::Min | ColumnStat::Max => {
                        let ty = Type::from_arrow(
                            schema
                                .field_with_name(col)
                                .map_err(|e| (None, e.to_string()))?
                                .data_type(),
                        )
                        .map_err(|e| (None, e))?;
                        let c = cast(c, ty.to_arrow());
                        (
                            ty,
                            if *stat == ColumnStat::Min {
                                agg::min(c)
                            } else {
                                agg::max(c)
                            }
                            .alias(name.clone()),
                        )
                    }
                }
            }
            DatasetField::Other(field) => {
                let p = c
                    .producers
                    .iter()
                    .find(|p| &p.field == field)
                    .expect("checker verified producers");
                let mut scratch = Params::default();
                let mut t = Translator {
                    schema,
                    params: &mut scratch,
                    dataset_columns: false,
                    derived: None,
                };
                let arg = match &p.kind {
                    crate::check::ProducerKind::Constant(_) => {
                        stats.push(StatSpec {
                            field: d.clone(),
                            column: name.clone(),
                            path: crate::cel_print::var_path(&Var::Dataset(d.clone())),
                            ty: p.ty.clone(),
                        });
                        continue; // a literal, added in stage 2
                    }
                    crate::check::ProducerKind::Aggregate { arg, .. } => match arg {
                        Some(a) => Some(
                            t.expr(&a.expr)
                                .map_err(|e| (Some(format!("dataset_other.{field}")), e))?,
                        ),
                        None => None,
                    },
                };
                use crate::check::Aggregate as A;
                let crate::check::ProducerKind::Aggregate { func, .. } = &p.kind else {
                    unreachable!()
                };
                let a = || arg.clone().expect("checker requires an argument");
                let e = match func {
                    A::Count => agg::count(lit(1i64)),
                    A::Sum => agg::sum(a()),
                    A::Avg => agg::avg(a()),
                    A::Min => agg::min(a()),
                    A::Max => agg::max(a()),
                    A::CountDistinct => agg::count_distinct(a()),
                    A::Median => agg::median(cast(a(), DataType::Float64)),
                };
                (p.ty.clone(), cast(e, p.ty.to_arrow()).alias(name.clone()))
            }
            DatasetField::WrittenAt | DatasetField::ContractHash => unreachable!("filtered above"),
        };
        if !aggs
            .iter()
            .any(|a| a.schema_name().to_string() == agg_expr.schema_name().to_string())
        {
            aggs.push(agg_expr);
        }
        stats.push(StatSpec {
            field: d.clone(),
            column: name,
            path: crate::cel_print::var_path(&Var::Dataset(d.clone())),
            ty,
        });
    }

    let source = Arc::new(LogicalTableSource::new(Arc::new(schema.clone())));
    let aggregated = LogicalPlanBuilder::scan(BINDING_TABLE, source, None)
        .map_err(fail)?
        .aggregate(Vec::<Expr>::new(), aggs)
        .map_err(fail)?
        .build()
        .map_err(fail)?;

    // Stage 2: derive rates and apply thresholds. Empty datasets have rate 0 nulls and pass rate 1.
    let rows = || cast(column("row_count"), DataType::Float64);
    let nonzero = || binary_expr(column("row_count"), Operator::Gt, lit(0i64));
    let mut out: Vec<Expr> = vec![column("row_count")];
    for f in flags {
        out.push(
            datafusion_functions::expr_fn::coalesce(vec![
                column(&format!("fail__{}", f.assert_id)),
                lit(0i64),
            ])
            .alias(format!("fail__{}", f.assert_id)),
        );
    }
    for s in &stats {
        let e = match &s.field {
            DatasetField::RowCount => continue,
            DatasetField::AssertionPassRate { assertion } => {
                let failed = cast(
                    datafusion_functions::expr_fn::coalesce(vec![
                        column(&format!("fail__{assertion}")),
                        lit(0i64),
                    ]),
                    DataType::Float64,
                );
                when(nonzero(), lit(1.0f64) - failed / rows())
                    .otherwise(lit(1.0f64))
                    .map_err(fail)?
            }
            DatasetField::Column {
                column: col,
                stat: ColumnStat::NullCount,
            } => datafusion_functions::expr_fn::coalesce(vec![
                column(&format!("{col}__nulls")),
                lit(0i64),
            ]),
            DatasetField::Column {
                column: col,
                stat: ColumnStat::NullRate,
            } => {
                let nulls = cast(
                    datafusion_functions::expr_fn::coalesce(vec![
                        column(&format!("{col}__nulls")),
                        lit(0i64),
                    ]),
                    DataType::Float64,
                );
                when(nonzero(), nulls / rows())
                    .otherwise(lit(0.0f64))
                    .map_err(fail)?
            }
            DatasetField::Other(field) => match c
                .producers
                .iter()
                .find(|p| &p.field == field)
                .map(|p| &p.kind)
            {
                Some(crate::check::ProducerKind::Constant(l)) => {
                    lit(crate::translate::lit_value(l))
                }
                _ => column(&s.column),
            },
            _ => column(&s.column),
        };
        out.push(e.alias(&s.column));
    }
    let with_stats = LogicalPlanBuilder::from(aggregated)
        .project(out)
        .map_err(fail)?
        .build()
        .map_err(fail)?;

    // Stage 3: the verdict.
    let mut params = Params::default();
    let mut t = Translator {
        schema,
        params: &mut params,
        dataset_columns: true,
        derived: None,
    };
    let mut verdict: Vec<Expr> = with_stats
        .schema()
        .columns()
        .into_iter()
        .map(Expr::Column)
        .collect();
    let mut must_hold: Vec<(String, Expr)> = Vec::new();
    for f in flags.iter().filter(|f| f.on_fail == AssertOnFail::Deny) {
        must_hold.push((
            f.assert_id.clone(),
            binary_expr(
                column(&format!("fail__{}", f.assert_id)),
                Operator::Eq,
                lit(0i64),
            ),
        ));
    }
    for r in &c.rules {
        if let CheckedRule::Guarantee {
            id, expr, on_fail, ..
        } = r
            && data_guarantees.contains(id)
        {
            let e = datafusion_functions::expr_fn::coalesce(vec![
                t.expr(&expr.expr).map_err(|e| (Some(id.clone()), e))?,
                lit(false),
            ]);
            verdict.push(e.clone().alias(format!("guarantee__{id}")));
            if *on_fail == GuaranteeOnFail::Deny {
                must_hold.push((id.clone(), e));
            }
        }
    }
    let valid = must_hold
        .iter()
        .map(|(_, e)| e.clone())
        .reduce(Expr::and)
        .unwrap_or(lit(true));
    let breached = datafusion_functions::expr_fn::concat_ws(
        lit(","),
        must_hold
            .iter()
            .map(|(id, e)| when(datafusion_expr::not(e.clone()), lit(id.clone())).end())
            .collect::<datafusion_common::Result<Vec<_>>>()
            .map_err(fail)?,
    );
    verdict.push(valid.alias("valid"));
    verdict.push(
        if must_hold.is_empty() {
            lit("")
        } else {
            breached
        }
        .alias("breached"),
    );
    let plan = LogicalPlanBuilder::from(with_stats)
        .project(verdict)
        .map_err(fail)?
        .build()
        .map_err(fail)?;

    Ok(ValidationPlan {
        plan,
        stats,
        asserts: flags.iter().map(|f| f.assert_id.clone()).collect(),
        data_guarantees,
        query_time_guarantees,
    })
}

/// The schema the binding is scanned with: the data's columns, plus `_other` when enrichers produce it.
pub fn scan_schema(c: &CheckedContract, schema: &Schema) -> Schema {
    match other_type(c) {
        Some(t) if schema.field_with_name(crate::ir::OTHER_COLUMN).is_err() => {
            let mut fields: Vec<Field> =
                schema.fields().iter().map(|f| f.as_ref().clone()).collect();
            fields.push(Field::new(crate::ir::OTHER_COLUMN, t, true));
            Schema::new(fields)
        }
        _ => schema.clone(),
    }
}

/// The Arrow struct type of `_other` as the enrichers produce it.
pub fn other_type(c: &CheckedContract) -> Option<DataType> {
    if c.enrichers.is_empty() {
        return None;
    }
    let fields: Vec<Field> = c
        .enrichers
        .iter()
        .map(|(f, x)| Field::new(f, x.expr.ty.to_arrow(), true))
        .collect();
    Some(DataType::Struct(fields.into()))
}

/// Bind lambda variables (from CEL macros) to their element types. Every plan that
/// embeds parcel expressions must pass through this before execution.
pub fn resolve(plan: LogicalPlan) -> datafusion_common::Result<LogicalPlan> {
    Ok(plan.resolve_lambda_variables()?.data)
}

/// Maximal row-only subtrees worth storing: they call a function, run a macro or match a regex.
/// Bare columns, literals and plain arithmetic are cheaper to recompute than to store.
fn split_candidates<'e>(e: &'e TExpr, out: &mut Vec<&'e TExpr>) {
    use crate::ir::Builtin;
    if e.ns == crate::ir::NsSet::ROW {
        let local = !e.free_locals().is_empty();
        let mut worth = false;
        e.walk(&mut |n| {
            if matches!(
                n.kind,
                ExprKind::Call(..)
                    | ExprKind::Macro { .. }
                    | ExprKind::Builtin(Builtin::Matches, _)
            ) {
                worth = true;
            }
        });
        if worth && !local {
            out.push(e);
            return;
        }
        if !local {
            return;
        }
    }
    for c in e.children() {
        split_candidates(c, out);
    }
}

fn dataset_fields(e: &TExpr) -> Vec<DatasetField> {
    let mut out = Vec::new();
    e.walk(&mut |n| {
        if let ExprKind::Var(Var::Dataset(d)) = &n.kind
            && !out.contains(d)
        {
            out.push(d.clone());
        }
    });
    out
}

fn registry_names(e: &TExpr) -> Vec<String> {
    e.pins().into_iter().map(|p| p.name).collect()
}

fn cost_note(_e: &TExpr, fns: Vec<String>) -> String {
    if fns.is_empty() {
        String::new()
    } else {
        format!("; calls {}", fns.join(", "))
    }
}

/// Can the scan use statistics for this predicate? True when it is built from AND/OR/NOT over
/// comparisons of a plain column with constants or caller context.
fn prunable(e: &TExpr) -> bool {
    use crate::ir::BinOp::*;
    let is_const = |x: &TExpr| !x.ns.row;
    let is_col = |x: &TExpr| matches!(x.kind, ExprKind::Var(Var::Row(_)));
    if !e.ns.row {
        return true;
    }
    match &e.kind {
        ExprKind::Binary(And | Or, a, b) => prunable(a) && prunable(b),
        ExprKind::Not(x) => prunable(x),
        ExprKind::Has(_) => true,
        ExprKind::Binary(Eq | Ne | Lt | Le | Gt | Ge, a, b) => {
            (is_col(a) && is_const(b)) || (is_const(a) && is_col(b))
        }
        ExprKind::Binary(In, a, b) => is_col(a) && is_const(b),
        _ => false,
    }
}

/// Columns compared for equality against caller context benefit from bloom filters.
fn bloom_columns(c: &CheckedContract) -> Vec<String> {
    let mut out = BTreeSet::new();
    for r in &c.rules {
        if let CheckedRule::Admit { expr, .. } = r {
            expr.expr.walk(&mut |n| {
                if let ExprKind::Binary(crate::ir::BinOp::Eq, a, b) = &n.kind {
                    for (x, y) in [(a, b), (b, a)] {
                        if let ExprKind::Var(Var::Row(col)) = &x.kind
                            && y.ns.ctx
                            && !y.ns.row
                        {
                            out.insert(col.clone());
                        }
                    }
                }
            });
        }
    }
    out.into_iter().collect()
}

fn ser_expr<S: serde::Serializer>(e: &Expr, s: S) -> Result<S::Ok, S::Error> {
    s.collect_str(e)
}

fn ser_named_exprs<S: serde::Serializer>(v: &[(String, Expr)], s: S) -> Result<S::Ok, S::Error> {
    use serde::ser::SerializeMap;
    let mut m = s.serialize_map(Some(v.len()))?;
    for (k, e) in v {
        m.serialize_entry(k, &e.to_string())?;
    }
    m.end()
}

fn ser_schema<S: serde::Serializer>(schema: &SchemaRef, s: S) -> Result<S::Ok, S::Error> {
    use serde::ser::SerializeMap;
    let mut m = s.serialize_map(Some(schema.fields().len()))?;
    for f in schema.fields() {
        m.serialize_entry(f.name(), &f.data_type().to_string())?;
    }
    m.end()
}

fn ser_plan<S: serde::Serializer>(p: &LogicalPlan, s: S) -> Result<S::Ok, S::Error> {
    s.collect_str(&p.display_indent())
}
