# Command line

Use `parcel --help` or `parcel COMMAND --help` for the options available in your installed version.

## Commands

| Command | Purpose |
| --- | --- |
| `parcel check CONTRACT --data SAMPLE` | Compile, validate sample data, evaluate test callers and compare CEL with DataFusion. |
| `parcel compile CONTRACT --schema SAMPLE` | Compile against the sample's schema and print the execution report. |
| `parcel schema` | Print the contract document's JSON Schema. |
| `parcel import odcs FILE` | Convert an ODCS v3 document. |
| `parcel function verify MODULE --manifest FILE --owner TENANT` | Verify a WebAssembly function and print its pin hash. |

Samples can be CSV, Parquet files or directories of Parquet files. Compilation uses the schema; checking evaluates data values as well.

## Shared options for check and compile

| Option | Meaning |
| --- | --- |
| `--type COLUMN=TYPE` | Override CSV type inference. Repeat for multiple columns. |
| `--function MODULE=MANIFEST` | Load a WebAssembly function for the contract's owner. Repeatable. |
| `--json` | Print the check result or compiled artifacts as JSON. |

## Check options

| Option | Meaning |
| --- | --- |
| `--caller FILE` | YAML or JSON caller profile. Repeat for different callers. |
| `--sample N` | Maximum rows in the differential comparison; default 1,000. Validation still uses all supplied data. |

Caller profiles require `id`, `tenant` and `purpose`. Optional fields are `tier`, `clearance`, `classification`, `roles`, `now` and `other`. Without a profile, the default caller has ID `check`, an empty tenant and purpose `analytics`.

The JSON result includes `report`, `verdict`, `query_time_guarantees`, `callers` and `differential`. A caller refusal appears under `refused_by`; it does not by itself make the command fail.

## Compile options

| Option | Meaning |
| --- | --- |
| `-o FILE`, `--out FILE` | Write a compiled bundle. |
| `--sql DIALECT` | Print the validation query as SQL. |
| `--table NAME` | Table name used in an export; default `contract_data`. |
| `--substrait FILE` | Write a Substrait validation plan when supported by the build. |

For other engines, see {doc}`exports`. For ODCS selection and output options, see {doc}`odcs`.

## Exit status and errors

- **0:** the command completed; for `check`, the verdict is valid and the differential comparison passed.
- **1:** compilation diagnostics, an invalid verdict or differential mismatches.
- **2:** a command, input, parsing or runtime error.

Compiler diagnostics identify the affected rule where possible. Common fixes:

| Diagnostic | Check |
| --- | --- |
| `UnknownColumn` or `UnknownField` | Column spelling, schema and extension declarations. |
| `TypeMismatch` or `ExposeTypeMismatch` | Source and output types; CSV inference may need `--type`. |
| `Namespace` | Whether that operation may read `row`, `ctx` or `dataset`. |
| `OutsideProfile` or `Untranslatable` | Whether the expression uses supported CEL forms. |
| `Inheritance` | Parent location, rule IDs and whether the child widens access. |

A successful compile does not mean that data passes validation. Run `check` against representative data before handing the contract to an engine.
