//! The parcel bundle: a compiled contract as one portable file.
//!
//! A bundle carries the contract document, the schema it was compiled against,
//! the human-readable artifacts, and the executable ones (every expression and
//! the validation plan) encoded with `datafusion-proto`. Loading a bundle
//! recompiles the document and checks that the result matches the bundle,
//! byte for byte, so a receiver never has to trust the sender's compiler
//! (design 9.4).

use std::collections::BTreeMap;
use std::sync::Arc;

use datafusion::arrow::datatypes::{Schema, SchemaRef};
use datafusion::catalog::TableProvider;
use datafusion::common::TableReference;
use datafusion::common::tree_node::{Transformed, TreeNode};
use datafusion::datasource::empty::EmptyTable;
use datafusion::datasource::provider_as_source;
use datafusion::error::{DataFusionError, Result as DFResult};
use datafusion::execution::TaskContext;
use datafusion::logical_expr::{Expr, Extension, LogicalPlan, LogicalPlanBuilder};
use datafusion::prelude::SessionContext;
use datafusion_proto::bytes::{
    Serializeable, logical_plan_from_bytes_with_extension_codec,
    logical_plan_to_bytes_with_extension_codec,
};
use datafusion_proto::logical_plan::LogicalExtensionCodec;
use parcel_core::compile::{BINDING_TABLE, Compilation, PARCEL_VERSION};
use parcel_core::{ContractDoc, Registry, compile_with};
use serde::{Deserialize, Serialize};

pub const FORMAT: &str = "parcel-bundle/1";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BundledFunction {
    pub owner: String,
    pub manifest: parcel_core::registry::FunctionManifest,
    /// The module, hex.
    pub module: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Bundle {
    pub format: String,
    pub parcel_version: String,
    pub contract: String,
    pub version: u32,
    pub contract_hash: String,
    pub compilation_hash: String,
    pub document: ContractDoc,
    /// The documents it inherits from, parent first; needed to recompile a child.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ancestors: Vec<ContractDoc>,
    pub row_schema: Vec<ColumnDef>,
    /// The tenants' WebAssembly functions it is pinned to, so a verifier can recompile and run it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub functions: Vec<BundledFunction>,
    /// The three artifacts, readable: report, parameters, rules, statistics, layout.
    pub artifacts: serde_json::Value,
    /// Executable expressions, `datafusion-proto` encoded, hex: `admit/<id>`, `flag/<id>`, `project/<column>`.
    pub exprs: BTreeMap<String, String>,
    /// The validation plan, `datafusion-proto` encoded, hex. Its scan is `__parcel_binding`.
    pub validation_plan: String,
}

impl Bundle {
    pub fn new(
        doc: &ContractDoc,
        ancestors: &[ContractDoc],
        schema: &Schema,
        c: &Compilation,
    ) -> DFResult<Bundle> {
        Self::with_functions(doc, ancestors, schema, c, Vec::new())
    }

    pub fn with_functions(
        doc: &ContractDoc,
        ancestors: &[ContractDoc],
        schema: &Schema,
        c: &Compilation,
        functions: Vec<BundledFunction>,
    ) -> DFResult<Bundle> {
        let cc = &c.contract;
        let mut exprs = BTreeMap::new();
        for (id, e) in &cc.admits {
            exprs.insert(format!("admit/{id}"), hex::encode(e.to_bytes()?));
        }
        for f in &cc.flags {
            exprs.insert(
                format!("flag/{}", f.assert_id),
                hex::encode(f.expr.to_bytes()?),
            );
        }
        for (col, e) in &cc.projection {
            exprs.insert(format!("project/{col}"), hex::encode(e.to_bytes()?));
        }
        for (id, e) in &cc.admits_stored {
            exprs.insert(format!("admit_stored/{id}"), hex::encode(e.to_bytes()?));
        }
        for (col, e) in &cc.projection_stored {
            exprs.insert(format!("project_stored/{col}"), hex::encode(e.to_bytes()?));
        }
        for d in &cc.derived {
            exprs.insert(
                format!("derived/{}", d.column),
                hex::encode(d.expr.to_bytes()?),
            );
        }
        for e in &cc.enrich {
            exprs.insert(
                format!("enrich/{}", e.field),
                hex::encode(e.expr.to_bytes()?),
            );
        }
        let plan = portable_plan(&c.validation.plan, &cc.scan_schema)?;
        let plan_bytes = logical_plan_to_bytes_with_extension_codec(&plan, &BindingCodec)?;
        Ok(Bundle {
            format: FORMAT.into(),
            parcel_version: PARCEL_VERSION.into(),
            contract: cc.name.clone(),
            version: cc.version,
            contract_hash: cc.contract_hash.clone(),
            compilation_hash: cc.compilation_hash.clone(),
            document: doc.clone(),
            ancestors: ancestors.to_vec(),
            functions,
            row_schema: schema_to_defs(schema),
            artifacts: serde_json::to_value(c)
                .map_err(|e| DataFusionError::External(Box::new(e)))?,
            exprs,
            validation_plan: hex::encode(plan_bytes),
        })
    }

    pub fn to_json(&self) -> serde_json::Result<String> {
        serde_json::to_string_pretty(self)
    }

    pub fn from_json(s: &str) -> serde_json::Result<Bundle> {
        serde_json::from_str(s)
    }

    pub fn schema(&self) -> Result<Schema, String> {
        schema_from_defs(&self.row_schema)
    }

    /// Recompile the document and confirm every executable artifact in the bundle matches.
    pub fn verify(&self) -> Result<Compilation, String> {
        if self.format != FORMAT {
            return Err(format!("unknown bundle format `{}`", self.format));
        }
        let schema = self.schema()?;
        let mut registry = Registry::builtin();
        for f in &self.functions {
            let module = hex::decode(&f.module).map_err(|e| e.to_string())?;
            registry.insert(crate::wasm::install(&module, &f.manifest, &f.owner)?);
        }
        let c = compile_with(&self.document, &schema, &registry, &|n| {
            self.ancestors.iter().find(|a| a.contract == n).cloned()
        })
        .map_err(|d| {
            d.iter()
                .map(|d| d.to_string())
                .collect::<Vec<_>>()
                .join("; ")
        })?;
        if c.contract.compilation_hash != self.compilation_hash {
            return Err(format!(
                "compilation hash mismatch: bundle says {}, recompiling gives {}",
                self.compilation_hash, c.contract.compilation_hash
            ));
        }
        let ctx = session_with(&c, &registry);
        let decode = |key: &str| -> Result<Expr, String> {
            let hexed = self
                .exprs
                .get(key)
                .ok_or_else(|| format!("bundle lacks `{key}`"))?;
            let bytes = hex::decode(hexed).map_err(|e| e.to_string())?;
            Expr::from_bytes_with_ctx(&bytes, &ctx.task_ctx()).map_err(|e| format!("`{key}`: {e}"))
        };
        // Every expression must decode, and must be byte-identical to what recompiling produces.
        let check = |key: String, want: &Expr| -> Result<(), String> {
            decode(&key)?;
            let want = hex::encode(want.to_bytes().map_err(|e| e.to_string())?);
            if self.exprs.get(&key) != Some(&want) {
                return Err(format!(
                    "`{key}` in the bundle differs from the recompiled contract"
                ));
            }
            Ok(())
        };
        for (id, e) in &c.contract.admits {
            check(format!("admit/{id}"), e)?;
        }
        for f in &c.contract.flags {
            check(format!("flag/{}", f.assert_id), &f.expr)?;
        }
        for (col, e) in &c.contract.projection {
            check(format!("project/{col}"), e)?;
        }
        for (id, e) in &c.contract.admits_stored {
            check(format!("admit_stored/{id}"), e)?;
        }
        for (col, e) in &c.contract.projection_stored {
            check(format!("project_stored/{col}"), e)?;
        }
        for d in &c.contract.derived {
            check(format!("derived/{}", d.column), &d.expr)?;
        }
        for e in &c.contract.enrich {
            check(format!("enrich/{}", e.field), &e.expr)?;
        }
        let expected = c.contract.admits.len()
            + c.contract.flags.len()
            + c.contract.projection.len()
            + c.contract.admits_stored.len()
            + c.contract.projection_stored.len()
            + c.contract.derived.len()
            + c.contract.enrich.len();
        if self.exprs.len() != expected {
            return Err(format!(
                "the bundle carries {} expressions; the contract compiles to {expected}",
                self.exprs.len()
            ));
        }
        let plan = self.validation_plan(&ctx.task_ctx())?;
        let want = portable_plan(&c.validation.plan, &c.contract.scan_schema)
            .map_err(|e| e.to_string())?;
        if plan.display_indent().to_string() != want.display_indent().to_string() {
            return Err(
                "the validation plan in the bundle differs from the recompiled contract".into(),
            );
        }
        Ok(c)
    }

    /// Decode the validation plan; bind it to data with [`crate::plan::bind_binding`].
    pub fn validation_plan(&self, task: &TaskContext) -> Result<LogicalPlan, String> {
        let bytes = hex::decode(&self.validation_plan).map_err(|e| e.to_string())?;
        let plan = logical_plan_from_bytes_with_extension_codec(&bytes, task, &BindingCodec)
            .map_err(|e| e.to_string())?;
        parcel_core::compile::resolve(plan).map_err(|e| e.to_string())
    }
}

/// A decoding session that also knows the user functions a compilation is pinned to.
pub fn session_with(c: &Compilation, registry: &Registry) -> SessionContext {
    let ctx = session();
    for pin in &c.contract.functions {
        if let Some(e) = registry
            .get(&pin.name)
            .filter(|e| e.hash == pin.hash && !e.is_builtin())
            && let Some(sig) = e.signatures.first()
        {
            let args = sig.args.iter().map(|t| t.to_arrow()).collect();
            ctx.register_udf(parcel_core::translate::user_function_udf(
                pin,
                args,
                sig.ret.to_arrow(),
            ));
        }
    }
    ctx
}

/// A session with parcel's own functions registered, for decoding.
pub fn session() -> SessionContext {
    let ctx = SessionContext::new();
    ctx.register_udf(parcel_core::translate::bytes_len_udf());
    ctx.register_udf(parcel_core::translate::bytes_len_udf());
    ctx
}

/// Replace the compile-time placeholder source with an empty provider `datafusion-proto` can encode.
fn portable_plan(plan: &LogicalPlan, schema: &Schema) -> DFResult<LogicalPlan> {
    let schema: SchemaRef = Arc::new(schema.clone());
    let out = plan.clone().transform_up(|node| {
        if let LogicalPlan::TableScan(ts) = &node
            && ts.table_name.table() == BINDING_TABLE
        {
            let empty: Arc<dyn TableProvider> = Arc::new(EmptyTable::new(schema.clone()));
            let scan = LogicalPlanBuilder::scan(
                TableReference::bare(BINDING_TABLE),
                provider_as_source(empty),
                None,
            )?
            .build()?;
            return Ok(Transformed::yes(scan));
        }
        Ok(Transformed::no(node))
    })?;
    out.data.recompute_schema()
}

/// Encodes the binding placeholder as an empty table of the row schema.
#[derive(Debug)]
struct BindingCodec;

impl LogicalExtensionCodec for BindingCodec {
    fn try_decode(&self, _: &[u8], _: &[LogicalPlan], _: &TaskContext) -> DFResult<Extension> {
        Err(DataFusionError::NotImplemented(
            "parcel bundles contain no extension nodes".into(),
        ))
    }
    fn try_encode(&self, _: &Extension, _: &mut Vec<u8>) -> DFResult<()> {
        Err(DataFusionError::NotImplemented(
            "parcel bundles contain no extension nodes".into(),
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
                "parcel bundles scan only the contract binding".into(),
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
                "parcel bundles scan only the contract binding".into(),
            ));
        }
        buf.extend_from_slice(BINDING_TABLE.as_bytes());
        Ok(())
    }
}

/// One column of a persisted Arrow schema. `type` is Arrow's display form, which parses back.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ColumnDef {
    pub name: String,
    #[serde(rename = "type")]
    pub data_type: String,
    pub nullable: bool,
}

pub fn schema_to_defs(schema: &Schema) -> Vec<ColumnDef> {
    schema
        .fields()
        .iter()
        .map(|f| ColumnDef {
            name: f.name().clone(),
            data_type: f.data_type().to_string(),
            nullable: f.is_nullable(),
        })
        .collect()
}

pub fn schema_from_defs(defs: &[ColumnDef]) -> Result<Schema, String> {
    use std::str::FromStr;
    let fields = defs
        .iter()
        .map(|d| {
            let t = datafusion::arrow::datatypes::DataType::from_str(&d.data_type)
                .map_err(|e| format!("column `{}`: {e}", d.name))?;
            Ok(datafusion::arrow::datatypes::Field::new(
                &d.name, t, d.nullable,
            ))
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(Schema::new(fields))
}

#[cfg(test)]
mod schema_tests {
    use super::*;
    use datafusion::arrow::datatypes::{DataType, Field, TimeUnit};

    #[test]
    fn schemas_round_trip() {
        let schema = Schema::new(vec![
            Field::new("a", DataType::Int32, false),
            Field::new(
                "b",
                DataType::List(Field::new_list_field(DataType::Utf8, true).into()),
                true,
            ),
            Field::new("c", DataType::Decimal128(18, 2), true),
            Field::new(
                "d",
                DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into())),
                true,
            ),
            Field::new("e", DataType::Date32, false),
        ]);
        assert_eq!(schema_from_defs(&schema_to_defs(&schema)).unwrap(), schema);
    }
}
