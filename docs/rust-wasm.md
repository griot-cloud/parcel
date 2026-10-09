# WebAssembly runtime

`parcel_runtime::wasm` is available with the `wasm` feature, enabled by default. For building a module and writing its manifest, see {doc}`functions`.

## `install`

```rust
pub fn install(
    module_bytes: &[u8],
    manifest: &FunctionManifest,
    owner: &str,
) -> Result<FunctionEntry, String>;
```

| Parameter | Meaning |
| --- | --- |
| `module_bytes` | Compiled WebAssembly module bytes. |
| `manifest` | Function name, version, signature, and execution properties. |
| `owner` | Identifier of the owner whose contracts may call the function. |

Loads and verifies the module, makes its implementation available to CEL and DataFusion, and returns the entry to insert into the compilation registry. Invalid modules, unsupported interfaces, or failed test batches return an error string.

For existing `module_bytes` and a parsed `manifest`:

```rust
let entry = parcel_runtime::wasm::install(&module_bytes, &manifest, "acme")?;
let mut registry = parcel_core::Registry::builtin();
registry.insert(entry);
```

## `WasmFunction`

The only public field is `entry: FunctionEntry`, the function's verified metadata.

```rust
impl WasmFunction {
    pub fn load(module_bytes: &[u8], entry: FunctionEntry) -> Result<WasmFunction, String>;
    pub fn call(&self, args: &[ArrayRef], rows: usize) -> Result<ArrayRef, String>;
    pub fn call_values(&self, args: &[cel::Value]) -> Result<cel::Value, String>;
}
```

| Method | Arguments and result |
| --- | --- |
| `load` | Verifies module bytes against the owned entry and creates a callable instance. |
| `call` | Executes ordered Arrow argument arrays for `rows` rows and returns the output array. |
| `call_values` | Executes one ordered set of CEL arguments and returns a CEL value. |

Calls can fail on argument mismatches, module execution errors, or resource limits. `install` is the higher-level entry point when preparing functions for contract compilation.

## `verify_pins`

This function is at the `parcel_runtime` crate root:

```rust
pub fn verify_pins<'a>(
    pins: impl IntoIterator<Item = &'a FunctionPin>,
) -> Result<(), String>;
```

Checks that the runtime has the exact function implementations referenced by the supplied pins. Returns `Ok(())` when all are available; otherwise returns the missing or mismatched function in an error message. Load custom implementations before checking their pins.
