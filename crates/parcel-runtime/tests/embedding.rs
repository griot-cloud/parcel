use cel::Value;
use datafusion::arrow::array::{Array, FixedSizeListArray, Float32Array, RecordBatch};
use datafusion::arrow::datatypes::{DataType, Field, Schema};
use parcel_core::compile::Compilation;
use parcel_core::types::Type;
use parcel_core::{ContractDoc, Registry, compile};
use parcel_runtime::compiled::CompiledBytes;
use parcel_runtime::{Caller, differential::differential, reference};
use std::sync::Arc;

fn schema() -> Schema {
    Schema::new(vec![Field::new(
        "embedding",
        Type::Embedding(3).to_arrow(),
        true,
    )])
}
fn document() -> ContractDoc {
    ContractDoc::parse("contract: demo/vectors\nversion: 1\nbinding: {parquet: vectors/}\nexpose: [{name: embedding, type: 'fixed_size_list<float32, 3>'}]\nrules:\n  - {id: dimensions, op: assert, expr: 'size(row.embedding) == 3', on_fail: report}\n  - {id: retain, op: transform, column: embedding, expr: 'row.embedding'}").unwrap()
}

#[test]
fn cel_scalar_round_trip_keeps_float32_and_refuses_wrong_dimension_or_elements() {
    let value =
        reference::json_value(&serde_json::json!([0.1, 0.2, 0.3]), &Type::Embedding(3)).unwrap();
    let scalar = reference::to_scalar(&value, &Type::Embedding(3)).unwrap();
    assert_eq!(scalar.data_type(), Type::Embedding(3).to_arrow());
    assert_eq!(reference::from_scalar(&scalar), Some(value.clone()));
    assert!(reference::to_scalar(&value, &Type::Embedding(4)).is_err());
    assert!(reference::json_value(&serde_json::json!([1, 2]), &Type::Embedding(3)).is_none());
    assert!(reference::json_value(&serde_json::json!([1, "x", 3]), &Type::Embedding(3)).is_none());
    assert!(
        reference::to_scalar(
            &Value::List(Arc::new(vec![Value::Int(1); 3])),
            &Type::Embedding(3)
        )
        .is_err()
    );
}

#[test]
fn compiled_bytes_preserve_embedding_schema_and_hashes() {
    let doc = document();
    let compilation = compile(&doc, &schema(), &Registry::builtin()).unwrap();
    let back = Compilation::from_bytes(&compilation.to_bytes().unwrap()).unwrap();
    assert_eq!(
        back.contract.exposed_schema.field(0).data_type().clone(),
        Type::Embedding(3).to_arrow()
    );
    assert_eq!(
        back.contract.compilation_hash,
        compilation.contract.compilation_hash
    );
}

#[tokio::test]
async fn cel_and_datafusion_agree_for_embedding_rules_and_null_rows() {
    let compilation = compile(&document(), &schema(), &Registry::builtin()).unwrap();
    let vectors = FixedSizeListArray::try_new(
        Arc::new(Field::new_list_field(DataType::Float32, true)),
        3,
        Arc::new(Float32Array::from(vec![0.1, 0.2, 0.3, 0.0, 0.0, 0.0])),
        Some(vec![true, false].into()),
    )
    .unwrap();
    assert!(vectors.is_null(1));
    let batch = RecordBatch::try_new(Arc::new(schema()), vec![Arc::new(vectors)]).unwrap();
    let report = differential(
        &compilation,
        &batch,
        &[Caller::new("alice", "demo", "test")],
    )
    .await
    .unwrap();
    assert!(report.passed(), "{report:?}");
    assert!(report.evaluations > 0);
}

#[cfg(feature = "wasm")]
fn identity_module() -> Vec<u8> {
    wat::parse_str(r#"(module
      (memory (export "memory") 1)
      (func (export "parcel_abi_version") (result i32) i32.const 1)
      (func (export "parcel_alloc") (param i32) (result i32) i32.const 8192)
      (func (export "parcel_free") (param i32 i32))
      (func (export "parcel_fn_embed_identity") (param $ptr i32) (param $len i32) (result i64)
        i32.const 4096 i32.const 0 i32.store8
        i32.const 4097 local.get $ptr i32.const 8 i32.add local.get $len i32.const 8 i32.sub memory.copy
        i64.const 4096 i64.const 32 i64.shl
        local.get $len i32.const 7 i32.sub i64.extend_i32_u i64.or))"#).unwrap()
}

#[cfg(feature = "wasm")]
#[tokio::test]
async fn wasm_embedding_is_checked_and_runs_on_both_engines_and_from_compiled_bytes() {
    use parcel_core::registry::{Cost, FunctionManifest};
    use parcel_runtime::wasm::install;
    let module = identity_module();
    let manifest = FunctionManifest {
        name: "embed_identity".into(),
        version: 1,
        signatures: vec!["(fixed_size_list<float32, 3>) -> fixed_size_list<float32, 3>".into()],
        deterministic: true,
        cost: Cost::Moderate,
    };
    let mut registry = Registry::builtin();
    registry.insert(install(&module, &manifest, "demo").unwrap());
    let source = "contract: demo/vectors\nversion: 1\nowner: demo\nbinding: {parquet: vectors/}\nexpose: [{name: embedding, type: 'fixed_size_list<float32, 3>'}]\nrules:\n  - {id: produce, op: transform, column: embedding, expr: 'embed_identity(row.embedding)'}";
    let doc = ContractDoc::parse(source).unwrap();
    let c = compile(&doc, &schema(), &registry).unwrap();
    let vectors = FixedSizeListArray::try_new(
        Arc::new(Field::new_list_field(DataType::Float32, true)),
        3,
        Arc::new(Float32Array::from(vec![0.1, 0.2, 0.3, 0.0, 0.0, 0.0])),
        Some(vec![true, false].into()),
    )
    .unwrap();
    let batch = RecordBatch::try_new(Arc::new(schema()), vec![Arc::new(vectors)]).unwrap();
    let report = differential(&c, &batch, &[Caller::new("alice", "demo", "test")])
        .await
        .unwrap();
    assert!(report.passed(), "{report:?}");
    let back = Compilation::from_bytes(&c.to_bytes().unwrap()).unwrap();
    let report = differential(&back, &batch, &[Caller::new("alice", "demo", "test")])
        .await
        .unwrap();
    assert!(report.passed(), "{report:?}");
    let mut wrong = manifest;
    wrong.signatures[0] = "(fixed_size_list<float32, 3>) -> fixed_size_list<float32, 4>".into();
    assert!(
        install(&module, &wrong, "demo")
            .unwrap_err()
            .contains("dimension")
    );
    wrong.signatures[0] =
        "(fixed_size_list<float32, 3>) -> fixed_size_list<float32, 2147483647>".into();
    assert!(
        install(&module, &wrong, "demo")
            .unwrap_err()
            .contains("memory limit")
    );
}
