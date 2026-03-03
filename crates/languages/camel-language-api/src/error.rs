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

    #[error(
        "language `{0}` is already registered; use a different name or remove the existing registration first"
    )]
    AlreadyRegistered(String),
}
