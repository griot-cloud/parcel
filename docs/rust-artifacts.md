# Compiled artifacts and exports

`parcel_runtime` saves and loads compilations, exports validation plans, and represents stored column definitions.

## `CompiledBytes`

The trait is in `parcel_runtime::compiled` and is implemented by `parcel_core::Compilation`:

```rust
pub trait CompiledBytes: Sized {
    fn to_bytes(&self) -> Result<Vec<u8>, String>;
    fn from_bytes(bytes: &[u8]) -> Result<Self, String>;
}
```

| Method | Arguments and result |
| --- | --- |
| `to_bytes` | Borrows a compilation and returns its serialized contract, validation plan, and write plan. Encoding errors return a string. |
| `from_bytes` | Reads the supplied bytes and returns the saved compilation without recompiling. Invalid data, format differences, or a different parcel version return an error. |

`compiled::FORMAT` identifies the format, currently `parcel-compiled/1`. Import the trait to call these methods:

```rust
use parcel_core::Compilation;
use parcel_runtime::compiled::CompiledBytes;

fn round_trip(artifacts: &Compilation) -> Result<Compilation, String> {
    let bytes = artifacts.to_bytes()?;
    Compilation::from_bytes(&bytes)
}
```

## SQL export

```rust
pub fn validation_sql(
    c: &Compilation, dialect: &str, table: &str,
) -> Result<SqlExport, String>;
```

This function is in `parcel_runtime::export`. `c` supplies the validation plan, `dialect` selects the SQL syntax, and `table` names the data table in the receiving engine. Unsupported dialects or unrepresentable plans return an error.

`export::DIALECTS` lists the accepted names: `datafusion`, `duckdb`, `postgres`, `mysql`, `sqlite`, `bigquery`, and `snowflake`.

### `SqlExport` fields

| Field | Type | Meaning |
| --- | --- | --- |
| `dialect` | `String` | Selected dialect. |
| `sql` | `String` | Validation query text. |
| `warnings` | `Vec<String>` | Constructs whose behavior has not been verified for that dialect. |

For an existing compilation:

```rust
let exported = parcel_runtime::export::validation_sql(&artifacts, "duckdb", "orders")?;
println!("{};", exported.sql);
for warning in exported.warnings {
    eprintln!("{warning}");
}
```

This exports quality validation, not the contract's access policies or masking.

## Substrait export

```rust
pub fn validation_substrait(
    c: &Compilation, table: &str,
) -> Result<(Vec<u8>, Vec<String>), String>;
```

Also in `parcel_runtime::export`, available with the `substrait` feature. `c` supplies the validation plan and `table` names the receiving data table. The tuple contains protobuf plan bytes and warnings, in that order. Conversion failures return an error string.

## Stored schema helpers

These items are in `parcel_runtime::compiled`:

| Item | Fields or signature | Meaning |
| --- | --- | --- |
| `ColumnDef` | `name: String`, `data_type: String`, `nullable: bool` | Persisted Arrow column definition; `data_type` is serialized under the JSON key `type`. |
| `schema_to_defs` | `(schema: &Schema) -> Vec<ColumnDef>` | Converts an Arrow schema to stored definitions. |
| `schema_from_defs` | `(defs: &[ColumnDef]) -> Result<Schema, String>` | Restores the schema; invalid Arrow type strings return an error. |
| `BundledFunction` | `owner: String`, `manifest: FunctionManifest`, `module: String` | A module and its manifest; `module` contains hexadecimal bytes. |
| `session` | `() -> SessionContext` | Creates a session with parcel functions registered for decoding. |

For loading and verifying function implementations, see {doc}`rust-wasm`.

```{toctree}
:hidden:

rust-wasm
```
