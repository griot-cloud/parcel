use datafusion_common::arrow::datatypes::{DataType, Field, Schema};
use parcel_core::registry::{Cost, FunctionEntry, FunctionManifest, parse_signature};
use parcel_core::types::{Type, exposable_as, parse_type_name};
use parcel_core::{ContractDoc, Registry, check_contract, compile};

fn doc(ty: &str, expr: &str) -> ContractDoc {
    ContractDoc::parse(&format!("contract: demo/vectors\nversion: 1\nowner: demo\nbinding: {{parquet: vectors/}}\nexpose: [{{name: embedding, type: '{ty}'}}]\nrules:\n  - {{id: output, op: transform, column: embedding, expr: '{expr}'}}\n")).unwrap()
}
fn schema(n: i32) -> Schema {
    Schema::new(vec![Field::new(
        "embedding",
        Type::Embedding(n).to_arrow(),
        true,
    )])
}

#[test]
fn embedding_type_retains_element_width_and_dimension() {
    let t = parse_type_name("fixed_size_list<float32, 3>").unwrap();
    assert_eq!(Type::from_arrow(&t).unwrap(), Type::Embedding(3));
    assert_eq!(
        Type::Embedding(3).to_string(),
        "fixed_size_list<float32, 3>"
    );
    assert!(!exposable_as(&t, &Type::Embedding(4).to_arrow()));
    for ty in [
        "fixed_size_list<float64, 3>",
        "fixed_size_list<int32, 3>",
        "fixed_size_list<float32, 0>",
        "fixed_size_list<float32, -1>",
        "fixed_size_list<float32, 2147483648>",
    ] {
        assert!(parse_type_name(ty).is_err(), "{ty}");
        assert!(
            check_contract(&doc(ty, "row.embedding"), &schema(3), &Registry::builtin()).is_err(),
            "{ty}"
        );
    }
    assert!(
        Type::from_arrow(&DataType::FixedSizeList(
            Field::new_list_field(DataType::Int32, true).into(),
            3
        ))
        .is_err()
    );
    assert!(
        Type::from_arrow(&DataType::FixedSizeList(
            Field::new_list_field(DataType::Float32, true).into(),
            0
        ))
        .is_err()
    );
}

#[test]
fn producer_dimension_must_equal_column_dimension() {
    let mut registry = Registry::builtin();
    let manifest = FunctionManifest {
        name: "embed".into(),
        version: 1,
        signatures: vec!["(fixed_size_list<float32, 3>) -> fixed_size_list<float32, 4>".into()],
        deterministic: true,
        cost: Cost::Moderate,
    };
    registry.insert(FunctionEntry::user(&manifest, "demo", "module").unwrap());
    let errors = check_contract(
        &doc("fixed_size_list<float32, 3>", "embed(row.embedding)"),
        &schema(3),
        &registry,
    )
    .unwrap_err();
    assert!(format!("{errors:?}").contains("TypeMismatch"));
    let mut manifest = manifest;
    manifest.signatures[0] = "(fixed_size_list<float32, 3>) -> fixed_size_list<float32, 3>".into();
    registry.insert(FunctionEntry::user(&manifest, "demo", "module").unwrap());
    let compilation = compile(
        &doc("fixed_size_list<float32, 3>", "embed(row.embedding)"),
        &schema(3),
        &registry,
    )
    .unwrap();
    assert_eq!(compilation.contract.functions.len(), 1);
    assert_eq!(
        compilation
            .contract
            .exposed_schema
            .field(0)
            .data_type()
            .clone(),
        Type::Embedding(3).to_arrow()
    );
}

#[test]
fn signatures_split_argument_commas_without_splitting_dimensions() {
    let signature = parse_signature("(fixed_size_list<float32, 3>, int, fixed_size_list<float32, 4>) -> fixed_size_list<float32, 7>").unwrap();
    assert_eq!(
        signature.args,
        vec![Type::Embedding(3), Type::Int, Type::Embedding(4)]
    );
    assert_eq!(signature.ret, Type::Embedding(7));
    for signature in [
        "() -> fixed_size_list<float64, 3>",
        "() -> fixed_size_list<float32, 0>",
    ] {
        assert!(parse_signature(signature).is_err());
    }
}
