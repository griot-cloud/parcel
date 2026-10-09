# Compiled artifacts and exports

## `CompiledBytes`

```rust
use parcel_runtime::compiled::CompiledBytes;

artifacts.to_bytes() -> Result<Vec<u8>, String>
Compilation::from_bytes(bytes: &[u8]) -> Result<Compilation, String>
```

`Compilation` implements `CompiledBytes`. `to_bytes` serializes the three artifacts deterministically; `from_bytes` loads exactly those artifacts without compiling again. It rejects a different format or parcel version. The format is identified by `parcel_runtime::compiled::FORMAT`. Treat bytes as trusted only when their source has been authenticated.

## Function pins and WebAssembly

```rust
parcel_runtime::verify_pins(pins: impl IntoIterator<Item = &FunctionPin>)
    -> Result<(), String>
parcel_runtime::wasm::install(
    module_bytes: &[u8], manifest: &FunctionManifest, owner: &str,
) -> Result<FunctionEntry, String>
```

`verify_pins` checks that the runtime has the implementations and hashes pinned by the artifact. With the default `wasm` feature, `install` loads and verifies a tenant's module, makes it callable by CEL and DataFusion, and returns its registry entry. The `wasm` module is absent when that feature is disabled. A contract carrying an unavailable function must be refused. See [Custom functions](functions.md) for authoring and manifests.

## SQL export

```rust
parcel_runtime::export::validation_sql(
    c: &Compilation, dialect: &str, table: &str,
) -> Result<SqlExport, String>
```

`c` is the compiled contract, `dialect` is `datafusion`, `duckdb`, `postgres`, `mysql`, `sqlite`, `bigquery` or `snowflake`, and `table` names the receiver's data table. Returns `SqlExport { dialect, sql, warnings }`. Read warnings before executing the SQL: they identify constructs whose behavior parcel has not verified for that engine. This exports validation, not caller access enforcement.

## Substrait export

```rust
parcel_runtime::export::validation_substrait(
    c: &Compilation, table: &str,
) -> Result<(Vec<u8>, Vec<String>), String>
```

Available only with the `substrait` feature. Returns protobuf plan bytes and warnings for the receiver. The receiving engine must support the plan and its function extensions. This exports the validation plan, not the complete contract's query policy.
