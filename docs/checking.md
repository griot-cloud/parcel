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

Parcel checks the contract's definitions against the supplied columns and types, then evaluates every rule against the rows. The output starts with the contract, the row count and the verdict, then has one section per caller:

```text
purchasing/orders v1
4 rows  orders.csv  valid

==== default caller ========================================
columns  2/3
rows     0/4

Policies
  supplier_orders  ✗  admit      4 removed
```

The default caller has an empty tenant, so `supplier_orders` removes every order. Pass callers to see the quality rules over the rows each one receives.

Order 3 fails `positive_amount` because its amount is `-5`. The dataset remains valid because `on_fail: drop` excludes that row. With `on_fail: deny`, the same failure makes the dataset invalid. With `on_fail: report`, it is recorded without excluding the row.

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

The output is:

```text
purchasing/orders v1
4 rows  orders.csv  valid

==== acme/purchasing-team ==================================
columns  2/3
rows     3/4

Policies
  supplier_orders  ✓  admit

Quality                 pass
  positive_amount  !   75.0%  1 dropped
                              -5

==== globex/supplier-analyst ===============================
columns  2/3
rows     2/4

Policies
  supplier_orders  ✓  admit      1 removed

Quality                 pass
  positive_amount  !   66.7%  1 dropped
                              -5

differential  ✓  12
```

Each caller section shows:

| Line | Meaning |
| --- | --- |
| `columns` | Exposed columns out of the data's columns, and any the caller's transforms change. |
| `rows` | Rows the caller receives out of all rows, or `refused`. |
| Policies | Each `decide`, `admit`, `transform` and `shape` rule for this caller: rows removed, values changed, or the field that refused them. |
| Quality | Each `assert` over the rows the caller's `admit` rules keep: pass rate, rows dropped or failed, and the commonest failing values. Then each `guarantee`. |

Acme receives orders 1, 2 and 4. Globex receives orders 1 and 4: `supplier_orders` removes order 2, and `positive_amount` drops order 3. The report prints counts and failing values, not rows. A caller refused by a `decide` rule gets the rule, its condition and the caller's value:

```text
Policies
  analytics_only     ✗  decide
                        ctx.purpose in ["analytics", "reporting"]
                        ctx.purpose = "marketing"
```

A rule's `message`, when the contract gives one, appears after it.

On a terminal, each rule shows a progress bar while it runs. All rules for all callers run at the same time.

Profiles can be YAML or JSON. `id`, `tenant`, and `purpose` are required. Optional fields are `tier`, `clearance`, `classification`, `roles`, `now`, and `other`. Without `--caller`, the command uses `id: check`, `tenant: ""`, and `purpose: analytics`.

The report ends with `differential`: the number of evaluations on which parcel's two rule evaluators, the CEL interpreter and DataFusion, agreed. This is separate from the quality verdict and caller access counts. `--sample` limits this internal comparison; quality validation still checks every supplied row.

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
| `--rows DIR` | Write the rows each caller receives to `DIR/<caller id>.parquet`. |
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
| `1` | Problems in the contract, invalid data, or differing evaluator results. |
| `2` | An input, parsing, or execution error. |

A caller refusal alone does not make the command fail. Check the caller results when testing access policies.
