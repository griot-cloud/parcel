//! The customers example compiled the way `parcel compile` does, for the target tests.
#![allow(dead_code)]

use datafusion::arrow::datatypes::{DataType, Field, Schema};
use parcel_core::{Compilation, ContractDoc, Registry};

pub const CUSTOMERS: &str = include_str!("../../../examples/customers/customers.yaml");

pub fn schema() -> Schema {
    let s = |n: &str| Field::new(n, DataType::Utf8, true);
    Schema::new(vec![
        s("customer_id"),
        s("customer_name"),
        Field::new("customer_age", DataType::Int64, true),
        s("gender"),
        s("customer_segment"),
        s("customer_city"),
        s("customer_state"),
        s("customer_country"),
        s("region"),
        s("customer_postal_code"),
        Field::new("customer_acquisition_cost", DataType::Float64, true),
    ])
}

pub fn compiled(source: &str) -> Compilation {
    let doc = ContractDoc::parse(source).unwrap();
    parcel_core::compile(&doc, &schema(), &Registry::builtin()).unwrap_or_else(|d| panic!("{d:#?}"))
}
