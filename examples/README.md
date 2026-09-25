# Examples

| Workspace | Shows |
|---|---|
| [`quickstart`](quickstart) | A three-tenant orders contract: check, what each caller sees, refusals, bundles, and an inheriting child (`sales/orders_ea`). |
| [`utility`](utility) | A tenant's own Rust functions, compiled to WebAssembly, called from rules and enrichers. |
| [`udf-meter-serial`](udf-meter-serial) | The source of that function module (`parcel_udf::export!`). |

Each example holds contracts, callers and sample data. Writing and querying data under these contracts is peQL's job; see its examples.

Run everything from a clean state:

```sh
cargo build -p parcel-cli
PARCEL=$PWD/target/debug/parcel examples/run-all.sh
```

`verify-duckdb.py` runs a contract's validation plan, exported as DuckDB SQL, inside DuckDB, and checks it reproduces parcel's verdict value for value (`pip install duckdb`).
