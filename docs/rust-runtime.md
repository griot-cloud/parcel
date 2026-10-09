# Runtime: `parcel-runtime`

The runtime evaluates compiled contracts against Arrow data and caller context. Use the pages below for public fields, method signatures, parameters, return values, and errors.

| API | Main types and functions |
| --- | --- |
| [Caller context](rust-caller.md) | `Caller`, `refusal`, `param_values`, `active_shapes`. |
| [Data validation](rust-validation.md) | `validate`, `validate_in`, `Verdict`, `selectivity`, `RuntimeError`. |
| [Result shapes](rust-shapes.md) | `apply`, `Shaped`, `Charge`, suppression helpers. |
| [Comparing rule evaluators](rust-differential.md) | `differential`, `DiffReport`, `Mismatch`. |
| [CEL evaluation](rust-evaluation.md) | `Scope`, `context`, `eval`, value conversion. |

## Validation

For validation signatures and returned fields, see {doc}`rust-validation`. Loading and exporting compiled contracts are covered in {doc}`rust-artifacts`.

The default `wasm` feature enables WebAssembly functions. The optional `substrait` feature enables Substrait export.

```{toctree}
:hidden:

rust-caller
rust-validation
rust-shapes
rust-differential
rust-evaluation
```
