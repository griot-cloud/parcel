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

Documentation: **[griot-cloud.github.io/parcel](https://griot-cloud.github.io/parcel/)** (sources in [`docs/`](docs/)). The design is in [`design/parcel-README.md`](design/parcel-README.md). The v0 cut is in [`design/parcel-v0.md`](design/parcel-v0.md), and how parcel fits the wider ecosystem is in [`design/parcel-ecosystem.md`](design/parcel-ecosystem.md).

## Install

Linux and macOS:

```sh
curl -LsSf https://github.com/griot-cloud/parcel/releases/latest/download/install.sh | sh
```

Windows (PowerShell):

```powershell
powershell -ExecutionPolicy ByPass -c "irm https://github.com/griot-cloud/parcel/releases/latest/download/install.ps1 | iex"
```

Or with pip, on any of those platforms: `pip install griot-parcel`. Each
[release](https://github.com/griot-cloud/parcel/releases) also carries the binary for Linux
(x86_64, arm64), macOS (Apple silicon, Intel) and Windows (x64) as an archive with its sha256.
The released binary includes `--substrait`. To build from source instead: `cargo build --release`.

## Quickstart

```sh
cd examples/quickstart
```

**Check** a contract against sample data. This prints how each rule will execute, the verdict, what each caller would see (or which `decide` rule refuses them), and a differential test that evaluates every rule with a CEL interpreter and with DataFusion and requires them to agree:

```sh
parcel check contracts/orders.yaml --data incoming/orders.csv --type msisdn=utf8 \
  --caller callers/globex-analyst.yaml --caller callers/acme-admin.yaml --caller callers/marketing.yaml
```

Add `--json` for a machine-readable result, for CI.

**Compile** it into a bundle, the portable artifact an engine loads:

```sh
parcel compile contracts/orders.yaml --schema incoming/orders.csv --type msisdn=utf8 -o orders.parcel.json
```

The bundle holds the contract document, its ancestors, the data schema, any tenant functions it pins, and the three artifacts encoded with `datafusion-proto`. An engine recompiles it and refuses it unless the compilation hash matches.

Other commands:
- `parcel compile ... --sql duckdb --table orders` prints the validation plan as SQL for another engine. The dialects are datafusion, duckdb, postgres, mysql, sqlite, bigquery and snowflake. Anything not verified in the target dialect is printed as a warning. `examples/verify-duckdb.py` runs the DuckDB SQL in DuckDB and checks that it reproduces parcel's verdict.
- `parcel compile ... --substrait plan.bin` writes the validation plan as a Substrait plan. The released binary has it; a source build needs `--features substrait` and `protoc`.
- `parcel schema` prints the JSON Schema of contract documents, also checked in at [`schema/contract.schema.json`](schema/contract.schema.json). Put `# yaml-language-server: $schema=https://raw.githubusercontent.com/griot-cloud/parcel/main/schema/contract.schema.json` at the top of a contract to get completion and validation in editors.
- `parcel import odcs contract.odcs.yaml -o contract.yaml` imports an Open Data Contract Standard (v3) document, and says what it did not carry over.

## Where contracts run

parcel compiles; it does not store data or serve queries. [peQL](https://github.com/griot-cloud/peql) is the runtime: it writes data under a contract, keeps manifests, evaluates `decide` rules per query, splices `admit` and `transform` into the caller's plan, gates on guarantees, and applies shapes. Any other Arrow engine can run the validation plan through `parcel-runtime` or the SQL and Substrait exports.

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
parcel function verify functions/meter_serial.wasm --manifest functions/is_meter_serial.yaml --owner kplc
parcel check contracts/tokens.yaml --data incoming/tokens.csv \
  --function functions/meter_serial.wasm=functions/is_meter_serial.yaml ...
```

A contract with `owner: kplc` can call `is_meter_serial(row.meter)` in any rule or enricher. `parcel compile -o` carries the module in the bundle, so an engine can verify and run it. See `examples/udf-meter-serial` for the module and `examples/utility` for a workspace that uses it.

## Crates

| Crate | What it is |
|---|---|
| `parcel-core` | The compiler: parse, check, classify, translate, assemble. No I/O. |
| `parcel-runtime` | What an engine needs to run the artifacts: the `Caller` type, the reference CEL interpreter and built-ins, parameter binding, validation over any table, WebAssembly function loading, bundles, SQL and Substrait export, and the differential test. |
| `parcel-udf` | Write user-defined functions in Rust for WebAssembly (no dependencies). |
| `parcel-cli` | The `parcel` command. |

peQL embeds `parcel-core` and `parcel-runtime`.

## The rule language

parcel accepts a strict subset of CEL. Anything it cannot translate faithfully is rejected at check time with a reason; nothing is silently degraded.

- **Types:** bool, int, uint, double, string, bytes, timestamp, duration, lists, and exact decimals (up to 18 digits; no division, one scale per comparison).
- **Operators:** comparison, arithmetic, logical and ternary operators, and `in`.
- **String methods:** `startsWith`, `endsWith`, `contains`, `matches` (literal pattern), `size`.
- **Conversions:** `int`, `uint`, `double`, `string`, `timestamp`, `duration`.
- **Timestamp accessors:** `getFullYear`, `getMonth`, `getDayOfMonth`, `getDayOfWeek`, `getDayOfYear`, `getHours`, `getMinutes`, `getSeconds`.
- **Presence:** `has(row.col)`.
- **Macros over lists:** `exists`, `all`, `filter`, `map`, which compile to DataFusion lambdas.
- **Registry functions:** `hash_sha256`, `redact` (a fixed `***`, so the length is not revealed), `partial(value, n)` (`***` and the last `n` characters; fully masked when the value is no longer than `n`), `is_msisdn`, `is_email`.
- **Caller context:** `ctx.id`, `tenant`, `purpose`, `tier`, `clearance`, `classification`, `roles`, `now`, and `ctx.other.<field>`.
- **Nulls:** a null in a row field a rule reads makes an `admit` or `assert` false and a `transform` null. `has()` is how a rule tests for presence. `null` may be written as one branch of `?:` (`ctx.clearance > 2 ? row.salary : null`) and takes the other branch's type.
- **Retyping:** a transform may change a column's type when `expose` declares the new one, e.g. `expose: {name: amount, type: utf8}` with `hash_sha256(string(row.amount))`.
- **Noise:** `shape: noise` adds Laplace noise either to aggregates over a column (`at: aggregate`, the default) or to each value (`at: row`).

## License

Apache-2.0; see [`LICENSE`](LICENSE).
