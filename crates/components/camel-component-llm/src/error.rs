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

    /// A deadline lapsed. Carries the duration that elapsed: the producer
    /// `timeout_secs` deadline on the chat path, the per-request total on the
    /// decision path.
    #[error("timeout: deadline {0:?} exceeded")]
    Timeout(Duration),

    /// Transport timeout inside one HTTP attempt; retryable.
    #[error("timeout: {}", .phase.display_message(*.after))]
    TransportTimeout {
        /// Transport layer that fired.
        phase: TransportTimeoutPhase,
        /// Duration bound that elapsed.
        after: Duration,
    },

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

/// Transport-level timeout layer that fired.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportTimeoutPhase {
    /// TCP connection not established within the connect bound.
    Connect,
    /// No response activity (or headers, pre-headers) within the
    /// transport read-idle guard.
    Idle,
}

impl TransportTimeoutPhase {
    /// Human-readable description of the bound that fired.
    pub fn display_message(self, after: Duration) -> String {
        match self {
            Self::Connect => format!("connect not established within {after:?}"),
            Self::Idle => format!("no response activity for {after:?}"),
        }
    }
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

    /// Create a protocol error from text that may carry provider-supplied
    /// values (answer names, choice values, score keys).
    ///
    /// Control characters become `?` so provider data cannot inject log
    /// lines, and the message is truncated to 200 bytes at a UTF-8 safe
    /// boundary.
    pub fn protocol(msg: impl Into<String>) -> Self {
        let cleaned: String = msg
            .into()
            .chars()
            .map(|c| if c.is_control() { '?' } else { c })
            .collect();
        LlmError::Protocol(truncate_for_display(&cleaned))
    }

    /// Returns `true` if the error is any kind of timeout.
    ///
    /// Callers must not have to match [`LlmError::Timeout`] and
    /// [`LlmError::TransportTimeout`] separately.
    pub fn is_timeout(&self) -> bool {
        matches!(
            self,
            LlmError::Timeout(_) | LlmError::TransportTimeout { .. }
        )
    }
}

/// Returns `true` if the error is transient and the operation may succeed
/// on retry.
///
/// [`LlmError::QuotaExceeded`] is not retryable, while [`LlmError::RateLimit`] is retryable.
///
/// Retryable errors:
/// - [`LlmError::RateLimit`] — provider asks us to back off
/// - [`LlmError::Network`] — transient connection issues
/// - [`LlmError::ProviderUnavailable`] — server-side transient
/// - [`LlmError::ModelUnavailable`] — model overloaded
/// - [`LlmError::Timeout`] — request timed out
/// - [`LlmError::TransportTimeout`] — transport-level timeout (connect or read-idle)
pub fn is_retryable(err: &LlmError) -> bool {
    matches!(
        err,
        LlmError::RateLimit { .. }
            | LlmError::Network(_)
            | LlmError::ProviderUnavailable(_)
            | LlmError::ModelUnavailable(_)
            | LlmError::Timeout(_)
            | LlmError::TransportTimeout { .. }
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
    fn quota_exhausted_not_retryable_while_rate_limit_is() {
        let quota = LlmError::QuotaExceeded {
            detail: "billing limit".into(),
        };
        let throttled_without_retry_after = LlmError::RateLimit { retry_after: None };
        let throttled_with_retry_after = LlmError::RateLimit {
            retry_after: Some(Duration::from_secs(7)),
        };

        assert!(!is_retryable(&quota), "quota exhaustion must fail fast");
        assert!(
            is_retryable(&throttled_without_retry_after),
            "rate limiting is retryable without retry-after"
        );
        assert!(
            is_retryable(&throttled_with_retry_after),
            "rate limiting is retryable with retry-after"
        );
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
    fn transport_timeout_display_names_layer_and_bound() {
        let connect = LlmError::TransportTimeout {
            phase: TransportTimeoutPhase::Connect,
            after: Duration::from_secs(10),
        };
        let display = connect.to_string();
        assert!(display.contains("timeout"), "missing timeout: {display}");
        assert!(display.contains("connect"), "missing layer: {display}");
        assert!(display.contains("10s"), "missing bound: {display}");

        let idle = LlmError::TransportTimeout {
            phase: TransportTimeoutPhase::Idle,
            after: Duration::from_secs(600),
        };
        let display = idle.to_string();
        assert!(display.contains("timeout"), "missing timeout: {display}");
        assert!(
            display.contains("no response activity"),
            "missing layer: {display}"
        );
        assert!(display.contains("600s"), "missing bound: {display}");
    }

    #[test]
    fn producer_deadline_display_keeps_timeout_word() {
        let display = LlmError::Timeout(Duration::from_secs(90)).to_string();
        assert!(display.contains("timeout"), "missing timeout: {display}");
        assert!(display.contains("90s"), "missing deadline: {display}");
    }

    #[test]
    fn transport_timeout_is_retryable_and_is_timeout() {
        let transport = LlmError::TransportTimeout {
            phase: TransportTimeoutPhase::Idle,
            after: Duration::from_secs(1),
        };
        assert!(is_retryable(&transport));
        assert!(transport.is_timeout());
        assert!(LlmError::Timeout(Duration::from_secs(1)).is_timeout());
        assert!(!LlmError::Network("x".into()).is_timeout());
    }

    #[test]
    fn protocol_constructor_truncates_and_strips_control_chars() {
        let err = LlmError::protocol(format!("unknown answer '{}\n{}'", "x".repeat(300), "y"));
        let LlmError::Protocol(msg) = &err else {
            panic!("expected Protocol, got {err:?}");
        };
        assert!(!msg.contains('\n'), "control chars must be replaced: {msg}");
        assert!(
            msg.contains("[truncated]"),
            "long text must be truncated: {msg}"
        );
        assert!(
            msg.len() <= 200 + 20,
            "message too long: {} bytes",
            msg.len()
        );
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
