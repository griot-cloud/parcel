# peQL 0.4 on parcel: Architecture Design

**Document type:** full architecture design (not an ADR)
**Status:** APPROVED DIRECTION, revision 2 · 2026-09-25
**Scope:** peQL 0.3.0 becomes the runtime for parcel contracts. Every policy feature peQL has today is re-expressed through parcel, keeps the same observable behaviour, and is improved where 0.3 was weak. Both repositories change as needed so they form one ecosystem. **Out of scope:** Flight SQL, federated execution, certificate issuance, ODCS export, and compatibility with 0.3's JSON contract format. **Removed from the engine:** graph datasets (§5.5).
**Companion:** `design/parcel-README.md` (the contract language), the peQL design README (the runtime shape), `design/parcel-ecosystem.md`.

---

## 1. Problem statement

**P1. peQL and parcel each have their own policy model.** peQL 0.3 reads JSON contracts: `purposes`, `columns[].mask`, `masks`, `row_filter` as a SQL string, `dp_columns`, and `owner_tenant`. It resolves each one into a `ResolvedPolicy` per caller. parcel compiles CEL contracts into DataFusion expressions that do the same jobs: `decide`, `expose`, `admit`, `transform`, and `shape: noise`. The same intent is written in two languages and enforced by two sets of code, which drift apart. Contract authors pay by learning which format a feature lives in. Maintainers pay by fixing every enforcement bug twice.

**P2. peQL enforces policy with runtime operators the optimiser cannot see.** `ContractTableProvider::scan` in 0.3 does the following:
- scans the inner table with no projection and no filters
- stacks `ScanMetricsExec → ContractApprovedExec → RowFilterExec → MaskingExec → LaplaceNoiseExec` on top
- only then applies the caller's projection and limit

The OSS binding also loads the whole Parquet file into a `MemTable`. So no query pushes a predicate down, prunes a row group, or skips a column. Every query costs the whole dataset.

**P3. Parts of the contract lifecycle are missing, and parcel defines them.** peQL 0.3 has none of the following:
- a write path
- manifests, flags or dataset statistics, so no `guarantee` rules
- a validation verdict
- tenant functions
- a publication model

parcel compiles a `WritePlan` and a `ValidationPlan`, but nothing runs them over stored data.

**P4. The two projects run on different DataFusion versions.** peQL is on DataFusion 47 with Arrow 55. parcel is on 55.1 with Arrow 59. parcel's artifacts are DataFusion values, so peQL cannot consume them until the versions match.

**P5. peQL has policy features parcel cannot express yet.** These would be lost if peQL simply adopted parcel:
- the `tokenize`, `partial` and `null` masks
- hashing a non-string column into text
- noise on row values rather than on aggregates
- the caller's `classification`

parcel gains each one (§5.2).

**P6. The graph layer sits inside the engine but is really a consumer of it.** The graph layer is seven table functions, a bundle format, CSR traversal and its own policy compiler. The compiler evaluates SQL-string filters with a throwaway `SessionContext` and reuses the masking operators by hand. That is a second enforcement path inside a policy engine. Nodes and edges are two tables. Recursive SQL over two governed views gives traversal, and the wall rule falls out of the edge contract. The engine does not need to know about graphs. The design removes the layer from the engine (§5.5).

## 2. Context

### 2.1 Constraints that shape the design

- **parcel is the language; peQL is the runtime.** parcel parses, checks and compiles. `parcel-runtime` holds what any engine needs to execute artifacts: the reference interpreter, parameter binding, validation over any table, WebAssembly loading, bundles and exports. peQL owns everything that touches stored data or a live caller. **Consequence:** every policy feature in peQL is re-expressed as a parcel concept. peQL keeps only machinery that executes parcel artifacts or serves them: storage, planning, operators, adapters, APIs.
- **Re-expressed means the same behaviour, done better.** Each 0.3 behaviour a caller can observe is kept: rows returned, masked values, refusals, budget exhaustion. Where 0.3's mechanism was weak (full-table scans, SQL strings evaluated at query time, a cache key that ignores roles), 0.4 fixes it. Superseded code is deleted, not deprecated: there is no JSON compatibility layer and no shim for `ResolvedPolicy`.
- **Changes land on both sides.** A mismatch between parcel and peQL is fixed wherever it belongs: in parcel when it is language, in peQL when it is runtime.
- **Open core.** Both repositories are public. The Griot platform adapters (T03, T04 storaged, T05 notary) stay behind feature flags, and the open-source build needs no platform service.
- **Shared types come by crate, not by copy.** peQL depends on `parcel-core` and `parcel-runtime` as a git dependency until they are published on crates.io.

### 2.2 Verified current state

The executing agent re-verifies every row before acting on it.

| Fact | Where verified |
|---|---|
| peQL is at `17ea448`, tagged `v0.3.0`, on DataFusion 47 and Arrow 55 | `git -C peql log -1`, `Cargo.toml` |
| peQL's source is 12,348 lines of Rust in `src/`; the tests hold 219 test functions | `wc -l`, `grep -c` |
| `Engine` builds a per-caller catalog. `PeqlSchemaProvider::table` resolves `ContractSource` → `ResolvedPolicy` → `ContractTableProvider` | `src/engine.rs`, `src/catalog.rs` |
| `ContractTableProvider::scan` scans with no pushdown, then stacks the enforcement operators | `src/contract_table_provider.rs` |
| `LaplaceNoiseExec` sits below the caller's aggregates, so it noises row values, with a permissive budget | `contract_table_provider.rs` (`new_permissive`) |
| The four optimiser rules (`contract_check`, `row_filter`, `masking`, `dp_noise`) are used by `K04DEngine` paths and tests, not by `Engine` | `grep -rln build_pipeline` |
| The OSS binding reads a whole Parquet file into a `MemTable` | `src/binding.rs` |
| `MaskingExec` gives `"***"` for redact and `"***" + last 4` for partial. Hash, tokenize and partial on non-string columns output `Utf8` | `src/physical/masking_exec.rs:558-567` |
| parcel's `redact` returns `"***"` | `parcel-runtime/src/reference.rs:325` |
| `QueryCache` keys on `(tenant_id, sql_sha256)` | `src/query_cache.rs` |
| Python bindings: `Caller(id, purpose, tenant, tier, classification)`, `Engine.from_json_contracts_dir/from_json_contracts/query` | `bindings/python/src/lib.rs` |
| DataFusion 55 enables recursive CTEs by default (`enable_recursive_ctes = true`) | `datafusion-common-55.*/src/config.rs:989` |
| `lance` 6.0.1 depends on DataFusion 53.1 | `Cargo.lock` |
| Whether `--features lance` and the 0.3 test suite pass at v0.3.0 | **Not verified.** Settle with `cargo test`, `cargo test --features platform`, `cargo check --features lance` |
| parcel's former reference executor is at `d9a059e` | parcel git history |

## 3. Sufficiency criteria

- **S1. No lost behaviour.** Every behaviour row of the ledger (§5.1) is re-expressed through parcel or kept in the runtime. The graph layer is the only removal, and it is replaced by a documented recursive-SQL pattern that passes the wall tests (S8).
- **S2. Same answers.** 0.3's behavioural tests are ported to parcel contracts and pass: masking outputs byte for byte, row filtering, projection hiding, owner-sees-raw, purpose denial, DP noise and budget exhaustion, and the DDL guard.
- **S3. One policy language.** peQL 0.4 contains no rule evaluator and no shape logic other than parcel's compiled expressions, `parcel_runtime::shape` and parcel-runtime's interpreter. `ResolvedPolicy`, `MaskAction`, `RowFilterExec`, `MaskingExec`, `ContractApprovedExec`, `LaplaceNoiseExec`, `PrivacyBudgetTracker`'s parameter validation and `optimizer_rules/` no longer exist.
- **S4. Pushdown works.** For Parquet bindings, an admit on a partition column prunes files, a safe caller predicate reaches the Parquet scan, and a transform on an unselected column is absent from the physical plan. Each is asserted on `EXPLAIN` output.
- **S5. Enforcement can be checked.** The engine refuses to execute any physical plan that lacks a gate for every contract in the query. A hand-built ungated plan is refused in a test.
- **S6. The full lifecycle runs:**
  - a write stores flags, derived columns and a manifest
  - validation returns the verdict and the data hash
  - guarantees deny or annotate at query time
  - tenant WebAssembly functions run under their pinned hash

  parcel's former end-to-end, shapes, wasm and split tests pass against peQL.
- **S7. The handoff is a file.** `parcel compile -o x.parcel.json` produces a bundle, and `peql register x.parcel.json` accepts it after matching the compilation hash.
- **S8. Graph traversal works outside the engine.** Nodes and edges registered as two contracts, traversed with `WITH RECURSIVE`, reproduce the wall rule: no path through a hidden node, and hidden edges are unused.
- **S9. Downstream keeps working:**
  - the Python package, with the same `Caller` signature
  - `K04DEngine`
  - `LongRunningPoolManager`
  - `QueryCache`
  - `ResultFormatter`
  - the `platform` and `lance` features

  Each builds and passes its tests.

## 4. Architecture

### 4.1 The boundary

```
            parcel (language)                         peQL (runtime)
 ┌────────────────────────────────────┐      ┌─────────────────────────────────────────┐
 │ parcel-core                        │      │ Contract store  ◀── bundles, YAML        │
 │   parse · check · compile          │      │ Function store  ◀── wasm + manifest      │
 │   → CompiledContract               │      │ Resolver        decide · guarantee · unless
 │   → ValidationPlan  → WritePlan    │ ───▶ │ View builder    admit · expose · transform
 │ parcel-runtime                     │bundle│ Gate            proof + scan metrics     │
 │   Caller · interpreter · params    │      │ Shapes          noise · suppress · sample │
 │   validate(plan, table) · wasm     │      │ Write path      flags · layout · manifest │
 │   bundles · SQL/Substrait export   │      │ Envelope · attestation · audit           │
 │ parcel-cli: compile · check ...    │      │ Adapters        platform · lance         │
 └────────────────────────────────────┘      └─────────────────────────────────────────┘
```

Two things cross the line. **Artifacts** travel as a bundle file or as Rust values. **`parcel-runtime` functions** are ones peQL calls rather than reimplements. peQL never parses CEL and never decides what a rule means. parcel never opens a data file, keeps state, or sees a caller except as the `ctx` value it is given.

### 4.2 The query path in 0.4

1. `Engine::query(sql, caller)`: the SQL guard first. The 0.3 comment-aware verb check runs, then DataFusion `SQLOptions` with DDL, DML and statements off, and `EXPLAIN` is refused for callers.
2. For each table reference, the **contract store** returns the current compiled contract if the caller's tenant can see it. Otherwise it reports that no such contract exists, with the same message whether the contract is invisible or absent.
3. The **resolver** does three things:
   - evaluates `decide` rules (`parcel_runtime::plan::refusal`)
   - loads the manifest, then evaluates `guarantee` rules against it (`dataset_value`)
   - evaluates each shape's `unless`
4. The **view builder** assembles the view:
   - it scans the binding, streaming, with pruning
   - it filters on the admits and drop-level flags, using the stored variants when the manifest is current
   - it projects the exposed columns and transforms
   - it binds `ctx` parameters
   - it adds a `Gate` node on top
5. The caller's SQL is planned over the views, and shapes are applied (§5.4).
6. The engine checks the physical plan for gates, then executes it. Scan and gate metrics fill the **envelope**. `AttestationExec` hashes the result. An **audit record** is written for both success and refusal.

## 5. Component design

### 5.1 The ledger

| 0.3 behaviour or module | 0.4 | Improvement over 0.3 |
|---|---|---|
| Purpose gate (`purposes`) | `decide: ctx.purpose in [...]` | the refusal names the rule; any `ctx` condition, not only purpose |
| Owner sees raw values | `transform: ctx.tenant == owner ? row.c : …` and `admit: … \|\| ctx.tenant == owner` | owners are an ordinary rule, so "owner plus auditors" is one more condition |
| Column projection (`columns`) | `expose` | unexposed columns do not exist for the caller, even in `WHERE` |
| Row filter (`row_filter`, SQL string) | `admit` in CEL | type-checked at compile time; pushed into the scan; prunes partitions and row groups |
| Masks `redact`, `hash_sha256`, `tokenize`, `partial`, `null` (`MaskingExec`) | `transform` rules over built-ins: `redact` (fixed `***`), `hash_sha256`, `partial(value, n)`, and `cond ? row.x : null`. `tokenize` was an alias of SHA-256 and is `hash_sha256` | a projection the optimiser prunes when unselected, folded away for callers who see raw. `redact` no longer depends on value length, `partial` no longer reveals values of 4 characters or fewer, and `null` is a real null instead of 0.3's empty string |
| Hashing a numeric column to text | a transform may retype to `utf8` when `expose` declares `utf8` | the type is declared in the contract, not inferred by an operator |
| Row-level DP noise with a permissive budget (`LaplaceNoiseExec`, `dp_columns`) | `shape: noise` with `at: row`, compiled by parcel into the view's projection (`parcel_laplace`), with `unless` a bound parameter. `LaplaceNoiseExec` is deleted | noise is part of the plan; the budget is charged once per query, only when the query reads the column; the permissive mode is an explicit store setting |
| (new) aggregate-level DP | `shape: noise` with `at: aggregate`: a rewrite of the caller's plan in `parcel_runtime::shape` | noise on aggregate outputs; refuses reads outside aggregates and grouping by the column |
| DP budget tracker | budget store trait with in-memory and file implementations | survives restarts |
| Deny without an existence oracle | contract store visibility (§5.3) | also covers unpublished contracts |
| `DdlGuard` | kept, plus `SQLOptions` and an `EXPLAIN` refusal | closes the plan-level bypasses parcel's executor found (DDL, `EXPLAIN`, `UNION` branch, scalar subquery) |
| `ScanMetricsExec`, `query_with_stats` | kept for providers without native metrics; Parquet scans report their own | adds pruning counters (files and row groups skipped) |
| `AttestationExec` | the envelope's attestation block | carries contract and compilation hashes and the data hash |
| T05 signing, T03 bundle source (feature `platform`) | T03 bundles convert to a parcel contract | one enforcement path for platform and OSS |
| Lance via storaged (feature `lance`) | a binding resolver | pushdown through the Lance scanner, as today, now under a gate |
| `K04DEngine`, `EngineCore`, `InitConfig`, `ContractBundleHandle` | a bundle handle carries a parcel bundle, and `K04DEngine` wraps `Engine` | one enforcement path; the sealed trait is kept |
| Optimiser rules, `ResolvedPolicy`, `RowFilterExec`, `MaskingExec`, `ContractApprovedExec` | deleted; their jobs are the view and the gate | about 3,000 lines of duplicate enforcement gone |
| JSON contracts, `JsonContractSource` | deleted; contracts are parcel YAML or bundles | one format |
| `QueryCache` | key becomes `(sql, compilation hashes, bound parameters, active shapes)` | fixes a cross-caller leak: 0.3 would serve one caller's rows to another in the same tenant once rules depend on roles |
| `LongRunningPoolManager`, `ResultFormatter` | kept, on the new engine | none needed |
| Python bindings | kept; `Caller` unchanged; `Engine.open`, `register`, `query`, `write`, `validate`, `describe` | the full lifecycle from Python |
| Graph layer (`graph/`, 7 UDTFs) | removed from the engine (§5.5) | traversal becomes recursive SQL over governed views |
| **New from the design:** contract store with versions and publication, function store, resolver, manifests, write path, validation and data hash, suppress and sample, audit log, `describe`, `peql` CLI | ported from parcel `d9a059e` and the peQL design | n/a |

### 5.2 Changes to parcel

Everything here is expressed in parcel's own terms: rules, CEL, built-ins, and the compiled artifacts.

1. **Built-in `partial(value, n)`.** `redact` returns a fixed `***`.
2. **`null` as a `?:` branch.** It takes the other branch's type.
3. **Retyping transforms.** A transform may produce `utf8` for a non-string column when `expose` declares `utf8`. The checker already allowed this, and tests now cover it.
4. **`Caller.classification`**, available to rules as `ctx.classification`.
5. **Shapes.**
   - `noise` gains `at: row | aggregate`, and requires a numeric column.
   - `sample` and row-level noise compile into the view, using `parcel_core::udfs`.
   - `suppress` and aggregate noise are rewrites in `parcel_runtime::shape`.
   - `parcel_runtime::plan::active_shapes` evaluates `unless`.
6. **A fix found on the way:** `bundle::session()` registered `bytes_len` twice.

### 5.3 Contract store and visibility

`ContractStore` is a trait with `put`, `get(name, caller)`, `list(caller)` and version history. There are two implementations:
- **`DirStore`** keeps bundles at `<root>/_peql/contracts/<name>/v<version>.parcel.json` and verifies each one by recompiling on open.
- **`MemoryStore`** is used for tests and embedding.

A contract is visible to a caller when any one of these holds:
- it has no `owner`
- the caller's tenant is the owner
- the owner published it to the caller's tenant or to `public`

Absent and invisible give the same error.

### 5.4 Shapes

Every shape is defined in parcel. peQL only charges budgets.

- **In the view, compiled by parcel:**
  - `sample` becomes a filter: `unless OR parcel_sample_bucket(key) < fraction`.
  - `noise at: row` becomes a projection: `CASE WHEN unless THEN col ELSE col + parcel_laplace(scale) END`, rounded back for integers.
  - `unless` is a bound `ctx` parameter, so the optimiser removes whichever branch does not apply to the caller.
  - The functions live in `parcel_core::udfs`, and every engine registers them.
- **On the caller's plan, by `parcel_runtime::shape::apply`:**
  - `suppress` drops groups under `k` in every aggregate, including `UNION` branches and subqueries. A query with no aggregate is one group.
  - `noise at: aggregate` noises aggregate outputs, and refuses reads outside aggregates and grouping by the column.
- **Charges:** `apply` returns them, one per budget at the largest epsilon, and only for noised columns the optimised plan reads. peQL charges them before execution and refuses the query when a budget is spent.

### 5.5 Graphs, outside the engine

In 0.3, peQL can read snapshot bundles (`nodes.parquet`, `edges.parquet`, `edges_rev.parquet`, `manifest.json`) with precomputed CSR offsets and digest checks. It offers seven table functions over them: `graph_node`, `graph_neighbors`, `graph_edges`, `graph_subtree`, `graph_path`, `graph_reachable` and `graph_nodes`. Its governance has three parts:
- hidden nodes are walls
- an edge filter hides edges
- output is masked

In 0.4, `nodes.parquet` and `edges.parquet` are ordinary contracted datasets. The node contract's `admit` decides visibility. The edge contract admits an edge only when its own rule holds and both endpoints are visible to the caller. Traversal is recursive SQL:

```sql
WITH RECURSIVE reach(pos, depth) AS (
  SELECT pos, 0 FROM "process/nodes" WHERE ulid = '01J...'
  UNION
  SELECT e.dst, r.depth + 1 FROM reach r JOIN "process/edges" e ON e.src = r.pos
  WHERE r.depth < 20
)
SELECT n.* FROM reach JOIN "process/nodes" n USING (pos)
```

A hidden node never appears in either view, so no path can route through it. That is the wall rule, with no special code. What is given up:
- CSR-offset speed (fine at the v1 size limits of 50k nodes and 200k edges)
- the near-miss name suggestions
- the ready-made function names

A helper crate can bring the names back as SQL macros later without touching the engine. The documented pattern and a test that reproduces the 0.3 wall cases are part of 0.4 (S8).

### 5.6 Write path, manifests, validation

These are ported from `d9a059e`:
- **Write** conforms and enriches the batch, computes flags and derived columns, clusters and partitions, stamps the contract hash into the Parquet metadata, validates, and writes the manifest.
- **Validation** is `parcel_runtime::plan::validate` over the binding plus the data hash.

Manifests live at `<binding root>/_peql/manifests/`. Lance bindings are read-only, as they are in 0.3.

### 5.7 APIs

- **`Engine`**: `open`, `in_memory`, `register_contract(yaml, schema)`, `register_bundle`, `register_function`, `publish`, `write`, `validate`, `describe`, `query`, `query_with_envelope`, `query_with_stats`, and `explain` (operators only).
- **`Caller`**: peQL keeps its 0.3 `Caller` (`new(id, purpose, tenant)`, `tier`, `classification`), plus `roles` and `other`, and converts it into parcel-runtime's `Caller`.
- **The `peql` CLI**: `register`, `write`, `validate`, `query`, `describe`, `list`, `publish`, and `function register|list`.

## 6. Handoff

The bundle (`format: parcel-bundle/1`) is the interface. It carries:
- the document and its ancestors
- the row schema
- the pinned function modules
- the three artifacts, encoded with `datafusion-proto`
- the compilation hash

peQL accepts a bundle only after `Bundle::verify` recompiles it and the hashes match. A breaking change to the artifacts bumps the format version, and peQL declares which versions it reads.

The two projects move DataFusion versions together, parcel first. Both documentation sites publish a *parcel and peQL* page describing this boundary. parcel's docs site is built the way peQL's is: Sphinx, MyST and Shibuya, deployed to GitHub Pages.

## 7. Failure modes and degraded behaviour

- **The manifest was written under an older contract hash:** rules run live. Results are the same, only slower, and the envelope says so.
- **No manifest exists (data written outside peQL):** the first query validates the binding to create one. A `deny` breach refuses queries until the data is fixed.
- **A bundle fails verification:** registration fails with both hashes shown, and the previous version keeps serving.
- **A WebAssembly function runs out of fuel or memory:** the query fails and names the function. No partial result is returned.
- **The budget store is unavailable:** queries that use `noise` fail, and all other queries are unaffected.
- **A recursive traversal has no depth bound:** DataFusion stops at its recursion limit and returns an error. The documented pattern always bounds depth.

## 8. Risks and accepted positions

- **Lance on DataFusion 55.** `lance` 6.0.1 uses DataFusion 53.1. If there is no Lance release on 55 by release time, the `lance` feature follows in 0.4.1. Settle by checking the latest `lance` crate's DataFusion dependency.
- **Caller predicates pushed below the contract filter.** A predicate that can raise an error (division by zero, a failed cast) could raise it on a hidden row and reveal that the row exists. 0.3 applied caller predicates after enforcement, so it was not exposed to this. **Position:** only predicates built from functions that cannot error are pushed through the gate (comparisons, boolean logic, `IN`, `IS NULL`, `LIKE`). The rest stay above it. A test covers the division case.
- **Graph traversal speed.** Recursive SQL is slower than CSR arithmetic. This is accepted within the v1 limits; a faster traversal can come back as a function outside the engine if it is ever needed.
- **Aggregate noise is the default for new contracts.** Contracts ported from 0.3 set `at: row`.

## 9. Implementation plan

All work goes on `main` of each repository. Each gate names the criteria it proves.

1. **parcel changes (§5.2).** *Gate: parcel's tests and the differential test pass, including the new built-ins.*
2. **peQL rebuilt on DataFusion 55.** Delete every module the ledger replaces with parcel (including `LaplaceNoiseExec` and `graph/`). Rewrite the runtime pieces against parcel's artifacts (`ScanMetricsExec`, `AttestationExec`, `DdlGuard`, cache, pool, formatter, platform, lance). *Gate: the kept modules' tests pass (S9, partly).*
3. **Core runtime.** Build the contract store, resolver, view builder, gate, shapes, envelope, audit and `Engine`. Port 0.3's behavioural tests to parcel contracts. *Gate: S2, S3, S4, S5.*
4. **Lifecycle.** Build the write path, manifests, validation, function store, budget store and bundle registration. Port parcel's tests from `d9a059e`. *Gate: S6, S7.*
5. **Graph pattern.** Add the documented recursive-SQL pattern and a wall test. *Gate: S8.*
6. **Adapters and bindings.** Update `K04DEngine`, the pool, the T03 mapping, Lance and Python. *Gate: S9.*
7. **CLI, examples, docs.** Build the `peql` CLI, the example workspaces and peQL's docs rewrite, plus parcel's docs site and the shared boundary page. *Gate: examples run in CI; both sites build with `-W`.*
8. **Release peQL 0.4.0**, with a changelog listing every ledger row.

## 10. Decisions taken

- JSON contracts are not carried forward (owner, 2026-09-25).
- The graph layer leaves the engine and traversal is expressed in SQL over governed views (owner, 2026-09-25, "we can have this easily expressed as a function not part of the engine").
- Superseded enforcement code is deleted in 0.4, not deprecated (owner: re-express through parcel, improve freely).
