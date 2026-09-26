# Export validation

Export a contract's **data validation plan** to run quality checks in another engine. The export does not enforce caller access, masking or result shapes on that engine's other queries.

The commands below run from `examples/suppliers` after the {doc}`quickstart`.

## SQL

```bash
parcel compile orders.yaml --schema orders.csv --sql duckdb --table orders > validation.sql
```

Load the data into a table named `orders` in the target engine, then execute `validation.sql`. It returns one summary row with `valid`, `breached`, `row_count`, assertion failure counts, data-only guarantee results and required statistics.

Supported dialect names are `datafusion`, `duckdb`, `postgres`, `mysql`, `sqlite`, `bigquery` and `snowflake`. Read any warnings: a function's behaviour may not be verified for the chosen dialect. Test the exported query in the target engine. The repository's `examples/verify-duckdb.py` demonstrates comparing a DuckDB result with parcel's verdict.

## Substrait

Substrait represents a query plan as a portable binary format:

```bash
parcel compile orders.yaml --schema orders.csv --substrait validation.bin --table orders
```

The receiving engine must support the plan and required functions. Official release binaries include this option. A source build needs `protoc` and the feature enabled:

```bash
cargo build --release -p parcel-cli --features substrait
```

## DataFusion

Rust applications can execute the compiled validation plan over a compatible table with `parcel_runtime::plan::validate`. See {doc}`crates` for the library entry points and {doc}`execution` for the separate compiled outputs.
