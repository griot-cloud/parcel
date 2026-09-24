# parcel

**Data contracts in CEL, compiled into query plans.**

A parcel contract describes a dataset from the outside: where it lives, which columns a caller may see, and rules written in [CEL](https://cel.dev). parcel checks the contract against the data's Arrow schema, works out when each rule can run, and compiles it into [DataFusion](https://datafusion.apache.org) expressions and plans. The rules are enforced inside the query plan, where the optimiser can see them, so a contracted query costs what a filter costs and often less.

```yaml
contract: sales/orders
version: 1
binding:
  parquet: data/orders/
  partitioned_by: [region]
expose:
  - {name: order_id, type: int64}
  - {name: email, type: utf8}
  - {name: region, type: utf8}
  - {name: amount_cents, type: int64}
rules:
  - {id: analytics_only, op: decide, expr: "ctx.purpose in ['analytics', 'reporting']"}
  - {id: own_or_admin,   op: admit,  expr: "row.tenant_id == ctx.tenant || 'admin' in ctx.roles"}
  - {id: pk_present,     op: assert, expr: "has(row.order_id)", on_fail: deny}
  - {id: amount_ok,      op: assert, expr: "row.amount_cents == row.unit_price_cents * row.qty", on_fail: drop}
  - {id: mask_email,     op: transform, column: email, expr: "ctx.tenant == 'acme' ? row.email : hash_sha256(row.email)"}
  - {id: ids_present,    op: guarantee, expr: "dataset.customer_id.null_rate < 0.02", on_fail: deny}
  - {id: small_cells,    op: shape, operator: suppress, params: {k: 5}, unless: "ctx.tenant == 'acme'"}
```

## How it works

Each rule reads from up to three namespaces, and the namespaces decide how the rule executes:

| Namespace | Holds | Rules that only read it cost |
|---|---|---|
| `ctx` | the caller: tenant, purpose, roles, clearance, time | nothing: evaluated once per query |
| `dataset` | statistics stored when the data was written | one manifest read |
| `row` | one record | a filter pushed into the scan, or a flag computed at write time |

There are seven operations: `decide`, `expose`, `admit`, `assert`, `transform`, `guarantee` and `shape`. parcel rejects a rule whose namespaces do not fit its operation. For example, an `assert` that reads `ctx` fails with "assertions describe data, not callers; use admit".

Compiling produces three artifacts that share one hash:

- **CompiledContract:** the expressions a query engine splices into a caller's query. Caller context becomes placeholders, bound per query and folded away by the optimiser.
- **ValidationPlan:** one aggregate query that returns a single verdict row, saying whether the data satisfies the contract.
- **WritePlan:** what must be on disk for every rule to be cheap: flag columns, clustering, partitioning and bloom filters.

The design is in [`design/parcel-README.md`](design/parcel-README.md). The v0 cut is in [`design/parcel-v0.md`](design/parcel-v0.md), and how parcel fits the wider ecosystem is in [`design/parcel-ecosystem.md`](design/parcel-ecosystem.md).

## Quickstart

```sh
cargo build --release
export PATH="$PWD/target/release:$PATH"
cd examples/quickstart
```

**Check** a contract against sample data. This prints how each rule will execute, the verdict, what each caller would see, and a differential test that evaluates every rule with a CEL interpreter and with DataFusion and requires them to agree:

```sh
parcel check contracts/orders.yaml --data incoming/orders.csv --type msisdn=utf8 \
  --caller callers/globex-analyst.yaml --caller callers/acme-admin.yaml
```

**Write** data under the contract. parcel computes flags, partitions and clusters the files, stamps the contract hash into each Parquet file, runs the validation plan, and writes a manifest:

```sh
parcel write contracts/orders.yaml --input incoming/orders.csv --type msisdn=utf8
```

**Query** through the contract. Every table in the SQL is a contract, and each caller gets what the contract allows them:

```sh
parcel query 'SELECT region, SUM(amount_cents) FROM "sales/orders" GROUP BY region' \
  --caller callers/globex-analyst.yaml
parcel query 'SELECT order_id, email FROM "sales/orders" LIMIT 5' --caller callers/acme-admin.yaml
parcel query 'SELECT COUNT(*) FROM "sales/orders"' --caller callers/marketing.yaml     # refused
parcel query '...' --caller callers/globex-analyst.yaml --explain                       # see the pruning
```

Other commands:
- `parcel compile contracts/orders.yaml --schema incoming/orders.csv --sql duckdb --table orders` prints the validation plan as SQL for another engine. The dialects are datafusion, duckdb, postgres, mysql, sqlite, bigquery and snowflake. Anything not verified in the target dialect is printed as a warning. `examples/verify-duckdb.py` runs the DuckDB SQL in DuckDB and checks that it reproduces parcel's verdict.
- `parcel schema` prints the JSON Schema of contract documents, also checked in at [`schema/contract.schema.json`](schema/contract.schema.json). Put `# yaml-language-server: $schema=https://raw.githubusercontent.com/griot-cloud/parcel/main/schema/contract.schema.json` at the top of a contract to get completion and validation in editors.
- `parcel validate sales/orders` re-runs the verdict.
- `parcel describe sales/orders --tenant globex --purpose analytics` shows what a caller would see.
- `parcel list` lists the workspace.
- `parcel compile contract.yaml --schema data.csv -o contract.parcel.json` writes a bundle.

## Your own functions, in WebAssembly

A tenant can extend the rule language with functions written in Rust and compiled to WebAssembly. A module imports nothing, so it has no clock, randomness or I/O. It runs under fuel and memory limits, and parcel verifies it with a smoke batch at registration. Contracts pin a function by hash, so a later version never changes an existing contract. The DataFusion side and the CEL interpreter call the same module, so there is no second implementation to drift.

```rust
parcel_udf::export! {
    fn is_meter_serial(s: &str) -> bool {
        s.len() == 12 && s.starts_with("MK") && s.as_bytes()[2..].iter().all(|b| b.is_ascii_digit())
    }
}
```

```sh
cd examples/utility
parcel function register functions/meter_serial.wasm --manifest functions/is_meter_serial.yaml --owner kplc
```

A contract with `owner: kplc` can then call `is_meter_serial(row.meter)` in any rule or enricher. See `examples/udf-meter-serial` for the module and `examples/utility` for a workspace that uses it.

## Crates

| Crate | What it is |
|---|---|
| `parcel-core` | The compiler: parse, check, classify, translate, assemble. No I/O. |
| `parcel-runtime` | The reference CEL interpreter, with parcel's built-in functions and the `Caller` type. |
| `parcel-engine` | A reference executor on DataFusion: write, validate, query, differential test, bundles. |
| `parcel-udf` | Write user-defined functions in Rust for WebAssembly (no dependencies). |
| `parcel-cli` | The `parcel` command. |

peQL, the production query engine, embeds the same artifacts.

## The rule language

parcel accepts a strict subset of CEL. Anything it cannot translate faithfully is rejected at check time with a reason; nothing is silently degraded.

- **Types:** bool, int, uint, double, string, bytes, timestamp, duration, and lists.
- **Operators:** comparison, arithmetic, logical and ternary operators, and `in`.
- **String methods:** `startsWith`, `endsWith`, `contains`, `matches` (literal pattern), `size`.
- **Conversions:** `int`, `uint`, `double`, `string`, `timestamp`, `duration`.
- **Timestamp accessors:** `getFullYear`, `getMonth`, `getDayOfMonth`, `getDayOfWeek`, `getDayOfYear`, `getHours`, `getMinutes`, `getSeconds`.
- **Presence:** `has(row.col)`.
- **Macros over lists:** `exists`, `all`, `filter`, `map`, which compile to DataFusion lambdas.
- **Registry functions:** `hash_sha256`, `redact`, `is_msisdn`, `is_email`.
- **Nulls:** a null in a row field a rule reads makes an `admit` or `assert` false and a `transform` null. `has()` is how a rule tests for presence.

## License

Apache-2.0.
