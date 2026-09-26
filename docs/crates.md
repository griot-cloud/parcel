# Rust libraries

Use parcel's libraries to compile contracts or integrate their execution into a DataFusion application. The repository uses a shared version and DataFusion dependency across its crates.

| Crate | Responsibility |
| --- | --- |
| `parcel-core` | Parse, type-check and compile contracts; resolve inheritance; define built-ins. Performs no I/O. |
| `parcel-runtime` | Caller values, parameter binding, validation, bundles, export, shape rewrites, reference evaluation and WebAssembly loading. |
| `parcel-udf` | Export Rust functions as WebAssembly modules for parcel. |
| `parcel-cli` | The `parcel` executable. |

## Compiler entry points

1. Parse YAML or JSON with `ContractDoc::parse`.
2. Supply the source Arrow schema and `Registry::builtin()` plus any custom functions.
3. Call `check_contract` for type checking, or `compile` to produce all three compiled outputs.
4. Use `compile_with` and a parent lookup function when contracts inherit.

`Compilation` contains `contract`, `validation` and `write`. Compilation failures return diagnostics with a code, an optional rule ID and an explanation. See {doc}`execution` for the outputs' roles.

## Runtime entry points

| API | Purpose |
| --- | --- |
| `Caller` | Supply caller identity, tenant, purpose and optional attributes. |
| `plan::refusal` | Evaluate caller-level `decide` rules. |
| `plan::param_values` | Bind caller values to compiled expression parameters. |
| `plan::validate` | Run the validation plan over a table. |
| `plan::dataset_value` | Build the dataset namespace from stored statistics. |
| `plan::active_shapes` | Evaluate shape exemptions for a caller. |
| `shape::apply` | Rewrite a query for suppression and aggregate noise; report budget charges. |
| `bundle::Bundle` | Package, read and verify compiled contracts. |
| `differential::differential` | Compare CEL and DataFusion rule results over sample rows and callers. |

A bundle's `from_json` method parses it; call `verify` before using its executable artifacts. Applications must also register the required functions with their DataFusion sessions.

The runtime's `wasm` feature is enabled by default. `substrait` enables plan export and requires `protoc` at build time.

## Integrating enforcement

An application must connect the compiled outputs to its write and query paths, authenticate callers, keep validation statistics current and manage any privacy budgets. Calling the compiler alone does not establish those controls. [peQL](https://griot-cloud.github.io/peQL/rust.html) provides an engine with these paths already connected.

For local API documentation, run `cargo doc --workspace --no-deps --open` from the repository root.
