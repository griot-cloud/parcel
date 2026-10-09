# Caller context

`parcel_runtime::Caller` supplies the values rules read through `ctx`. Construct a caller for a person, agent, or service; tests can use mock values.

## `Caller` fields

| Field | Type | Meaning |
| --- | --- | --- |
| `id` | `String` | Caller identifier. |
| `tenant` | `String` | Organization identifier. |
| `purpose` | `String` | Purpose of the request. |
| `tier` | `String` | Application-defined service or access tier. |
| `clearance` | `i64` | Numeric clearance level. |
| `classification` | `String` | Data classification the caller is cleared for. |
| `roles` | `Vec<String>` | Role names. |
| `now` | `DateTime<Utc>` | Time used to evaluate the request. |
| `other` | `std::collections::BTreeMap<String, serde_json::Value>` | Custom values declared under `extensions.ctx`. |

## Constructors and methods

```rust
impl Caller {
    pub fn new(id: &str, tenant: &str, purpose: &str) -> Caller;
    pub fn with_roles(self, roles: &[&str]) -> Caller;
    pub fn at(self, now: chrono::DateTime<chrono::Utc>) -> Caller;
}
```

| Method | Arguments and result |
| --- | --- |
| `new` | Copies the three identifiers into a new caller. Other strings are empty, clearance is `0`, roles and custom values are empty, and `now` is the current UTC time. |
| `with_roles` | Replaces the role list with the supplied names. Consumes and returns the caller. |
| `at` | Replaces `now` with a fixed time. Consumes and returns the caller. Useful for repeatable time-based tests. |

These methods are infallible. Fields are public and can also be changed directly.

## Example

```rust
use parcel_runtime::Caller;

let mut caller = Caller::new("supplier-analyst", "globex", "purchasing")
    .with_roles(&["analyst"]);
caller.clearance = 2;
caller.classification = "internal".into();

assert_eq!(caller.tenant, "globex");
```

A rule such as `row.supplier_id == ctx.tenant` now compares the supplier identifier with `globex`. `Caller` holds context values; constructing one does not authenticate the caller.

## Evaluate access

Functions below are in `parcel_runtime::plan`; `Result<T>` means `Result<T, RuntimeError>`.

```rust
pub fn refusal(
    cc: &CompiledContract, caller: &Caller,
) -> Result<Option<String>>;

pub fn param_values(
    cc: &CompiledContract, caller: &Caller,
) -> Result<ParamValues>;

pub fn active_shapes<'a>(
    cc: &'a CompiledContract, caller: &Caller,
) -> Result<Vec<&'a ShapeRule>>;
```

| Function | Parameters | Return value |
| --- | --- | --- |
| `refusal` | `cc`: compiled contract rules; `caller`: context to test. | The first refusing `decide` rule's ID, or `None` when all decisions pass. |
| `param_values` | The same contract and caller. | DataFusion parameter values for the contract's `$c0`, `$c1`, … placeholders. |
| `active_shapes` | The same contract and caller. | References to shape rules whose `unless` condition is absent or false. The references borrow from `cc`. |

Expression evaluation failures return `RuntimeError`. A refusal is an `Ok(Some(rule_id))`, not an execution error.

For a compiled `artifacts: Compilation` and the caller above:

```rust
match parcel_runtime::plan::refusal(&artifacts.contract, &caller)? {
    Some(rule_id) => println!("Refused by {rule_id}"),
    None => println!("Caller passed the decision rules"),
}
```

Row access counts are returned by [selectivity](rust-validation.md#selectivity).
