# parcel v1: what v0 left out, scoped

v0 works end to end. This is the deferred list from `parcel-v0.md`, plus gaps found while building it. Each item has a scope and a definition of done, in the order they are built.

| # | Item | Scope | Done when |
|---|---|---|---|
| 1 ✅ | Tests for `noise` and `sample` | Engine tests only; the operators exist | Noise changes aggregates within bounds and spends budget until refused. A sample is stable across queries and close to its fraction. |
| 2 ✅ | Split pass (design 8) | core: find row-only subtrees of mixed `admit`/`transform` rules that are worth storing; WritePlan `derived`; a materialised variant of each admit and transform. engine: write derived columns, use them when current. | `is_msisdn(row.phone) \|\| ctx.clearance > 3` reads a stored `_d_*` column. Results are identical with and without materialisation. |
| 3 ✅ | Inheritance (design 12) | core: `resolve_inherits(doc, lookup)` gives a flat document. Expose is intersected, decide/admit/assert/guarantee rules are unioned, transforms compose parent first, shapes accumulate. Cycles and id clashes are errors. engine: resolve from the workspace. | A child can only restrict. A child that tries to widen the parent's expose is rejected. |
| 4 ✅ | Extensions: `ctx.other`, `dataset.other`, `row.other` + `enrich` (design 3.1, 10) | Declared `extensions` are typed. Undeclared `ctx.other`/`dataset.other` fields are `dyn` and must be cast. `row.other` is an `_other` struct column filled by enrichers at write. `dataset_other` holds constants and aggregates. | Rules over declared extension fields push down. Enrichers run at write, and the differential test covers them. |
| 5 ✅ | JSON Schema for the contract document | Generated from the Rust types (schemars); `parcel schema` prints it. | Editors autocomplete contracts. The schema accepts every test fixture. |
| 6 ✅ | Validation plan as SQL | `parcel compile --sql <dialect>` through DataFusion's unparser; built-ins that have no SQL form are reported. | The DuckDB/Postgres SQL for the quickstart contract is emitted and parses. |
| 7 | WebAssembly user functions (design 7.4) | `parcel-udf` macro and ABI (arrow-udf convention). Registration verifies exports, imports and a smoke batch. `parcel-runtime` loads modules with wasmtime under fuel and memory limits, as a DataFusion UDF and a CEL function. The registry is a store keyed by tenant and hash. | A tenant function compiled to wasm32 is registered, pinned, and used in an assert. The differential test passes. |
| 8 | Substrait output of the validation plan | datafusion-substrait. Constructs it cannot express are listed. | A round trip through Substrait keeps the verdict for the quickstart. |
| 9 | ODCS import | `parcel import odcs`: schema properties to expose, `required` to `has()` asserts, quality rules where they map. | The ODCS v3 example imports and compiles. |
| 10 | Decimals in rules | A reference evaluator that handles exact decimals, then checker and translator support. | Decimal comparisons and arithmetic pass the differential test. |

Out of scope for v1: Python bindings (packaging work, not compiler work), federated execution (peQL), and certificate issuance (Griot product).

## Progress notes

- **1–4** shipped as scoped. Two changes from the plan:
  - Manifests became per contract (`<root>/_parcel/<contract>.json`), so several contracts can bind one copy of the data without sharing a verdict.
  - An undeclared `ctx.other` field is an error, not `dyn`: declaring it costs one line and removes a class of runtime type errors.
- **6** goes further than "parses". `examples/verify-duckdb.py` runs the exported SQL inside DuckDB 1.5, and the verdict matches parcel's to the last value. That covers counts, rates, distinct counts, extrema, regex, SHA-256, integer division and modulo. It found and fixed two export bugs:
  - DataFusion's DuckDB dialect writes every division as `//`, which is integer division in DuckDB
  - DuckDB calls `regexp_like` `regexp_matches`
