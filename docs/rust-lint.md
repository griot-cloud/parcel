# Lints

`parcel_core::lint` points at contracts that compile but probably do not do what their author meant. A lint never refuses a contract.

## `lint`

```rust
pub fn lint(doc: &ContractDoc, checked: &CheckedContract) -> Vec<Lint>;
```

`doc` is the document as written, before inheritance; `checked` is the result of `check_contract` (or `check::check_layers` for an inheriting contract).

## `Lint`

| Field | Type | Meaning |
| --- | --- | --- |
| `name` | `&'static str` | Stable name, such as `admit-reads-only-ctx`. |
| `severity` | `Severity` | `Warning` for a likely mistake, `Note` for a suggestion. |
| `rule` | `Option<String>` | Affected rule ID, when there is one. |
| `message` | `String` | What looks wrong. |
| `help` | `String` | What to write instead. |
| `line` | `Option<usize>` | The 1-based line of the document, when known. `lint` leaves it empty; `parcel_core::structure::Lines` finds rules and columns in the source. |

## Lints

| Name | Severity | When |
| --- | --- | --- |
| `admit-reads-only-ctx` | Warning | An `admit` reads no row, so every caller sees all rows or none. A `decide` rule refuses by name instead. |
| `constant-rule` | Warning | A condition reads no namespace, so it always gives the same answer. |
| `duplicate-condition` | Warning | Two rules of one operation check the same condition. |
| `unsatisfiable-callers` | Warning | Two caller conditions require different values of one `ctx` field, so no caller passes both. |
| `unused-extension` | Warning | A declared `extensions` field is never read. |
| `open-access` | Note | No `decide` or `admit` rule: every caller may read every row. |
| `no-owner` | Note | The contract names no `owner`. |
| `contract-name` | Note | The name has no `<area>/` prefix. |

The command line adds `binding-not-found`, a note, when a local Parquet binding does not exist.
