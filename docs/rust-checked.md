# Checked contract types

`check_contract` returns `parcel_core::check::CheckedContract`. These types contain checked definitions; executable plans are created by compilation.

## `CheckedContract`

| Field | Type | Meaning |
| --- | --- | --- |
| `name` | `String` | Contract name. |
| `version` | `u32` | Version number. |
| `contract_hash` | `String` | Canonical document hash. |
| `owner` | `Option<String>` | Resolved owner. |
| `binding` | `Binding` | Resolved data location. |
| `residency` | `crate::Residency` | Resolved placement requirements. |
| `exposed` | `Vec<ExposedColumn>` | Checked output columns. |
| `rules` | `Vec<CheckedRule>` | Typed rule definitions. |
| `functions` | `BTreeSet<FunctionPin>` | Required function identities. |
| `other` | `OtherTypes` | Declared extension types. |
| `enrichers` | `Vec<(String, CheckedExpr)>` | Custom row field names paired with checked calculations. |
| `producers` | `Vec<Producer>` | Custom dataset calculations. |

## `CheckedExpr` and `ExposedColumn`

| Type | Public fields |
| --- | --- |
| `CheckedExpr` | `source: String` contains the original expression; `expr: TExpr` is its typed expression tree. |
| `ExposedColumn` | `name: String`, `type_name: String` as written in the contract, and `data_type: DataType` as an Arrow type. |
| `OtherTypes` (in `parcel_core::checker`) | `row`, `ctx`, and `dataset`: each `BTreeMap<String, Type>` containing declared custom fields. |

## `CheckedRule`

```rust
pub enum CheckedRule {
    Decide { id: String, expr: CheckedExpr },
    Admit { id: String, expr: CheckedExpr },
    Assert { id: String, expr: CheckedExpr, on_fail: AssertOnFail },
    Transform { id: String, column: String, expr: CheckedExpr },
    Guarantee { id: String, expr: CheckedExpr, on_fail: GuaranteeOnFail },
    Shape { id: String, shape: ShapeOp, unless: Option<CheckedExpr> },
}
```

`id` identifies the rule, `expr` contains its checked condition or calculation, `column` identifies a transformed output, and `on_fail` records the failure action. `shape` contains the checked shape parameters; `unless` can exempt a caller.

```rust
impl CheckedRule {
    pub fn id(&self) -> &str;
    pub fn exprs(&self) -> Vec<&CheckedExpr>;
}
```

`id` borrows the identifier. `exprs` borrows the rule's expressions; for a shape it returns its `unless` expression if present.

## Dataset producers

`Producer` has public fields `field: String`, `ty: Type`, and `kind: ProducerKind`.

`ProducerKind` is `Constant(parcel_core::ir::Lit)` or `Aggregate { func: Aggregate, arg: Option<CheckedExpr> }`. `Aggregate` variants are `Count`, `Sum`, `Avg`, `Min`, `Max`, `CountDistinct`, and `Median`.

## `Residency`

`parcel_core::Residency` contains `jurisdictions: BTreeSet<String>` and `approved_output_may_leave: bool`.

| Method | Meaning |
| --- | --- |
| `permits(&self, jurisdiction: &str) -> bool` | Tests whether a recognized jurisdiction code is in the allowed set. |
| `validate(&self) -> Result<(), String>` | Rejects an empty set or unknown codes. |

The accepted codes are the implementation's ISO two-letter country codes and `EU`. `default()` creates an empty set and sets output release to false.
