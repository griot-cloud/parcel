# Runtime: `parcel-runtime`

These functions are building blocks for an engine that enforces a compiled contract. The engine must authenticate callers, register functions, store data and invoke the checks on its read/write paths.

## `Caller`

```rust
Caller::new(id: &str, tenant: &str, purpose: &str) -> Caller
caller.with_roles(roles: &[&str]) -> Caller
caller.at(now: chrono::DateTime<chrono::Utc>) -> Caller
```

`new` sets `tier` and `classification` to empty strings, `clearance` to `0`, `roles` to empty, `now` to the current UTC time, and `other` to an empty map. Builder methods consume and return the caller. Public fields let you set `tier`, `classification`, `clearance` and `other` directly. `other` becomes declared `ctx.other` fields when a rule uses them. These values must come from your authentication layer.

## Validation

```rust
plan::session() -> SessionContext
plan::validate(
    c: &Compilation, validation_plan: LogicalPlan, data: Arc<dyn TableProvider>,
) -> plan::Result<Verdict>  // async
plan::validate_in(
    ctx: &SessionContext, c: &Compilation,
    validation_plan: LogicalPlan, data: Arc<dyn TableProvider>,
) -> plan::Result<Verdict>  // async
```

`session` registers parcel's functions in a DataFusion session. `validate` creates that session and runs the plan against `data`; `validate_in` uses your session, which is needed when the data relies on object stores registered there. Both replace the plan's placeholder table and return `Verdict` with `contract`, hashes, `valid`, `breached`, `row_count`, assertion `failures`, `guarantees` and `stats`. `plan::RuntimeError` has `Invalid` and `DataFusion` variants.

## Caller and dataset values

| Function signature | Result |
| --- | --- |
| `plan::param_values(cc: &CompiledContract, caller: &Caller) -> Result<ParamValues>` | Evaluate caller-context placeholders for the compiled plan. |
| `plan::refusal(cc: &CompiledContract, caller: &Caller) -> Result<Option<String>>` | First `decide` rule id refusing this caller, or `None`. Call before opening data. |
| `plan::active_shapes<'a>(cc: &'a CompiledContract, caller: &Caller) -> Result<Vec<&'a ShapeRule>>` | Shape rules whose `unless` condition does not hold. |
| `plan::dataset_value(c: &Compilation, stats: &BTreeMap<String, serde_json::Value>, written_at: DateTime<Utc>, contract_hash: &str) -> cel::Value` | Build the dataset namespace from stored stats and write metadata. |
| `plan::bind_binding(plan: LogicalPlan, cc: &CompiledContract, data: Arc<dyn TableProvider>) -> Result<LogicalPlan>` | Replace the validation placeholder with real data and apply enrichment if needed. |

These calls do not themselves gate a user's SQL query. An embedding engine must bind parameters and insert the contract view into its own query plan.

## Result shapes and comparison

```rust
shape::apply(
    plan: LogicalPlan, optimized: &LogicalPlan, active: &[&ShapeRule],
) -> plan::Result<Shaped>
shape::suppress_ungrouped(batches: Vec<RecordBatch>, k: u64) -> Vec<RecordBatch>
differential::differential(
    c: &Compilation, batch: &RecordBatch, callers: &[Caller],
) -> plan::Result<DiffReport>  // async
```

`shape::apply` rewrites aggregate noise and suppression and returns `Shaped { plan, charges, suppress_k, has_aggregate }`. Each `Charge` has `budget` and `epsilon`; the embedding engine must enforce and record those charges. `suppress_ungrouped` applies the threshold after executing an ungrouped result. `differential` compares CEL and DataFusion rule results on a sample for each caller. `DiffReport` contains row/caller/evaluation counts and `mismatches`; `passed() -> bool` is true when there are none.
