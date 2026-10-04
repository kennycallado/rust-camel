use thiserror::Error;

/// Errors produced by the JavaScript language plugin.
#[derive(Debug, Error)]
pub enum JsLanguageError {
    /// The JavaScript source failed to parse or execute.
    ///
    /// `message` is engine diagnostic text and may embed exchange data. It is
    /// redacted at the [`LanguageError`] boundary in `expression.rs`; never
    /// forward it into a `CamelError`, log record, or DLC payload.
    #[error("JS execution error: {message}")]
    Execution { message: String },

    /// The evaluation exceeded its wall-clock or queuing deadline.
    #[error("JS execution timeout")]
    Timeout,

    /// The engine surfaced a native `TypeError` (structured kind preserved
    /// from Boa before any rendering). Carries no engine message.
    #[error("JS type error")]
    TypeMismatch,

    /// A configured runtime limit (loop iterations, recursion depth, or stack
    /// size) was exceeded. Structured from Boa's `RuntimeLimitError`; carries
    /// no engine message.
    #[error("JS execution limit exceeded")]
    Limit,

    /// The value returned by the script could not be converted to the expected type.
    #[error("JS type conversion error: {message}")]
    TypeConversion { message: String },

    /// A required header or property was not found on the exchange.
    #[error("JS exchange access error: {message}")]
    ExchangeAccess { message: String },

    /// A read-only evaluation attempted to mutate the exchange snapshot
    /// through the `camel.*` surface (dynamic dispatch, alias capture,
    /// destructuring, `Object.defineProperty`, `Reflect.set`, or a nested
    /// object write). Carries no engine message: the refusal is a private
    /// sentinel match, not a rendered diagnostic.
    #[error("JS read-only mutation refused")]
    ReadOnlyMutation,

    /// The JS source could not be compiled/parsed.
    #[error("JS parse error: {message}")]
    Parse { message: String },

    /// The engine does not implement read-only evaluation. Returned by the
    /// default [`JsEngine::eval_read_only`](crate::JsEngine::eval_read_only)
    /// so a custom engine that only implements the writable path fails closed
    /// instead of silently running the writable path for a read-only
    /// expression or predicate.
    #[error("JS read-only evaluation is not supported by this engine")]
    ReadOnlyUnsupported,
}
