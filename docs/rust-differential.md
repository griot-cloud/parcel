# Comparing rule evaluators

`parcel_runtime::differential` compares the CEL interpreter and DataFusion on the same data and callers. This is the comparison used by `parcel check`.

## `differential`

```rust
pub async fn differential(
    c: &Compilation,
    batch: &RecordBatch,
    callers: &[Caller],
) -> parcel_runtime::plan::Result<DiffReport>;
```

`c` supplies compiled rules, `batch` supplies the rows to test, and `callers` supplies each context. The batch is conformed to the contract's row schema. Schema conversion and execution failures return `RuntimeError`; differing evaluation results are recorded in the returned report.

For existing `artifacts`, `batch`, and `caller` values:

```rust
let report = parcel_runtime::differential::differential(
    &artifacts, &batch, &[caller],
).await?;
assert!(report.passed(), "{:?}", report.mismatches);
```

## `DiffReport`

| Field | Type | Meaning |
| --- | --- | --- |
| `rows` | `usize` | Rows tested. |
| `callers` | `usize` | Caller profiles tested. |
| `evaluations` | `usize` | Individual comparisons performed. |
| `both_errored` | `usize` | Comparisons where both evaluators rejected the input, such as division by zero. |
| `mismatches` | `Vec<Mismatch>` | Comparisons whose results differ. |

```rust
impl DiffReport {
    pub fn passed(&self) -> bool;
}
```

Returns `true` when `mismatches` is empty. It can be true when `both_errored` is nonzero: matching errors count as agreement.

## `Mismatch`

| Field | Type | Meaning |
| --- | --- | --- |
| `rule` | `String` | Rule or calculated-field identifier. |
| `caller` | `String` | Caller label for this comparison. |
| `row` | `usize` | Zero-based row index in the tested batch. |
| `reference` | `String` | CEL interpreter result as text. |
| `datafusion` | `String` | DataFusion result as text. |
