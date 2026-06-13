use std::time::Duration;

use camel_api::CamelError;

/// Error taxonomy for LLM component operations.
///
/// Provider-agnostic — no provider-specific variants. Use the `Provider(String)`
/// catch-all for provider-specific errors; the constructor truncates messages
/// to 200 bytes with UTF-8 safety.
#[derive(Debug, Clone, thiserror::Error)]
#[non_exhaustive]
pub enum LlmError {
    /// Rate limited by provider; optionally carries a retry-after duration.
    #[error("rate limited by provider")]
    RateLimit {
        /// Optional duration to wait before retrying.
        retry_after: Option<Duration>,
    },

    /// Usage quota exceeded (e.g., billing limit reached).
    #[error("quota exceeded: {detail}")]
    QuotaExceeded {
        /// Human-readable detail.
        detail: String,
    },

    /// Request exceeds provider context window.
    #[error("context window exceeded: {max_tokens} tokens max")]
    ContextLengthExceeded {
        /// Maximum allowed tokens for the model.
        max_tokens: u32,
    },

    /// Authentication / authorization failure.
    #[error("authentication failed: {detail}")]
    AuthFailed {
        /// Human-readable detail.
        detail: String,
    },

    /// Requested model does not exist.
    #[error("model not found: {0}")]
    ModelNotFound(String),

    /// Model is temporarily unavailable (e.g., overloaded).
    #[error("model unavailable: {0}")]
    ModelUnavailable(String),

    /// Provider endpoint is unreachable or returning server errors.
    #[error("provider unavailable: {0}")]
    ProviderUnavailable(String),

    /// Response was blocked by the provider's content safety policy.
    #[error("content filtered by safety policy: {detail}")]
    ContentFiltered {
        /// Human-readable detail.
        detail: String,
    },

    /// Transient network error (connection reset, DNS failure, etc.).
    #[error("network error: {0}")]
    Network(String),

    /// Request exceeded the configured timeout.
    #[error("timeout after {0:?}")]
    Timeout(Duration),

    /// Request was malformed or violated provider constraints.
    #[error("invalid request: {0}")]
    InvalidRequest(String),

    /// The operation is not supported by this provider.
    #[error("capability not supported: {0}")]
    UnsupportedCapability(String),

    /// Response could not be decoded or parsed.
    #[error("malformed provider response: {0}")]
    Protocol(String),

    /// Stream was terminated before completion.
    #[error("stream interrupted: {0}")]
    StreamInterrupted(String),

    /// Provider-specific error (catch-all). Message is automatically truncated
    /// to 200 bytes with UTF-8 safety via [`LlmError::provider`].
    #[error("provider error: {0}")]
    Provider(String),
}

/// Maximum length for provider error display strings.
const MAX_PROVIDER_ERROR_BYTES: usize = 200;

/// Truncate a string for display, ensuring it fits within
/// `MAX_PROVIDER_ERROR_BYTES` at a UTF-8 boundary.
fn truncate_for_display(msg: &str) -> String {
    if msg.len() <= MAX_PROVIDER_ERROR_BYTES {
        msg.to_string()
    } else {
        let cut = msg.floor_char_boundary(MAX_PROVIDER_ERROR_BYTES);
        format!("{}...[truncated]", &msg[..cut])
    }
}

impl LlmError {
    /// Create a provider error with automatic message truncation.
    ///
    /// The message is truncated to 200 bytes at a UTF-8 safe boundary,
    /// preventing unbounded error messages from propagating.
    pub fn provider(msg: impl Into<String>) -> Self {
        LlmError::Provider(truncate_for_display(&msg.into()))
    }
}

/// Returns `true` if the error is transient and the operation may succeed
/// on retry.
///
/// Retryable errors:
/// - [`LlmError::RateLimit`] — provider asks us to back off
/// - [`LlmError::Network`] — transient connection issues
/// - [`LlmError::ProviderUnavailable`] — server-side transient
/// - [`LlmError::ModelUnavailable`] — model overloaded
/// - [`LlmError::Timeout`] — request timed out
pub fn is_retryable(err: &LlmError) -> bool {
    matches!(
        err,
        LlmError::RateLimit { .. }
            | LlmError::Network(_)
            | LlmError::ProviderUnavailable(_)
            | LlmError::ModelUnavailable(_)
            | LlmError::Timeout(_)
    )
}

impl From<LlmError> for CamelError {
    fn from(e: LlmError) -> Self {
        match &e {
            LlmError::AuthFailed { .. } => CamelError::Unauthenticated(e.to_string()),
            LlmError::Network(_) | LlmError::ProviderUnavailable(_) => {
                CamelError::Io(e.to_string())
            }
            _ => CamelError::ProcessorError(e.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn retryable_errors() {
        assert!(is_retryable(&LlmError::Network("conn reset".into())));
        assert!(is_retryable(&LlmError::Timeout(Duration::from_secs(30))));
        assert!(is_retryable(&LlmError::RateLimit { retry_after: None }));
        assert!(is_retryable(&LlmError::ProviderUnavailable("503".into())));
        assert!(is_retryable(&LlmError::ModelUnavailable(
            "overloaded".into()
        )));
    }

    #[test]
    fn non_retryable_errors() {
        assert!(!is_retryable(&LlmError::AuthFailed {
            detail: "bad key".into()
        }));
        assert!(!is_retryable(&LlmError::QuotaExceeded {
            detail: "billing".into()
        }));
        assert!(!is_retryable(&LlmError::ContextLengthExceeded {
            max_tokens: 4096
        }));
        assert!(!is_retryable(&LlmError::ModelNotFound("gpt-99".into())));
        assert!(!is_retryable(&LlmError::ContentFiltered {
            detail: "safety".into()
        }));
        assert!(!is_retryable(&LlmError::InvalidRequest("bad json".into())));
        assert!(!is_retryable(&LlmError::Protocol("decode".into())));
        assert!(!is_retryable(&LlmError::StreamInterrupted(
            "dropped".into()
        )));
        assert!(!is_retryable(&LlmError::UnsupportedCapability(
            "embed".into()
        )));
        assert!(!is_retryable(&LlmError::Provider("generic error".into())));
    }

    #[test]
    fn converts_to_camel_error() {
        let err: camel_api::CamelError = LlmError::Timeout(Duration::from_secs(5)).into();
        assert!(err.to_string().contains("timeout"));

        // Verify specific mapping arms
        assert!(matches!(
            CamelError::from(LlmError::Network("conn".into())),
            CamelError::Io(_)
        ));
        assert!(matches!(
            CamelError::from(LlmError::ProviderUnavailable("503".into())),
            CamelError::Io(_)
        ));
        assert!(matches!(
            CamelError::from(LlmError::AuthFailed {
                detail: "bad key".into()
            }),
            CamelError::Unauthenticated(_)
        ));
        // Catch-all arm
        assert!(matches!(
            CamelError::from(LlmError::InvalidRequest("bad json".into())),
            CamelError::ProcessorError(_)
        ));
    }

    #[test]
    fn provider_constructor_truncates_long_messages() {
        let long = "x".repeat(300);
        let err = LlmError::provider(long);
        let display = err.to_string();
        assert!(display.contains("provider error"));
        assert!(display.contains("[truncated]"));
        // 200 bytes + "provider error: " prefix + "...[truncated]" suffix
        assert!(
            display.len() <= 200 + 40,
            "display too long: {} bytes",
            display.len()
        );
    }
}
