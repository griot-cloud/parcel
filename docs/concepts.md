# How rules execute

## Three namespaces

A rule reads from up to three namespaces, and which ones decide how it runs.

```{list-table}
:header-rows: 1
:class: namespaces

* - Namespace
  - Holds
  - A rule that reads only this costs
* - `ctx`
  - the caller: id, tenant, purpose, tier, clearance, classification, roles, now, `other`
  - nothing: evaluated once per query, before any file is opened
* - `dataset`
  - statistics stored when the data was written
  - one manifest read
* - `row`
  - one record
  - a filter pushed into the scan, or a flag computed when the data is written
```

A rule that mixes `row` and `ctx` (`row.tenant_id == ctx.tenant`) has its `ctx` parts turned
into parameters, bound per query and folded away by the optimiser. A caller for whom a rule
cannot matter pays nothing for it.

## Seven operations

| Operation | Reads | Becomes |
| --- | --- | --- |
| `decide` | `ctx` | A yes or no per caller, before any data is read. |
| `expose` | (the schema) | The columns the contract offers; nothing else exists for a caller. |
| `admit` | `row`, `ctx` | A filter in the view. |
| `assert` | `row` | A flag computed at write; failing rows are dropped (`drop`), counted (`report`), or make the data unservable (`deny`). |
| `transform` | `row`, `ctx` | The column's projection in the view. |
| `guarantee` | `dataset`, `ctx.now` | A check against the manifest per query: refuse (`deny`) or note (`annotate`). |
| `shape` | `ctx` (in `unless`) | `sample` and row `noise` in the view; `suppress` and aggregate `noise` on the query. |

parcel refuses a rule whose namespaces do not fit its operation: an `assert` that reads `ctx`
fails with "assertions describe data, not callers; use admit".

## Three artifacts

Compiling produces three artifacts that share one hash:

- **CompiledContract**: what an engine splices into a caller's query. Admits, flags, the
  projection, parameters, shapes, and variants that read precomputed columns.
- **ValidationPlan**: one aggregate query over the data that returns a single verdict row.
  Every engine that runs it over the same data gets the same verdict, which is what a
  certificate signs.
- **WritePlan**: what must be on disk for every rule to be cheap: flag columns, precomputed
  subtrees, clustering, partitioning, bloom filters, and the `row.other` enrichers.

`parcel compile` prints the report of how each rule executes; `--json` prints the artifacts.
