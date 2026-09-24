# parcel v0: the first working version

**Status: done.** The end-to-end test (`crates/parcel-engine/tests/e2e.rs`) passes, and so does the profile-wide differential test (`crates/parcel-engine/tests/differential.rs`). The CLI runs the whole workflow on `examples/quickstart`. What was built beyond the plan is listed at the end.

What "working" means, what we build to get there, and what we deliberately leave out. The full design is in `parcel-README.md`; this document is the cut of it we build first.

## Definition of done

One end-to-end test passes:

1. The `sales/orders` contract (`crates/parcel-core/tests/fixtures/sales_orders.yaml`) and a Parquet file compile into all three artifacts: `CompiledContract`, `ValidationPlan`, `WritePlan`.
2. `parcel check` over that file reports each assertion's pass rate, and the CEL interpreter and DataFusion agree on every rule for every sample row.
3. A test harness writes data under the contract, queries it as tenant `acme` and as tenant `globex`, and gets different, correct results.
4. Compiling the same contract twice gives the same hash.

## Crates

```
parcel-core      parse, check, classify, translate, assemble; no I/O
parcel-runtime   registers built-in functions into a DataFusion SessionContext
parcel-cli       compile, report, check
```

`parcel-udf` (WebAssembly user functions) waits for v1.

## Work items

| # | Item | Crate | Milestone |
|---|---|---|---|
| 1 | Contract document model (serde, YAML/JSON), canonical form and hash | core | M1 ✅ |
| 2 | Type system and Arrow mapping | core | M1 ✅ |
| 3 | Namespace tables: `ctx` fixed, `row` from schema, `dataset` from schema and assertion ids | core | M1 ✅ |
| 4 | Parse (the `cel` crate) | core | M1 ✅ |
| 5 | Check: typed IR, profile restrictions, registry resolution and pinning, namespace tags | core | M1 ✅ |
| 6 | Classify: namespaces fit the operation; shape parameters validated | core | M1 ✅ |
| 7 | Translate: IR → DataFusion `Expr`, `ctx` → placeholders, null-safe predicates | core | M2 ✅ |
| 8 | Assemble `CompiledContract` and the per-rule report | core | M2 ✅ |
| 9 | Built-in registry implementations (CEL function + DataFusion UDF per entry) | runtime | M2 ✅ |
| 10 | `register(session, pins)` | runtime | M2 ✅ |
| 11 | Assemble `ValidationPlan` (one-row aggregate `LogicalPlan`) | core | M3 ✅ |
| 12 | Differential test: interpreter vs DataFusion on a sample | cli | M3 ✅ |
| 13 | `parcel compile`, `parcel report`, `parcel check` | cli | M2–M3 ✅ |
| 14 | Assemble `WritePlan` (flags, stats, layout) | core | M4 ✅ |
| 15 | Serialise artifacts through `datafusion-proto` | core | M4 ✅ |
| 16 | Stand-in peQL in `tests/` (bind ctx, write flags, view, gate, query) and the end-to-end test | tests | M4 ✅ |

## Out of scope for v0

- WebAssembly user functions and `parcel-udf` (design 7.4)
- `inherits` (design 12)
- `extensions`, `other` fields, `enrich`, `dataset_other` (design 3.1, 10)
- The split pass (design 8, "Split")
- Substrait output of the validation plan
- Executing shape operators. parcel validates them and passes them through; peQL runs them.

The checker rejects each of these with code `Unsupported` rather than ignoring it.

## Decisions made in M1

**parcel lowers CEL into its own typed IR (`ir.rs`).** The `cel` crate's parser expands macros into raw comprehensions and carries no types. The checker lowers its AST into `TExpr`, in which every node carries:
- its type
- the namespaces it reads
- pinned function references
- the macro kind (`exists`, `all`, `filter`, `map`), named again

Classify, split, translate and hashing will all read the IR, never CEL's AST.

**No decimals in rules.** The reference interpreter has no exact decimal type, so the differential test could not vouch for a decimal rule. Decimal columns can be exposed as pass-through; a rule that reads one is rejected with `UnreadableColumn`. The fixture holds money as integer cents. Supporting decimals means adding them to the reference evaluator first.

**Strict typing and no `null` literal.**
- `row.qty > 1.5` is an error when `qty` is an integer; the message suggests `double()`.
- `null` literals are rejected in favour of `has(row.col)`. CEL's null and SQL's NULL mean different things, and allowing the literal invites rules that behave differently in the two engines.

**Patterns must be literals.** `matches()` takes a literal pattern, validated at check time. `duration()` and `timestamp()` of a string take literals. That keeps both engines on one known value.

**`dataset` paths resolve by depth:**
- `dataset.row_count`, `dataset.written_at`, `dataset.contract_hash`
- `dataset.<column>.<stat>`
- `dataset.assertions.<id>.pass_rate`, where `<id>` must name an assert in the same contract

**Rule ids match `[a-z][a-z0-9_]*`**, because they become column names (`_c_<id>`).

## Risks carried into M2–M3

- **Macros in DataFusion.** `exists`, `all`, `filter` and `map` with arbitrary bodies may have no direct DataFusion equivalent. Fallback: the common shapes (`x in list`, `list.exists(v, v == c)`) become array functions, and the rest becomes an opaque residual with the report saying so.
- **Semantic drift between CEL and DataFusion** in integer overflow, regex dialect (CEL uses RE2; DataFusion uses Rust `regex`, which is close) and time zones. The differential test in M3 is the guard. It runs on every `check`, not only in CI.

## What v0 shipped beyond the plan

- **`parcel-engine`**, a reference executor. The plan called for a stand-in peQL inside `tests/`. It became a crate, because the CLI needs the same machinery to write, validate and query. It follows peQL's design:
  - views with a gate-free, catalog-free session: callers can reach only contract views
  - resolution per caller
  - flags read from storage when every file was written under the current contract hash
  - `suppress`, deterministic `sample`, and Laplace `noise` with a per-caller budget
- **CEL macros compile to DataFusion 55 lambdas** (`array_any_match`, `array_filter`, `array_transform`) instead of being residual, so they are checked by the differential test like everything else.
- **Bundles** (`parcel compile -o`). The artifacts are encoded with `datafusion-proto`. Loading a bundle recompiles and compares byte for byte, and a verifier holding only the bundle reproduces the verdict.
- **Reference-side conformance fixes**, found by the differential test:
  - the `cel` crate counts string `size` in bytes, so the reference uses `parcel_strlen`
  - `string()` is limited to int, uint and string, because CEL and Arrow format doubles, bools and times differently
  - DataFusion has no `size` for bytes, so parcel adds the `parcel_bytes_len` UDF
