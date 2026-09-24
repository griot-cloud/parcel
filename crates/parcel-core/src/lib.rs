//! parcel: the contract language and compiler for peQL.
//!
//! A contract is a YAML/JSON document whose rules are CEL expressions. parcel
//! checks it against the Arrow schema of the data it binds and compiles it into
//! artifacts a DataFusion-based engine can enforce. This crate performs no I/O.
//!
//! ```
//! use parcel_core::{ContractDoc, Registry, check_contract};
//! use datafusion_common::arrow::datatypes::{DataType, Field, Schema};
//!
//! let doc = ContractDoc::parse(r#"
//! contract: demo/people
//! version: 1
//! binding: {parquet: "file:///data/people/"}
//! expose: [{name: name, type: utf8}]
//! rules:
//!   - {id: adults, op: admit, expr: "row.age >= 18"}
//! "#).unwrap();
//! let schema = Schema::new(vec![
//!     Field::new("name", DataType::Utf8, true),
//!     Field::new("age", DataType::Int32, true),
//! ]);
//! let checked = check_contract(&doc, &schema, &Registry::builtin()).unwrap();
//! assert_eq!(checked.rules.len(), 1);
//! ```

pub mod cel_print;
pub mod check;
pub mod checker;
pub mod compile;
pub mod diag;
pub mod document;
pub mod hash;
pub mod inherit;
pub mod ir;
pub mod registry;
pub mod translate;
pub mod types;

pub use check::{
    CheckedContract, CheckedExpr, CheckedRule, ShapeOp, check_contract, contract_hash,
};
pub use compile::{
    Compilation, CompiledContract, ValidationPlan, WritePlan, compile, compile_with,
};
pub use diag::{Code, Diagnostic};
pub use document::ContractDoc;
pub use registry::Registry;
