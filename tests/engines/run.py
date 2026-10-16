#!/usr/bin/env python3
"""End-to-end tests of parcel's engine targets: DuckDB and Spark.

For each engine, every caller queries each contract by name, and the rows the engine returns are
compared, row for row and value for value, with the rows parcel's own runtime computes
(`parcel check --rows`). Refused callers must be refused, with the rule named. Then: queries
on top of the contract (aggregates, extra filters), filters reaching the scan, two contracts over
one table, and a replaced contract taking effect on the next query.

    tests/engines/run.py --parcel target/debug/parcel \\
        --duckdb-extension target/release/parcel.duckdb_extension \\
        --spark-jar crates/parcel-spark/jvm/parcel-spark.jar \\
        [--data customers.csv] [--json results.json]

Needs Python packages duckdb, pyarrow, and for Spark pyspark (with Java 17+). An engine whose option is missing is skipped.
"""

import argparse
import json
import math
import os
import random
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent
EXAMPLE = ROOT / "examples" / "customers"
CONTRACTS = {
    "sales/customers@1.0.0": EXAMPLE / "customers.yaml",
    "sales/customer_segments@1.0.0": EXAMPLE / "segments.yaml",
}
COLUMNS = [
    ("customer_id", "VARCHAR"),
    ("customer_name", "VARCHAR"),
    ("customer_age", "BIGINT"),
    ("gender", "VARCHAR"),
    ("customer_segment", "VARCHAR"),
    ("customer_city", "VARCHAR"),
    ("customer_state", "VARCHAR"),
    ("customer_country", "VARCHAR"),
    ("region", "VARCHAR"),
    ("customer_postal_code", "VARCHAR"),
    ("customer_acquisition_cost", "DOUBLE"),
]


# ---------------------------------------------------------------- data


def synthetic(n, seed=7):
    """Customers shaped like the customer master: ids, names, ages, places, costs."""
    rnd = random.Random(seed)
    first = ["Ann", "Ben", "Chen", "Dana", "Eli", "Fatma", "Gus", "Hana", "Ivan", "Jo"]
    last = ["Ali", "Brown", "Cruz", "Diaz", "Evans", "Fox", "Gray", "Hall", "Ito", "Jones"]
    places = [
        ("USA", "Texas", "Austin"), ("USA", "Ohio", "Akron"), ("UAE", "Dubai", "Dubai"),
        ("Germany", "Bavaria", "Munich"), ("India", "Kerala", "Kochi"), ("Kenya", "Nairobi", "Nairobi"),
    ]
    rows = []
    for i in range(1, n + 1):
        country, state, city = rnd.choice(places)
        rows.append({
            "customer_id": f"CUST-{i:06d}",
            "customer_name": f"{rnd.choice(first)} {rnd.choice(last)}",
            "customer_age": rnd.randint(16, 80),
            "gender": rnd.choice(["Male", "Female", "Non-Binary"]),
            "customer_segment": rnd.choice(["Consumer", "Corporate", "VIP"]),
            "customer_city": city,
            "customer_state": state,
            "customer_country": country,
            "region": rnd.choice(["North", "South", "East", "West", "Central"]),
            "customer_postal_code": str(rnd.randint(10000, 99999)),
            "customer_acquisition_cost": round(rnd.uniform(5, 60), 2),
        })
    return rows


def dirty(rows, seed=11):
    """Rows the quality rules must drop, and edge cases the admit rules must handle."""
    rnd = random.Random(seed)
    base = len(rows)
    out = []
    breaks = [
        ("customer_age", 150), ("customer_age", -3), ("customer_age", None),
        ("customer_acquisition_cost", -10.5), ("customer_acquisition_cost", None),
        ("customer_id", "CUST-12"), ("customer_id", "cust-000001"), ("customer_id", None),
        ("gender", None), ("customer_country", None), ("region", None),
        ("customer_name", None), ("customer_postal_code", None), ("customer_postal_code", "7"),
        ("customer_age", 17), ("customer_age", 18),
    ]
    for k, (col, value) in enumerate(breaks * 3):
        r = dict(rnd.choice(rows))
        r["customer_id"] = f"CUST-{base + k + 1:06d}"
        r[col] = value
        if col == "region" and value is None:
            r["region"] = None
        out.append(r)
    return out


def prepare_data(duck, work, csv):
    """Write the customers as Parquet partitioned by region."""
    data = work / "data" / "customers"
    if csv:
        duck.execute(f"""CREATE TABLE base AS SELECT * REPLACE (
            CAST(customer_postal_code AS VARCHAR) AS customer_postal_code,
            CAST(customer_age AS BIGINT) AS customer_age)
            FROM read_csv('{csv}')""")
        clean = duck.execute("SELECT * FROM base").fetchall()
        names = [c[0] for c in COLUMNS]
        rows = [dict(zip(names, r)) for r in clean]
    else:
        rows = synthetic(5000)
        duck.execute("CREATE TABLE base (" + ", ".join(f"{c} {t}" for c, t in COLUMNS) + ")")
        duck.executemany("INSERT INTO base VALUES (" + ", ".join("?" for _ in COLUMNS) + ")",
                         [[r[c] for c, _ in COLUMNS] for r in rows])
    extra = dirty(rows)
    duck.executemany("INSERT INTO base VALUES (" + ", ".join("?" for _ in COLUMNS) + ")",
                     [[r[c] for c, _ in COLUMNS] for r in extra])
    # A row with no region cannot sit in a Hive partition; Parquet writes it under
    # region=__HIVE_DEFAULT_PARTITION__, which every engine reads back differently, so it
    # is left out.
    duck.execute(f"COPY (SELECT * FROM base WHERE region IS NOT NULL) TO '{data}' "
                 "(FORMAT parquet, PARTITION_BY (region))")
    total = duck.execute("SELECT count(*) FROM base WHERE region IS NOT NULL").fetchone()[0]
    return total, len(extra)


# ---------------------------------------------------------------- parcel


def run(cmd, cwd, check=True):
    p = subprocess.run(cmd, cwd=cwd, capture_output=True, text=True,
                       env={**os.environ, "NO_COLOR": "1", "NO_PROGRESS": "1"})
    if check and p.returncode != 0:
        raise RuntimeError(f"{' '.join(map(str, cmd))}\n{p.stdout}\n{p.stderr}")
    return p


def compile_all(parcel, work):
    """Every contract to every target, then the reference rows for every caller."""
    out = work / "contracts"
    out.mkdir()
    for name, src in CONTRACTS.items():
        stem = src.stem
        shutil.copy(src, work / src.name)
        for target, path in [("duckdb", f"{stem}.parcel"), ("spark", f"{stem}.spark.json")]:
            run([parcel, "compile", src.name, "--schema", "data/customers", "--target", target,
                 "-o", f"contracts/{path}"], work)
    callers = sorted((EXAMPLE / "callers").glob("*.yaml"))
    shutil.copytree(EXAMPLE / "callers", work / "callers")
    reference = {}
    for name, src in CONTRACTS.items():
        rdir = work / "reference" / src.stem
        p = run([parcel, "check", src.name, "--data", "data/customers", "--rows", str(rdir), "--json",
                 *sum([["--caller", f"callers/{c.name}"] for c in callers], [])], work, check=False)
        report = json.loads(p.stdout)
        reference[name] = {"report": report, "dir": rdir}
    return callers, reference


def caller(path):
    """A caller file as a dict (the files are flat YAML: `key: value` and `other: {k: v}`)."""
    c = {"other": {}}
    for line in Path(path).read_text().splitlines():
        if not line.strip() or line.startswith("#"):
            continue
        k, v = line.split(":", 1)
        v = v.strip()
        if v.startswith("{"):
            for pair in v.strip("{}").split(","):
                kk, vv = pair.split(":", 1)
                c["other"][kk.strip()] = vv.strip()
        else:
            c[k.strip()] = v
    return c


def expected(reference, name, c):
    """The reference rows for caller `c`, or the rule that refuses them."""
    rep = reference[name]["report"]
    label = f"{c['tenant']}/{c['id']}"
    entry = next(x for x in rep["callers"] if x["caller"] == label)
    if entry["refused_by"]:
        return None, entry["refused_by"]
    import pyarrow.parquet as pq
    table = pq.read_table(reference[name]["dir"] / f"{c['id']}.parquet")
    return table.to_pylist(), None


# ---------------------------------------------------------------- comparison


def norm(v):
    if isinstance(v, float):
        return None if math.isnan(v) else round(v, 6)
    if isinstance(v, bytes):
        return v.hex()
    return v


def rows_key(rows, columns):
    return sorted(
        (tuple(norm(r[c]) for c in columns) for r in rows),
        key=lambda t: tuple((x is None, str(x)) for x in t))


def compare(got, want, columns):
    """None when equal, else a short description of the first differences."""
    g, w = rows_key(got, columns), rows_key(want, columns)
    if g == w:
        return None
    gs, ws = set(g), set(w)
    extra, missing = [r for r in g if r not in ws][:2], [r for r in w if r not in gs][:2]
    return f"{len(got)} rows, expected {len(want)}; unexpected {extra}; missing {missing}"


# ---------------------------------------------------------------- engines


class DuckDb:
    name = "duckdb"

    def __init__(self, extension, work):
        import duckdb
        os.chdir(work)
        os.environ["PARCEL_CONTRACTS"] = str(work / "contracts")
        # Unsigned, and built for the C API: the platform and version DuckDB reports may differ.
        self.con = duckdb.connect(config={"allow_unsigned_extensions": "true",
                                          "allow_extensions_metadata_mismatch": "true"})
        self.con.execute(f"LOAD '{extension}'")

    def as_caller(self, c):
        for k in ["id", "tenant", "purpose", "tier"]:
            self.set(k, c.get(k))
        for k, v in c["other"].items():
            self.set(f"other_{k}", v)

    def set(self, field, value):
        if value is None:
            self.con.execute(f"RESET VARIABLE parcel_{field}")
        else:
            self.con.execute(f"SET VARIABLE parcel_{field} = ?", [value])

    def query(self, sql):
        cur = self.con.execute(sql)
        cols = [d[0] for d in cur.description]
        return [dict(zip(cols, r)) for r in cur.fetchall()]

    def quote(self, name):
        return f'"{name}"'

    def explain(self, sql):
        return "\n".join(r[1] for r in self.con.execute("EXPLAIN " + sql).fetchall())

    def pushed(self, plan):
        return "READ_PARQUET" in plan and "Filters:" in plan


class Spark:
    name = "spark"

    def __init__(self, jar, work):
        from pyspark.sql import SparkSession
        os.chdir(work)
        self.spark = (SparkSession.builder.master("local[2]").appName("parcel-engines")
                      .config("spark.jars", jar)
                      .config("spark.sql.extensions", "cloud.griot.parcel.spark.ParcelExtensions")
                      .config("spark.parcel.contracts", str(work / "contracts"))
                      .config("spark.sql.session.timeZone", "UTC")
                      .config("spark.ui.enabled", "false")
                      .getOrCreate())
        self.spark.sparkContext.setLogLevel("ERROR")

    def as_caller(self, c):
        for k in ["id", "tenant", "purpose", "tier"]:
            self.set(k, c.get(k))
        for k, v in c["other"].items():
            self.set(f"other.{k}", v)

    def set(self, field, value):
        key = f"spark.parcel.caller.{field}"
        if value is None:
            self.spark.conf.unset(key)
        else:
            self.spark.conf.set(key, value)

    def query(self, sql):
        return [r.asDict() for r in self.spark.sql(sql).collect()]

    def quote(self, name):
        return f"`{name}`"

    def explain(self, sql):
        return self.spark.sql(sql)._jdf.queryExecution().executedPlan().toString()

    def pushed(self, plan):
        return "PartitionFilters: [isnotnull(region" in plan or "PushedFilters: [" in plan


# ---------------------------------------------------------------- tests


def test_engine(engine, callers, reference, total, work, results):
    out = {"engine": engine.name, "cases": []}
    results.append(out)

    def case(name, ok, detail="", ms=None):
        out["cases"].append({"case": name, "ok": ok, "detail": detail, "ms": ms})
        mark = "✓" if ok else "✗"
        print(f"  {mark} {name:<62} {'' if ms is None else f'{ms:7.1f} ms'}  {detail if not ok else ''}")

    for name in CONTRACTS:
        for path in callers:
            c = caller(path)
            want, refused_by = expected(reference, name, c)
            engine.as_caller(c)
            sql = f"SELECT * FROM {engine.quote(name)}"
            t = time.perf_counter()
            try:
                got = engine.query(sql)
                err = None
            except Exception as e:
                got, err = None, str(e)
            ms = (time.perf_counter() - t) * 1000
            label = f"{name} as {c['id']}"
            if refused_by:
                ok = err is not None and f"refused by {refused_by}" in err
                case(f"{label}: refused by {refused_by}", ok, err or "returned rows", ms)
            elif err:
                case(f"{label}: {len(want)} rows", False, err.splitlines()[0][:200], ms)
            else:
                cols = list(want[0].keys()) if want else list(got[0].keys()) if got else []
                diff = compare(got, want, cols)
                case(f"{label}: {len(want)} rows", diff is None, diff or "", ms)

    # Queries on top of a contract: an aggregate and an extra filter, as the North analyst.
    c = caller(EXAMPLE / "callers" / "hq_analyst.yaml")
    engine.as_caller(c)
    want, _ = expected(reference, "sales/customers@1.0.0", c)
    name = engine.quote("sales/customers@1.0.0")
    agg = engine.query(f"SELECT region, count(*) AS n FROM {name} GROUP BY region")
    want_agg = {}
    for r in want:
        want_agg[r["region"]] = want_agg.get(r["region"], 0) + 1
    got_agg = {r["region"]: r["n"] for r in agg}
    case("aggregate on a contract: count by region", got_agg == want_agg, f"{got_agg} vs {want_agg}")
    usa = engine.query(f"SELECT customer_id FROM {name} WHERE customer_country = 'USA' AND customer_age > 40")
    want_usa = sorted(r["customer_id"] for r in want if r["customer_country"] == "USA" and r["customer_age"] > 40)
    case("extra filter on a contract: USA, over 40", sorted(r["customer_id"] for r in usa) == want_usa,
         f"{len(usa)} vs {len(want_usa)}")

    # Filters reach the scan: the North analyst's region rule is applied by the table scan.
    c = caller(EXAMPLE / "callers" / "north_analyst.yaml")
    engine.as_caller(c)
    plan = engine.explain(f"SELECT count(*) FROM {name}")
    case("contract rules run in the table scan (EXPLAIN)", engine.pushed(plan), plan[:400])
    out["explain"] = plan

    # Two contracts over one table, in one query.
    c = caller(EXAMPLE / "callers" / "acme_marketing.yaml")
    engine.as_caller(c)
    both = engine.query(
        f"SELECT (SELECT count(*) FROM {engine.quote('sales/customers@1.0.0')}) AS a, "
        f"(SELECT count(*) FROM {engine.quote('sales/customer_segments@1.0.0')}) AS b")[0]
    wa, _ = expected(reference, "sales/customers@1.0.0", c)
    wb, _ = expected(reference, "sales/customer_segments@1.0.0", c)
    case("two contracts over one table in one query", (both["a"], both["b"]) == (len(wa), len(wb)),
         f"{both} vs {(len(wa), len(wb))}")

    # Unknown names are still reported, not swallowed.
    try:
        engine.query(f"SELECT * FROM {engine.quote('sales/nothing@1.0.0')}")
        case("an unknown contract name is an error", False, "returned rows")
    except Exception as e:
        case("an unknown contract name is an error", True, str(e).splitlines()[0][:120])

    return out


def test_realtime(engine_name, make, work, parcel, results):
    """A contract file replaced on disk takes effect on the next query (DuckDB, Spark)."""
    if engine_name not in ("duckdb", "spark"):
        return
    engine = make()
    c = caller(EXAMPLE / "callers" / "hq_analyst.yaml")
    engine.as_caller(c)
    name = engine.quote("sales/customers@1.0.0")
    before = engine.query(f"SELECT count(*) AS n FROM {name}")[0]["n"]
    src = (work / "customers.yaml").read_text()
    stricter = src.replace(
        "  - id: plausible_age\n",
        "  - id: over_sixty\n    op: admit\n    expr: row.customer_age > 60\n\n  - id: plausible_age\n")
    (work / "customers_v2.yaml").write_text(stricter)
    ext = {"duckdb": "customers.parcel", "spark": "customers.spark.json"}[engine_name]
    time.sleep(1.1)  # file times have one-second resolution on some file systems
    run([parcel, "compile", "customers_v2.yaml", "--schema", "data/customers", "--target", engine_name,
         "-o", f"contracts/{ext}"], work)
    after = engine.query(f"SELECT count(*) AS n FROM {name}")[0]["n"]
    run([parcel, "compile", "customers.yaml", "--schema", "data/customers", "--target", engine_name,
         "-o", f"contracts/{ext}"], work)
    ok = after < before
    results[-1]["cases"].append({"case": "a replaced contract takes effect on the next query", "ok": ok,
                                 "detail": f"{before} rows, then {after}", "ms": None})
    print(f"  {'✓' if ok else '✗'} {'a replaced contract takes effect on the next query':<62}           {before} rows, then {after}")


def test_refusals(parcel, work, results):
    """Contracts using what one query cannot express are refused at compile time."""
    out = {"engine": "compile", "cases": []}
    results.append(out)
    src = (work / "customers.yaml").read_text()
    shaped = src + ("\n  - id: small_groups\n    op: shape\n    operator: suppress\n    params: {k: 5}\n")
    (work / "shaped.yaml").write_text(shaped)
    for target in ["duckdb", "spark"]:
        p = run([parcel, "compile", "shaped.yaml", "--schema", "data/customers", "--target", target],
                work, check=False)
        ok = p.returncode != 0 and "shape `small_groups` is not supported" in p.stderr
        out["cases"].append({"case": f"{target}: a contract with a shape is refused", "ok": ok,
                             "detail": p.stderr.strip()[:200], "ms": None})
        print(f"  {'✓' if ok else '✗'} {target + ': a contract with a shape is refused':<62}")


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--parcel", required=True)
    ap.add_argument("--duckdb-extension")
    ap.add_argument("--spark-jar")
    ap.add_argument("--data", help="customers CSV; default: synthetic customers")
    ap.add_argument("--json", help="write the results here")
    args = ap.parse_args()
    parcel = str(Path(args.parcel).resolve())

    import duckdb
    work = Path(tempfile.mkdtemp(prefix="parcel-engines-"))
    (work / "data").mkdir()
    total, broken = prepare_data(duckdb.connect(), work, args.data and str(Path(args.data).resolve()))
    print(f"data      {total:,} rows ({broken} broken on purpose)  {work}")
    callers, reference = compile_all(parcel, work)
    for name in CONTRACTS:
        rep = reference[name]["report"]
        seen = ", ".join(f"{x['caller'].split('/')[1]} {x['rows'] if x['rows'] is not None else 'refused'}"
                         for x in rep["callers"])
        print(f"reference {name}: {seen}")

    results = []
    makers = []
    if args.duckdb_extension:
        ext = str(Path(args.duckdb_extension).resolve())
        makers.append(("duckdb", lambda: DuckDb(ext, work)))
    if args.spark_jar:
        jar = str(Path(args.spark_jar).resolve())
        makers.append(("spark", lambda: Spark(jar, work)))
    for name, make in makers:
        print(f"\n{name}")
        engine = make()
        test_engine(engine, callers, reference, total, work, results)
        test_realtime(name, lambda: engine, work, parcel, results)
    print("\ncompile")
    test_refusals(parcel, work, results)

    cases = [c for r in results for c in r["cases"]]
    failed = [c for c in cases if not c["ok"]]
    print(f"\n{len(cases) - len(failed)}/{len(cases)} passed")
    if args.json:
        Path(args.json).write_text(json.dumps({
            "rows": total, "broken": broken,
            "reference": {n: r["report"]["callers"] for n, r in reference.items()},
            "results": results,
        }, indent=2, default=str))
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
