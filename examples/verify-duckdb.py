#!/usr/bin/env python3
"""Run a contract's validation plan, exported as DuckDB SQL, inside DuckDB and compare the
verdict with parcel's own. Proves the SQL export means the same thing in another engine.

    python3 examples/verify-duckdb.py <contract.yaml> <data.csv> [column=type ...]

Requires `pip install duckdb` and a built `parcel` on PATH (or PARCEL=/path/to/parcel).
parcel's own verdict comes from `parcel check --json` over the same file.
"""
import json
import math
import os
import subprocess
import sys

import duckdb

parcel = os.environ.get("PARCEL", "parcel")
contract, data, *types = sys.argv[1:]
type_args = [a for t in types for a in ("--type", t)]

sql = subprocess.run(
    [parcel, "compile", contract, "--schema", data, "--sql", "duckdb", "--table", "t", *type_args],
    check=True, capture_output=True, text=True,
).stdout
checked = json.loads(subprocess.run(
    [parcel, "check", contract, "--data", data, "--json", *type_args], capture_output=True, text=True,
).stdout)
name, verdict = checked["contract"], checked["verdict"]

duck_types = {t.split("=")[0]: {"utf8": "VARCHAR", "int64": "BIGINT", "float64": "DOUBLE"}[t.split("=")[1]] for t in types}
con = duckdb.connect()
con.execute(f"CREATE TABLE t AS SELECT * FROM read_csv('{data}', types={duck_types!r})")
cur = con.execute(sql)
row = dict(zip([d[0] for d in cur.description], cur.fetchone()))

mismatches = []
def same(a, b):
    if isinstance(a, float) or isinstance(b, float):
        return math.isclose(float(a), float(b), rel_tol=1e-12, abs_tol=1e-12)
    return a == b

checks = {"row_count": verdict["row_count"], "valid": verdict["valid"]}
checks.update({f"fail__{k}": v for k, v in verdict["failures"].items()})
checks.update(verdict["stats"])
for key, want in checks.items():
    got = row.get(key)
    if not same(got, want):
        mismatches.append(f"{key}: duckdb={got!r} parcel={want!r}")
print(f"{len(checks)} values compared for {name}")
if mismatches:
    print("MISMATCHES:\n  " + "\n  ".join(mismatches))
    sys.exit(1)
print("DuckDB reproduces parcel's verdict exactly")
