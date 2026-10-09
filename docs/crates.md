# Rust API reference

The Rust API provides contract parsing, compilation, evaluation, and export. Structs expose data through fields; constructors and methods are listed with their types. Free functions are documented under the module that contains them.

| Reference | What it covers |
| --- | --- |
| [Compiler: `parcel-core`](rust-core.md) | `ContractDoc`, `Registry`, type checking, compilation and diagnostics. |
| [Runtime: `parcel-runtime`](rust-runtime.md) | `Caller`, validation, caller decisions, shape rules and differential testing. |
| [Artifacts and exports](rust-artifacts.md) | Compiled bytes, function pins, SQL and Substrait export, WebAssembly loading. |
| [WebAssembly functions](functions.md) | Writing and verifying a custom function with `parcel-udf`. |

`parcel-core` parses and compiles contracts. `parcel-runtime` evaluates the compiled results and provides export and loading functions.

```{toctree}
:hidden:
:maxdepth: 1

rust-core
rust-runtime
rust-artifacts
```
