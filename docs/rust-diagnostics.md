# Compiler diagnostics

`parcel_core::Diagnostic` and `parcel_core::Code` describe parse, check, and compilation failures.

## `Diagnostic`

| Field | Type | Meaning |
| --- | --- | --- |
| `code` | `Code` | Machine-readable error category. |
| `rule` | `Option<String>` | Affected rule ID, when available. |
| `message` | `String` | Human-readable explanation. |
| `help` | `Option<String>` | What to change, when parcel can say: a suggested name, a type, or the operation that fits. |
| `line` | `Option<usize>` | The 1-based line of the document, when known. |

```rust
impl Diagnostic {
    pub fn new(code: Code, rule: Option<&str>, message: impl Into<String>) -> Self;
    pub fn with_help(self, help: impl Into<String>) -> Self;
    pub fn at_line(self, line: usize) -> Self;
}
```

The constructor copies the optional rule ID and takes the message. `Diagnostic` implements `Display` and `std::error::Error`. Match `code` in tooling; message and help wording can change. In JSON, `help` and `line` are left out when absent.

`parcel_core::diag::did_you_mean(word, candidates)` returns the candidate closest to a likely typo, if one is close enough.

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

`ContractDoc::parse` returns one diagnostic; `ContractDoc::parse_all`, checking and compilation return a vector so multiple problems can be reported together.
