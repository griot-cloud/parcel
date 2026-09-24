# parcel

**The contract language and compiler for peQL.**

You write a data contract in CEL. parcel checks it against the data's schema, works out what kind of rule each line is, and compiles it into something a query engine can enforce without slowing down. peQL is the engine that enforces it. parcel is the part that turns intent into an executable plan.

---

## 1. The problem, in plain terms

Organisations share data by copying it. Org A exports a file, strips a few columns, hands it to Org B, and hopes B does the right thing with it. Every copy is a place where the rules stop applying. Every recipient becomes a new custodian who was never party to the original agreement.

Transactional systems solved this problem years ago with APIs. You do not copy a bank's ledger to check a balance; you call an endpoint, the endpoint decides what you may see, and the data never leaves the bank. Analytical data has no equivalent. There is no interface that sits between a dataset and the people (or agents) querying it, that knows the rules, and that applies them inside the query itself.

peQL is that interface. It is a query engine where you never query a table; you query a **contract**. The contract knows where the data is and what each caller is allowed to do with it. The caller only ever sees the contract.

That leaves one hard question: **how do you write the contract?** It has to say things as different as "only the analytics team may use this", "drop rows outside East Africa", "the customer ID must never be null", "hash the phone number unless the caller owns the data", and "add statistical noise to salaries". It has to be readable by a person, checkable by a machine, and cheap to enforce on a large file. And it has to be extensible, because the fifth organisation to adopt it will need a rule the first four never imagined.

parcel is the answer to that question.

## 2. What a contract is

A contract is a document that describes one dataset from the outside. It has three parts:

1. **A binding.** Where the data physically is. Callers never see this.
2. **A schema.** Which columns the contract exposes, with their types. This is what a caller sees when they ask what the contract looks like, and what `SELECT *` returns.
3. **A set of rules.** Each rule is a CEL expression plus a small amount of metadata (an id, an operation type, and what to do on failure).

A contract can only restrict. It can hide columns, drop rows, transform values, and refuse callers. It can never reveal something the underlying data does not contain or that a parent contract already hid. This makes inheritance simple: a child contract is the parent's rules plus its own, and the result is always at least as restrictive as the parent.

## 3. Namespaces: where a rule's values come from

Every rule refers to variables. parcel defines exactly three groups of variables, and which group a rule uses determines how the rule is executed. This is the single most important idea in the design.

**`row`** holds the values of one record. If the bound data has columns `msisdn`, `amount`, `region`, a rule can refer to `row.msisdn`, `row.amount`, `row.region`. These values come from the data file.

**`ctx`** holds information about the caller. Who is asking, which tenant they belong to, the purpose they declared, their clearance level. These values come from the application that authenticated the caller. They are never read from the data.

**`dataset`** holds summary statistics about the whole bound dataset, computed when the data was written. Row count, null count and null rate per column, distinct count, min and max, and the pass rate of every row-level assertion in the contract. These values come from a statistics manifest stored next to the data.

Why this matters: look at four rules.

```
ctx.purpose == 'analytics'
row.region == 'EA'
row.tenant_id == ctx.tenant
dataset.msisdn.null_rate < 0.05
```

The first uses only `ctx`. It can be answered before any file is opened. The second and third use `row`, so they must be applied to every record: they become filters pushed into the scan. The fourth uses only `dataset`, so it is answered by reading one stored number. Three of the four rules cost nothing at scan time, and the author never had to say which was which.

The initial field lists:

| Namespace | Fields (v1) |
|---|---|
| `ctx` | `id` (string), `tenant` (string), `purpose` (string), `tier` (string), `clearance` (int), `roles` (list of string), `now` (timestamp, fixed at plan time) |
| `row` | one field per column of the bound schema, typed from Arrow |
| `dataset` | `row_count` (int), `written_at` (timestamp), `contract_hash` (string), and per column `<col>.null_count`, `<col>.null_rate`, `<col>.distinct_count`, `<col>.min`, `<col>.max`; and per assertion `assertions.<id>.pass_rate` |

These lists are owned by parcel and versioned with it. Adding a field is a minor version; removing or retyping one is a major version.

### 3.1 Extension fields: `other`

Each namespace has one more field, `other`, which is a map. It exists so that a deployment can attach information parcel never anticipated without waiting for a new parcel version: `row.other.risk_band`, `ctx.other.department`, `dataset.other.source_system`.

Where the values come from follows the namespace. `ctx.other` is supplied by the embedding application alongside the rest of the caller. `dataset.other` is written into the manifest by the write path. `row.other` is a physical column in the data, stored as an Arrow struct (or a JSON string column, which the writer converts to a struct), named `_other`. It is populated at write time by enrichers (see section 10).

How much the checker knows about `other` decides how rules over it execute. A contract may declare the shape of each namespace's `other` in an `extensions` section:

```yaml
extensions:
  row:
    risk_band: utf8
    geo_cell: int64
  ctx:
    department: utf8
  dataset:
    source_system: utf8
```

A declared field is typed and first-class: `row.other.risk_band == 'high'` is checked like any column reference, compiles to a field access on the `_other` struct, and pushes down into the scan with statistics on the struct field. An undeclared field is still legal but has CEL type `dyn`: the author must cast it (`int(row.other.score) > 5`), the checker reports the rule as residual, and it will not prune. The intent is that flexibility is free and pushdown is one declaration away.

`ctx.other` and `dataset.other` are bound at plan time regardless of declaration, so undeclared fields there cost nothing; the declaration only buys type errors at check time instead of at query time.

## 4. Operations: the kinds of rule a contract can contain

A namespace tells you where values come from. An operation tells you what the rule does with them. parcel defines seven operation types. Every rule in a contract is exactly one of them, and the checker rejects a rule whose namespaces do not fit its operation.

### 4.1 `decide`

Allow or deny the caller before anything is opened.

Namespaces allowed: `ctx` only. Result type: bool. Evaluated once at plan time. A false result denies the query.

```yaml
- id: analytics_only
  op: decide
  expr: ctx.purpose == 'analytics' || ctx.tenant == 'acme'
```

### 4.2 `expose`

Declare which columns the contract shows, in order, with the type each will have after any transform. This is the only operation that is not an expression; it is the schema section of the contract. A column not listed does not exist for the caller, not even in a filter.

```yaml
expose:
  - {name: order_id, type: int64}
  - {name: email, type: utf8}
  - {name: amount, type: decimal(18,2)}
```

### 4.3 `admit`

Decide, per row, whether the row exists for this caller.

Namespaces allowed: `row`, `ctx`. Result type: bool. Compiled to a predicate on the scan. A row for which the expression is false or null is not returned and cannot participate in any aggregate.

```yaml
- id: own_tenant_rows
  op: admit
  expr: row.tenant_id == ctx.tenant || ctx.roles.exists(r, r == 'admin')
```

### 4.4 `assert`

State something that should be true of every row, and say what happens when it is not.

Namespaces allowed: `row` only (no `ctx`, because an assertion is about the data, not the caller). Result type: bool. Evaluated at write time into a stored flag column and a pass rate. The `on_fail` field decides the query-time behaviour:

- `drop`: rows failing the assertion are not served. Behaves like `admit` but is precomputed.
- `deny`: if any row fails, the dataset is not servable under this contract until fixed.
- `report`: nothing is dropped; the pass rate is recorded in `dataset.assertions.<id>.pass_rate` and available to `guarantee` rules and to certificates.

```yaml
- id: pk_present
  op: assert
  expr: has(row.order_id)
  on_fail: deny

- id: msisdn_format
  op: assert
  expr: row.msisdn.matches('^254[17][0-9]{8}$')
  on_fail: drop
```

### 4.5 `transform`

Replace the value of an exposed column with an expression over the row.

Namespaces allowed: `row`, `ctx`. Result type: must match the type declared for that column in `expose`. Compiled to a projection. Applied after `admit` and before the caller's own query, so a caller's filter on a transformed column sees the transformed value, never the original.

```yaml
- id: mask_email
  op: transform
  column: email
  expr: ctx.tenant == 'acme' ? row.email : hash_sha256(row.email)
```

### 4.6 `guarantee`

State something that must be true of the dataset as a whole for it to be served.

Namespaces allowed: `dataset` only. Result type: bool. Evaluated once at plan time from the stored manifest. `on_fail` is `deny` (refuse the query) or `annotate` (serve, and attach the failed guarantee to the result envelope so the caller, human or agent, knows what they are getting).

```yaml
- id: fresh_enough
  op: guarantee
  expr: dataset.written_at > ctx.now - duration('72h')
  on_fail: annotate

- id: ids_mostly_present
  op: guarantee
  expr: dataset.customer_id.null_rate < 0.02
  on_fail: deny
```

(`ctx.now` is permitted here as the one exception to "dataset only", because freshness is a dataset property relative to query time.)

### 4.7 `shape`

Apply an operation that cannot be expressed row by row because it carries state across rows or across queries. These are not CEL expressions; they are named operators with parameters, and peQL implements each as a physical operator. parcel validates the parameters and passes them through.

Initial operators:

- `noise`: differential-privacy noise on an aggregate over a column, with `sensitivity`, `epsilon`, and a `budget` reference so spend is tracked across queries.
- `suppress`: drop any result group whose size is below `k` (minimum group size, the familiar small-cell suppression).
- `sample`: serve a deterministic fraction of rows keyed on a stable column, for callers who need a preview rather than the whole set.

```yaml
- id: salary_dp
  op: shape
  operator: noise
  column: salary
  params: {sensitivity: 1000, epsilon: 0.5, budget: hr_budget}
  unless: ctx.tenant == 'acme'
```

The `unless` clause is a `ctx`-only expression; when true, the operator is not inserted.

### 4.8 Summary

| Operation | Namespaces | When evaluated | Scan cost | Compiles to |
|---|---|---|---|---|
| decide | ctx | plan time | none | allow / deny |
| expose | (schema) | plan time | none | view schema |
| admit | row, ctx | per row | pushed to scan | filter Expr |
| assert | row | write time | pushed to scan (flag) | flag column + pass rate |
| transform | row, ctx | per row | projection | projection Expr |
| guarantee | dataset (+ ctx.now) | plan time | none | manifest check |
| shape | (params) + ctx for unless | per query | operator cost | physical operator |

Everything that touches data at query time is either a filter or a projection the engine already knows how to optimise, or a `shape` operator that was chosen deliberately.

## 5. The contract document

parcel reads YAML or JSON; both map to the same structure. A complete example:

```yaml
contract: sales/orders
version: 3
inherits: sales/base          # optional; rules are unioned, exposure intersected

binding:
  parquet: s3://acme-lake/orders/       # callers never see this
  partitioned_by: [region, dt]

expose:
  - {name: order_id, type: int64}
  - {name: customer_id, type: int64}
  - {name: email, type: utf8}
  - {name: region, type: utf8}
  - {name: amount, type: decimal(18,2)}

rules:
  - id: analytics_only
    op: decide
    expr: ctx.purpose in ['analytics', 'reporting']

  - id: own_or_admin
    op: admit
    expr: row.tenant_id == ctx.tenant || 'admin' in ctx.roles

  - id: pk_present
    op: assert
    expr: has(row.order_id)
    on_fail: deny

  - id: amount_consistent
    op: assert
    expr: row.amount == row.unit_price * row.qty
    on_fail: drop

  - id: mask_email
    op: transform
    column: email
    expr: ctx.tenant == 'acme' ? row.email : hash_sha256(row.email)

  - id: ids_present
    op: guarantee
    expr: dataset.customer_id.null_rate < 0.02
    on_fail: deny

  - id: small_cells
    op: shape
    operator: suppress
    params: {k: 5}
    unless: ctx.tenant == 'acme'
```

Note that `admit` and `assert` may reference columns (`tenant_id`, `unit_price`, `qty`) that are not exposed. Rules see the whole bound schema; callers see only `expose`.

## 6. The CEL profile

parcel accepts a strict subset of CEL. Strict means valid CEL that parcel cannot translate soundly is rejected at check time with a reason, never silently degraded.

**Types.** bool, int, uint, double, string, bytes, timestamp, duration, list, and null. Arrow decimals are surfaced as a parcel `decimal` type with exact comparison and arithmetic; they are never silently widened to double. Nested structs are supported through field access. Maps appear only as each namespace's `other` field (section 3.1); arbitrary map literals and map-valued columns are not in v1.

**Operators.** All CEL comparison, arithmetic, logical and ternary operators. `in` against a literal list. String methods `startsWith`, `endsWith`, `contains`, `matches`, `size`. Conversions `int()`, `double()`, `string()`, `timestamp()`, `duration()`. Timestamp accessors. `has()` on `row` fields, meaning "is not null".

**Macros.** `exists`, `all`, `filter`, `map` over list-typed columns and over `ctx.roles`. Comprehensions over anything else are rejected.

**Nulls.** CEL has no SQL-style null. parcel's rule is: a null in a `row` field makes any `admit` or `assert` expression that reads it evaluate to false, and any `transform` that reads it produce null. The compiler enforces this by wrapping predicates so that three-valued logic can never let a null row through.

**Determinism.** No `now()` call; use `ctx.now`, which is bound once per query. No random. No I/O. Every expression is a pure function of its namespaces.

**Numbers.** Integer overflow and division by zero are errors, not wraparound, and the compiler configures the engine to match.

## 7. The function registry

Everything beyond the core profile enters through the registry. The profile in section 6 is deliberately small; the registry is where a contract gets its vocabulary, and it is designed from the start to be extended by the people writing contracts, not only by parcel's maintainers.

### 7.1 What an entry is

A registry entry is a record describing one function: how it is called, what it does, and what the compiler is allowed to assume about it.

```
FunctionEntry {
  name:          "is_meter_serial"
  version:       2
  hash:          <hash of implementation and manifest>
  signatures:    [ (string) -> bool ]              // typed overloads
  impl:          Native { cel_fn, expr_builder }   // built-in, see 7.3
              |  Wasm   { module_hash, export }     // user-supplied, see 7.4
  deterministic: true
  nulls:         propagate                          // null in, null out; or custom
  cost:          moderate                           // cheap | moderate | expensive
  selectivity:   none                               // optional hint, or measured
  pushdown:      none                               // or monotone, or a bounds function
  owner:         "core" | tenant id
}
```

`signatures` is what the checker uses to type a call. `deterministic` and `nulls` are what the translator needs to wrap a call correctly. `cost`, `selectivity` and `pushdown` are what the query engine's planner needs to place the call; without them a function is opaque and goes last.

### 7.2 The registry is a store, and functions are pinned

The registry is not a file in the code base. It is a store with the same shape as the contract store: entries keyed by name, version and hash, with the current version marked. Built-in entries are compiled into `parcel-runtime` and owned by `core`; user-defined entries are stored per owning tenant.

A contract refers to a function by name. When the compiler resolves the name it pins the call to a specific entry hash and records that hash in every artifact it produces. A compiled contract therefore names exactly which implementation of `is_meter_serial` it was compiled against, and a validation plan re-executed a year later uses that implementation even if the tenant has since published version 3. Changing a function does not change any existing compiled contract; recompiling does.

### 7.3 Built-in functions

A built-in entry has two halves written in Rust: a reference implementation registered into the CEL interpreter, and a DataFusion `Expr` builder that emits the equivalent expression tree. Because both halves come from one entry, the interpreter and the translator cannot disagree. Built-ins are the only functions that can push down and prune, because the `Expr` builder gives the planner something it can see inside.

Built-in entries at v1: `hash_sha256`, `tokenize` (keyed HMAC), `redact`, `partial(n)`, `generalize_date(granularity)`, `bucket(width)`, `is_msisdn`, `is_kra_pin`, `is_email`, `luhn`.

### 7.4 User-defined functions

Anyone who owns a contract can add a function to it. The path is: write it in Rust, compile it to WebAssembly, submit the module with a manifest, and call it from a contract by name.

WebAssembly is the container for three reasons. It is sandboxed: a tenant's function cannot read another tenant's data or touch the host. It is deterministic by construction: the module is given no clock, no randomness and no I/O imports, so it cannot be anything but a pure function of its inputs. And it is one artifact: the same module is called by the CEL interpreter during authoring and by DataFusion at execution, so a user function has one body and no second implementation to drift.

**The interface.** Inputs and outputs are Arrow, one batch at a time. A function receives a `RecordBatch` with one column per argument and returns a `RecordBatch` with one column, the result. Batching is what makes crossing the WebAssembly boundary affordable; per-row calls would not be. The ABI follows the `arrow-udf` convention rather than a parcel-specific one, so existing tooling and documentation apply.

**Writing one.** The `parcel-udf` crate provides a macro that generates the ABI from an ordinary function:

```rust
use parcel_udf::function;

#[function("is_meter_serial(string) -> bool")]
fn is_meter_serial(s: &str) -> bool {
    s.len() == 12 && s.starts_with("MK") && s[2..].chars().all(|c| c.is_ascii_digit())
}
```

Build with `cargo build --target wasm32-unknown-unknown`, or submit the source to a build service that does the same; the artifact is identical either way.

**The manifest.** Submitted with the module:

```yaml
name: is_meter_serial
version: 2
signatures: ["(string) -> bool"]
deterministic: true
nulls: propagate
cost: moderate
```

**Registration.** parcel verifies the module exports the symbols the ABI requires and imports nothing outside the allowed set, runs it against a smoke batch to confirm the declared signature and null behaviour, hashes module and manifest together, and stores the entry under the submitting tenant. From then on the function is callable from any contract that tenant owns.

**Execution.** `parcel-runtime` loads registered modules with wasmtime under memory and fuel limits, and wraps each one as a DataFusion `ScalarUDFImpl` and as a CEL function. A call to a user function compiles as a residual: the planner cannot see inside it, so it will not prune, and the `cost` and any selectivity measured by the authoring check are what the planner uses to place it last.

**Why the cost is acceptable.** The write plan (section 10) is what makes user functions cheap in practice. An `assert` or an enricher that calls a custom function runs once, at flush, and is stored as a flag or an `other` field; every query afterwards reads a boolean column. Only `admit` and `transform` rules that combine a custom function with `ctx` execute it at query time, and the split pass still materialises the data-only part. A user's expensive logic is paid once per write, not once per query.

**Limits.** A module may embed a lookup table in its data segment, which is how a custom enricher works without network access, subject to a size cap. v1 accepts Rust only; the same ABI supports other source languages compiled to WebAssembly, and adding one is a registration-time change, not a compiler change.

### 7.5 What the compiler assumes

The registry is the single source of truth about functions, and the compiler assumes nothing it does not state. A function without `pushdown` does not prune. A function without `selectivity` is placed by `cost` alone. A function declared non-deterministic is refused in `assert` and `guarantee` rules, because a proof must be reproducible. The `report` in section 9.1 names, for every rule, which functions made it residual, so an author can see the price of a custom function before the contract is registered.

## 8. The compiler

parcel is a library crate. Its public surface is one function: given a contract document and the Arrow schema of the binding, return a `Compilation` (three artifacts, described in section 9) or a list of errors. Internally it runs six passes.

**Parse.** The CEL parser (the `cel-parser` crate) turns each `expr` into an AST. parcel does not modify the parser; all restrictions are applied afterwards.

**Check.** Types every node bottom-up from the bound schema and the namespace field tables. Resolves every function call against the registry and pins it to an entry hash (section 7.2). Rejects unknown identifiers, functions not in the registry, macros outside the profile, non-deterministic functions in `assert` or `guarantee`, and type mismatches (comparing a decimal to a string, a `transform` whose result type does not match its exposed column). Tags every node with the namespaces it depends on.

**Classify.** Confirms each rule's namespace tags fit its declared operation. An `assert` that mentions `ctx` is an error with the message "assertions describe data, not callers; use admit". A `guarantee` that mentions `row` is an error. This pass is what makes the namespaces enforceable rather than advisory.

**Split.** For `admit` and `transform` rules that mix `row` and `ctx`, finds the largest subtrees that use only `row`. These are candidates for write-time materialisation: the data-only part is computed once and stored, and the query-time predicate compares the stored value to the caller's context.

**Translate.** Walks each checked AST and emits a DataFusion `Expr`. Identifiers under `row` become column references; identifiers under `ctx` become placeholders that peQL binds to literals at plan time; registry calls become their `Expr` builders; anything untranslatable becomes a residual UDF wrapping the interpreter. Predicates are wrapped for null safety.

**Assemble.** Arranges the translated expressions into the three artifacts. The same `Expr`s appear in each; they differ only in how they are put together.

## 9. The outputs

One translation, three assemblies. They are produced together and share a hash so that any one of them can be traced to the contract that produced it.

### 9.1 `CompiledContract`: expressions to embed

The query-time artifact. It is not runnable on its own; it is a set of fragments that peQL splices into a caller's query to turn a contract into a view.

```
CompiledContract {
  id, version, hash,
  binding,                       // opaque to callers
  exposed_schema,                // Arrow schema after transforms
  decisions:  [(id, CelAst)],    // evaluated by interpreter at plan time
  admits:     [(id, Expr)],      // conjoined into the scan filter
  transforms: [(column, Expr)],  // the projection
  guarantees: [(id, CelAst, on_fail)],
  shapes:     [(id, operator, params, unless: CelAst)],
  functions:  [(name, version, hash)],     // every registry entry this contract is pinned to
  report:     [(rule_id, tier, reason)],   // how each rule will execute
}
```

The `report` is shown to the author. It says, for every rule, whether it prunes fragments, filters at scan, runs as a residual, or costs nothing, and why. This is the feedback loop that teaches authors to write pushable contracts.

### 9.2 `ValidationPlan`: a query that proves the contract holds

The standalone artifact. It is a complete DataFusion `LogicalPlan`: scan the binding, evaluate every `assert`, compute every statistic any `guarantee` reads, and return exactly one row. Run it against a dataset and the row is the verdict.

Its shape is a single aggregation followed by a projection that applies each rule's threshold:

```sql
SELECT
  count(*)                                                    AS rows,
  count(*) FILTER (WHERE NOT (order_id IS NOT NULL))         AS fail_pk_present,
  count(*) FILTER (WHERE NOT (amount = unit_price * qty))    AS fail_amount_consistent,
  avg(CASE WHEN customer_id IS NULL THEN 1.0 ELSE 0.0 END)   AS null_rate_customer_id,
  ...
FROM <binding>
-- then: valid = (fail_pk_present = 0) AND (null_rate_customer_id < 0.02) AND ...
--       breached = [ids of rules that failed]
```

The plan succeeding is necessary; the `valid` column being true is the proof. Because it is one mergeable aggregation, it runs over data of any size in chunks, and its per-row intermediates (the assertion results) are exactly the flag columns the write path stores. The write plan in section 10 is this plan with a second sink.

Three things consume it. peQL's write path runs it natively on every flush and refuses or annotates according to `on_fail`. The authoring `check` command runs it over a sample. And a certificate points at it: the tuple (contract hash, validation plan hash, data hash, verdict row) is what gets signed, and a verifier recomputes the verdict by running the same plan over the same data.

Any statistic computed in floating point (an average, a rate) can differ in the last bits between runs with different parallelism because summation order differs. Counts and sums over integer or decimal columns are exact and preferred; a guarantee over a `double` statistic should carry a tolerance, and parcel warns when one does not.

### 9.3 `WritePlan`: what must exist on disk

Described in section 10. It is the validation plan's stages plus enrichment, layout and manifest instructions, so that after a write every rule in the contract is evaluable and cheap at query time.

### 9.4 Serialisation and the runtime

All three artifacts serialise through `datafusion-proto`, which round-trips `Expr` and `LogicalPlan` exactly and refers to registry functions by name. The validation plan is additionally emitted as Substrait, so an engine other than DataFusion can run it; registry functions travel as Substrait extension functions.

A deserialised plan needs the registry's implementations available under those names. That is the job of a small companion crate, `parcel-runtime`, which does one thing: given the function hashes an artifact is pinned to, it registers the built-in implementations and loads the user-defined WebAssembly modules from the registry store into a DataFusion `SessionContext`. It contains no compiler. peQL depends on it; anyone else who wants to execute parcel output does the same.

The crate layout:

```
parcel-core      parse, check, classify, split, translate, assemble; no I/O
parcel-runtime   registers built-ins and loads user modules into a DataFusion session
parcel-udf       the macro and ABI for writing user-defined functions (section 7.4)
parcel-cli       check, compile, report, register-function
```

`parcel-core` never performs I/O and never depends on an execution engine. That is what keeps compilation deterministic: the same contract and schema produce the same three artifacts and the same hash wherever they are compiled, so a server can recompile a submitted contract and compare rather than trust.

## 10. The write plan

parcel does not write data, but it is responsible for saying exactly what must exist on disk for every rule in the contract to be evaluable and cheap. That instruction set is the `WritePlan`, and producing it is as much parcel's job as producing the query-time expressions. peQL's write path executes it; parcel decides it.

The write plan is the validation plan (section 9.2) with a second sink. Where the validation plan aggregates the assertion results into a verdict, the write plan also persists them per row as flag columns, adds the enrichment stage before them and the layout and manifest stages after, and merges the statistics into the running totals for the dataset. Running the write plan therefore always yields the validation verdict as a by-product; a write is refused or annotated on the same `on_fail` rules.

The plan is declarative and ordered. When a batch is flushed under a contract, the writer performs these stages in this order, because later stages depend on earlier ones.

**Stage 1: Enrich.** Populate `row.other`. A contract may declare enrichers: registry functions or lookups that run once per row at write time and produce fields of the `_other` struct. This is how a rule gets to reference something that is not in the source data.

```yaml
enrich:
  - field: risk_band
    expr: risk_band_lookup(row.customer_id)     # registry function backed by a table
  - field: geo_cell
    expr: h3_cell(row.lat, row.lon, 7)
```

Enrichers are CEL expressions over `row` (including previously enriched `other` fields, in declaration order) and must be pure. They may call built-in functions or a tenant's own WebAssembly functions (section 7.4); a lookup is either a registry function whose backing table is versioned and named in the manifest, or a user module carrying its table in its data segment. Either way the enrichment is reproducible from the pinned function hash. The plan lists each enricher with its `Expr`, its output type, and the lookup tables it needs. Enrichers run first so that assertions and derived columns can use their output.

**Stage 2: Flags.** Evaluate every `assert` into a boolean column `_c_<id>`. The plan carries the `Expr` for each.

**Stage 3: Derived columns.** Evaluate every data-only subtree the Split pass identified in `admit` and `transform` rules into its own column, so the query-time predicate is a comparison against a stored value.

**Stage 4: Statistics.** Compute the `dataset` namespace: row count, per-column null count and rate, distinct count, min and max, the pass rate of every assertion, and any `dataset.other` fields the contract declares producers for:

```yaml
dataset_other:
  - field: source_system
    value: "mpesa-settlement-v2"                # constant per write
  - field: median_amount
    expr: quantile(row.amount, 0.5)             # aggregate over the batch
```

The plan lists exactly which statistics are needed, so the writer does not compute what no rule reads.

**Stage 5: Layout.** Cluster the batch by the columns in `cluster_by` (flags and derived columns the contract's `admit` rules filter on, most selective first) and partition by the binding's partition columns. Enable statistics and bloom filters on the columns the plan names. This is what makes fragments homogeneous so that pruning skips them rather than reading them.

**Stage 6: Manifest.** Write the manifest: the statistics from stage 4, per-fragment all-pass / all-fail / mixed status for every flag, the lookup table versions used by enrichers, the file-level sort order, and the contract hash. Stamp the contract hash into each Parquet file's key-value metadata.

The `WritePlan` struct that carries all of this:

```
WritePlan {
  enrich:      [(field, Expr, type, lookups)],
  flags:       [(assert_id, Expr)],
  derived:     [(name, Expr, type)],
  stats:       {columns: [..], assertions: [..], other: [(field, Expr | literal)]},
  layout:      {cluster_by: [..], partition_by: [..], bloom: [..]},
  manifest_fields: [..],
}
```

At query time peQL's view builder is handed the manifest. Any query-time `Expr` whose hash matches a stored flag or derived column is rewritten to a column reference; any `row.other` reference resolves to the `_other` struct field. If a file's contract hash does not match the current contract, the rule is evaluated live over that file instead. Nothing is ever wrong; stale files are only slower, and the `report` in section 9 tells the operator which files would benefit from a rewrite.

A `WritePlan` is also the answer to "what does this contract cost to store?" The number of flag and derived columns, the enrichers and their lookups, and the statistics list are all visible before a single byte is written.

## 11. Authoring loop and correctness

The contract author always has the data at hand when writing the contract, so parcel ships a `check` command that takes a contract and a sample of the bound data and reports:

- per `assert`, the pass rate and a sample of failing rows;
- per `admit`, the selectivity for a supplied `ctx`;
- per `guarantee`, the current value of the statistic it tests;
- per rule, its execution tier from the `report`.

The same command runs the **differential test**: every expression is evaluated twice over the sample, once by the CEL interpreter and once by the translated `Expr` through DataFusion, and any disagreement is a hard failure. This test is what makes a materialised flag trustworthy. It runs on every `check`, not only in CI.

## 12. Versioning and inheritance

A contract's `hash` is the hash of its canonical form. Files carry the hash they were written under. A contract change produces a new hash; existing files remain servable through the live path until rewritten.

Inheritance is restriction-only by construction: the child's `expose` is intersected with the parent's, `decide`, `admit`, `assert` and `guarantee` rules are unioned (conjoined), `transform` rules compose with the parent's applied first, and `shape` operators accumulate. A child cannot remove a parent rule. parcel resolves inheritance before compiling, so peQL only ever sees flat contracts.

## 13. Non-goals

parcel does not execute queries, store data, authenticate callers, or track privacy budgets. It does not try to be a general policy language; it is a data contract language with a fixed set of operations and an open set of functions. It does not accept arbitrary CEL; it accepts the profile in section 6 and says no to the rest. It does not run user code outside a sandbox: a user-defined function exists only as a WebAssembly module with no imports beyond the ABI.

## 14. Open questions

- Should `guarantee` failures with `on_fail: annotate` be visible in the result schema (a metadata column) or only in the result envelope? The envelope is cleaner; a column is easier for agents that only read tables.
- Should `transform` be allowed to reference other exposed columns after their own transforms (a chain), or only raw `row` values? v1 says raw only; chains can be added without breaking contracts.
- Whether to support a `sql` dialect for `admit` alongside CEL, parsed by DataFusion's own parser, for teams migrating existing row filters. Cheap to add behind the same `Expr` boundary; the risk is two ways to say the same thing.
