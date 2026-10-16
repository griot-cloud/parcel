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

The row expressions are the compiled ones, written in the engine's SQL, so the engine evaluates what parcel evaluates. Each engine resolves the name at query time and plans the query like any other: filters reach the scan, and partition and file pruning apply.

| Engine | Loads | Resolves the name with | Caller from |
| --- | --- | --- | --- |
| DuckDB | `.parcel` files | the `parcel` extension's replacement scan | variables: `SET VARIABLE parcel_tenant = '...'` |
| Spark | a `.spark.json` file per contract | the `parcel` Spark extension's analyser rule | configuration: `spark.parcel.caller.<field>` |

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
SELECT * FROM parcel_contracts();
```

The extension finds contracts in the directories `PARCEL_CONTRACTS` lists (separated by `:`), else in `./contracts` and the working directory. A contract file that changes is read again on the next query. `parcel_contracts()` lists the contracts it can resolve.

Each caller field is a variable named `parcel_<field>`: `parcel_tenant`, `parcel_purpose`, `parcel_other_region` for `ctx.other.region`. An unset variable is null, so a rule that reads it is false.

A `.parcel` reads the data its binding names: `read_parquet('data/customers/**/*.parquet')`, relative to DuckDB's working directory. To read another relation, such as a DuckDB table, write the DuckDB artifact instead:

```bash
parcel compile customers.yaml --schema data/customers --target duckdb --source main.customers -o contracts/customers.duckdb.json
```

Build the extension with `cargo build --release -p parcel-duckdb-extension`, then add DuckDB's metadata footer:

```bash
cargo run --release -p parcel-duckdb-extension --bin package -- target/release/libparcel_duckdb.dylib parcel.duckdb_extension
```

The extension is not signed: load it with `allow_unsigned_extensions` set (`duckdb -unsigned`).

## Spark

Compile to a JSON file, then start Spark with the extension:

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

The extension reads the `*.spark.json` files in the directories `spark.parcel.contracts` lists (comma-separated). Caller fields are `spark.parcel.caller.<field>`, set at start or with `SET` / `spark.conf.set` between queries: `spark.parcel.caller.other.region` for `ctx.other.region`. A list (`roles`) is comma-separated.

## What runs, and what is refused

| Contract feature | In an engine |
| --- | --- |
| `expose`, `transform` | the query's columns |
| `admit`, `assert` with `on_fail: drop` | the query's filter |
| `decide` | a guard: a refused caller's query fails with `refused by <rule>: <message>` |
| `assert` with `report` or `deny`, `guarantee` | not part of a caller's view: they judge the dataset (`parcel check`, peQL) |
| `shape` | refused: shapes act on the caller's aggregates or spend a privacy budget |
| `enrich`, WebAssembly functions | refused |

## Limits

- **Governance, not enforcement.** In DuckDB and Spark the caller states who they are, and anyone who can read the files can read them directly. Use peQL where the caller must be authenticated.
- **Caller conditions.** DuckDB and Spark fold caller fields into constants when they plan, so a rule comparing a column with a caller field prunes partitions and row groups.
- **A refused caller and no rows.** The `decide` guard is part of the filter: over an empty table, a refused caller gets no rows rather than an error.
- **The DuckDB extension** carries parcel's runtime to read `.parcel` files (about 128 MB unstripped); nothing of it runs while a query executes.
- **Strings in Spark.** Spark reads a backslash in a string literal as an escape; parcel doubles each one.
