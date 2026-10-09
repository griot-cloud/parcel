# Compiled rule types

These public structs are in `parcel_core::compile` and are returned within [Compilation](rust-compilation.md). They have no public constructors or methods of their own.

## `CelRule`

| Field | Type | Meaning |
| --- | --- | --- |
| `id` | `String` | Rule ID. |
| `cel` | `String` | Expression evaluated by the CEL interpreter. |

## `GuaranteeRule`

| Field | Type | Meaning |
| --- | --- | --- |
| `id` | `String` | Rule ID. |
| `cel` | `String` | Dataset requirement expression. |
| `on_fail` | `GuaranteeOnFail` | Action when false: `Deny` or `Annotate`. |
| `reads` | `Vec<String>` | Statistics-column names read by the expression. |

## `ShapeRule`

| Field | Type | Meaning |
| --- | --- | --- |
| `id` | `String` | Rule ID. |
| `shape` | `ShapeOp` | Typed shape operation. |
| `unless` | `Option<String>` | Optional caller condition that skips this shape. |

## `Flag`

| Field | Type | Meaning |
| --- | --- | --- |
| `assert_id` | `String` | Assertion ID. |
| `column` | `String` | Stored flag column name. |
| `expr` | `Expr` | DataFusion assertion expression. |
| `on_fail` | `AssertOnFail` | Action when false: `Drop`, `Deny`, or `Report`. |

## `RowRule`

| Field | Type | Meaning |
| --- | --- | --- |
| `id` | `String` | Rule ID. |
| `kind` | `RowRuleKind` | `Admit`, `Assert`, or `Transform { column: String }`. |
| `cel` | `String` | Reference CEL expression. |
| `reads` | `Vec<String>` | Columns read by value; nulls affect rule evaluation. |
| `ty` | `Type` | Expression return type. |

## `Derived`

| Field | Type | Meaning |
| --- | --- | --- |
| `column` | `String` | Stored calculation column name. |
| `cel` | `String` | Canonical CEL calculation. |
| `expr` | `Expr` | DataFusion calculation. |
| `ty` | `Type` | Result type. |

## `EnrichSpec`

| Field | Type | Meaning |
| --- | --- | --- |
| `field` | `String` | Custom row field name. |
| `cel` | `String` | CEL expression over raw columns. |
| `reads` | `Vec<String>` | Raw columns read. |
| `expr` | `Expr` | DataFusion calculation. |
| `ty` | `Type` | Result type. |

## `StatSpec`

| Field | Type | Meaning |
| --- | --- | --- |
| `field` | `DatasetField` | Typed dataset-field identifier. |
| `column` | `String` | Output name in the verdict row. |
| `path` | `String` | CEL field path, such as `dataset.amount.min`. |
| `ty` | `Type` | Statistic type. |

## `ShapeOp` and `NoiseAt`

`ShapeOp` is defined in `parcel_core::check`:

```rust
pub enum ShapeOp {
    Noise { column: String, sensitivity: f64, epsilon: f64, budget: String, at: NoiseAt },
    Suppress { k: u64 },
    Sample { fraction: f64, key: String },
}
pub enum NoiseAt { Aggregate, Row }
```

`column` identifies the noised output, `sensitivity` and `epsilon` control noise, and `budget` identifies the charged budget. `k` is the minimum group size. `fraction` is the sampling proportion and `key` determines the stable sample.
