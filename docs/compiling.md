# Compiling contracts

`parcel compile` checks a contract against column names and types, then builds its rules into a compiled contract. It can save that contract or export its data validation plan.

Compilation checks definitions and expressions. To evaluate whether data values satisfy the rules, use {doc}`checking`.

## Compile a contract

These commands use `orders.yaml` and `orders.csv` in `examples/suppliers`, introduced in the {doc}`quickstart`:

```bash
cd examples/suppliers
parcel compile orders.yaml --schema orders.csv
```

`--schema` reads column names and types from a CSV file, Parquet file, or directory of Parquet files. The command prints the contract name, version, compilation hash, and a report for each rule. For this example, the report includes:

| Rule | Operation | Evaluation |
| --- | --- | --- |
| `supplier_orders` | `admit` | Uses supplier comparisons to skip files or row groups where possible. |
| `positive_amount` | `assert` | Can be calculated at write time and stored as a flag used to filter rows. |

This command creates no output file.

## Save the compiled contract

```bash
parcel compile orders.yaml --schema orders.csv --out orders.parcel
```

The command prints the report and writes `orders.parcel`, followed by:

```text
wrote orders.parcel
```

The file contains the compiled contract, its validation plan, and its write plan. It includes the column definitions, compiled rule expressions, hashes, and parcel version needed to load it. Its format is `parcel-compiled/1`, a JSON envelope containing serialized plans and expressions.

Applications load this file without recompiling the YAML. The loader requires the same parcel version that compiled it. For the Rust loading API, see [Compiled contract bytes](rust-artifacts.md#compiledbytes).

## Inspect the compilation as JSON

```bash
parcel compile orders.yaml --schema orders.csv --json > compilation.json
```

`compilation.json` contains the JSON representation of the compilation for inspection. Use `--out` to create the loadable compiled file; the inspection JSON has a different representation.

Both can be produced together:

```bash
parcel compile orders.yaml --schema orders.csv --json --out orders.parcel > compilation.json
```

## Export validation as SQL

```bash
parcel compile orders.yaml --schema orders.csv --sql duckdb --table orders > validation.sql
```

This creates `validation.sql`, a query that checks data quality in a table named `orders`. Running it in the target engine returns one summary row containing `valid`, `breached`, `row_count`, assertion failure counts, guarantee results, and the statistics needed by those checks.

For the example's four orders, the summary reports four rows, one failure of `positive_amount`, and a valid dataset because that rule uses `on_fail: drop`.

Choose the SQL dialect of the engine that will run the query. The command options below list supported dialects. Export warnings are printed separately from the SQL. They identify behavior that may need verification in the target engine.

The exported query covers data validation. It does not include the contract's caller access policies, masking, or result shapes.

## Export validation as Substrait

Substrait is a portable query-plan format. To create a binary validation plan:

```bash
parcel compile orders.yaml --schema orders.csv --substrait validation.bin --table orders
```

The command writes `validation.bin` and prints `wrote validation.bin`. The receiving engine must support the plan and its functions.

This option requires a build with the `substrait` feature. For a source build, install `protoc`, then run:

```bash
cargo build --release -p parcel-cli --features substrait
```

## Command options

```text
parcel compile CONTRACT --schema SCHEMA [OPTIONS]
```

| Argument or option | Meaning |
| --- | --- |
| `CONTRACT` | Required path to a YAML or JSON contract. |
| `--schema SCHEMA` | Required CSV file, Parquet file, or Parquet directory supplying column names and types. |
| `-o FILE`, `--out FILE` | Write a loadable compiled contract to this file. |
| `--json` | Print compilation details as JSON instead of the text report. |
| `--sql DIALECT` | Print validation SQL. Dialects: `duckdb`, `postgres`, `mysql`, `sqlite`, `bigquery`, `snowflake`, and `datafusion` (the execution library used by parcel). |
| `--table NAME` | Table name used in SQL or Substrait output; default `contract_data`. |
| `--substrait FILE` | Write a binary validation plan; requires the `substrait` feature. |
| `--type COLUMN=TYPE` | Override a CSV column's inferred type. Repeat for multiple columns. |
| `--function MODULE=MANIFEST` | Load a custom WebAssembly function with its manifest. Repeat for multiple functions. |
| `-h`, `--help` | Print command help. |

For example, to treat a CSV identifier as text and save the result:

```bash
parcel compile orders.yaml --schema orders.csv --type supplier_id=utf8 --out orders.parcel
```

For custom function examples, see {doc}`functions`.

`--out` can accompany the text report, `--json`, or `--sql`. If both `--sql` and `--json` are supplied, SQL takes precedence. `--substrait` writes its plan and finishes without producing the other outputs; run a separate command to save a compiled contract.

Compilation returns status `0` on success, `1` for compilation diagnostics, and `2` for input, parsing, or execution errors.
