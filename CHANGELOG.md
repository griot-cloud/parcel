# Changelog

A version reaches users when it lands on `main` untagged: CI builds, tests and publishes it,
then tags it. See [CONTRIBUTING](docs/contributing.md#releasing).

## [0.0.3]: embedding columns

- Embedding columns: a contract can expose `fixed_size_list<float32, N>` with a positive
  dimension N. The checker refuses other element types, a zero or negative dimension, and a
  column whose dimension differs from the data's or from the function that produces it.
- CEL and DataFusion both evaluate rules over embeddings and agree, null rows included,
  keeping float32 and the dimension; `size` works on an embedding.
- A bundle keeps an embedding schema through serialisation and verification, and fails
  verification if the declared dimension is changed.
- WebAssembly functions take and return embeddings: in Rust, `parcel_udf::export!` accepts
  `[f32; N]`, and function signatures may name `fixed_size_list<float32, N>`. The host refuses
  output of the wrong dimension or length.
- The residency check and the embedding decoder now pass clippy on Rust 1.98, with no change
  in behaviour.
- 0.0.2 was not published on its own; its changes ship in this release.

## [0.0.2]: residency terms

- Residency terms in contracts, as `residency: {jurisdictions: [...],
  approved_output_may_leave: ...}`. Jurisdictions are ISO 3166-1 alpha-2 country codes or
  `EU`; the checker refuses an empty list, an unknown code and wildcards. A contract without
  terms permits no placement.
- The terms are part of the contract hash, and a verified bundle exposes them through
  `Bundle::residency()`; a bundle that fails verification permits nothing.
- An inheriting contract keeps its parent's terms or narrows them; widening them, or adding
  terms the parent lacks, is refused.
- `plan::validate_in` runs a validation plan in the engine's own DataFusion session, so a scan
  can read from an object store registered there.
- `parcel-runtime` builds without default features. Without the `wasm` feature, a bundle that
  carries a function fails verification and names the function.
- The documentation is rewritten around contracts, with tested supplier examples.

## [0.0.1]: first public release

- The contract language: seven operations (`decide`, `expose`, `admit`, `assert`, `transform`,
  `guarantee`, `shape`) over three namespaces (`ctx`, `dataset`, `row`), checked against the
  data's Arrow schema.
- The compiler: CompiledContract, ValidationPlan and WritePlan sharing one hash, and the
  `parcel-bundle/1` artifact an engine verifies by recompiling.
- Shapes: small-cell suppression, sampling, and Laplace noise at the row or the aggregate.
- Exports: SQL for seven dialects, Substrait, and ODCS v3 import.
- Tenant functions as WebAssembly modules, pinned by hash.
- The `parcel` command: `check`, `compile`, `schema`, `import odcs`, `function verify`.
- Binaries for Linux, macOS and Windows, with `install.sh` and `install.ps1` installers.
