# Writing contracts

Start with a small dataset and the access you want each caller to have. The {doc}`quickstart` provides a working example; the {doc}`language` reference lists the available fields and expressions.

## Choose the rule for the requirement

| Requirement | Use |
| --- | --- |
| Only approved purposes may query. | `decide` with `ctx.purpose`. |
| Suppliers may read only their orders. | `admit` comparing a row field with the caller. |
| Every amount must be positive. | `assert` with an explicit failure action. |
| External callers should receive masked email addresses. | `transform` on an exposed column. |
| The dataset must contain records or meet a freshness requirement. | `guarantee` over dataset statistics. |
| Small groups should be suppressed, or results sampled or noised. | `shape` with the appropriate operator. |

Use `expose` to list the columns callers may query. A rule can still use other source columns to decide access.

## Make failure behaviour explicit

For a row assertion, `drop` removes failing rows from query results, `deny` makes the dataset verdict invalid, and `report` counts failures without excluding rows. These choices do not delete source records.

For a dataset guarantee, `deny` blocks use when the requirement fails; `annotate` records the failure. Guarantees involving stored write time or the query's current time must be evaluated by the serving engine.

## Test with representative callers

From the supplier example directory:

```bash
parcel check orders.yaml --data orders.csv --caller acme.yaml --caller globex.yaml --json
```

Include callers who should receive different results. Test missing values and failing records as well as valid data. Read the verdict and caller results separately: an intended caller refusal is not itself a failing `parcel check` exit status.

`check` validates the supplied dataset and compares rule evaluators on up to 1,000 rows by default. `--sample N` changes the latter limit; it does not limit the dataset used for the validation verdict.

For CSV identifiers that look numeric, preserve their intended type with an override, such as `--type supplier_id=utf8`. Compile a bundle using a schema representative of the data the engine will actually serve.

## Reuse a parent contract

A child contract can inherit a parent and add restrictions. For example, save this next to `orders.yaml`:

```yaml
contract: purchasing/large_orders
version: 1
owner: acme
inherits: purchasing/orders
rules:
  - id: large_orders
    op: admit
    expr: row.amount >= 100
```

It retains both supplier access and positive-amount checks, then limits results to amounts of at least 100. The CLI finds parent contracts by name among YAML and JSON files in the same directory. A child cannot remove a parent rule, expose a hidden column or bind different data.

## Other ways to author and use contracts

```{toctree}
:maxdepth: 1

odcs
functions
exports
```
