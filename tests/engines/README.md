# Engine target tests

`run.py` compiles the contracts in `examples/customers` for DuckDB, Postgres and Spark, queries
each by name as every caller, and compares the engine's rows, value for value, with the rows
parcel's runtime computes (`parcel check --rows`). It also checks refusals, aggregates and filters
on top of a contract, filters reaching the table scan, two contracts over one table, a replaced
contract taking effect, and shapes refused at compile time.

```bash
cargo build -p parcel-cli
cargo build --release -p parcel-duckdb-extension
cargo run --release -p parcel-duckdb-extension --bin package -- \
  target/release/libparcel_duckdb.dylib target/release/parcel.duckdb_extension
crates/parcel-spark/jvm/build.sh crates/parcel-spark/jvm/parcel-spark.jar   # needs pyspark or SPARK_HOME

pip install duckdb pyarrow 'psycopg[binary]' pyspark
python tests/engines/run.py --parcel target/debug/parcel \
  --duckdb-extension target/release/parcel.duckdb_extension \
  --postgres 'host=127.0.0.1 port=5432 user=postgres dbname=postgres' \
  --spark-jar crates/parcel-spark/jvm/parcel-spark.jar \
  [--data customers.csv] [--json results.json]
```

Without `--data`, it uses 5,000 synthetic customers. Either way it adds 48 rows that break the
contract's quality rules on purpose. An engine whose option is left out is skipped. The Postgres
database is written to: the script creates a `customers` table and the `parcel` schema.
