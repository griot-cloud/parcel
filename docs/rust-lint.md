# Lint

`parcel_core::lint` checks a contract from the document alone: no schema, no column types, no functions.

## `lint`

```rust
pub fn lint(doc: &ContractDoc) -> Report;
```

`doc` is the document as written, before inheritance. For a child contract, checks that need the parent's `expose` or `extensions` are left to `check_contract`.

## `Report`

| Field | Type | Meaning |
| --- | --- | --- |
| `problems` | `Vec<Diagnostic>` | What `check_contract` would also refuse: rule ids, type names, expressions that do not parse, an operation reading a namespace it may not, unknown `ctx` fields, undeclared `other` fields, transforms of unexposed columns, shape parameters, residency terms. |
| `lints` | `Vec<Lint>` | Rules that are legal but probably not what their author meant. |

Expressions are read with `parcel_core::syntax::reads(rule, source)`, which returns the namespaces, `ctx` fields, columns and extension fields an expression reads, without types.

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
