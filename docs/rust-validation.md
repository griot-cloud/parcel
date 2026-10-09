# Data validation

Functions and types on this page are in `parcel_runtime::plan`. They use DataFusion's `LogicalPlan`, `SessionContext`, `TableProvider`, and Arrow types.

## `session`

```rust
pub fn session() -> SessionContext;
```

Creates a DataFusion session with parcel's built-in functions registered. Use it as the starting point when registering object stores or other session resources.

## `validate` and `validate_in`

```rust
pub async fn validate(
    c: &Compilation,
    validation_plan: LogicalPlan,
    data: Arc<dyn TableProvider>,
) -> Result<Verdict>;

pub async fn validate_in(
    ctx: &SessionContext,
    c: &Compilation,
    validation_plan: LogicalPlan,
    data: Arc<dyn TableProvider>,
) -> Result<Verdict>;
```

| Parameter | Meaning |
| --- | --- |
| `c` | Compilation that produced the validation plan. |
| `validation_plan` | Plan to execute, normally `c.validation.plan.clone()`. Passed by value. |
| `data` | Table provider supplying the contract's columns. Shared through `Arc`. |
| `ctx` | Existing session with parcel functions registered; used only by `validate_in`. |

Both bind the plan to `data`, execute it, and read its summary row into a `Verdict`. `validate` creates its own session. `validate_in` uses the supplied session, including its registered object stores.

A failed data quality requirement is returned as a verdict with `valid: false`. Input or execution failures return `RuntimeError`.

### Example

This helper accepts an already compiled contract and a DataFusion table provider:

```rust
use std::sync::Arc;
use datafusion::datasource::TableProvider;
use parcel_core::Compilation;
use parcel_runtime::plan::{self, Verdict};

async fn check_table(
    artifacts: &Compilation,
    data: Arc<dyn TableProvider>,
) -> plan::Result<Verdict> {
    plan::validate(artifacts, artifacts.validation.plan.clone(), data).await
}
```

## `Verdict`

All fields are public. The type implements `Clone`, `Debug`, `PartialEq`, and `Serialize`.

| Field | Type | Meaning |
| --- | --- | --- |
| `contract` | `String` | Contract name. |
| `contract_hash` | `String` | Hash of the contract definition. |
| `compilation_hash` | `String` | Hash identifying this compilation. |
| `valid` | `bool` | Whether the dataset passes requirements that can invalidate it. |
| `breached` | `Vec<String>` | IDs of breached requirements. |
| `row_count` | `i64` | Number of rows checked. |
| `failures` | `BTreeMap<String, i64>` | Assertion ID to failing-row count. |
| `guarantees` | `BTreeMap<String, bool>` | Data-only guarantee ID to its Boolean result. |
| `stats` | `BTreeMap<String, serde_json::Value>` | Computed statistics keyed by output-column name; exact decimals are strings. |

## `selectivity`

```rust
pub async fn selectivity(
    c: &Compilation,
    data: Arc<dyn TableProvider>,
    caller: &Caller,
) -> Result<usize>;
```

Counts rows in `data` that pass the compiled `admit` rules and assertions with `on_fail: drop`, using the supplied caller. `c` provides the rules and enrichment expressions. Returns the count, not the rows. Call `refusal` first when `decide` rules must also be tested; `selectivity` does not evaluate them.

## Plan and batch helpers

| Signature | Parameters and result |
| --- | --- |
| `bind_binding(plan: LogicalPlan, cc: &CompiledContract, data: Arc<dyn TableProvider>) -> Result<LogicalPlan>` | Replaces the plan's placeholder scan with `data`, projects the required columns, and applies enrichment when needed. |
| `enrich_plan(input: LogicalPlan, cc: &CompiledContract) -> Result<LogicalPlan>` | Adds calculated `row.other` values to `input` using `cc.enrich`. Returns the input unchanged when there are no enrichers. |
| `conform(b: RecordBatch, schema: &SchemaRef) -> Result<RecordBatch>` | Selects and casts columns in `b` to match `schema`. Missing columns or failed casts return an error. |
| `col_ref(name: &str) -> Expr` | Creates an unqualified column expression preserving the supplied name. |
| `scalar_json(s: &ScalarValue) -> serde_json::Value` | Converts a DataFusion scalar to the JSON representation used for statistics. |
| `parse_decimal(text: &str, scale: i8) -> Option<i64>` | Converts decimal text to a scaled integer; `"100.50"` at scale `2` becomes `Some(10050)`. Use a non-negative scale. Invalid text, excess fractional digits, or overflow return `None`. |

## `dataset_value`

```rust
pub fn dataset_value(
    c: &Compilation,
    stats: &BTreeMap<String, serde_json::Value>,
    written_at: chrono::DateTime<chrono::Utc>,
    contract_hash: &str,
) -> cel::Value;
```

Builds the `dataset` namespace for rule evaluation. `c` defines the required statistics and types, `stats` supplies their values (normally from `Verdict.stats`), and the remaining arguments supply the data's write time and contract hash. Missing or unconvertible statistics are omitted from the resulting CEL value.

## `RuntimeError` and `Result`

```rust
pub enum RuntimeError {
    Invalid(String),
    DataFusion(datafusion::error::DataFusionError),
}
pub type Result<T> = std::result::Result<T, RuntimeError>;
```

`Invalid` describes invalid inputs or parcel evaluation failures. `DataFusion` wraps planning and execution failures. Both implement `Display` and `std::error::Error`.
