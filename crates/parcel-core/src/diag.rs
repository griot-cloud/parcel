//! Diagnostics: every reason parcel refuses a contract.

use std::fmt;

use serde::Serialize;

/// Stable, machine-readable error codes. Tests and tooling match on these;
/// messages are for people and may change.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Code {
    /// The document is not valid YAML/JSON or does not match the contract shape.
    Document,
    /// A feature the design defines but parcel v0 does not implement yet.
    Unsupported,
    InvalidRuleId,
    DuplicateRuleId,
    DuplicateExpose,
    UnknownColumn,
    BadType,
    ExposeTypeMismatch,
    /// The CEL source failed to parse.
    Parse,
    /// Valid CEL that is outside the parcel profile (design section 6).
    OutsideProfile,
    UnknownIdentifier,
    UnknownField,
    /// A column exists but its type cannot be read by rules.
    UnreadableColumn,
    UnknownFunction,
    NoMatchingOverload,
    TypeMismatch,
    /// A rule reads a namespace its operation does not allow (the classify pass).
    Namespace,
    NonDeterministic,
    TransformTarget,
    DuplicateTransform,
    UnknownShapeOperator,
    /// A child contract that would widen its parent, or an inheritance chain that cannot resolve.
    Inheritance,
    /// Checked, but the translator has no DataFusion equivalent.
    Untranslatable,
    ShapeParams,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Diagnostic {
    pub code: Code,
    /// The rule the diagnostic is about, if any.
    pub rule: Option<String>,
    pub message: String,
}

impl Diagnostic {
    pub fn new(code: Code, rule: Option<&str>, message: impl Into<String>) -> Self {
        Diagnostic {
            code,
            rule: rule.map(str::to_owned),
            message: message.into(),
        }
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.rule {
            Some(r) => write!(f, "[{:?}] rule '{r}': {}", self.code, self.message),
            None => write!(f, "[{:?}] {}", self.code, self.message),
        }
    }
}

impl std::error::Error for Diagnostic {}
