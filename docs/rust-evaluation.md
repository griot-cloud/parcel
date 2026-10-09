# CEL evaluation

`parcel_runtime::reference` evaluates CEL expressions and converts caller, dataset, and Arrow values. Most applications can use the higher-level [caller](rust-caller.md) and [validation](rust-validation.md) functions.

## `Scope`

| Field | Type | Meaning |
| --- | --- | --- |
| `ctx` | `Option<cel::Value>` | Caller namespace, when needed. |
| `dataset` | `Option<cel::Value>` | Dataset namespace, when needed. |
| `row` | `Option<cel::Value>` | Row namespace; absent keys represent null values. |
| `pins` | `BTreeSet<FunctionPin>` | Exact custom function implementations the expression can call. |

`Scope::default()` has no namespace values and an empty pin set.

## Context and evaluation

```rust
pub fn context(scope: &Scope) -> cel::Context<'static>;
pub fn eval(src: &str, ctx: &cel::Context) -> Result<cel::Value, String>;
pub fn eval_bool(src: &str, ctx: &cel::Context) -> Result<bool, String>;
```

`context` registers the standard library, parcel functions, and the supplied namespaces. `src` is CEL expression text. Evaluation returns a value or an error string; `eval_bool` also rejects results that are not Boolean.

```rust
use parcel_runtime::{Caller, reference::{self, Scope}};

let caller = Caller::new("analyst", "globex", "purchasing");
let scope = Scope {
    ctx: Some(reference::ctx_value(&caller)),
    ..Scope::default()
};
let context = reference::context(&scope);
assert_eq!(reference::eval_bool("ctx.tenant == 'globex'", &context), Ok(true));
```

## Value conversion

Signatures below use `Value` for `cel::Value` and `Type` for `parcel_core::types::Type`.

| Signature | Parameters and result |
| --- | --- |
| `ctx_value(c: &Caller) -> Value` | Converts standard caller fields into a CEL namespace. |
| `ctx_value_typed(c: &Caller, declared: &BTreeMap<String, Type>) -> Value` | Also converts declared custom caller fields according to their types. |
| `json_value(j: &serde_json::Value, ty: &Type) -> Option<Value>` | Converts JSON to the requested CEL type; returns `None` when incompatible. |
| `dataset_value(entries: &[(String, Value)]) -> Value` | Builds nested dataset values from dotted field paths and values. |
| `string(s: &str) -> Value` | Creates a CEL string. |
| `timestamp(t: DateTime<Utc>) -> Value` | Creates a CEL timestamp. |
| `to_scalar(v: &Value, ty: &Type) -> Result<ScalarValue, String>` | Converts a CEL value to a typed DataFusion scalar, or returns an error. |
| `from_scalar(s: &ScalarValue) -> Option<Value>` | Converts a supported non-null scalar to CEL; otherwise returns `None`. |
| `list_type(elem: &Type) -> DataType` | Creates the Arrow list type for the element type. |
| `timestamp_type() -> DataType` | Returns the Arrow timestamp type used by the interpreter. |
