# Import ODCS contracts

parcel can convert an Open Data Contract Standard (ODCS v3) document into a parcel contract:

```bash
parcel import odcs contract.odcs.yaml -o contract.yaml
```

If the document has multiple schema objects, choose one with `--object NAME`. Without `-o`, the result is printed to standard output.

| ODCS field | parcel result |
| --- | --- |
| Schema properties | Exposed columns and types. |
| `required` or `primaryKey` | Presence assertions with `on_fail: deny`. |
| `unique` | A distinct-count guarantee with `on_fail: annotate`. |
| Quality rules with `engine: parcel` | parcel rule definitions. |

The importer prints notes for content it cannot preserve. Read those notes and review the generated binding, types and failure actions before use. Then check the result against representative data:

```bash
parcel check contract.yaml --data sample.csv
```

Imported metadata alone does not verify the data or enforce access. Add the policies you need and use a serving engine to apply them.
