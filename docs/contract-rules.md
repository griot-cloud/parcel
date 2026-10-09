# Rules

`rules` defines data quality checks and access policies. For example, this rule allows each supplier to access only its own orders:

```yaml
rules:
  - id: supplier_orders
    op: admit
    expr: "row.supplier_id == ctx.tenant"
```

An order with `supplier_id: globex` passes this rule when the caller's organization is also `globex`. An order belonging to `initech` does not.

## Parts of a rule

| Field | Meaning in this example |
| --- | --- |
| `id` | `supplier_orders` is the rule's unique name. |
| `op` | `admit` specifies a rule that selects which rows are allowed. |
| `expr` | The condition compares the order's supplier with the caller's organization. `==` means “equals”. |

Other rule types have additional fields, such as `on_fail` for the action taken when a quality check fails.

## Namespaces

A namespace identifies where a value used in a rule comes from. Parcel has three: `row`, `ctx`, and `dataset`.

| Namespace | Contains | Example |
| --- | --- | --- |
| `row` | Values from one record in the data. | `row.amount` is the amount on an order. |
| `ctx` | Details about the user, agent, or service requesting access. | `ctx.tenant` is their organization identifier. |
| `dataset` | Information about the dataset as a whole. | `dataset.row_count` is the number of records. |

For an order whose `supplier_id` is `globex`, this condition compares that value with the organization requesting access:

```text
row.supplier_id == ctx.tenant
```

It is true when `ctx.tenant` is also `globex`. A quality check such as `row.amount > 0` uses only the order's values. A check such as `dataset.row_count > 0` uses the total number of records.

The application supplies `ctx` values. For more details on the available fields, see [Namespaces](language.md#namespaces).

## Rule types

`op` selects one of six rule types. Each type can use specific namespaces:

| Rule | Purpose | Values it can read |
| --- | --- | --- |
| `decide` | Define who is allowed access. | `ctx` |
| `admit` | Define which rows they may access. | `row`, `ctx` |
| `assert` | Check the quality of individual rows. | `row` |
| `transform` | Replace a column's value. | `row`, `ctx` |
| `guarantee` | Check the dataset as a whole. | `dataset`, plus `ctx.now` for the current time |
| `shape` | Limit small groups, sample rows, or add noise to numeric values. | `ctx` in its optional `unless` condition |

## Caller access: `decide`

`decide` defines whether a user, agent, or service is allowed access. For example, access to orders can be limited to fulfilment and audit work:

```yaml
rules:
  - id: approved_purpose
    op: decide
    expr: "ctx.purpose == 'fulfilment' || ctx.purpose == 'audit'"
```

`ctx.purpose` is the purpose supplied by the application. `||` means “or”. This rule allows either listed purpose and refuses any other purpose.

## Which records: `admit`

`admit` defines which rows are allowed. To limit each supplier to its own orders:

```yaml
rules:
  - id: own_orders
    op: admit
    expr: "row.supplier_id == ctx.tenant"
```

`row.supplier_id` is the supplier recorded on an order. `ctx.tenant` is the caller's organization identifier, supplied by the application. An order marked `globex` passes when the caller's organization is also `globex`.

When there are several `admit` rules, a row must pass all of them.

## Record quality: `assert`

`assert` checks each row against a data quality requirement. This rule requires a positive amount:

```yaml
rules:
  - id: positive_amount
    op: assert
    expr: "row.amount > 0"
    on_fail: drop
```

An amount of `120` passes. An amount of `0`, `-5`, or a missing amount fails. `on_fail` specifies the response:

| `on_fail` | Behavior |
| --- | --- |
| `drop` | Exclude failing rows. |
| `deny` | Any failing row makes the dataset invalid for use. |
| `report` | Failures are recorded; the check does not exclude rows. |

For a required order identifier, the check can instead be `expr: "has(row.order_id)"`. With `on_fail: deny`, a missing identifier makes the dataset invalid. These actions do not delete the original data.

## Returned values: `transform`

`transform` replaces a column's value. For example, an owner may want to share email addresses with internal staff and mask them for other callers:

```yaml
expose:
  - {name: email, type: utf8}
rules:
  - id: mask_email
    op: transform
    column: email
    expr: "ctx.tier == 'internal' ? row.email : redact(row.email)"
```

This expression keeps the email when `ctx.tier` is `internal`; otherwise, it replaces it with `***`. Missing emails remain null. `column` names the column being changed. Its replacement value must match the type declared in `expose`—text (`utf8`) here.

## Dataset quality: `guarantee`

`guarantee` checks a requirement for the whole dataset. This rule requires at least one row:

```yaml
rules:
  - id: nonempty
    op: guarantee
    expr: "dataset.row_count > 0"
    on_fail: deny
```

`dataset.row_count` is the number of rows. An empty dataset fails this rule. `on_fail: deny` marks it as invalid for use; `on_fail: annotate` records the failure without marking it invalid.

A requirement can also use column statistics. For example, `dataset.amount.null_rate < 0.01` requires fewer than 1% of amounts to be missing.

## Result shape: `shape`

Shape rules control how results are returned. This rule removes groups calculated from fewer than five rows, such as a sales total based on only three orders:

```yaml
rules:
  - id: suppress_small_groups
    op: shape
    operator: suppress
    params: {k: 5}
    unless: "ctx.tier == 'internal'"
```

With `k: 5`, a total calculated from three orders is excluded; one calculated from six orders is kept. `unless` skips this rule for callers whose `ctx.tier` is `internal`.

### Sampling

A sample rule selects a proportion of rows using a column as the selection key:

```yaml
rules:
  - id: sample_orders
    op: shape
    operator: sample
    params: {fraction: 0.1, key: order_id}
```

`fraction: 0.1` requests a 10% sample. Selection is repeatable for the same `order_id` values; it does not guarantee an exact row count. The key must be listed in `expose`.

### Numeric noise

A noise rule adds random variation to numeric results:

```yaml
rules:
  - id: noise_revenue
    op: shape
    operator: noise
    column: revenue
    params: {sensitivity: 100.0, epsilon: 1.0, budget: partner_reports}
```

`column` names the numeric column. `sensitivity` and `epsilon` control the amount of noise; `budget` names the privacy budget the application must track. The example applies noise to calculations over `revenue`, such as totals. `at: row` inside `params` applies it to individual values instead. For parameter limits, see [Shapes](language.md#shapes).

## Rule expressions

Expressions use CEL, a language for conditions and calculations. `&&` means “and”, `||` means “or”, and `has(row.field)` checks for a value that is not null. For more details, see [Expressions](language.md#expressions).
