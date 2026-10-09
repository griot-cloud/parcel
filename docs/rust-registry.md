# Function registry

`parcel_core::Registry` maps function names to the definitions used when checking and compiling expressions. Its supporting types are in `parcel_core::registry`.

## `Registry` methods

```rust
impl Registry {
    pub fn builtin() -> Self;
    pub fn insert(&mut self, entry: FunctionEntry);
    pub fn get(&self, name: &str) -> Option<&FunctionEntry>;
    pub fn visible_to(&self, owner: Option<&str>) -> Registry;
    pub fn entries(&self) -> impl Iterator<Item = &FunctionEntry>;
}
```

| Method | Arguments and result |
| --- | --- |
| `builtin` | Creates a registry containing parcel's built-in functions. |
| `insert` | Takes ownership of an entry; replaces any entry with the same name. |
| `get` | Looks up a name and borrows the entry, or returns `None`. |
| `visible_to` | Creates a registry containing built-ins and functions for the supplied owner; `None` selects built-ins only. |
| `entries` | Borrows all current entries through an iterator. |

```rust
use parcel_core::Registry;

let registry = Registry::builtin();
let redact = registry.get("redact").expect("built-in function");
assert!(redact.is_builtin());
```

## `FunctionEntry`

| Field | Type | Meaning |
| --- | --- | --- |
| `name` | `String` | Function name used in expressions. |
| `version` | `u32` | Function revision. |
| `signatures` | `Vec<Signature>` | Supported argument and return types. |
| `deterministic` | `bool` | Whether identical inputs produce identical outputs. |
| `nulls` | `Nulls` | Null handling; currently `Nulls::Propagate`. |
| `cost` | `Cost` | Execution cost: `Cheap`, `Moderate`, or `Expensive`. |
| `pushdown` | `Pushdown` | Optimization hint: `None` or `Monotone`. |
| `owner` | `String` | Owner identifier; built-ins use `core`. |
| `hash` | `String` | Hash of implementation identity and manifest. |
| `module` | `Option<String>` | WebAssembly module hash, or `None` for built-ins. |

```rust
impl FunctionEntry {
    pub fn user(
        manifest: &FunctionManifest,
        owner: &str,
        module_sha256: &str,
    ) -> Result<FunctionEntry, String>;
    pub fn pin(&self) -> FunctionPin;
    pub fn is_builtin(&self) -> bool;
}
```

`user` constructs metadata from a manifest, owner, and module hash; invalid names or signatures return an error. It does not load executable code. [wasm::install](rust-wasm.md) loads the module and creates an entry together. `pin` returns the exact implementation identity. `is_builtin` is true when `module` is absent.

## `FunctionManifest`

| Field | Type | Meaning |
| --- | --- | --- |
| `name` | `String` | Exported function name. |
| `version` | `u32` | Function revision. |
| `signatures` | `Vec<String>` | Signature strings, such as `(string) -> bool`; a v1 function has one signature. |
| `deterministic` | `bool` | Defaults to `true` when deserializing. |
| `cost` | `Cost` | Defaults to `Moderate` when deserializing. |

## `Signature` and `FunctionPin`

| Type | Public fields |
| --- | --- |
| `Signature` | `args: Vec<Type>` for ordered argument types; `ret: Type` for the return type. |
| `FunctionPin` | `name: String`, `version: u32`, `hash: String` identifying an exact function implementation. |

```rust
pub fn parse_signature(s: &str) -> Result<Signature, String>;
```

Parses a signature such as `(string, int) -> bool`. Returns an error for malformed signatures or unsupported scalar types.
