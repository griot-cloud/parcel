# peQL 0.4 on parcel: Architecture Design

**Document type:** full architecture design (not an ADR)
**Status:** DRAFT for review · 2026-09-25
**Scope:** How peQL 0.3.0 becomes a runtime for parcel contracts without losing any capability it has today. This covers every 0.3 module, what replaces it or keeps it, the changes parcel needs to take over peQL's policy features, and the order of work. **Out of scope:** Flight SQL, federated execution across peQL instances, certificate issuance, and ODCS export.
**Companion:** `design/parcel-README.md` (the contract language), the peQL design README (the target runtime shape), `design/parcel-ecosystem.md`.

---

## 1. Problem statement

**P1. peQL and parcel each have their own policy model.** peQL 0.3 reads JSON contracts: `purposes`, `columns[].mask`, `masks`, `row_filter` as a SQL string, `dp_columns`, and `owner_tenant`. It resolves each one into a `ResolvedPolicy` per caller. parcel compiles CEL contracts into DataFusion expressions that do the same jobs: `decide` for purpose gating, `expose` for projection, `admit` for row filtering, `transform` for masking, and `shape: noise` for differential privacy. Two languages describe the same intent, and two sets of code enforce it. Every rule has to be written twice, and the two copies drift. Contract authors pay first: they have to learn which format a given feature lives in. Then peQL's maintainers pay, because every enforcement bug has to be fixed in both places.

**P2. peQL enforces policy with runtime operators the optimiser cannot see.** In 0.3, `ContractTableProvider::scan` does the following:
- scans the whole inner table with no projection and no filters
- wraps the scan in `ScanMetricsExec → ContractApprovedExec → RowFilterExec → MaskingExec → LaplaceNoiseExec`
- only then applies the caller's projection and limit

The bindings make it worse: `load_parquet_as_provider` reads the entire Parquet file into a `MemTable`. As a result, no query pushes a predicate into the scan, prunes a row group, or skips an unread column. Every query pays for the whole dataset. Data owners pay in compute, and callers pay in latency. parcel's view model puts the same rules into a logical plan that the optimiser can push down and fold away per caller.

**P3. Parts of the contract lifecycle are missing, and parcel defines them.** peQL 0.3 has no write path, manifests, flags, validation verdict or dataset statistics, so it has no `guarantee` rules. It also has no tenant functions and no publication model. parcel compiles a `WritePlan` and a `ValidationPlan` for these, but nothing executes them in production. Until something does, parcel contracts can only be checked on a laptop, never enforced over real data.

**P4. The two projects run on different DataFusion versions.** peQL is on DataFusion 47 with Arrow 55. parcel is on DataFusion 55.1 with Arrow 59. parcel's artifacts are DataFusion `Expr` and `LogicalPlan` values, so peQL cannot consume them until the versions match. Any DataFusion type in peQL's public API changes along with the upgrade.

**P5. Some 0.3 capabilities have no equivalent in parcel yet.** If peQL simply adopted parcel, these would be lost:
- graph datasets, with edge filters and the wall rule
- the mask vocabulary `partial`, `tokenize` and `null`
- masking that turns a numeric column into its hash string
- noise on row values rather than on aggregates
- `classification` on the caller
- owner-sees-raw as a one-line default
- JSON contracts that already exist

Removing any of these breaks downstream users. The design must give each one a place in parcel or keep it in peQL.

## 2. Context

### 2.1 Constraints that shape the design

- **parcel is the language; peQL is the runtime.** parcel parses, checks and compiles contracts, and `parcel-runtime` holds what any engine needs to execute the artifacts: the reference interpreter, parameter binding, validation over any table, WebAssembly loading, bundles and exports. peQL owns everything that touches stored data or a live caller. No concept is implemented on both sides. **Consequence:** anything policy-shaped in peQL either becomes a parcel concept or stays in peQL as runtime machinery that executes parcel artifacts. It never stays in peQL as a second policy language.
- **No capability is removed.** A 0.3 user must be able to do everything in 0.4 that they did in 0.3. How they spell it may change, since 0.4 is a minor version before 1.0. **Consequence:** there are three outcomes for any 0.3 feature:
  - it is re-expressed through parcel with the same observable behaviour
  - it is kept and rewired onto parcel artifacts
  - it is kept unchanged

  A public type that parcel supersedes is deprecated for one release and deleted in 0.5, never deleted in 0.4.
- **Open core.** Both repositories are public (the unauthenticated GitHub API returns 200 for both). The Griot platform adapters (T03, T04 storaged, T05 notary) are part of peQL today behind feature flags. **Consequence:** they stay behind feature flags, and the open-source build needs no platform service.
- **Shared types come by crate, not by copy.** peQL depends on `parcel-core` and `parcel-runtime` as a git dependency until they are published on crates.io, which is blocked on a registry token.

### 2.2 Verified current state

The executing agent re-verifies every row before acting on it.

| Fact | Where verified |
|---|---|
| peQL is at `17ea448`, tagged `v0.3.0`, on DataFusion 47 and Arrow 55 | `git -C peql log -1`, `Cargo.toml` |
| peQL's source is 12,348 lines of Rust in `src/`; the tests hold 219 `#[test]`/`#[tokio::test]` functions across `src/` and `tests/` | `wc -l`, `grep -c` |
| `Engine` (the high-level API) builds a per-caller catalog. `PeqlSchemaProvider::table` resolves `ContractSource` → `ResolvedPolicy` and returns a `ContractTableProvider` | `src/engine.rs`, `src/catalog.rs` |
| `ContractTableProvider::scan` ignores filters and projection on the inner scan, then stacks `ScanMetricsExec`, `ContractApprovedExec`, `RowFilterExec`, `MaskingExec` and optionally `LaplaceNoiseExec` | `src/contract_table_provider.rs` |
| `LaplaceNoiseExec` sits below the caller's aggregates, so it noises row values, with a permissive budget by default | `contract_table_provider.rs` (`new_permissive`), `laplace_noise_exec.rs` |
| The four optimiser rules in `optimizer_rules/` (`contract_check`, `row_filter`, `masking`, `dp_noise`) are not used by `Engine`. They are used by `K04DEngine` paths, `long_running_pool_manager` and their own tests | `grep -rln build_pipeline` |
| The OSS binding loads a whole Parquet file into a `MemTable` | `src/binding.rs::load_parquet_as_provider` |
| `MaskingExec` redacts strings to `"***"`, `partial` keeps `"***" + last 4`, and hash, tokenize and partial on a non-string column output `Utf8` | `src/physical/masking_exec.rs:558-567`, module docs |
| parcel's `redact` built-in returns `"***"` | `parcel-runtime/src/reference.rs:325` |
| Graph: 7 SQL table functions over G01 snapshot bundles. A `row_filter`/`node_filter` node is a wall; `edge_filter` hides edges; output masking reuses `ContractApprovedExec → MaskingExec` | `src/graph/*.rs`, `docs/GRAPH-QUERY.md` |
| The graph policy compiler evaluates SQL-string filters with a throwaway `SessionContext` over the node and edge batches | `src/graph/policy.rs::surviving_keys` |
| `peql` Python bindings expose `Caller(id, purpose, tenant, tier, classification)` and `Engine.from_json_contracts_dir/from_json_contracts/query` | `bindings/python/src/lib.rs` |
| peQL's `Caller::new(id, purpose, tenant)` differs in argument order from parcel's `Caller::new(id, tenant, purpose)`, and parcel's has no `classification` | `src/contract_source.rs`, `parcel-runtime/src/lib.rs` |
| parcel requires `expose` unless the contract inherits. peQL treats an omitted `columns` as "expose everything" | `parcel-core/src/check.rs:257`, `contract_source.rs` |
| `lance` 6.0.1 depends on DataFusion 53.1, while peQL uses 47, so two DataFusion versions are already in the lock file | `Cargo.lock` |
| Whether `cargo build --features lance` succeeds at v0.3.0 | **Not verified from this seat.** Settle with `cargo check --features lance` (needs `protoc`) |
| Whether the 0.3 test suite passes at v0.3.0 on current stable | **Not verified.** Settle with `cargo test && cargo test --features platform` |
| parcel's reference executor was removed at `e6d70ce`. Its write, validate, query, manifest, shape and budget code is at `d9a059e` | parcel git history |

### 2.3 Build versus adopt

peQL already adopts DataFusion for planning. The question is how much of 0.3's enforcement machinery to keep. The runtime operators (`RowFilterExec`, `MaskingExec`) exist because 0.3 enforces after the scan. They are correct, hardened against real findings, and well tested. Their weakness is where they sit, below the optimiser's view, not what they compute. The view model hands row filtering and masking to DataFusion's own `Filter` and `Projection` nodes, which the optimiser understands. Keeping the operators would mean keeping a second evaluator for the same rules. So the operators' behaviour is kept, as parcel built-ins with identical outputs checked by ported tests, and the operators themselves are deprecated. `LaplaceNoiseExec`, `ScanMetricsExec` and `AttestationExec` do things a view cannot, so they stay and are rewired.

## 3. Sufficiency criteria

- **S1. Nothing is lost.** Every row of the capability ledger (§5.1) ends in *re-expressed*, *kept* or *kept and rewired*. None ends in *removed*.
- **S2. Same answers.** 0.3's behavioural tests run against 0.4. Changes are limited to import paths, the `Caller` constructor and DataFusion type names, and every test passes. That covers masking outputs, row filtering, projection hiding, purpose denial, DP budget enforcement, the DDL guard and graph walls. Where a test asserts on an internal type that 0.4 deprecates, it keeps running against the deprecated type until 0.5.
- **S3. Old contracts keep working.** Every JSON contract accepted by 0.3's `JsonContractSource` is accepted by 0.4's `Engine::from_json_contracts_dir`, and it gives the same rows to the same callers. Behind the scenes it is converted to a parcel contract and compiled. A JSON contract that cannot be converted exactly is refused with a message naming the construct. It is never downgraded silently.
- **S4. One policy language.** peQL 0.4 has no code that evaluates a contract rule other than parcel's compiled expressions and parcel-runtime's interpreter. `grep` for `MaskAction`, `row_filter`, `dp_columns` or `ResolvedPolicy` in non-deprecated modules finds nothing.
- **S5. Pushdown works.** For a Parquet binding, an `admit` on a partition column prunes files. A caller predicate is pushed into the Parquet scan, and a transform on an unselected column is left out of the physical plan. Each of these is asserted on `EXPLAIN` output in a test.
- **S6. Enforcement can be checked.** Every physical plan the engine executes has a gate for every contract named in the query, and the engine refuses to execute a plan without one. A test builds a plan by hand without a gate and sees it refused.
- **S7. The full contract lifecycle runs:**
  - a write stores flags, derived columns and a manifest
  - validation returns the verdict and the data hash
  - `guarantee` rules deny or annotate at query time
  - tenant WebAssembly functions run in both engines under their pinned hash

  parcel's former end-to-end, shapes, wasm and split tests, ported from `d9a059e`, pass against peQL.
- **S8. The handoff is a file.** `parcel compile -o x.parcel.json` produces a bundle, and `peql register x.parcel.json` (or `Engine::register_bundle`) accepts it after recompiling and matching its compilation hash. Nothing else crosses the boundary.
- **S9. Downstream builds keep building.** The Python package, `K04DEngine`, `LongRunningPoolManager`, `QueryCache`, `ResultFormatter` and the `platform` and `lance` features all still compile and pass their tests in 0.4.

## 4. Architecture

### 4.1 The boundary

```
            parcel (language)                         peQL (runtime)
 ┌────────────────────────────────────┐      ┌─────────────────────────────────────────┐
 │ parcel-core                        │      │ Contract store  ◀── bundles, JSON, YAML  │
 │   parse · check · compile          │      │ Function store  ◀── wasm + manifest      │
 │   → CompiledContract               │      │ Resolver        decide · guarantee · unless
 │   → ValidationPlan  → WritePlan    │ ───▶ │ View builder    admit · expose · transform
 │ parcel-runtime                     │bundle│ Gate            proof + scan metrics     │
 │   Caller · interpreter · params    │      │ Shapes          noise · suppress · sample │
 │   validate(plan, table) · wasm     │      │ Write path      flags · layout · manifest │
 │   bundles · SQL/Substrait export   │      │ Envelope · attestation · audit           │
 │ parcel-cli: compile · check ...    │      │ Graph           snapshots under contracts│
 └────────────────────────────────────┘      │ Adapters        platform · lance         │
                                             └─────────────────────────────────────────┘
```

Only three things cross the line. The **artifacts** travel as a bundle file or as Rust values. **`parcel-runtime` functions** are ones peQL calls rather than reimplements. **Imported documents** are other formats (0.3 JSON, ODCS, T03) converted into a parcel `ContractDoc` by an importer that lives in parcel, because it is language knowledge.

peQL never parses CEL, and never decides what a rule means or when it can run. parcel never opens a file, keeps state, or sees a caller except as the `ctx` value it is given.

### 4.2 The query path in 0.4

1. `Engine::query(sql, caller)` runs the DDL guard (the 0.3 string check first, then DataFusion `SQLOptions` with DDL, DML and statements off, and `EXPLAIN` refused).
2. The engine gathers the table references. For each one, the **contract store** returns the current compiled contract, if the caller's tenant can see it (§5.3). Otherwise it reports "no contract", with the same message whether the contract is invisible or absent.
3. The **resolver**:
   - evaluates `decide` rules with `parcel_runtime::plan::refusal`
   - loads the manifest through the binding resolver
   - evaluates `guarantee` rules with `parcel_runtime::plan::dataset_value`
   - evaluates `unless` for each shape

   It returns a `Resolution`.
4. The **view builder** assembles scan, filter and projection in that order: scan the binding; filter on the admits and drop-level flags (the stored variants when the manifest is current, else live); project the stored or live expressions. It then binds `ctx` parameters with `param_values` and places a `Gate` node on top. The view is registered under the contract's name for this session only.
5. The caller's SQL is planned over the views. Shapes are applied to the caller's aggregates: suppress, noise at aggregate or row level (§5.4), and sample as a predicate in the view.
6. The engine checks that the physical plan holds a `GateExec` for every resolved contract, then executes it. The metrics from the gate and the scan fill the **envelope**. `AttestationExec` hashes the result, and an **audit record** is written for success and refusal alike.

### 4.3 How the retained parts attach

- **The graph layer** keeps its bundle loader, CSR traversal, caches and seven table functions. Only its governance input changes: the parcel contract's view replaces `ResolvedPolicy`'s SQL strings (§5.5).
- **`LaplaceNoiseExec`** becomes the implementation of `noise` at row level (§5.4).
- **`ScanMetricsExec`** wraps any binding whose provider reports no native scan metrics, such as `MemTable` or Lance.
- **`AttestationExec`**'s hashing becomes the envelope's attestation block. The T05 signing client stays behind the `platform` feature.
- **The platform `ContractSource` (T03)** maps its bundle to a parcel `ContractDoc` instead of a `ResolvedPolicy`.
- **The Lance provider** becomes a binding resolver behind the `lance` feature.
- **`K04DEngine`**, **`LongRunningPoolManager`**, **`QueryCache`** and **`ResultFormatter`** keep their APIs and call the new engine underneath (§5.7).

## 5. Component design

### 5.1 The capability ledger

Every 0.3 capability, its home in 0.4, and whether callers or authors see a change.

| 0.3 capability (module) | 0.4 home | Outcome | Visible change |
|---|---|---|---|
| JSON contract format (`contract_source.rs`) | parcel importer `parcel import peql-json`. `Engine::from_json_contracts_dir` calls it | re-expressed | none for authors. `parcel` can print the equivalent YAML |
| Purpose gate (`purposes`) | `decide: ctx.purpose in [...]` | re-expressed | refusal message names the rule |
| Owner sees raw values | importer emits `ctx.tenant == '<owner>' ? row.c : mask(row.c)` and `admit … \|\| ctx.tenant == '<owner>'` | re-expressed | none |
| Column projection (`columns`, omitted = all) | `expose`. Omitted `columns` imports as every column of the bound schema | re-expressed | none |
| Row filter as SQL (`row_filter`, `node_filter`) | `admit`. The importer parses the SQL with DataFusion's parser and prints CEL for the supported subset | re-expressed | an unsupported SQL construct is refused with its name (S3) |
| Masks `redact`, `hash_sha256`, `tokenize`, `partial`, `null`, `noop` (`MaskingExec`) | parcel built-ins `redact`, `hash_sha256`, **new** `tokenize`, **new** `mask_partial`, `null` literal in transforms, and no rule for `noop` | re-expressed | identical outputs, checked by ported tests |
| Hashing a numeric column to a string | **new in parcel:** a `transform` may change a column's type to `utf8` when its expose entry declares `utf8` | re-expressed | none |
| DP on row values, permissive budget (`LaplaceNoiseExec`, `dp_columns`) | parcel `shape: noise` with **new** `params.at: row`, run by `LaplaceNoiseExec` | kept and rewired | none. `at: aggregate` (parcel's current behaviour) is the new default for YAML authors only |
| DP budget tracker (`PrivacyBudgetTracker`) | budget store trait with in-memory and file implementations. Permissive mode kept as a store setting | kept and rewired | none |
| Deny without an existence oracle | contract store visibility (§5.3) | kept | none |
| DDL guard (`DdlGuard`) | kept as-is, plus `SQLOptions` and an `EXPLAIN` refusal | kept | `EXPLAIN` is now refused for callers (operators use `Engine::explain`) |
| Scan metrics, `query_with_stats` (`ScanMetricsExec`) | kept; also read from Parquet scans; carried in the envelope | kept and rewired | `QueryStats` gains pruning counters |
| Attestation envelope (`AttestationExec`) | the envelope's attestation block (query hash, result hash, contract and compilation hashes, events) | kept and rewired | extra fields |
| T05 signing (`t05_client.rs`, feature `platform`) | unchanged | kept | none |
| T03 signed bundle source (`platform/`) | maps to a parcel `ContractDoc` through the same importer path | kept and rewired | none |
| Lance tables via storaged (`lance_table.rs`, `storaged_client.rs`) | binding resolver `lance`, behind the feature | kept and rewired | none. Needs a Lance release built on DataFusion 55 (§8) |
| Graph queries: 7 UDTFs, walls, edge filter, masking (`graph/`) | kept. Governance comes from the parcel view (§5.5) | kept and rewired | graph contracts written in YAML use `binding: {graph: …}`. JSON graph contracts import unchanged |
| `K04DEngine`, sealed `EngineCore`, `InitConfig`, `ContractBundleHandle` | kept. `inject_contract_bundle` accepts a parcel bundle or (feature `platform`) a T03 bundle | kept and rewired | none |
| Optimiser rules `contract_check`, `row_filter`, `masking`, `dp_noise`, and `ResolvedPolicy` | deprecated in 0.4, deleted in 0.5 | deprecated | compiler warnings |
| `RowFilterExec`, `MaskingExec`, `ContractApprovedExec` | deprecated in 0.4 (the view and gate do their work), deleted in 0.5 | deprecated | compiler warnings |
| `QueryCache` | kept. Key changes from `(tenant, sql)` to `(sql, resolution fingerprint)` (§5.7) | kept and rewired | fewer false hits |
| `LongRunningPoolManager`, `ResultFormatter` | kept | kept | none |
| Python bindings (`peql` wheel) | kept; `Caller` signature unchanged; gains `write`, `validate`, `describe` | kept and extended | none |
| Docs site (Sphinx, GitHub Pages) | kept; pages rewritten for 0.4; `CONTRACT-FORMAT` becomes "JSON contracts (compatibility)" and links to parcel's language docs | kept | content |
| **New:** contract store with versions and publication, function store, resolver, manifests, write path, validation and data hash, suppress and sample, audit log, `describe`, `peql` CLI | from the peQL design; code ported from parcel `d9a059e` | added | new API |

### 5.2 Changes parcel makes first

These go into parcel before peQL starts its core work, so no 0.3 capability has to wait on peQL:

1. **Built-ins** `tokenize(x)` and `mask_partial(x)`, and the `null` literal in transforms. Outputs match `MaskingExec` byte for byte on `Utf8`, `Int64`, `Float64`, `Date32`, `Timestamp` and `Boolean`. The differential test covers them.
2. **Retyping transforms.** A `transform` may produce `utf8` for a non-string column when the column's `expose` entry says `utf8`. Today the checker requires the source type.
3. **`noise` at row level**: `params: {at: row | aggregate}`, default `aggregate`. The checker passes `at` through, and the runtime (peQL) implements both.
4. **`Caller.classification`** (string, default empty) is exposed as `ctx.classification`.
5. **The graph binding**: `binding: {graph: <dir>}`. `row.` means a node row. Rules scoped with `on: edges` read edge columns and apply only to edges. This is the single-contract model that 0.3 already uses: one policy governs both nodes and edges.
6. **The `peql-json` importer** in `parcel-core::import`, alongside ODCS. It parses 0.3 JSON, converts SQL predicates to CEL for the subset of comparisons, `AND`/`OR`/`NOT`, `IN` and `IS NULL`, and refuses anything else with the construct's name.

### 5.3 Contract store and visibility

`ContractStore` is a trait with a `put`, `get(name, caller)` and `list(caller)` interface, plus version history. There are two implementations:
- **`DirStore`** keeps bundles at `<root>/_peql/contracts/<name>/v<version>.parcel.json`. On open, it verifies each bundle by recompiling.
- **`MemoryStore`** is used for tests and for JSON contracts loaded at start.

A contract is visible to a caller when any one of these holds:
- it has no `owner`
- the caller's tenant is the owner
- the owner has published it to the caller's tenant or to `public`

Absent and invisible return the same error, which keeps 0.3's no-oracle property.

### 5.4 Shapes

`suppress` and `sample` are ported from `d9a059e`. `noise` has two modes:
- **`at: row`** is 0.3's behaviour. `LaplaceNoiseExec` is inserted directly above the gate for the named column, using 0.3's sampling (difference of two `Exp(1)` draws) and budget debits.
- **`at: aggregate`** is parcel's current behaviour. Noise goes on aggregate outputs, grouping by the noised column is refused, and queries that read it outside an aggregate are refused.

Both charge the same budget store.

### 5.5 Graph governance

A graph snapshot binding produces three tables to the view builder: nodes, edges and reverse edges. The graph layer asks the view builder for the node view and the edge view for the caller. Each is filtered by that scope's admits and projected by its transforms. It collects the surviving `pos` and `row` keys to build `GraphPolicy`, then applies the same projection to traversal output. That output replaces the `ContractApprovedExec → MaskingExec` stack.

Keeping the wall rule, the no-oracle messages and the caches means the graph tests pass unchanged. The graph code loses its own SQL-string evaluator and gains `ctx`-dependent rules for free, such as an edge visible only to one role.

### 5.6 Write path, manifests, validation

These are ported from `d9a059e`:
- **Write** conforms the batch, enriches it, computes flags and derived columns, clusters and partitions it, stamps the contract hash into the Parquet key-value metadata, validates, and writes the manifest.
- **Validation** calls `parcel_runtime::plan::validate` over the binding's provider and adds the data hash.

Manifests live at `<binding root>/_peql/manifests/`. Writes are available for Parquet bindings; Lance and graph bindings are read-only in 0.4, matching 0.3, which has no writes.

### 5.7 Keeping the downstream APIs

- **`Engine`**: `from_json_contracts_dir`, `from_json_contracts`, `query` and `query_with_stats` keep their signatures, apart from the DataFusion version inside `RecordBatch`. New methods are `open`, `register_contract`, `register_bundle`, `register_function`, `publish`, `write`, `validate`, `describe`, `explain` and `query_with_envelope`.
- **`Caller`**: peQL keeps its own `Caller` with the 0.3 constructor `(id, purpose, tenant)` and fields. `impl From<&peql::Caller> for parcel_runtime::Caller` maps `classification` and `tier` through.
- **`QueryCache`**: 0.3 keys on `(tenant_id, sql_sha256)`. Once rules depend on `ctx.roles`, `ctx.id` or `ctx.clearance`, two callers in one tenant can see different rows, so the old key would serve one caller's result to another. The key becomes `(sql_sha256, contract compilation hashes, bound parameter values, active shapes)`. Results with `noise` are cached as they are, since serving the same noisy answer spends no extra budget.
- **`K04DEngine`**: its bundle handle carries a parcel bundle (or a T03 bundle behind `platform`). `LongRunningPoolManager` is unchanged above it.

## 6. Handoff and compatibility

### 6.1 The handoff contract

The bundle is the interface, and it is versioned (`format: parcel-bundle/1`). It carries:
- the document and its ancestors
- the row schema
- the function modules it pins
- the three artifacts encoded with `datafusion-proto`
- the compilation hash

peQL accepts a bundle only after `Bundle::verify` recompiles it and the hashes match. So peQL runs exactly what parcel compiled, or nothing. A peQL build states the bundle format versions it reads, and parcel never changes an existing format version's meaning.

Both repositories publish the same page, *parcel and peQL*, describing this boundary. parcel's docs site (new, built the same way as peQL's: Sphinx, MyST and Shibuya, deployed to GitHub Pages) documents the language, the CLI and the crates. peQL's documents the runtime.

### 6.2 Versioning between the two

peQL pins parcel by git revision until the crates are published, then by semver. Each breaking change to parcel artifacts bumps the bundle format version, and peQL declares which versions it reads. The DataFusion version is the other shared dependency: the two projects move to a new DataFusion together, parcel first.

## 7. Failure modes and degraded behaviour

- **A JSON contract uses SQL the importer cannot convert.** Registration fails and names the construct and the contract. The engine does not start with a partial catalog. This is stricter than 0.3, which passed the SQL to DataFusion at query time. It is the price of S4.
- **The manifest was written under an older contract hash.** The view evaluates rules live instead of reading stored flags. Results are the same, only slower, and the envelope says `flags_materialised: false`.
- **No manifest (data written outside peQL).** The first query generates one by validating the binding. That first query is slow, and a `deny`-level breach makes the contract refuse queries until the data is fixed.
- **A bundle fails verification.** Registration fails, and the error shows both hashes. The stored contract keeps serving its previous version.
- **A WebAssembly function runs out of fuel or memory.** The query fails with the function name. Nothing partial is returned.
- **The budget store is unavailable (file store).** Queries with `noise` fail closed, and queries without it are unaffected.
- **Lance or the platform feature cannot build on DataFusion 55.** The feature is marked unavailable in 0.4.0, and the default build still passes. §8 covers it.

## 8. Explicit risks and accepted positions

- **Lance and DataFusion 55.** `lance` 6.0.1 pulls in DataFusion 53.1. peQL 0.4 needs the Lance release built on DataFusion 55. If none exists at release time, the `lance` feature ships in 0.4.1 rather than holding 0.4.0. That is a delay, not a removal, and it is stated in the changelog. Settle with `cargo search lance` and the crate's DataFusion dependency on crates.io.
- **Caller predicates pushed below the contract filter.** With pushdown (S5), a caller predicate that errors, such as a division by zero, can error on a row the contract would have hidden. That reveals that such a row exists. 0.3 did not have this exposure, because it applied the caller's predicates after enforcement. **Position:** predicates are pushed only when every function in them is on a no-error list (comparisons, boolean logic, `IN`, `IS NULL`, `LIKE`). Anything else stays above the gate. The gate's `prevent_predicate_push_down_columns` enforces this, and a test covers the division case.
- **Deprecation instead of deletion** keeps about 3,000 lines of superseded operators and rules for one release. Accepted, because it means downstream code keeps compiling.
- **parcel's SQL-to-CEL conversion** covers a subset. Accepted, because it refuses loudly (S3), never silently.
- **Aggregate-level noise as the default for new YAML contracts** differs from 0.3's row-level default. Accepted: imported JSON contracts keep `at: row`, so no existing contract changes behaviour.

## 9. Implementation plan (appendix)

Each phase ends at a gate that names the S-criteria it proves. All work goes on `main` of each repository, as the owner asked.

1. **Baseline.** Build and test peQL v0.3.0 as it stands, with the default features and then `platform` and `lance`. Record which tests pass as the compatibility suite. *Gate: the baseline is recorded (input to S2 and S9).*
2. **parcel changes (§5.2).** Add the built-ins, retyping transforms, `noise at`, `classification`, the graph binding with `on: edges`, and the `peql-json` importer. Every 0.3 JSON contract in peQL's tests and examples imports. *Gate: S3 at the importer level; the parcel differential test covers the new built-ins.*
3. **DataFusion 55 upgrade of the kept modules** (graph, `LaplaceNoiseExec`, `ScanMetricsExec`, `AttestationExec`, cache, pool, formatter, platform, lance). *Gate: the kept modules' own tests pass (S9, partly).*
4. **Core runtime.** Build the contract store, resolver, view builder, gate, envelope, audit and the `Engine` API. Wire `from_json_contracts_dir` through the importer, and deprecate the superseded rules and operators. *Gate: the 0.3 behavioural tests pass through the new path (S2), plus S4, S5 and S6.*
5. **Lifecycle.** Port the write path, manifests, validation, function store, suppress and sample, noise in both modes, and the budget store. Port the parcel tests from `d9a059e`. *Gate: S7 and S8.*
6. **Graph rewire (§5.5).** *Gate: the graph tests pass unchanged (S2).*
7. **Adapters.** Rewire `K04DEngine`, the T03 mapping, the Lance binding and the Python bindings. *Gate: S9.*
8. **CLI, examples, docs.** Build the `peql` CLI, move the example workspaces (quickstart, utility) from parcel, rewrite the peQL docs, and add the parcel docs site and the shared boundary page. *Gate: the examples run in CI, and both docs sites build with `-W`.*
9. **Release 0.4.0** with a changelog that lists every row of §5.1.

## 10. Open questions for Brackly

1. **How long does the deprecation window last?** This design deprecates the four optimiser rules, `ResolvedPolicy` and the three superseded operators in 0.4 and deletes them in 0.5. Downstream code that names them directly, such as Griot Cloud's K04D, has one release to move. Is one release enough for the platform, or should they stay until 1.0?
2. **Should JSON contracts remain an authoring format?** In this design they keep working through the importer, but new features (guarantees, shapes other than noise, functions, inheritance) are only available in parcel YAML. The alternative is to extend the JSON format. The design recommends leaving JSON at its 0.3 features.
