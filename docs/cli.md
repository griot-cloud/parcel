# The command line

```text
parcel compile CONTRACT --schema SAMPLE [-o BUNDLE] [--json] [--sql DIALECT --table NAME] [--substrait FILE]
parcel check   CONTRACT --data SAMPLE [--caller FILE]... [--sample N] [--json]
parcel schema
parcel import odcs FILE [--object NAME] [-o CONTRACT]
parcel function verify MODULE --manifest FILE --owner TENANT
```

| Option | For |
| --- | --- |
| `--type COLUMN=TYPE` | CSV samples: override an inferred type, e.g. `--type msisdn=utf8`. |
| `--function MODULE=MANIFEST` | `compile` and `check`: load one of the contract owner's WebAssembly functions. Repeatable. |
| `--caller FILE` | `check`: a caller to evaluate `decide`, `admit` and `transform` for (YAML: `id`, `tenant`, `purpose`, `tier`, `clearance`, `classification`, `roles`, `now`). Repeatable. |

`compile` prints how each rule will execute; `-o` writes the bundle an engine loads. `check`
exits non-zero when the verdict is invalid or the differential test finds a disagreement, which
makes it a CI gate. Parents named by `inherits` are found among the documents next to the
contract.
