parcel_udf::export! {
    fn embedding_identity(v: [f32; 3]) -> [f32; 3] { v }
}

#[test]
fn an_embedding_function_declares_its_rust_dimension() {
    assert_eq!(embedding_identity([0.1, 0.2, 0.3]), [0.1, 0.2, 0.3]);
    assert_eq!(<[f32; 3] as parcel_udf::Ret>::DIMENSION, 3);
    assert_eq!(<[f32; 3] as parcel_udf::Arg>::DIMENSION, 3);
}

#[test]
fn the_embedding_builder_reports_malformed_outputs() {
    let mut builder = parcel_udf::Builder::embedding(1, 3);
    builder.push(0, Some(parcel_udf::Value::Embedding(vec![0.1, 0.2])));
    let bytes = builder.finish();
    assert_eq!(bytes[0], 1);
    assert!(
        std::str::from_utf8(&bytes[1..])
            .unwrap()
            .contains("dimension")
    );
    let bytes = parcel_udf::Builder::embedding(1, 0).finish();
    assert_eq!(bytes[0], 1);
}
