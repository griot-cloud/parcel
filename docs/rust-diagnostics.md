# Compiler diagnostics

`parcel_core::Diagnostic` and `parcel_core::Code` describe parse, check, and compilation failures.

## `Diagnostic`

| Field | Type | Meaning |
| --- | --- | --- |
| `code` | `Code` | Machine-readable error category. |
| `rule` | `Option<String>` | Affected rule ID, when available. |
| `message` | `String` | Human-readable explanation. |

```rust
impl Diagnostic {
    pub fn new(code: Code, rule: Option<&str>, message: impl Into<String>) -> Self;
}
```

The constructor copies the optional rule ID and takes the message. `Diagnostic` implements `Display` and `std::error::Error`. Match `code` in tooling; message wording can change.

## `Code` variants

| Category | Variants |
| --- | --- |
| Document and placement | `Document`, `Residency`, `Unsupported`, `Inheritance` |
| Rule identifiers | `InvalidRuleId`, `DuplicateRuleId` |
| Columns and types | `DuplicateExpose`, `UnknownColumn`, `BadType`, `ExposeTypeMismatch`, `UnreadableColumn`, `TypeMismatch` |
| Expressions | `Parse`, `OutsideProfile`, `UnknownIdentifier`, `UnknownField`, `Namespace`, `NonDeterministic`, `Untranslatable` |
| Functions | `UnknownFunction`, `NoMatchingOverload` |
| Transformations | `TransformTarget`, `DuplicateTransform` |
| Shapes | `UnknownShapeOperator`, `ShapeParams` |

Parsing returns one diagnostic. Checking and compilation return a vector so multiple problems can be reported together.
