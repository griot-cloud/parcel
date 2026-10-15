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
    /// Invalid placement terms.
    Residency,
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
    /// What to do about it, when parcel can say.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,
    /// The 1-based line of the document it is about, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,
}

impl Diagnostic {
    pub fn new(code: Code, rule: Option<&str>, message: impl Into<String>) -> Self {
        Diagnostic {
            code,
            rule: rule.map(str::to_owned),
            message: message.into(),
            help: None,
            line: None,
        }
    }

    pub fn with_help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }

    pub fn at_line(mut self, line: usize) -> Self {
        self.line = Some(line);
        self
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.rule {
            Some(r) => write!(f, "[{:?}] rule '{r}': {}", self.code, self.message)?,
            None => write!(f, "[{:?}] {}", self.code, self.message)?,
        }
        if let Some(h) = &self.help {
            write!(f, " (help: {h})")?;
        }
        Ok(())
    }
}

impl std::error::Error for Diagnostic {}

/// What each operation may read, as the rules guide puts it. Namespace errors end with it.
pub const NAMESPACES_BY_OP: &str = "decide: ctx · admit: row, ctx · assert: row · transform: row, ctx · \
     guarantee: dataset, ctx.now · unless: ctx";

/// The closest of `candidates` to `word`, if one is close enough to be a likely typo.
pub fn did_you_mean<'a>(
    word: &str,
    candidates: impl IntoIterator<Item = &'a str>,
) -> Option<&'a str> {
    let w = word.to_ascii_lowercase();
    candidates
        .into_iter()
        .map(|c| (edit_distance(&w, &c.to_ascii_lowercase()), c))
        .filter(|(d, c)| *d <= (c.len().max(w.len()) / 3).max(1))
        .min_by_key(|(d, _)| *d)
        .map(|(_, c)| c)
}

fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut cur = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            let sub = prev[j] + usize::from(ca != *cb);
            cur.push(sub.min(prev[j + 1] + 1).min(cur[j] + 1));
        }
        prev = cur;
    }
    prev[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggests_close_names_only() {
        assert_eq!(
            did_you_mean("ammount", ["amount", "order_id"]),
            Some("amount")
        );
        assert_eq!(
            did_you_mean("Tenant", ["tenant", "purpose"]),
            Some("tenant")
        );
        assert_eq!(did_you_mean("zebra", ["amount", "order_id"]), None);
    }
}
