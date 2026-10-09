# Command line

The parcel CLI checks data against contracts, compiles contracts, imports ODCS definitions, and verifies custom functions.

```text
parcel COMMAND [OPTIONS]
```

## Check and compile

| Command | Result | Guide and options |
| --- | --- | --- |
| `parcel check CONTRACT --data DATA` | Quality verdict, caller access results, and a comparison of rule evaluators. | [Checking contracts](checking.md) |
| `parcel compile CONTRACT --schema SCHEMA` | Compilation report, with options to save the contract or export validation. | [Compiling contracts](compiling.md) |

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
| `1` | Compilation diagnostics, an invalid dataset, or differing evaluator results. |
| `2` | A command, input, parsing, or execution error. |

Compiler diagnostics identify the affected rule where possible:

| Diagnostic | What to check |
| --- | --- |
| `UnknownColumn` or `UnknownField` | Column spelling and custom field declarations. |
| `TypeMismatch` or `ExposeTypeMismatch` | Input and output types; CSV inference may need `--type`. |
| `Namespace` | Whether the rule can read the namespace used in its expression. |
| `OutsideProfile` or `Untranslatable` | Whether the expression uses supported syntax. |
| `Inheritance` | Parent location, rule IDs, and whether the child widens access. |
