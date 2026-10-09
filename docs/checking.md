# Checking contracts

`parcel check` evaluates data quality rules, tests access for callers, and reports the results. It does not change the input data.

## Example data and contract

These commands use the files in `examples/suppliers` from the {doc}`quickstart`:

```bash
cd examples/suppliers
```

`orders.csv` contains four orders:

```{literalinclude} ../examples/suppliers/orders.csv
:language: text
```

The example contract, `orders.yaml`, allows Acme to access all suppliers' orders and each supplier to access only its own. Its quality rule excludes amounts that are zero or negative:

```{literalinclude} ../examples/suppliers/orders.yaml
:language: yaml
```

## Check data quality

```bash
parcel check orders.yaml --data orders.csv
```

Parcel checks the contract's definitions against the supplied columns and types, then evaluates its quality rules against every row. The quality section of the output is:

```text
verdict over 4 rows: VALID
  assert    positive_amount            75.00% pass  (1 failing)
```

Order 3 fails because its amount is `-5`. The dataset remains valid because `on_fail: drop` excludes that row. With `on_fail: deny`, the same failure makes the dataset invalid. With `on_fail: report`, it is recorded without excluding the row.

`--data` accepts a CSV file, a Parquet file, or a directory of Parquet files. This is the data checked by the command; it does not load the location in `binding`.

## Mock caller context

A caller profile lets you mock **`ctx`** to test how a rule behaves for different people or organizations. For example, this profile represents a Globex analyst working on purchasing:

```{literalinclude} ../examples/suppliers/globex.yaml
:language: yaml
```

These values supply `ctx.id`, `ctx.tenant`, and `ctx.purpose`. Here, `tenant` is the organization identifier, so the contract compares `row.supplier_id` with `globex`.

The second profile represents Acme's purchasing team:

```{literalinclude} ../examples/suppliers/acme.yaml
:language: yaml
```

Pass both profiles to compare their access:

```bash
parcel check orders.yaml --data orders.csv --caller acme.yaml --caller globex.yaml
```

The caller section of the output is:

```text
caller acme/purchasing-team sees 3 of 4 rows
caller globex/supplier-analyst sees 2 of 4 rows
```

Acme can see orders 1, 2, and 4. Globex can see orders 1 and 4. Order 3 fails the quality rule for both callers. The report prints counts, rather than the rows themselves. If a `decide` rule refuses a caller, it names that rule instead.

Profiles can be YAML or JSON. `id`, `tenant`, and `purpose` are required. Optional fields are `tier`, `clearance`, `classification`, `roles`, `now`, and `other`. Without `--caller`, the command uses `id: check`, `tenant: ""`, and `purpose: analytics`.

The report ends with an internal consistency check. “Interpreter and DataFusion agree” means parcel's two rule evaluators produced matching results. This is separate from the quality verdict and caller access counts. `--sample` limits this internal comparison; quality validation still checks every supplied row.

## Save a JSON report

```bash
parcel check orders.yaml --data orders.csv --caller globex.yaml --json > check-results.json
```

This creates `check-results.json` instead of printing the text report. Selected fields from this example are shown below:

```json
{
  "contract": "purchasing/orders",
  "version": 1,
  "verdict": {
    "valid": true,
    "breached": [],
    "row_count": 4,
    "failures": {"positive_amount": 1}
  },
  "callers": [
    {"caller": "globex/supplier-analyst", "refused_by": null, "rows": 2}
  ],
  "differential": {
    "rows": 4,
    "callers": 1,
    "evaluations": 8,
    "both_errored": 0,
    "mismatches": []
  }
}
```

The full report also includes hashes, per-rule compilation details, dataset statistics, guarantee results, and `query_time_guarantees` for checks deferred until query time.

## Command options

```text
parcel check CONTRACT --data DATA [OPTIONS]
```

| Argument or option | Meaning |
| --- | --- |
| `CONTRACT` | Required path to a YAML or JSON contract. |
| `--data DATA` | Required CSV file, Parquet file, or Parquet directory to check. |
| `--caller FILE` | Mock caller profile. Repeat to test multiple callers. |
| `--sample N` | Maximum rows in the evaluator comparison; default `1000`. |
| `--json` | Print the report as JSON. Redirect it to a file to save it. |
| `--type COLUMN=TYPE` | Override a CSV column's inferred type. Repeat for multiple columns. |
| `--function MODULE=MANIFEST` | Load a custom WebAssembly function with its manifest. Repeat for multiple functions. |
| `-h`, `--help` | Print command help. |

For example, `--type supplier_id=utf8` treats the CSV supplier identifier as text:

```bash
parcel check orders.yaml --data orders.csv --type supplier_id=utf8
```

It produces the same report format. For contracts that use custom functions, see {doc}`functions` for module and manifest examples.

## Exit status

| Status | Meaning |
| --- | --- |
| `0` | The dataset is valid and the two evaluators agree. |
| `1` | Compilation diagnostics, invalid data, or differing evaluator results. |
| `2` | An input, parsing, or execution error. |

A caller refusal alone does not make the command fail. Check the caller results when testing access policies.
