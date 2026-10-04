use camel_api::{ErrorPosition, ExpressionErrorClass};
use thiserror::Error;

#[derive(Debug, Error)]
#[non_exhaustive]
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

    /// Structured evaluation failure with a typed class and an optional
    /// engine-reported position inside the expression source.
    ///
    /// `detail` is optional already-redacted diagnostic text: the emitting
    /// crate MUST strip exchange data before storing it here. The `Display`
    /// output stays short and carries no value text.
    #[error(
        "evaluation failure: {class}{}{}",
        position.map(|p| format!(" at {p}")).unwrap_or_default(),
        detail.as_deref().map(|d| format!(": {d}")).unwrap_or_default()
    )]
    EvalFailure {
        class: ExpressionErrorClass,
        position: Option<ErrorPosition>,
        detail: Option<String>,
    },

    /// A value's type did not match the type expected by the destination.
    /// `expected`/`actual` are TYPE names, never runtime values.
    #[error(
        "type mismatch: expected {expected}, actual {actual}{}",
        position.map(|p| format!(" at {p}")).unwrap_or_default()
    )]
    TypeMismatch {
        expected: String,
        actual: String,
        position: Option<ErrorPosition>,
    },

    /// The evaluation result could not be converted to the declared
    /// destination type. `source_type`/`target` are type/destination names,
    /// never runtime values (see `crate::eval::to_expression_failed` for the
    /// trusted-target rewrite).
    #[error("conversion error: cannot convert {source_type} to {target}")]
    ConversionError { source_type: String, target: String },
}

impl LanguageError {
    /// Create an `EvalError` that includes the expression being evaluated.
    ///
    /// This preserves the expression context in the error message for easier debugging.
    pub fn eval_error(expr: &str, message: impl std::fmt::Display) -> Self {
        LanguageError::EvalError(format!("in expression `{expr}`: {message}"))
    }

    /// Typed failure classification for structured transport
    /// (see `crate::eval::to_expression_failed`).
    ///
    /// Registry/lifecycle variants (`UnknownVariable`, `NotFound`,
    /// `NotSupported`) are not evaluation classes and yield `None`.
    pub fn class(&self) -> Option<ExpressionErrorClass> {
        match self {
            Self::ParseError { .. } => Some(ExpressionErrorClass::Parse),
            Self::EvalError(_) => Some(ExpressionErrorClass::Runtime),
            Self::EvalFailure { class, .. } => Some(*class),
            Self::TypeMismatch { .. } => Some(ExpressionErrorClass::TypeMismatch),
            Self::ConversionError { .. } => Some(ExpressionErrorClass::Conversion),
            Self::UnknownVariable(_) | Self::NotFound(_) | Self::NotSupported { .. } => None,
        }
    }

    /// Engine-reported 1-based position inside the expression source, when
    /// the variant carries one.
    pub fn position(&self) -> Option<ErrorPosition> {
        match self {
            Self::EvalFailure { position, .. } | Self::TypeMismatch { position, .. } => *position,
            _ => None,
        }
    }
}
