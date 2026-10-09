# Compilation results

Types on this page are in `parcel_core::compile`. They expose public fields; compilation constructs them together. `SchemaRef` is a shared Arrow schema, and `Expr` and `LogicalPlan` are DataFusion types.

## `Compilation`

| Field | Type | Meaning |
| --- | --- | --- |
| `contract` | `CompiledContract` | Rules and column definitions used when applying the contract. |
| `validation` | `ValidationPlan` | Plan that computes quality results and statistics. |
| `write` | `WritePlan` | Calculations and storage layout for writing data under the contract. |

The [CompiledBytes](rust-artifacts.md#compiledbytes) trait provides `to_bytes` and `from_bytes` for saving and loading all three artifacts.

## `CompiledContract`

| Field | Type | Meaning |
| --- | --- | --- |
| `name` | `String` | Contract name. |
| `version` | `u32` | Contract revision. |
| `contract_hash` | `String` | Hash of the contract definition. |
| `compilation_hash` | `String` | Hash of this compilation. |
| `owner` | `Option<String>` | Resolved owner identifier. |
| `binding` | `Binding` | Resolved data location. |
| `residency` | `crate::Residency` | Resolved data-placement requirements. |
| `row_schema` | `SchemaRef` | Schema of the input data. |
| `exposed_schema` | `SchemaRef` | Schema of the exposed output. |
| `params` | `Vec<CtxParam>` | Caller-only expressions bound to DataFusion placeholders. |
| `decisions` | `Vec<CelRule>` | Caller decision rules in CEL. |
| `admits` | `Vec<(String, Expr)>` | Rule IDs paired with live row-filter expressions. |
| `flags` | `Vec<Flag>` | Quality assertions and their stored flag definitions. |
| `projection` | `Vec<(String, Expr)>` | Output column names paired with live expressions, in output order. |
| `guarantees` | `Vec<GuaranteeRule>` | Dataset requirements in CEL. |
| `shapes` | `Vec<ShapeRule>` | Result-shaping rules. |
| `functions` | `BTreeSet<FunctionPin>` | Exact function implementations required by the contract. |
| `report` | `Vec<ReportEntry>` | Per-rule compilation explanations. |
| `row_rules` | `Vec<RowRule>` | Reference CEL expressions used to compare evaluators. |
| `derived` | `Vec<Derived>` | Row calculations that can be stored at write time. |
| `scan_schema` | `SchemaRef` | Input schema including calculated custom row fields. |
| `enrich` | `Vec<EnrichSpec>` | Calculations producing custom row fields. |
| `ctx_other` | `BTreeMap<String, Type>` | Declared types of custom caller fields. |
| `admits_stored` | `Vec<(String, Expr)>` | Filters using stored calculated values; only valid for data written under this contract. |
| `projection_stored` | `Vec<(String, Expr)>` | Output expressions using stored calculated values. |

## `ValidationPlan`

| Field | Type | Meaning |
| --- | --- | --- |
| `plan` | `LogicalPlan` | DataFusion plan returning one validation-summary row. |
| `stats` | `Vec<StatSpec>` | Statistics computed by the plan. |
| `asserts` | `Vec<String>` | Assertion IDs with failure counts in the result. |
| `data_guarantees` | `Vec<String>` | Guarantee IDs evaluated from data alone. |
| `query_time_guarantees` | `Vec<String>` | Guarantee IDs requiring write metadata or request time. |

## `WritePlan`

| Field | Type | Meaning |
| --- | --- | --- |
| `enrich` | `Vec<EnrichSpec>` | Custom row calculations performed first. |
| `flags` | `Vec<Flag>` | Assertion results to store. |
| `derived` | `Vec<Derived>` | Reusable row calculations to store. |
| `layout` | `Layout` | Suggested clustering, partition, and Bloom filter columns. |
| `manifest_fields` | `Vec<String>` | Additional metadata keys required alongside statistics. |

## `Layout`

| Field | Type | Meaning |
| --- | --- | --- |
| `cluster_by` | `Vec<String>` | Stored flag columns used for clustering. |
| `partition_by` | `Vec<String>` | Partition columns. |
| `bloom` | `Vec<String>` | Columns identified for Bloom filters. |

## `ReportEntry` and `Tier`

| Field | Type | Meaning |
| --- | --- | --- |
| `rule` | `String` | Rule ID. |
| `op` | `String` | Operation name. |
| `tier` | `Tier` | Evaluation category. |
| `reason` | `String` | Explanation of the category. |
| `cel` | `Option<String>` | Canonical CEL expression, when present. |

| `Tier` variant | Meaning |
| --- | --- |
| `PlanTime` | Evaluated once per request without reading rows. |
| `Prunes` | Filter can use statistics to skip files or row groups. |
| `ScanFilter` | Evaluated row by row during scanning. |
| `WriteTime` | Calculated at write time and stored. |
| `Projection` | Calculated when its output column is selected. |
| `Operator` | Requires a physical operation. |

Supporting rule and statistic types are listed in {doc}`rust-compiled-rules`.

```{toctree}
:hidden:

rust-compiled-rules
```
