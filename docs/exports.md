# Run the checks in other engines

The validation plan is a DataFusion plan, so it can leave DataFusion.

**As SQL**, in the dialect of another engine:

```bash
parcel compile contracts/orders.yaml --schema incoming/orders.csv --sql duckdb --table orders
```

Dialects: `datafusion`, `duckdb`, `postgres`, `mysql`, `sqlite`, `bigquery`, `snowflake`. The
query returns one row: `valid`, `breached`, `row_count`, a `fail__<id>` count per assertion, a
`guarantee__<id>` per data-only guarantee, and every statistic the guarantees read. Functions
whose meaning is verified only in DataFusion are listed as warnings. `examples/verify-duckdb.py`
runs the DuckDB SQL in DuckDB and checks that it reproduces parcel's verdict value for value.

**As Substrait**, for engines that consume it:

```bash
parcel compile ... --substrait plan.bin   # build with --features substrait (needs protoc)
```

**Natively**, in anything built on DataFusion: decode the plan from a bundle
(`Bundle::validation_plan`) and run it with `parcel_runtime::plan::validate` over any table with
the contract's columns.
