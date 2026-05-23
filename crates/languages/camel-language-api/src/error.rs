use thiserror::Error;

#[derive(Debug, Error)]
pub enum LanguageError {
    #[error("parse error in expression `{expr}`: {reason}")]
    ParseError { expr: String, reason: String },

    #[error("evaluation error: {0}")]
    EvalError(String),

    #[error("unknown variable: {0}")]
    UnknownVariable(String),

    #[error("language `{0}` not found in registry")]
    NotFound(String),

    #[error("feature '{feature}' not supported by language '{language}'")]
    NotSupported { feature: String, language: String },
}

impl LanguageError {
    /// Create an `EvalError` that includes the expression being evaluated.
    ///
    /// This preserves the expression context in the error message for easier debugging.
    pub fn eval_error(expr: &str, message: impl std::fmt::Display) -> Self {
        LanguageError::EvalError(format!("in expression `{expr}`: {message}"))
    }
}
