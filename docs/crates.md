# Rust API reference

parcel exposes Rust crates and a command-line tool. It does not ship a Python package. The pages below cover the public entry points used to compile contracts and execute their artifacts. See [Command line](cli.md) for the executable.

| Reference | What it covers |
| --- | --- |
| [Compiler: `parcel-core`](rust-core.md) | `ContractDoc`, `Registry`, type checking, compilation and diagnostics. |
| [Runtime: `parcel-runtime`](rust-runtime.md) | `Caller`, validation, caller decisions, shape rules and differential testing. |
| [Artifacts and exports](rust-artifacts.md) | Compiled bytes, function pins, SQL and Substrait export, WebAssembly loading. |
| [WebAssembly functions](functions.md) | Writing and verifying a custom function with `parcel-udf`. |

`parcel-core` parses and compiles without I/O. `parcel-runtime` provides operations an embedding engine calls; it does not authenticate users, store datasets or automatically enforce a contract on every query. [peQL](https://griot-cloud.github.io/peQL/rust.html) connects those operations to a query engine.

```{toctree}
:hidden:
:maxdepth: 1

rust-core
rust-runtime
rust-artifacts
```
