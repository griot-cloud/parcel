use parcel_core::Diagnostic;

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("contract does not compile:\n{}", .0.iter().map(|d| format!("  {d}")).collect::<Vec<_>>().join("\n"))]
    Compile(Vec<Diagnostic>),
    #[error("no contract named `{0}`")]
    UnknownContract(String),
    #[error("denied by `{contract}` rule `{rule}`")]
    Denied { contract: String, rule: String },
    #[error("`{contract}` has no data yet; write to it first")]
    NotWritten { contract: String },
    #[error("`{contract}` is not servable: {}", .breached.join(", "))]
    NotServable {
        contract: String,
        breached: Vec<String>,
    },
    #[error("`{contract}` guarantee `{rule}` does not hold")]
    GuaranteeFailed { contract: String, rule: String },
    #[error("privacy budget `{budget}` is exhausted for this caller")]
    BudgetExhausted { budget: String },
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    DataFusion(#[from] datafusion::error::DataFusionError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, EngineError>;

impl From<String> for EngineError {
    fn from(s: String) -> Self {
        EngineError::Invalid(s)
    }
}
