# Compiler: `parcel-core`

The crate re-exports `ContractDoc`, `Registry`, `check_contract`, `compile`, `compile_with`, `Compilation` and `Diagnostic` at its root. The snippets use DataFusion's Arrow `Schema`.

| Type reference | Contents |
| --- | --- |
| [Contract documents](rust-document.md) | `ContractDoc`, rule types, bindings, and parsing. |
| [Function registry](rust-registry.md) | `Registry`, entries, manifests, signatures, and pins. |
| [Compilation results](rust-compilation.md) | `Compilation`, `CompiledContract`, validation and write plans. |
| [Diagnostics](rust-diagnostics.md) | Error codes, fields, and constructors. |

## `check_contract`

```rust
check_contract(
    doc: &ContractDoc, schema: &Schema, registry: &Registry,
) -> Result<CheckedContract, Vec<Diagnostic>>
```

Checks rule expressions, column names/types and allowed namespaces against the source Arrow schema and function registry. Returns a typed [CheckedContract](rust-checked.md) or all collected diagnostics. It does not create executable plans.

## `compile`

```rust
compile(
    doc: &ContractDoc, schema: &Schema, registry: &Registry,
) -> Result<Compilation, Vec<Diagnostic>>
```

Checks and compiles a standalone contract. `schema` is the bound data's Arrow schema; `registry` supplies functions visible to the contract. Returns three artifacts in `Compilation` or diagnostics. Use `compile_with` when `doc.inherits` names a parent.

## `compile_with`

```rust
compile_with(
    doc: &ContractDoc,
    schema: &Schema,
    registry: &Registry,
    lookup: &dyn Fn(&str) -> Option<ContractDoc>,
) -> Result<Compilation, Vec<Diagnostic>>
```

Resolves parent contracts by name through `lookup`, then checks and compiles the flattened chain. Return `None` from the callback when a parent is absent; inheritance diagnostics describe the failure. The child may narrow, but cannot widen, its parent.

## Parameters and errors

| Parameter | Used by | Meaning |
| --- | --- | --- |
| `doc` | All three functions | Parsed contract to check or compile; borrowed without modification. |
| `schema` | All three functions | Arrow schema of the underlying data. |
| `registry` | All three functions | Function definitions available to this contract. |
| `lookup` | `compile_with` | Callback receiving a parent name and returning its document, or `None` if absent. |

Failures return `Vec<Diagnostic>` so a caller can display all collected problems. Successful checking returns a `CheckedContract`; successful compilation returns the three artifacts in [Compilation](rust-compilation.md).

## Example

This helper parses and compiles document text against a supplied Arrow schema, returning readable errors:

```rust
use datafusion::arrow::datatypes::Schema;
use parcel_core::{Compilation, ContractDoc, Registry, compile};

fn compile_text(source: &str, schema: &Schema) -> Result<Compilation, String> {
    let doc = ContractDoc::parse(source).map_err(|e| e.to_string())?;
    compile(&doc, schema, &Registry::builtin()).map_err(|errors| {
        errors.iter().map(ToString::to_string).collect::<Vec<_>>().join("\n")
    })
}
```

## Additional compiler functions

`parcel_core::check::contract_hash(doc: &ContractDoc) -> String` returns the hash of the document's canonical form.

`parcel_core::compile::compile_resolved(resolved: &Resolved, schema: &Schema, registry: &Registry) -> Result<Compilation, Vec<Diagnostic>>` compiles an already resolved inheritance chain. Most callers should use `compile_with` for this step.

```{toctree}
:hidden:

rust-document
rust-checked
rust-registry
rust-compilation
rust-diagnostics
```
