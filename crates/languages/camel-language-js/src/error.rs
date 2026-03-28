use thiserror::Error;

/// Errors produced by the JavaScript language plugin.
#[derive(Debug, Error)]
pub enum JsLanguageError {
    /// The JavaScript source failed to parse or execute.
    #[error("JS execution error: {message}")]
    Execution { message: String },

    /// The value returned by the script could not be converted to the expected type.
    #[error("JS type conversion error: {message}")]
    TypeConversion { message: String },

    /// A required header or property was not found on the exchange.
    #[error("JS exchange access error: {message}")]
    ExchangeAccess { message: String },

    /// The JS source could not be compiled/parsed.
    #[error("JS parse error: {message}")]
    Parse { message: String },
}
