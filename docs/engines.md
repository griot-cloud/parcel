# Engines

A compiled contract can be queried by name inside DuckDB and Spark. The engine runs the contract's rules as part of its own query: the data stays where it is, nothing is copied, and nothing is created per contract. Each caller gets the rows and values the contract allows them.

```sql
SELECT region, count(*) FROM "sales/customers@1.0.0" GROUP BY region;
```

The name is the contract's name and version, `<contract>@<version>`. peQL remains the engine that enforces contracts; these targets bring the same rules to engines people already use.

## How it works

`parcel compile --target ENGINE` checks that the contract can run as one query, then writes what the engine loads. The query is built from the compiled contract (`.parcel`), the same artifact peQL loads:

```text
SELECT <exposed columns, transforms applied>
FROM <the data the binding names>
WHERE <decide rules> AND <admit rules> AND <assert rules with on_fail: drop>
```

The row expressions are the compiled ones, written in the engine's SQL, so the engine evaluates what parcel evaluates. Each engine resolves the name when a query names it, and plans the result like any other query. Caller fields are constants by then, so a rule such as `region == ctx.other.region` reaches the scan and prunes partitions and row groups.

| Engine | Loads | Resolves the name with | Caller from |
| --- | --- | --- | --- |
| DuckDB | `.parcel` files | the `parcel` extension's replacement scan | variables: `SET VARIABLE parcel_tenant = '...'` |
| Spark | `.spark.json` files | the `parcel` Spark extension's analyser rule | configuration: `spark.parcel.caller.tenant` |

A contract file that changes is read again on the next query, in both engines.

### Caller fields

Each `ctx` field the contract reads is one variable in DuckDB and one configuration key in Spark:

| Field | DuckDB | Spark |
| --- | --- | --- |
| `ctx.tenant` | `parcel_tenant` | `spark.parcel.caller.tenant` |
| `ctx.other.region` | `parcel_other_region` | `spark.parcel.caller.other.region` |
| `ctx.roles` | a list: `['analyst', 'auditor']` | comma-separated: `analyst,auditor` |

A field that is not set is null, and a rule that reads it is false. `ctx.now` is the time of the query unless it is set.

## DuckDB

Compile to a `.parcel`, then load the extension:

```bash
parcel compile customers.yaml --schema data/customers --target duckdb -o contracts/customers.parcel
```

```sql
LOAD 'parcel.duckdb_extension';
SET VARIABLE parcel_tenant = 'our_company';
SET VARIABLE parcel_purpose = 'analytics';
SET VARIABLE parcel_tier = 'internal';
SELECT * FROM "sales/customers@1.0.0";
SELECT * FROM 'contracts/customers.parcel';
```

The extension looks for contracts in the directories `PARCEL_CONTRACTS` lists, separated by `:`. Without it, it looks in `./contracts` and the working directory. `SELECT * FROM parcel_contracts()` lists each contract file it finds, with its name, version and compilation hash, and an `error` for a file it cannot read.

A `.parcel` reads the data its binding names, `read_parquet('data/customers/**/*.parquet')`, relative to DuckDB's working directory. To read another relation, such as a DuckDB table, name it with `--source` and write a `.duckdb.json` instead:

```bash
parcel compile customers.yaml --schema data/customers --target duckdb --source main.customers -o contracts/customers.duckdb.json
```

### Build the extension

```bash
cargo build --release -p parcel-duckdb-extension
cargo run --release -p parcel-duckdb-extension --bin package -- target/release/libparcel_duckdb.dylib parcel.duckdb_extension
```

`package` adds the metadata footer DuckDB checks before loading an extension. On Linux the library is `libparcel_duckdb.so`. The extension is not signed: start DuckDB with `duckdb -unsigned`, or set `allow_unsigned_extensions`.

## Spark

Compile to a `.spark.json`, build the extension, and start Spark with it:

```bash
parcel compile customers.yaml --schema data/customers --target spark -o contracts/customers.spark.json
crates/parcel-spark/jvm/build.sh parcel-spark.jar
spark-sql --jars parcel-spark.jar \
  --conf spark.sql.extensions=cloud.griot.parcel.spark.ParcelExtensions \
  --conf spark.parcel.contracts=contracts \
  --conf spark.parcel.caller.tenant=our_company \
  --conf spark.parcel.caller.purpose=analytics
```

```sql
SELECT * FROM `sales/customers@1.0.0`;
```

`build.sh` compiles against the jars of `SPARK_HOME`, or of the installed `pyspark` package, and needs Java 17 or later.

The extension reads the `*.spark.json` files in the directories `spark.parcel.contracts` lists, separated by commas; the default is `contracts`. Caller fields are set when Spark starts, or between queries with `SET` or `spark.conf.set`.

A `.spark.json` reads the data its binding names. `--source` names a table instead, such as `--source sales.customers`.

## What runs, and what is refused

| Contract feature | In an engine |
| --- | --- |
| `expose`, `transform` | the query's columns |
| `admit`, `assert` with `on_fail: drop` | the query's filter |
| `decide` | a guard: a refused caller's query fails with `refused by <rule>: <message>` |
| `assert` with `report` or `deny`, `guarantee` | not part of a caller's view: they judge the dataset (`parcel check`, peQL) |
| `shape` | refused: shapes act on the caller's aggregates or spend a privacy budget |
| `enrich`, WebAssembly functions | refused |

A contract that uses a refused feature does not compile for an engine, and `parcel compile --target` names the rule.

## Limits

- **Governance, not enforcement.** The caller states who they are, and anyone who can read the files can read them directly. Use peQL where the caller must be authenticated.
- **A refused caller can get no rows instead of an error.** The `decide` guard is one condition of the filter, and the engine may stop at another first: over an empty table, or when no caller field is set and every condition is false.
- **The DuckDB extension** carries parcel's runtime to read `.parcel` files, about 128 MB unstripped. None of it runs while a query executes.
- **Strings in Spark.** Spark reads a backslash in a string literal as an escape; parcel doubles each one.

## Testing

`tests/engines/run.py` compiles the contracts in `examples/customers` for both engines, queries each one by name as every caller, and compares the rows, value for value, with the rows `parcel check --rows` computes. See `tests/engines/README.md`.
