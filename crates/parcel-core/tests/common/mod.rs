#![allow(dead_code)]
use datafusion_common::arrow::datatypes::{DataType, Field, Schema};

pub const SALES_ORDERS: &str = include_str!("../fixtures/sales_orders.yaml");

pub fn orders_schema() -> Schema {
    Schema::new(vec![
        Field::new("order_id", DataType::Int64, true),
        Field::new("customer_id", DataType::Int64, true),
        Field::new("email", DataType::Utf8, true),
        Field::new("msisdn", DataType::Utf8, true),
        Field::new("tenant_id", DataType::Utf8, false),
        Field::new("unit_price_cents", DataType::Int64, true),
        Field::new("qty", DataType::Int32, true),
        Field::new("amount_cents", DataType::Int64, true),
        Field::new("amount", DataType::Decimal128(18, 2), true),
        Field::new(
            "tags",
            DataType::List(Field::new_list_field(DataType::Utf8, true).into()),
            true,
        ),
        Field::new("region", DataType::Utf8, false),
        Field::new("dt", DataType::Date32, false),
    ])
}
