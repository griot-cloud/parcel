# Command line

The parcel CLI lints and validates contracts, checks data against them, compiles them, imports ODCS definitions, and verifies custom functions.

```text
parcel COMMAND [OPTIONS]
```

## Lint and validate

| Command | Reads | Answers |
| --- | --- | --- |
| `parcel lint CONTRACT` | the contract file only | Is it written well? |
| `parcel validate CONTRACT` | the file, then the data's column types | Is it written well, and does it compile against these columns? |
| `parcel check CONTRACT --data FILE` | the file, column types, rows and callers | What does it do to this data? See {doc}`checking`. |

### Lint

`lint` needs no data, no column types and no functions. It reports:

- **Problems**, which fail the command: YAML and field mistakes, rule ids, type names in `expose` and `extensions`, expressions that do not parse, an operation reading a namespace it may not, unknown `ctx` fields, undeclared `other` fields, and transforms of columns not in `expose`.
- **Warnings and notes**, which do not: rules that are legal but probably not what you meant.

```bash
parcel lint orders.yaml
```

```text
✓ orders.yaml  0 warnings  0 notes
```

| Lint | Points out |
| --- | --- |
| `admit-reads-only-ctx` | An `admit` that reads only `ctx` keeps every row or none. `decide` refuses the caller by name instead. |
| `unsatisfiable-callers` | Two rules that require different values of one `ctx` field, so no caller passes both. |
| `constant-rule` | A condition that reads nothing and always gives the same answer. |
| `duplicate-condition` | Two rules of one operation that check the same condition. |
| `unused-extension` | A declared `extensions` field that no rule reads. |
| `open-access`, `no-owner`, `contract-name` | Notes: no access rules, no `owner`, a name without an `<area>/` prefix. |

| Option | Meaning |
| --- | --- |
| `--deny-warnings` | Exit with status 1 on warnings, for CI. |
| `--json` | Print `diagnostics` and `lints` as JSON. |

### Validate

`validate` lints first. If the file has problems, it stops there. Otherwise it type-checks and compiles every rule against the data's column types: columns exist, types line up, functions are loaded. It reads no rows.

```bash
parcel validate orders.yaml --schema orders.csv
```

```text
✓ purchasing/orders v1.0.0
  rules     2  1 admit, 1 assert
  columns   2/3
  types     orders.csv
  lint      0 warnings  0 notes
```

`columns` is exposed columns out of the data's columns. Lint warnings and notes print above, and do not fail `validate`.

When a contract has problems, each one shows where it is and what to change:

```text
error[Document] `positive_amount` (assert) needs `on_fail`
  --> orders.yaml:14
   │
14 │   - id: positive_amount
   │
   = help: on_fail: drop (exclude row) | deny (invalidate dataset) | report (count only)

✗ orders.yaml  1 problem  0 warnings  0 notes
```

### Where column types come from

`validate` and `compile` take column names and types from the first of:

1. `--schema FILE`: a CSV file, Parquet file or directory of Parquet files.
2. The contract's `binding`, when it names Parquet data on this machine. A relative path is read from the contract's directory.
3. The types the contract declares in `expose`.

The `types` line says which was used. Without data, a rule that reads a column missing from `expose`, such as `supplier_id` in `orders.yaml`, cannot be checked: parcel names the column and asks for `--schema` or an `expose` entry.

| Option | Meaning |
| --- | --- |
| `--schema FILE` | Column names and types to check against. |
| `--type COLUMN=TYPE` | Read a CSV column as this type, e.g. `--type postal_code=utf8`. Repeatable. |
| `--function MODULE=MANIFEST` | Load a custom function. Repeatable. |
| `--json` | Print `diagnostics` and `lints` as JSON. |

## Check and compile

| Command | Result | Guide and options |
| --- | --- | --- |
| `parcel check CONTRACT --data DATA` | Quality verdict, caller access results, and a comparison of rule evaluators. | [Checking contracts](checking.md) |
| `parcel compile CONTRACT` | Compilation report, with options to save the contract or export validation. | [Compiling contracts](compiling.md) |
| `parcel compile CONTRACT --target ENGINE` | The contract as a query DuckDB or Spark resolves by name. | [Engines](engines.md) |

For example, from `examples/suppliers` in the repository:

```bash
parcel check orders.yaml --data orders.csv --caller globex.yaml
parcel compile orders.yaml --schema orders.csv --out orders.parcel
```

The first command evaluates the orders for the Globex caller. The second writes `orders.parcel` for an application to load. Each guide describes its command's inputs, complete options, and output.

## Generate the contract schema

```bash
parcel schema > contract.schema.json
```

This writes the JSON Schema for parcel contract documents. Editors and other tools can use it to check document structure. It describes the contract format, rather than the columns in a dataset.

## Import an ODCS contract

```bash
parcel import odcs contract.odcs.yaml --out contract.yaml
```

This converts an ODCS v3 document to parcel YAML. Without `--out`, the YAML is printed to the terminal.

| Argument or option | Meaning |
| --- | --- |
| `FILE` | Required ODCS document path. |
| `--object NAME` | Select a schema object when the document contains several. |
| `-o FILE`, `--out FILE` | Save the converted contract to a file. |

For field mappings and an example of checking the imported result, see {doc}`odcs`.

## Verify a custom function

```text
parcel function verify MODULE --manifest MANIFEST --owner OWNER
```

This loads a WebAssembly module, verifies its interface, and runs a small test batch. On success it prints the function's pin hash, which identifies the verified implementation.

| Argument or option | Meaning |
| --- | --- |
| `MODULE` | Required path to the `.wasm` file. |
| `--manifest FILE` | Required YAML manifest defining the function and its signatures. |
| `--owner OWNER` | Required owner identifier under which the function is loaded. |

For a complete module, manifest, and contract example, see {doc}`functions`.

## Help and version

```bash
parcel --help
parcel check --help
parcel compile --help
parcel --version
```

Every command accepts `-h` or `--help`. The installed binary's help lists the options available in that build, including optional features such as Substrait export.

## Exit status

| Status | Meaning |
| --- | --- |
| `0` | Success. For `check`, the dataset is valid and the evaluators agree. |
| `1` | Problems in the contract, an invalid dataset, differing evaluator results, or lint warnings with `--deny-warnings`. |
| `2` | A command, input, parsing, or execution error. |

Problems name the affected rule and line where possible, and most end with a `help:` line saying what to change. Common codes:

| Diagnostic | What to check |
| --- | --- |
| `UnknownColumn` or `UnknownField` | Column spelling and custom field declarations. |
| `TypeMismatch` or `ExposeTypeMismatch` | Input and output types; CSV inference may need `--type`. |
| `Namespace` | Whether the rule can read the namespace used in its expression: `decide` reads `ctx`, `admit` reads `row` and `ctx`, `assert` reads `row`, `transform` reads `row` and `ctx`, `guarantee` reads `dataset` and `ctx.now`. |
| `OutsideProfile` or `Untranslatable` | Whether the expression uses supported syntax. |
| `Inheritance` | Parent location, rule IDs, and whether the child widens access. |
| `Document` | The YAML itself: spacing, indentation, field names, and required fields such as `on_fail`. |
