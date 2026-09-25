# The contract language

A contract is a YAML (or JSON) document. `parcel schema` prints its JSON Schema, also published
at `schema/contract.schema.json`; start a file with
`# yaml-language-server: $schema=https://raw.githubusercontent.com/griot-cloud/parcel/main/schema/contract.schema.json`
for completion in editors.

## The document

| Field | Required | Holds |
| --- | --- | --- |
| `contract` | yes | The name callers query, e.g. `sales/orders`. |
| `version` | yes | An integer; engines keep every version. |
| `owner` | no | The tenant the contract belongs to. Its functions are the ones rules may call. |
| `inherits` | no | A parent contract: the child can only restrict it. |
| `binding` | yes, unless inherited | `parquet: <path or directory>`, and `partitioned_by: [columns]`. |
| `expose` | yes, unless inherited | `[{name, type}]`: the columns callers see, and their types. |
| `rules` | no | The rules, below. |
| `extensions` | no | Typed extra fields: `row`, `ctx`, `dataset` maps of field to type. |
| `enrich` | no | `[{field, expr}]`: how each `row.other` field is computed at write. |
| `dataset_other` | no | `[{field, value}]` or `[{field, expr}]`: `dataset.other` fields. |

## Rules

Every rule has an `id` (lower case, digits and underscores) and an `op`.

```yaml
- {id: analytics_only, op: decide, expr: "ctx.purpose in ['analytics', 'reporting']"}
- {id: own_rows, op: admit, expr: "row.tenant_id == ctx.tenant || 'admin' in ctx.roles"}
- {id: pk_present, op: assert, expr: "has(row.order_id)", on_fail: deny}      # deny | drop | report
- {id: mask_email, op: transform, column: email, expr: "ctx.tenant == 'acme' ? row.email : hash_sha256(row.email)"}
- {id: fresh, op: guarantee, expr: "dataset.written_at > ctx.now - duration('72h')", on_fail: annotate}  # deny | annotate
- {id: small_cells, op: shape, operator: suppress, params: {k: 5}, unless: "ctx.tenant == 'acme'"}
- {id: sampled, op: shape, operator: sample, params: {fraction: 0.1, key: order_id}}
- {id: noisy, op: shape, operator: noise, column: salary, params: {sensitivity: 1000, epsilon: 1, budget: salary, at: row}}
```

`noise` takes `at: aggregate` (the default: noise on aggregates over the column, which cannot
be read otherwise) or `at: row` (noise on each value), and a numeric column.

## Namespaces

| Namespace | Fields |
| --- | --- |
| `ctx` | `id`, `tenant`, `purpose`, `tier`, `classification` (strings), `clearance` (int), `roles` (list of strings), `now` (timestamp, fixed per query), `other.<field>` (declared in `extensions.ctx`) |
| `row` | one field per column of the bound data, and `other.<field>` (declared in `extensions.row`, produced by `enrich`) |
| `dataset` | `row_count`, `written_at`, `contract_hash`; per column `<col>.null_count`, `null_rate`, `distinct_count`, `min`, `max`; per assertion `assertions.<id>.pass_rate`; `other.<field>` |

## Expressions

parcel accepts a strict subset of CEL, and rejects anything it cannot translate faithfully,
with a reason.

- **Types:** bool, int, uint, double, string, bytes, timestamp, duration, lists, and exact
  decimals (up to 18 digits; no division; one scale per comparison).
- **Operators:** comparison, arithmetic, logical and ternary operators, and `in`.
- **Strings:** `startsWith`, `endsWith`, `contains`, `matches` (a literal pattern), `size`.
- **Conversions:** `int`, `uint`, `double`, `string`, `timestamp`, `duration`.
- **Timestamps:** `getFullYear`, `getMonth`, `getDayOfMonth`, `getDayOfWeek`, `getDayOfYear`,
  `getHours`, `getMinutes`, `getSeconds`.
- **Presence:** `has(row.col)`.
- **Macros over lists:** `exists`, `all`, `filter`, `map`.
- **Built-in functions:** `hash_sha256`, `redact` (a fixed `***`: the length is not revealed),
  `partial(value, n)` (`***` and the last `n` characters; fully masked when the value has `n`
  characters or fewer), `is_msisdn` (Kenyan mobile numbers, 254 then 7 or 1), `is_email`.
- **Tenant functions:** any the contract's owner registered; see {doc}`functions`.

## Nulls

A null in a row field a rule reads makes an `admit` or `assert` false and a `transform` null;
`has()` is how a rule tests presence. `null` may be written as one branch of `?:`
(`ctx.clearance > 2 ? row.salary : null`) and takes the other branch's type.

## Types in `expose`

`bool`, `int8` to `int64`, `uint8` to `uint64`, `float32`, `float64` (or `double`), `utf8` (or
`string`), `large_utf8`, `binary` (or `bytes`), `timestamp` (microseconds, UTC) or
`timestamp[s|ms|us|ns]`, `duration` or `duration[unit]`, `decimal(p,s)`, and `list<type>`. A transformed column may change type when `expose` declares the new one:
`{name: amount, type: utf8}` with `hash_sha256(string(row.amount))`.

## Inheritance

A child names its parent with `inherits`. It may narrow `expose`, add rules, and compose
transforms (the parent's applies first); it can never remove a parent's rule or reveal a column
the parent hid. The flattened result is what gets compiled.
