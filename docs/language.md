# Contract language

Contracts are YAML or JSON documents. Run `parcel schema` for the JSON Schema, or add this line to YAML files for editors that support it:

```yaml
# yaml-language-server: $schema=https://raw.githubusercontent.com/griot-cloud/parcel/main/schema/contract.schema.json
```

## Document fields

| Field | Required | Meaning |
| --- | --- | --- |
| `contract` | Yes | Contract name, such as `purchasing/orders`. |
| `version` | Yes | Unsigned integer version. |
| `owner` | No | Tenant whose registered functions the rules may use. |
| `inherits` | No | Parent contract name. See {doc}`authoring`. |
| `binding` | Unless inherited | `{parquet: path, partitioned_by: [columns]}`. |
| `expose` | Unless inherited | List of `{name, type}` columns available to queries. |
| `rules` | No | List of the operations below. |
| `extensions` | No | Typed custom fields under `row`, `ctx` and `dataset`. |
| `enrich` | No | List of `{field, expr}` producers for `row.other` fields. |
| `dataset_other` | No | List of `{field, value}` or `{field, expr}` producers for `dataset.other`. |

`expose` is a document field, not an `op` in the rules list. A transformed column may have a different type from its source when `expose` declares the output type.

## Rule operations

Each rule requires a unique `id` made of lowercase letters, digits and underscores, and an `op`.

| `op` | Allowed inputs | Required fields | Meaning |
| --- | --- | --- | --- |
| `decide` | `ctx` | `expr` | Refuse the caller when false. |
| `admit` | `row`, `ctx` | `expr` | Include rows for which the condition is true. |
| `assert` | `row` | `expr`, `on_fail` | Check row quality; fail with `drop`, `deny` or `report`. |
| `transform` | `row`, `ctx` | `column`, `expr` | Replace an exposed column's value. |
| `guarantee` | `dataset`, `ctx.now` | `expr`, `on_fail` | Check a dataset requirement; fail with `deny` or `annotate`. |
| `shape` | `ctx` in `unless` | `operator`, `params` | Sample rows, suppress small groups or add noise. |

Boolean operations require a true-or-false expression. A transform's result must match the exposed column's type.

These independent examples assume the referenced fields exist in the schema:

```yaml
rules:
  - {id: purchasing_only, op: decide, expr: "ctx.purpose == 'purchasing'"}
  - {id: own_orders, op: admit, expr: "row.supplier_id == ctx.tenant"}
  - {id: positive_amount, op: assert, expr: "row.amount > 0", on_fail: drop}
  - {id: mask_email, op: transform, column: email, expr: "redact(row.email)"}
  - {id: nonempty, op: guarantee, expr: "dataset.row_count > 0", on_fail: deny}
```

### Shapes

| Operator | Parameters | Applies to |
| --- | --- | --- |
| `suppress` | Integer `k >= 1`. | Groups with fewer than `k` contributing rows. |
| `sample` | `fraction` greater than 0 and at most 1; exposed `key` column. | A stable sample chosen from the key. |
| `noise` | Positive `sensitivity` and `epsilon`; a named `budget`; optional `at`. | Numeric exposed `column`; `at: aggregate` by default, or `at: row`. |

```yaml
- id: small_groups
  op: shape
  operator: suppress
  params: {k: 5}
  unless: "ctx.tenant == 'acme'"
```

`unless` skips the shape when its caller-only condition is true. Aggregate noise restricts how the protected column can be queried. The serving engine must apply the shape and account for budget charges; exporting a validation plan does not export these query policies.

## Namespaces

| Namespace | Available fields |
| --- | --- |
| `ctx` | Strings `id`, `tenant`, `purpose`, `tier`, `classification`; integer `clearance`; list `roles`; timestamp `now`; declared `other.<field>`. |
| `row` | Source columns and declared `other.<field>` values. |
| `dataset` | `row_count`, `written_at`, `contract_hash`; column statistics; `assertions.<id>.pass_rate`; declared `other.<field>`. |

Column statistics are `<column>.null_count`, `null_rate`, `distinct_count`, `min` and `max`. `ctx.now` is fixed for a query. Declare custom field types in `extensions`; use `enrich` for derived row fields and `dataset_other` for constants or dataset aggregates. The application supplies `ctx.other` values.

```yaml
extensions:
  row: {amount_major: double}
enrich:
  - {field: amount_major, expr: "double(row.amount) / 100.0"}
```

Rules can then read `row.other.amount_major`.

## Expressions

parcel supports a checked subset of CEL. Unsupported syntax, types and operations are rejected during compilation.

| Category | Supported forms |
| --- | --- |
| Values | Boolean, integer, unsigned integer, double, string, bytes, timestamp, duration, lists and exact decimals. |
| Operators | Comparisons, arithmetic, `&&`, `||`, `!`, conditional `? :`, and membership `in`. |
| Strings | `startsWith`, `endsWith`, `contains`, `matches` with a literal pattern, `size`. |
| Conversions | `int`, `uint`, `double`, `string`, `timestamp`, `duration`. |
| Timestamps | `getFullYear`, `getMonth`, `getDayOfMonth`, `getDayOfWeek`, `getDayOfYear`, `getHours`, `getMinutes`, `getSeconds`. |
| Presence | `has(row.column)`. |
| Lists | `exists`, `all`, `filter`, `map`. |

Exact decimals support up to 18 digits, no division, and one scale per comparison.

### Built-in functions

| Function | Result |
| --- | --- |
| `hash_sha256(value)` | SHA-256 hash of a string. |
| `redact(value)` | Fixed `***` string. |
| `partial(value, n)` | `***` followed by the final `n` characters; fully masked when the value is no longer than `n`. |
| `is_msisdn(value)` | Checks the supported Kenyan mobile-number format. |
| `is_email(value)` | Checks email format. |

Contracts may also call their owner's registered {doc}`functions`.

## Nulls

A null in a row field read by value makes an `admit` or `assert` false and a `transform` null. Use `has(row.column)` to test presence. A literal `null` is supported as a branch of a conditional, taking the other branch's type:

```text
ctx.clearance > 2 ? row.salary : null
```

## Column types

`expose` and extension declarations accept `bool`, `int8` through `int64`, `uint8` through `uint64`, `float32`, `float64` (or `double`), `utf8` (or `string`), `large_utf8`, `binary` (or `bytes`), `timestamp`, `duration`, `decimal(p,s)` and `list<type>`.

`timestamp` defaults to microseconds in UTC; explicit units use `timestamp[s]`, `[ms]`, `[us]` or `[ns]`. Duration units use the same bracket notation.
