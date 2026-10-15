# Changelog

A version reaches users when it lands on `main` untagged: CI builds, tests and publishes it,
then tags it. See [CONTRIBUTING](docs/contributing.md#releasing).

## Unreleased

- `parcel lint` reads the contract file only: no `--schema`, `--type` or `--function`, no data.
  It reports problems the file shows (rule ids, type names, expressions that do not parse,
  namespaces an operation may not read, unknown `ctx` fields, undeclared `other` fields,
  transforms of unexposed columns) and lints.
- `parcel validate` lints first and stops on problems; otherwise it type-checks and compiles,
  and prints lint warnings and notes with the result.
- `parcel_core::lint::lint(doc)` returns a `Report { problems, lints }` and no longer takes a
  checked contract. `parcel_core::syntax::reads` says what an expression reads, without types.
- The `binding-not-found` lint is gone; `validate`'s `types` line says when a binding has no
  Parquet files.

## [0.0.4]: validate, lint, per-caller check

- `parcel validate CONTRACT`: checks a contract compiles. No data needed.
- `parcel lint CONTRACT`: validates, then reports `admit-reads-only-ctx`,
  `unsatisfiable-callers`, `constant-rule`, `duplicate-condition`, `unused-extension`, and
  notes. `--deny-warnings` exits 1 on warnings.
- `--schema` is optional for `validate`, `lint` and `compile`. Column types come from the
  binding's local Parquet data, else `expose`. Without data, a rule reading a column not in
  `expose` is refused.
- `parcel check` reports one section per caller: exposed columns, rows received, each policy
  (`decide`, `admit`, `transform`, `shape`) and each quality rule over the caller's rows, with
  pass rates and the commonest failing values. A refused caller shows the rule, its condition
  and the caller's value. All counts run concurrently, each with a progress bar on a terminal.
- `version` is a semantic version: `1.0.0`, `2.1.0-beta.1`. A whole number N reads as N.0.0;
  a decimal such as `1.10` is refused with the fix (`1.10.0`). `parcel_core::Version` replaces
  `u32` on documents and compiled contracts, and serialises as a string, so contract hashes
  change. ODCS import keeps the full version.
- Rules take an optional `message`, shown in `check` output and kept in the compiled report
  (`ReportEntry::message`). Contracts without it hash as before.
- Every structural problem in a document is reported at once, with its line and a fix.
  `ContractDoc::parse_all` returns them all.
- `Diagnostic` has `help` and `line`. Messages suggest close names for columns, `ctx` fields,
  functions and types (`integer` → `int64`), name the operation that fits a namespace, and
  catch `and`, `=` and `is null`.
- A whole-number literal beside a double is a double (`row.cost > 0`); beside an unsigned
  column, unsigned. Only exact literals are converted.
- Output: `error[Code]` with the source line and `help:`, aligned columns, colour on
  terminals only (none when piped or with `NO_COLOR`). `parcel` with no command prints help.
- `compile` prints each rule's op and tier; reasons are in `--json`. An `admit` that reads
  only `ctx` is reported as plan time.
- A compiled contract round-trips through bytes: `Compilation::to_bytes` and
  `Compilation::from_bytes` (`parcel_runtime::compiled`). Loading never compiles.
- The `parcel-bundle/1` bundle and `Bundle::verify`, which recompiled it, are removed.
  `parcel compile -o` writes the compiled bytes. `BundledFunction`, `ColumnDef` and the schema
  helpers move to `parcel_runtime::compiled`.

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
