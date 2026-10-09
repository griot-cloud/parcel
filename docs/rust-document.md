# Contract documents

`parcel_core::ContractDoc` represents a parsed YAML or JSON contract. Related types are in `parcel_core::document`.

## `ContractDoc::parse`

```rust
pub fn parse(source: &str) -> Result<ContractDoc, Diagnostic>;
```

`source` is document text, not a filename. Returns the parsed document or a diagnostic for invalid syntax or document structure. Column and expression checking happens during `check_contract` or `compile`.

```rust
use parcel_core::ContractDoc;

let source = std::fs::read_to_string("orders.yaml")?;
let doc = ContractDoc::parse(&source)?;
println!("{} v{}", doc.contract, doc.version);
```

The example is intended inside a function returning `Result`; both file errors and `Diagnostic` implement standard error reporting.

## Public fields

| Field | Type | Meaning |
| --- | --- | --- |
| `contract` | `String` | Contract name. |
| `version` | `u32` | Contract revision. |
| `owner` | `Option<String>` | Owner of the data; selects the owner-specific functions available to the contract. |
| `inherits` | `Option<String>` | Parent contract name. |
| `binding` | `Option<Binding>` | Data location; may be inherited. |
| `residency` | `Option<crate::Residency>` | Optional data-placement requirements. |
| `expose` | `Option<Vec<ExposeColumn>>` | Output columns; may be inherited. |
| `rules` | `Vec<Rule>` | Rules in document order. |
| `extensions` | `Option<Extensions>` | Types for custom namespace fields. |
| `enrich` | `Option<Vec<Enricher>>` | Calculations producing custom row fields. |
| `dataset_other` | `Option<Vec<DatasetProducer>>` | Constants or aggregates producing custom dataset fields. |

## Data location and columns

| Type | Public fields or variants | Methods |
| --- | --- | --- |
| `Binding` | `source: Source`, `partitioned_by: Vec<String>` | — |
| `Source` | `Parquet(String)`, `Iceberg(IcebergTable)` | — |
| `IcebergTable` | `namespace: Vec<String>`, `table: String` | `parse(s: &str) -> Result<IcebergTable, String>` validates a dotted table identifier. |
| `ExposeColumn` | `name: String`, `type_name: String` | — |
| `Extensions` | `row`, `ctx`, `dataset`: each `BTreeMap<String, String>` mapping field names to type names. | `default()` creates empty maps. |
| `Enricher` | `field: String`, `expr: String` | — |
| `DatasetProducer` | `field: String`, `value: Option<serde_json::Value>`, `expr: Option<String>` | — |

## Rules

`Rule` has six variants, each wrapping its matching document type:

| Variant | Wrapped type and public fields |
| --- | --- |
| `Decide` | `DecideRule { id: String, expr: String }` |
| `Admit` | `AdmitRule { id: String, expr: String }` |
| `Assert` | `AssertRule { id: String, expr: String, on_fail: AssertOnFail }` |
| `Transform` | `TransformRule { id: String, column: String, expr: String }` |
| `Guarantee` | `GuaranteeRule { id: String, expr: String, on_fail: GuaranteeOnFail }` |
| `Shape` | `ShapeRule { id: String, operator: String, column: Option<String>, params: serde_json::Map<String, serde_json::Value>, unless: Option<String> }` |

`AssertOnFail` variants are `Drop`, `Deny`, and `Report`. `GuaranteeOnFail` variants are `Deny` and `Annotate`.

```rust
impl Rule {
    pub fn id(&self) -> &str;
    pub fn op_name(&self) -> &'static str;
}
```

`id` borrows the rule's identifier. `op_name` returns its operation name, such as `"assert"`. For each operation's behavior and examples, see {doc}`contract-rules`.

## `json_schema`

```rust
pub fn json_schema() -> serde_json::Value;
```

Returns the JSON Schema describing contract document structure, for editor and tooling integration.
