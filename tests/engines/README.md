# Engine target tests

`run.py` compiles the contracts in `examples/customers` for DuckDB and Spark, queries each by
name as every caller, and compares the engine's rows, value for value, with the rows parcel's
runtime computes (`parcel check --rows`). It also checks refusals, aggregates and filters on top
of a contract, filters reaching the table scan, two contracts over one table, a replaced
contract taking effect, and shapes refused at compile time.

```bash
cargo build -p parcel-cli
cargo build --release -p parcel-duckdb-extension
cargo run --release -p parcel-duckdb-extension --bin package -- \
  target/release/libparcel_duckdb.dylib target/release/parcel.duckdb_extension
crates/parcel-spark/jvm/build.sh crates/parcel-spark/jvm/parcel-spark.jar

pip install duckdb pyarrow pyspark
python tests/engines/run.py --parcel target/debug/parcel \
  --duckdb-extension target/release/parcel.duckdb_extension \
  --spark-jar crates/parcel-spark/jvm/parcel-spark.jar \
  [--data customers.csv] [--json results.json]
```

Without `--data`, it uses 5,000 synthetic customers. Either way it adds 48 rows that break the
contract's quality rules on purpose. An engine whose option is left out is skipped.

Spark needs Java 17 or later, and `build.sh` needs `pyspark` installed or `SPARK_HOME` set. On
Linux the DuckDB library is `libparcel_duckdb.so`.
