# Import from ODCS

The Open Data Contract Standard (ODCS, v3) is where many catalogues keep contracts. parcel
imports them:

```bash
parcel import odcs contract.odcs.yaml -o contract.yaml
```

Schema properties become `expose` entries. `required` (and `primaryKey`) becomes an assertion
that the column is present, failing with `deny`; `unique` becomes a guarantee that the distinct
count equals the non-null count, with `annotate`. Quality rules with `engine: parcel` come across
verbatim. Anything parcel cannot carry over
exactly is listed as a note rather than dropped silently. Review the binding, then check the
result against data with `parcel check`.
