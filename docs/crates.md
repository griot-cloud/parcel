# Crates

| Crate | What it is |
| --- | --- |
| `parcel-core` | The compiler: parse, check, classify, translate, assemble. No I/O. |
| `parcel-runtime` | What an engine needs to run the artifacts: the `Caller` type, the reference CEL interpreter and built-ins, parameter binding, `decide` and `unless` evaluation, validation over any table, shape rewrites, WebAssembly loading, bundles, SQL and Substrait export, and the differential test. |
| `parcel-udf` | Write functions in Rust for WebAssembly (no dependencies). |
| `parcel-cli` | The `parcel` command. |

```rust
use parcel_core::{compile, ContractDoc, Registry};

let doc = ContractDoc::parse(&std::fs::read_to_string("orders.yaml")?)?;
let compilation = compile(&doc, &schema, &Registry::builtin())?;
for r in &compilation.contract.report {
    println!("{} {} {}", r.rule, r.op, r.reason);
}
let verdict = parcel_runtime::plan::validate(&compilation, compilation.validation.plan.clone(), table).await?;
```

`parcel_runtime::plan` has the pieces an engine calls per query: `param_values` binds a caller,
`refusal` runs `decide` rules, `active_shapes` evaluates `unless`, and `dataset_value` builds
the `dataset` namespace from stored statistics. `parcel_runtime::shape::apply` applies
`suppress` and aggregate `noise` to a caller's plan and returns the budget charges. Engines
register `parcel_core::udfs::parcel_udfs()` on their sessions.
