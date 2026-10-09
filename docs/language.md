# Contract language

Contracts are YAML or JSON documents. Run `parcel schema` for the JSON Schema, or add this line to YAML files for editors that support it:

```yaml
# yaml-language-server: $schema=https://raw.githubusercontent.com/griot-cloud/parcel/main/schema/contract.schema.json
```

## Contract fields

The field definitions and examples are in [Contract identity](contract-identity.md), [Binding](contract-binding.md), [Expose](contract-expose.md), and [Rules](contract-rules.md).

The remaining sections specify expression syntax, supported types, and additional contract fields.

## Rule constraints

Rule IDs must be unique and match `[a-z][a-z0-9_]*`. Conditions for `decide`, `admit`, `assert`, and `guarantee` must return a Boolean. A `transform` must return the type declared for its column in `expose`.

## Shapes

| Operator | Parameters | Applies to |
| --- | --- | --- |
| `suppress` | Integer `k >= 1`. | Groups with fewer than `k` contributing rows. |
| `sample` | `fraction` greater than 0 and at most 1; exposed `key` column. | A stable sample chosen from the key. |
| `noise` | Positive `sensitivity` and `epsilon`; a named `budget`; optional `at`. | Numeric exposed `column`; `at: aggregate` by default, or `at: row`. |

`unless` is an optional Boolean condition using `ctx`; when true, it skips the shape.

## Namespaces

| Namespace | Available fields |
| --- | --- |
| `ctx` | Strings `id`, `tenant`, `purpose`, `tier`, `classification`; integer `clearance`; list `roles`; timestamp `now`; declared `other.<field>`. |
| `row` | Source columns and declared `other.<field>` values. |
| `dataset` | `row_count`, `written_at`, `contract_hash`; column statistics; `assertions.<id>.pass_rate`; declared `other.<field>`. |

Column statistics are `<column>.null_count`, `null_rate`, `distinct_count`, `min` and `max`. The application supplies `ctx.now` as the time used to evaluate rules. The application supplies `ctx.other` values.

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

Rules may also call {doc}`functions` loaded for the contract's owner.

## Nulls

A null in a row field read by value makes an `admit` or `assert` false and a `transform` null. Use `has(row.column)` to test presence. A literal `null` is supported as a branch of a conditional, taking the other branch's type:

```text
ctx.clearance > 2 ? row.salary : null
```

## Column types

`expose` and extension declarations accept `bool`, `int8` through `int64`, `uint8` through `uint64`, `float32`, `float64` (or `double`), `utf8` (or `string`), `large_utf8`, `binary` (or `bytes`), `timestamp`, `duration`, `decimal(p,s)` and `list<type>`.

`timestamp` defaults to microseconds in UTC; explicit units use `timestamp[s]`, `[ms]`, `[us]` or `[ns]`. Duration units use the same bracket notation.

## Additional fields

| Field | Value |
| --- | --- |
| `extensions` | Typed custom fields grouped under `row`, `ctx`, and `dataset`. |
| `enrich` | List of `{field, expr}` calculations for declared `row.other` fields. |
| `dataset_other` | List of `{field, value}` constants or `{field, expr}` calculations for declared `dataset.other` fields. |

For example, this declares a calculated amount in major currency units:

```yaml
extensions:
  row: {amount_major: double}
enrich:
  - {field: amount_major, expr: "double(row.amount) / 100.0"}
```

Rules can read the result as `row.other.amount_major`.

## Inheritance

A child contract names its parent with `inherits`. The CLI resolves parent contracts by name from YAML and JSON files in the same directory. A child can omit `binding` and `expose` to inherit them:

```yaml
contract: purchasing/large_orders
version: 1
owner: acme
inherits: purchasing/orders
rules:
  - id: large_orders
    op: admit
    expr: "row.amount >= 100"
```

The child's rules accumulate with the parent's rules. Here both the parent's supplier filter and the new amount filter apply. A child may narrow `expose` to a subset of parent columns with the same types; it cannot expose a hidden parent column. A repeated `binding` must match the parent's binding exactly, including its source form and partition columns. A child cannot remove or redefine a parent rule. Parent and child transforms compose in order, with the parent applied first.
