# Compiler: `parcel-core`

The crate re-exports `ContractDoc`, `Registry`, `check_contract`, `compile`, `compile_with`, `Compilation` and `Diagnostic` at its root. The snippets use DataFusion's Arrow `Schema`.

## `ContractDoc::parse`

```rust
ContractDoc::parse(source: &str) -> Result<ContractDoc, Diagnostic>
```

Parses YAML or JSON contract text. `source` is the document content, not a path. Document syntax and shape errors return a `Diagnostic` with `code`, optional `rule`, and `message`. Parsing does not check expressions against a dataset schema.

`ContractDoc` has public fields: `contract: String`, `version: u32`, optional `owner`, `inherits`, `binding`, `residency`, `expose`, `extensions`, `enrich`, `dataset_other`, and `rules: Vec<Rule>`. See [Contract language](language.md) for their document forms.

## `Registry`

```rust
Registry::builtin() -> Registry
registry.insert(entry: FunctionEntry)
registry.get(name: &str) -> Option<&FunctionEntry>
registry.visible_to(owner: Option<&str>) -> Registry
registry.entries() -> impl Iterator<Item = &FunctionEntry>
```

`builtin` provides parcel's standard functions. `insert` replaces an entry with the same name. `get` looks up one entry. `visible_to` returns built-ins plus functions owned by the given tenant. `entries` iterates current entries. Pass this registry to checking and compilation so function calls can be resolved. `FunctionEntry` includes name, version, overload signatures, determinism, cost, owner, hash and optional module hash.

## `check_contract`

```rust
check_contract(
    doc: &ContractDoc, schema: &Schema, registry: &Registry,
) -> Result<CheckedContract, Vec<Diagnostic>>
```

Checks rule expressions, column names/types and allowed namespaces against the source Arrow schema and function registry. Returns a typed `CheckedContract` or all collected diagnostics. It does not create executable plans.

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

## `Compilation` and diagnostics

| Field | Type | Meaning |
| --- | --- | --- |
| `contract` | `CompiledContract` | Query-time rules, schemas, caller parameters, function pins and contract hashes. |
| `validation` | `ValidationPlan` | One-row validation plan, assertions and required statistics. |
| `write` | `WritePlan` | Enrichment, flags, derived values and storage layout. |

`Diagnostic` exposes `code: Code`, `rule: Option<String>` and `message: String`. Match `code` in tooling; display the diagnostic for a human-readable explanation. `contract_hash(&ContractDoc) -> String` hashes the canonical document. For bytes that an engine can load without recompiling, see [Artifacts and exports](rust-artifacts.md).

```rust
use parcel_core::{ContractDoc, Registry, compile};

let doc = ContractDoc::parse(source)?;
let artifacts = compile(&doc, &arrow_schema, &Registry::builtin())
    .map_err(|diagnostics| format!("{diagnostics:?}"))?;
```
