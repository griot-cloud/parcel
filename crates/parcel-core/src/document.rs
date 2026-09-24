//! The contract document: what an author writes, as YAML or JSON.
//!
//! This module only deserialises. Nothing here knows about schemas, types or
//! CEL; that is the checker's job.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::diag::{Code, Diagnostic};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ContractDoc {
    /// Contract name, e.g. `sales/orders`. This is what callers put in `FROM`.
    pub contract: String,
    pub version: u32,
    /// The tenant that owns the contract: its registered functions are callable here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inherits: Option<String>,
    /// Where the data is. A child contract may omit it and inherit its parent's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binding: Option<Binding>,
    /// The caller's schema. A child contract may omit it (inherit the parent's) or narrow it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expose: Option<Vec<ExposeColumn>>,
    #[serde(default)]
    pub rules: Vec<Rule>,
    /// Declared shapes of each namespace's `other` field (design 3.1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<Extensions>,
    /// Write-time producers of `row.other` fields (design 10, stage 1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enrich: Option<Vec<Enricher>>,
    /// Write-time producers of `dataset.other` fields (design 10, stage 4).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dataset_other: Option<Vec<DatasetProducer>>,
}

/// Field name → type name, per namespace.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Extensions {
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub row: std::collections::BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub ctx: std::collections::BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub dataset: std::collections::BTreeMap<String, String>,
}

/// Computes one `row.other` field from the row, once, at write.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Enricher {
    pub field: String,
    pub expr: String,
}

/// Produces one `dataset.other` field at write: a constant, or an aggregate such as `avg(row.amount)`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DatasetProducer {
    pub field: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expr: Option<String>,
}

/// Where the data physically is. Callers never see this.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    pub parquet: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub partitioned_by: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExposeColumn {
    pub name: String,
    #[serde(rename = "type")]
    pub type_name: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "op", rename_all = "lowercase")]
pub enum Rule {
    Decide(DecideRule),
    Admit(AdmitRule),
    Assert(AssertRule),
    Transform(TransformRule),
    Guarantee(GuaranteeRule),
    Shape(ShapeRule),
}

impl Rule {
    pub fn id(&self) -> &str {
        match self {
            Rule::Decide(r) => &r.id,
            Rule::Admit(r) => &r.id,
            Rule::Assert(r) => &r.id,
            Rule::Transform(r) => &r.id,
            Rule::Guarantee(r) => &r.id,
            Rule::Shape(r) => &r.id,
        }
    }

    pub fn op_name(&self) -> &'static str {
        match self {
            Rule::Decide(_) => "decide",
            Rule::Admit(_) => "admit",
            Rule::Assert(_) => "assert",
            Rule::Transform(_) => "transform",
            Rule::Guarantee(_) => "guarantee",
            Rule::Shape(_) => "shape",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DecideRule {
    pub id: String,
    pub expr: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AdmitRule {
    pub id: String,
    pub expr: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum AssertOnFail {
    Drop,
    Deny,
    Report,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AssertRule {
    pub id: String,
    pub expr: String,
    pub on_fail: AssertOnFail,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TransformRule {
    pub id: String,
    pub column: String,
    pub expr: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum GuaranteeOnFail {
    Deny,
    Annotate,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GuaranteeRule {
    pub id: String,
    pub expr: String,
    pub on_fail: GuaranteeOnFail,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShapeRule {
    pub id: String,
    pub operator: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<String>,
    #[serde(default)]
    pub params: serde_json::Map<String, Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unless: Option<String>,
}

/// The JSON Schema of a contract document, for editors and CI.
pub fn json_schema() -> Value {
    let mut v =
        serde_json::to_value(schemars::schema_for!(ContractDoc)).expect("schemas serialise");
    if let Some(o) = v.as_object_mut() {
        o.insert("title".into(), "parcel contract".into());
    }
    v
}

impl ContractDoc {
    /// Parse a contract from YAML or JSON (JSON is valid YAML).
    pub fn parse(source: &str) -> Result<ContractDoc, Diagnostic> {
        yaml_serde::from_str(source)
            .map_err(|e| Diagnostic::new(Code::Document, None, e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unknown_rule_fields() {
        let src = r#"
contract: t
version: 1
binding: {parquet: "file:///tmp/x"}
expose: []
rules:
  - {id: a, op: decide, expr: "true", on_fail: deny}
"#;
        let err = ContractDoc::parse(src).unwrap_err();
        assert_eq!(err.code, Code::Document);
    }

    #[test]
    fn json_is_accepted() {
        let src = r#"{"contract":"t","version":1,"binding":{"parquet":"x"},"expose":[],
            "rules":[{"id":"a","op":"admit","expr":"row.x > 1"}]}"#;
        let doc = ContractDoc::parse(src).unwrap();
        assert!(matches!(doc.rules[0], Rule::Admit(_)));
    }
}
