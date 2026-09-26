# How it works

parcel checks a contract against an Arrow schema, then compiles its rules into DataFusion expressions and plans. A query engine uses those results to validate data and apply policies.

## From document to compiled contract

The compiler resolves inherited contracts, checks field names and expression types, and verifies that each operation reads only allowed namespaces. It rejects expressions it cannot translate into the supported execution model.

The compile report describes where each rule can run:

| Report tier | Meaning |
| --- | --- |
| `plan time` | Evaluate a caller-only condition once per query. |
| `prunes` | A predicate that may let the engine skip files or row groups using statistics. |
| `scan filter` | Evaluate a condition while reading rows. |
| `write time` | Compute a value or assertion flag that an engine can store for reuse. |
| `projection` | Compute an output column when it is needed. |
| `operator` | Apply a result operation such as suppression. |

These are execution choices, not performance guarantees. File layout, available statistics and the consuming engine determine which optimisations are used.

## Three compilation outputs

| Output | What an engine uses it for |
| --- | --- |
| `CompiledContract` | Caller decisions, row filters, column projections, parameters, guarantees and shape rules. |
| `ValidationPlan` | An aggregate query that measures assertion failures and statistics and checks data-only guarantees. |
| `WritePlan` | Instructions for enrichments, stored flags and reusable calculations, plus layout hints. |

A write plan describes work for an engine to perform; compiling does not write data. A validation plan checks the dataset, while the compiled contract supplies the rules used when serving queries.

Caller-dependent expressions use parameters. The engine binds those parameters for each query. Where available, an engine can use stored assertion flags and derived values instead of recomputing them.

## Bundles and verification

`parcel compile -o orders.parcel.json` packages the document, ancestor contracts, source schema, pinned custom functions and compiled outputs. A compatible runtime verifies a bundle by recompiling it and comparing its hashes and executable artifacts.

A compilation hash identifies the compiled result and depends on the compiler version and pinned functions. Keep the producing compiler and consuming runtime compatible; a bundle is not a promise that any Arrow engine or compiler version can execute it.

## The engine boundary

parcel supplies compilation and execution libraries. The engine supplies authenticated callers, data storage, query handling and the state needed for guarantees and privacy budgets.

```{toctree}
:maxdepth: 1

parcel-and-peql
```
