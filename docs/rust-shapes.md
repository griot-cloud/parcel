# Result shapes

`parcel_runtime::shape` applies suppression and aggregate noise to DataFusion plans. Active rules are selected with [active_shapes](rust-caller.md#evaluate-access).

## `apply`

```rust
pub fn apply(
    plan: LogicalPlan,
    optimized: &LogicalPlan,
    active: &[&ShapeRule],
) -> parcel_runtime::plan::Result<Shaped>;
```

| Parameter | Meaning |
| --- | --- |
| `plan` | Caller plan to rewrite. Consumed by this function. |
| `optimized` | The same plan after optimization, used to identify columns actually read. |
| `active` | Shape rules selected for this caller, potentially from multiple contracts. |

Returns the rewritten plan, privacy-budget charges, and suppression metadata. Unsupported plan shapes or invalid rewrites return `RuntimeError`.

## `Shaped`

| Field | Type | Meaning |
| --- | --- | --- |
| `plan` | `LogicalPlan` | Rewritten plan to execute. |
| `charges` | `Vec<Charge>` | Budget charges to apply before execution; the maximum epsilon for each budget. |
| `suppress_k` | `Option<u64>` | Suppression threshold, if any. |
| `has_aggregate` | `bool` | Whether the plan contains an aggregate. |

## `Charge`

| Field | Type | Meaning |
| --- | --- | --- |
| `budget` | `String` | Name of the privacy budget. |
| `epsilon` | `f64` | Amount charged to that budget. |

`apply` calculates charges; the application must check and record them before executing the plan.

## Suppression and inspection helpers

| Signature | Parameters and result |
| --- | --- |
| `suppress_ungrouped(batches: Vec<RecordBatch>, k: u64) -> Vec<RecordBatch>` | Treats all result batches as one group; removes the result when its total row count is below `k`. Consumes the batches. |
| `suppress_groups(plan: LogicalPlan, k: u64) -> plan::Result<LogicalPlan>` | Rewrites an aggregate plan to suppress groups below `k`. |
| `has_aggregate(plan: &LogicalPlan) -> bool` | Reports whether a plan contains an aggregate. |
| `columns_read(optimized: &LogicalPlan) -> Vec<String>` | Returns column names read by optimized scans. |

After executing `Shaped.plan`, use `suppress_ungrouped` when `has_aggregate` is false and `suppress_k` is set.
