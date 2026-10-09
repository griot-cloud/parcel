# Import ODCS contracts

parcel can convert an Open Data Contract Standard (ODCS v3) document into a parcel contract:

```bash
parcel import odcs contract.odcs.yaml -o contract.yaml
```

For documents with multiple schema objects, `--object NAME` selects one. Without `-o`, the result is printed to standard output.

| ODCS field | parcel result |
| --- | --- |
| Schema properties | Exposed columns and types. |
| `required` or `primaryKey` | Presence assertions with `on_fail: deny`. |
| `unique` | A distinct-count guarantee with `on_fail: annotate`. |
| Quality rules with `engine: parcel` | parcel rule definitions. |

The importer prints notes for content it cannot preserve, including `servers`. The generated binding is a Parquet placeholder, `{parquet: data/<object>/}`. It must be replaced with the data's Parquet location or Iceberg table (`{iceberg: namespace.table}`). The binding, types, and failure actions require review before use. The imported contract can be checked against representative data:

```bash
parcel check contract.yaml --data sample.csv
```

The import converts contract definitions. The `check` command evaluates those definitions against the sample data.
