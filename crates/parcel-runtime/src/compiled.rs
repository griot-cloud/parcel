//! A compiled contract as bytes, and back: [`Compilation::to_bytes`] and
//! [`Compilation::from_bytes`] (through [`CompiledBytes`]).
//!
//! The encoding is UTF-8 JSON with no insignificant whitespace. Its fields are written in a
//! fixed order, every ordered collection is a list, schemas keep each column's nullability, and
//! every expression and the validation plan are `datafusion-proto` bytes (hex). A user
//! function an expression calls carries its pin and signature inside those bytes, so decoding
//! needs no registry. Encoding the same compilation always gives the same bytes, and decoding
//! them gives back the same compilation: whoever loads it runs what was compiled, without
//! compiling it again. The bytes are only as trustworthy as whoever vouches for them.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use datafusion::arrow::datatypes::{DataType, SchemaRef};
use datafusion::catalog::TableProvider;
use datafusion::common::TableReference;
use datafusion::common::tree_node::{Transformed, TreeNode};
use datafusion::datasource::empty::EmptyTable;
use datafusion::error::{DataFusionError, Result as DFResult};
use datafusion::execution::TaskContext;
use datafusion::logical_expr::expr::Placeholder;
use datafusion::logical_expr::logical_plan::builder::LogicalTableSource;
use datafusion::logical_expr::{Expr, Extension, LogicalPlan, LogicalPlanBuilder, ScalarUDF};
use datafusion_proto::bytes::{
    logical_plan_from_bytes_with_extension_codec, logical_plan_to_bytes_with_extension_codec,
};
use datafusion_proto::logical_plan::LogicalExtensionCodec;
use datafusion_proto::logical_plan::from_proto::parse_expr;
use datafusion_proto::logical_plan::to_proto::serialize_expr;
use datafusion_proto::protobuf::LogicalExprNode;
use parcel_core::Residency;
use parcel_core::compile::{
    BINDING_TABLE, CelRule, Compilation, CompiledContract, Derived, EnrichSpec, Flag,
    GuaranteeRule, Layout, PARCEL_VERSION, ReportEntry, RowRule, ShapeRule, StatSpec,
    ValidationPlan, WritePlan,
};
use parcel_core::document::{AssertOnFail, Binding};
use parcel_core::registry::FunctionPin;
use parcel_core::translate::{CtxParam, placeholder_field, user_function_parts, user_function_udf};
use parcel_core::types::Type;
use prost::Message;
use serde::{Deserialize, Serialize};

use crate::bundle::{ColumnDef, portable_plan, schema_from_defs, schema_to_defs, session};

/// The format the bytes are in.
pub const FORMAT: &str = "parcel-compiled/1";

/// A [`Compilation`] as bytes, and back.
pub trait CompiledBytes: Sized {
    /// The compiled contract as bytes, in [`FORMAT`].
    fn to_bytes(&self) -> Result<Vec<u8>, String>;
    /// The compiled contract the bytes hold, as it was compiled. Never compiles. Refuses bytes
    /// in another format or compiled by another version of parcel.
    fn from_bytes(bytes: &[u8]) -> Result<Self, String>;
}

impl CompiledBytes for Compilation {
    fn to_bytes(&self) -> Result<Vec<u8>, String> {
        serde_json::to_vec(&Form::of(self)?).map_err(|e| e.to_string())
    }

    fn from_bytes(bytes: &[u8]) -> Result<Compilation, String> {
        let form: Form = serde_json::from_slice(bytes)
            .map_err(|e| format!("the compiled contract does not parse: {e}"))?;
        if form.format != FORMAT {
            return Err(format!(
                "the compiled contract is in format `{}`; this parcel reads `{FORMAT}`",
                form.format
            ));
        }
        if form.parcel_version != PARCEL_VERSION {
            return Err(format!(
                "the compiled contract was compiled by parcel {}; this is parcel {PARCEL_VERSION}",
                form.parcel_version
            ));
        }
        form.compilation()
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Form {
    format: String,
    parcel_version: String,
    contract: ContractForm,
    validation: ValidationForm,
    write: WriteForm,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ContractForm {
    name: String,
    version: u32,
    contract_hash: String,
    compilation_hash: String,
    owner: Option<String>,
    binding: Binding,
    residency: Residency,
    row_schema: Vec<ColumnDef>,
    exposed_schema: Vec<ColumnDef>,
    params: Vec<CtxParam>,
    decisions: Vec<CelRule>,
    admits: Vec<NamedExpr>,
    flags: Vec<FlagForm>,
    projection: Vec<NamedExpr>,
    guarantees: Vec<GuaranteeRule>,
    shapes: Vec<ShapeRule>,
    functions: BTreeSet<FunctionPin>,
    report: Vec<ReportEntry>,
    row_rules: Vec<RowRule>,
    derived: Vec<DerivedForm>,
    scan_schema: Vec<ColumnDef>,
    enrich: Vec<EnrichForm>,
    ctx_other: BTreeMap<String, Type>,
    admits_stored: Vec<NamedExpr>,
    projection_stored: Vec<NamedExpr>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct NamedExpr {
    name: String,
    expr: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FlagForm {
    assert_id: String,
    column: String,
    expr: String,
    on_fail: AssertOnFail,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DerivedForm {
    column: String,
    cel: String,
    expr: String,
    ty: Type,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct EnrichForm {
    field: String,
    cel: String,
    reads: Vec<String>,
    expr: String,
    ty: Type,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ValidationForm {
    plan: String,
    stats: Vec<StatSpec>,
    asserts: Vec<String>,
    data_guarantees: Vec<String>,
    query_time_guarantees: Vec<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WriteForm {
    enrich: Vec<EnrichForm>,
    flags: Vec<FlagForm>,
    derived: Vec<DerivedForm>,
    layout: Layout,
    manifest_fields: Vec<String>,
}

impl Form {
    fn of(c: &Compilation) -> Result<Form, String> {
        let cc = &c.contract;
        let plan = portable_plan(&c.validation.plan, &cc.scan_schema)
            .and_then(|p| logical_plan_to_bytes_with_extension_codec(&p, &Codec))
            .map_err(|e| format!("the validation plan: {e}"))?;
        Ok(Form {
            format: FORMAT.into(),
            parcel_version: PARCEL_VERSION.into(),
            contract: ContractForm {
                name: cc.name.clone(),
                version: cc.version,
                contract_hash: cc.contract_hash.clone(),
                compilation_hash: cc.compilation_hash.clone(),
                owner: cc.owner.clone(),
                binding: cc.binding.clone(),
                residency: cc.residency.clone(),
                row_schema: schema_to_defs(&cc.row_schema),
                exposed_schema: schema_to_defs(&cc.exposed_schema),
                params: cc.params.clone(),
                decisions: cc.decisions.clone(),
                admits: named(&cc.admits)?,
                flags: cc
                    .flags
                    .iter()
                    .map(FlagForm::of)
                    .collect::<Result<_, _>>()?,
                projection: named(&cc.projection)?,
                guarantees: cc.guarantees.clone(),
                shapes: cc.shapes.clone(),
                functions: cc.functions.clone(),
                report: cc.report.clone(),
                row_rules: cc.row_rules.clone(),
                derived: cc
                    .derived
                    .iter()
                    .map(DerivedForm::of)
                    .collect::<Result<_, _>>()?,
                scan_schema: schema_to_defs(&cc.scan_schema),
                enrich: cc
                    .enrich
                    .iter()
                    .map(EnrichForm::of)
                    .collect::<Result<_, _>>()?,
                ctx_other: cc.ctx_other.clone(),
                admits_stored: named(&cc.admits_stored)?,
                projection_stored: named(&cc.projection_stored)?,
            },
            validation: ValidationForm {
                plan: hex::encode(plan),
                stats: c.validation.stats.clone(),
                asserts: c.validation.asserts.clone(),
                data_guarantees: c.validation.data_guarantees.clone(),
                query_time_guarantees: c.validation.query_time_guarantees.clone(),
            },
            write: WriteForm {
                enrich: c
                    .write
                    .enrich
                    .iter()
                    .map(EnrichForm::of)
                    .collect::<Result<_, _>>()?,
                flags: c
                    .write
                    .flags
                    .iter()
                    .map(FlagForm::of)
                    .collect::<Result<_, _>>()?,
                derived: c
                    .write
                    .derived
                    .iter()
                    .map(DerivedForm::of)
                    .collect::<Result<_, _>>()?,
                layout: c.write.layout.clone(),
                manifest_fields: c.write.manifest_fields.clone(),
            },
        })
    }

    fn compilation(self) -> Result<Compilation, String> {
        let ctx = session();
        let task = ctx.task_ctx();
        let t = task.as_ref();
        let k = self.contract;
        let schema = |what: &str, defs: &[ColumnDef]| -> Result<SchemaRef, String> {
            schema_from_defs(defs)
                .map(Arc::new)
                .map_err(|e| format!("the {what}: {e}"))
        };
        let scan_schema = schema("scan schema", &k.scan_schema)?;
        let bytes =
            hex::decode(&self.validation.plan).map_err(|e| format!("the validation plan: {e}"))?;
        let plan = logical_plan_from_bytes_with_extension_codec(&bytes, t, &Codec)
            .and_then(|p| compile_time_scan(p, &scan_schema))
            .map_err(|e| format!("the validation plan: {e}"))?;
        Ok(Compilation {
            contract: CompiledContract {
                name: k.name,
                version: k.version,
                contract_hash: k.contract_hash,
                compilation_hash: k.compilation_hash,
                owner: k.owner,
                binding: k.binding,
                residency: k.residency,
                row_schema: schema("row schema", &k.row_schema)?,
                exposed_schema: schema("exposed schema", &k.exposed_schema)?,
                params: k.params,
                decisions: k.decisions,
                admits: unnamed("admit", k.admits, t)?,
                flags: k
                    .flags
                    .into_iter()
                    .map(|f| f.flag(t))
                    .collect::<Result<_, _>>()?,
                projection: unnamed("project", k.projection, t)?,
                guarantees: k.guarantees,
                shapes: k.shapes,
                functions: k.functions,
                report: k.report,
                row_rules: k.row_rules,
                derived: k
                    .derived
                    .into_iter()
                    .map(|d| d.derived(t))
                    .collect::<Result<_, _>>()?,
                scan_schema,
                enrich: k
                    .enrich
                    .into_iter()
                    .map(|e| e.enrich(t))
                    .collect::<Result<_, _>>()?,
                ctx_other: k.ctx_other,
                admits_stored: unnamed("admit_stored", k.admits_stored, t)?,
                projection_stored: unnamed("project_stored", k.projection_stored, t)?,
            },
            validation: ValidationPlan {
                plan,
                stats: self.validation.stats,
                asserts: self.validation.asserts,
                data_guarantees: self.validation.data_guarantees,
                query_time_guarantees: self.validation.query_time_guarantees,
            },
            write: WritePlan {
                enrich: self
                    .write
                    .enrich
                    .into_iter()
                    .map(|e| e.enrich(t))
                    .collect::<Result<_, _>>()?,
                flags: self
                    .write
                    .flags
                    .into_iter()
                    .map(|f| f.flag(t))
                    .collect::<Result<_, _>>()?,
                derived: self
                    .write
                    .derived
                    .into_iter()
                    .map(|d| d.derived(t))
                    .collect::<Result<_, _>>()?,
                layout: self.write.layout,
                manifest_fields: self.write.manifest_fields,
            },
        })
    }
}

impl FlagForm {
    fn of(f: &Flag) -> Result<FlagForm, String> {
        Ok(FlagForm {
            assert_id: f.assert_id.clone(),
            column: f.column.clone(),
            expr: encode(&f.expr, &format!("flag/{}", f.assert_id))?,
            on_fail: f.on_fail,
        })
    }

    fn flag(self, t: &TaskContext) -> Result<Flag, String> {
        Ok(Flag {
            expr: decode(&self.expr, &format!("flag/{}", self.assert_id), t)?,
            assert_id: self.assert_id,
            column: self.column,
            on_fail: self.on_fail,
        })
    }
}

impl DerivedForm {
    fn of(d: &Derived) -> Result<DerivedForm, String> {
        Ok(DerivedForm {
            column: d.column.clone(),
            cel: d.cel.clone(),
            expr: encode(&d.expr, &format!("derived/{}", d.column))?,
            ty: d.ty.clone(),
        })
    }

    fn derived(self, t: &TaskContext) -> Result<Derived, String> {
        Ok(Derived {
            expr: decode(&self.expr, &format!("derived/{}", self.column), t)?,
            column: self.column,
            cel: self.cel,
            ty: self.ty,
        })
    }
}

impl EnrichForm {
    fn of(e: &EnrichSpec) -> Result<EnrichForm, String> {
        Ok(EnrichForm {
            field: e.field.clone(),
            cel: e.cel.clone(),
            reads: e.reads.clone(),
            expr: encode(&e.expr, &format!("enrich/{}", e.field))?,
            ty: e.ty.clone(),
        })
    }

    fn enrich(self, t: &TaskContext) -> Result<EnrichSpec, String> {
        Ok(EnrichSpec {
            expr: decode(&self.expr, &format!("enrich/{}", self.field), t)?,
            field: self.field,
            cel: self.cel,
            reads: self.reads,
            ty: self.ty,
        })
    }
}

fn named(v: &[(String, Expr)]) -> Result<Vec<NamedExpr>, String> {
    v.iter()
        .map(|(name, e)| {
            Ok(NamedExpr {
                expr: encode(e, name)?,
                name: name.clone(),
            })
        })
        .collect()
}

fn unnamed(kind: &str, v: Vec<NamedExpr>, t: &TaskContext) -> Result<Vec<(String, Expr)>, String> {
    v.into_iter()
        .map(|n| {
            let e = decode(&n.expr, &format!("{kind}/{}", n.name), t)?;
            Ok((n.name, e))
        })
        .collect()
}

fn encode(e: &Expr, what: &str) -> Result<String, String> {
    let node = serialize_expr(e, &Codec).map_err(|x| format!("`{what}`: {x}"))?;
    Ok(hex::encode(node.encode_to_vec()))
}

fn decode(hexed: &str, what: &str, t: &TaskContext) -> Result<Expr, String> {
    let bytes = hex::decode(hexed).map_err(|x| format!("`{what}`: {x}"))?;
    let node = LogicalExprNode::decode(bytes.as_slice()).map_err(|x| format!("`{what}`: {x}"))?;
    let e = parse_expr(&node, t, &Codec).map_err(|x| format!("`{what}`: {x}"))?;
    // `datafusion-proto` keeps a placeholder's type but not its field: give it parcel's back.
    e.transform_up(|e| match e {
        Expr::Placeholder(Placeholder { id, field: Some(f) }) => {
            let field = placeholder_field(id.trim_start_matches('$'), f.data_type().clone());
            Ok(Transformed::yes(Expr::Placeholder(Placeholder {
                id,
                field: Some(field),
            })))
        }
        e => Ok(Transformed::no(e)),
    })
    .map(|t| t.data)
    .map_err(|x| format!("`{what}`: {x}"))
}

/// The binding scan as the compiler builds it: a placeholder source of the scan schema.
fn compile_time_scan(plan: LogicalPlan, schema: &SchemaRef) -> DFResult<LogicalPlan> {
    let out = plan.transform_up(|node| {
        if let LogicalPlan::TableScan(ts) = &node
            && ts.table_name.table() == BINDING_TABLE
        {
            let source = Arc::new(LogicalTableSource::new(schema.clone()));
            let scan = LogicalPlanBuilder::scan(BINDING_TABLE, source, None)?.build()?;
            return Ok(Transformed::yes(scan));
        }
        Ok(Transformed::no(node))
    })?;
    out.data.recompute_schema()
}

/// A user function as it travels inside an expression: what [`user_function_udf`] needs.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct UserFunctionForm {
    pin: FunctionPin,
    args: Vec<String>,
    ret: String,
}

/// Carries user functions inside the expression bytes and scans only the contract binding.
#[derive(Debug)]
struct Codec;

impl LogicalExtensionCodec for Codec {
    fn try_decode(&self, _: &[u8], _: &[LogicalPlan], _: &TaskContext) -> DFResult<Extension> {
        Err(DataFusionError::NotImplemented(
            "compiled contracts contain no extension nodes".into(),
        ))
    }

    fn try_encode(&self, _: &Extension, _: &mut Vec<u8>) -> DFResult<()> {
        Err(DataFusionError::NotImplemented(
            "compiled contracts contain no extension nodes".into(),
        ))
    }

    fn try_decode_table_provider(
        &self,
        buf: &[u8],
        _: &TableReference,
        schema: SchemaRef,
        _: &TaskContext,
    ) -> DFResult<Arc<dyn TableProvider>> {
        if buf != BINDING_TABLE.as_bytes() {
            return Err(DataFusionError::Plan(
                "compiled contracts scan only the contract binding".into(),
            ));
        }
        Ok(Arc::new(EmptyTable::new(schema)))
    }

    fn try_encode_table_provider(
        &self,
        _: &TableReference,
        node: Arc<dyn TableProvider>,
        buf: &mut Vec<u8>,
    ) -> DFResult<()> {
        if node.downcast_ref::<EmptyTable>().is_none() {
            return Err(DataFusionError::Plan(
                "compiled contracts scan only the contract binding".into(),
            ));
        }
        buf.extend_from_slice(BINDING_TABLE.as_bytes());
        Ok(())
    }

    fn try_encode_udf(&self, node: &ScalarUDF, buf: &mut Vec<u8>) -> DFResult<()> {
        if let Some((pin, args, ret)) = user_function_parts(node) {
            let form = UserFunctionForm {
                pin: pin.clone(),
                args: args.iter().map(|a| a.to_string()).collect(),
                ret: ret.to_string(),
            };
            serde_json::to_writer(buf, &form).map_err(|e| DataFusionError::External(e.into()))?;
        }
        Ok(())
    }

    fn try_decode_udf(&self, name: &str, buf: &[u8]) -> DFResult<Arc<ScalarUDF>> {
        if buf.is_empty() {
            return Err(DataFusionError::Plan(format!(
                "function `{name}` is neither one of parcel's nor a pinned user function"
            )));
        }
        let form: UserFunctionForm =
            serde_json::from_slice(buf).map_err(|e| DataFusionError::External(e.into()))?;
        let parse = |t: &str| -> DFResult<DataType> {
            t.parse()
                .map_err(|e: datafusion::arrow::error::ArrowError| {
                    DataFusionError::ArrowError(Box::new(e), None)
                })
        };
        let args = form
            .args
            .iter()
            .map(|a| parse(a))
            .collect::<DFResult<Vec<_>>>()?;
        Ok(Arc::new(user_function_udf(
            &form.pin,
            args,
            parse(&form.ret)?,
        )))
    }
}
