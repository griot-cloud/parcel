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
| 7 ✅ | WebAssembly user functions (design 7.4) | `parcel-udf` macro and ABI (arrow-udf convention). Registration verifies exports, imports and a smoke batch. `parcel-runtime` loads modules with wasmtime under fuel and memory limits, as a DataFusion UDF and a CEL function. The registry is a store keyed by tenant and hash. | A tenant function compiled to wasm32 is registered, pinned, and used in an assert. The differential test passes. |
| 8 ✅ | Substrait output of the validation plan | datafusion-substrait. Constructs it cannot express are listed. | A round trip through Substrait keeps the verdict for the quickstart. |
| 9 ✅ | ODCS import | `parcel import odcs`: schema properties to expose, `required` to `has()` asserts, quality rules where they map. | The ODCS v3 example imports and compiles. |
| 10 | Decimals in rules | A reference evaluator that handles exact decimals, then checker and translator support. | Decimal comparisons and arithmetic pass the differential test. |

Out of scope for v1: Python bindings (packaging work, not compiler work), federated execution (peQL), and certificate issuance (Griot product).

## Progress notes

- **1–4** shipped as scoped. Two changes from the plan:
  - Manifests became per contract (`<root>/_parcel/<contract>.json`), so several contracts can bind one copy of the data without sharing a verdict.
  - An undeclared `ctx.other` field is an error, not `dyn`: declaring it costs one line and removes a class of runtime type errors.
- **6** goes further than "parses". `examples/verify-duckdb.py` runs the exported SQL inside DuckDB 1.5, and the verdict matches parcel's to the last value. That covers counts, rates, distinct counts, extrema, regex, SHA-256, integer division and modulo. It found and fixed two export bugs:
  - DataFusion's DuckDB dialect writes every division as `//`, which is integer division in DuckDB
  - DuckDB calls `regexp_like` `regexp_matches`
- **7** shipped with one deliberate change: the ABI. The design suggested the `arrow-udf` IPC convention, but that puts an Arrow IPC stack inside every module. parcel-udf ABI v1 passes Arrow-layout columns (validity bitmaps, fixed-width values, offsets and data) through linear memory. The guest crate has no dependencies, and the example module is 13 KB. The host converts to and from Arrow arrays with no row-by-row work. If the ecosystem converges on arrow-udf, adopting its ABI changes registration, not contracts.
  - Loaded functions are keyed by pinned hash, never by name. The reference interpreter sees only the functions its contract is pinned to.
  - Name uniqueness is per workspace: a tenant cannot register a name another tenant owns.
  - Bundles embed the modules they are pinned to, so a verifier needs nothing else.
- **8** ships behind the `substrait` feature on `parcel-engine` and `parcel-cli`, because the `substrait` crate needs `protoc` at build time and parcel should not impose that on everyone. A round trip passes: produce Substrait, consume it in a fresh DataFusion session, run it, and get the same verdict.
- **9** imports the parts of ODCS v3 that have a faithful parcel meaning:
  - columns, with types taken from `physicalType` where recognisable, otherwise `logicalType`
  - `required` and `primaryKey`, as deny-level presence asserts
  - `unique`, as a distinct-count guarantee
  - quality rules with `engine: parcel`, whose `implementation` carries parcel rules verbatim

  Everything else (library, SQL and text quality rules, servers, SLAs, classification) is reported as a note, never guessed. Tested against a fixture shaped like the standard's own examples; check it against the ODCS version you use.
