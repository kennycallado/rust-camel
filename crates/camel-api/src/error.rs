use std::sync::Arc;
use thiserror::Error;

/// Core error type for the Camel framework.
#[derive(Debug, Clone, Error)]
#[non_exhaustive]
pub enum CamelError {
    #[error("Component not found: {0}")]
    ComponentNotFound(String),

    #[error("Endpoint creation failed: {0}")]
    EndpointCreationFailed(String),

    #[error("Processor error: {0}")]
    ProcessorError(String),

    /// Like `ProcessorError` but preserves the source error chain
    /// for downstream inspection (e.g. via `std::error::Error::source()`).
    #[error("Processor error: {0}")]
    ProcessorErrorWithSource(String, #[source] Arc<dyn std::error::Error + Send + Sync>),

    #[error("Type conversion failed: {0}")]
    TypeConversionFailed(String),

    #[error("Invalid URI: {0}")]
    InvalidUri(String),

    #[error("Channel closed")]
    ChannelClosed,

    #[error("Route error: {0}")]
    RouteError(String),

    #[error("IO error: {0}")]
    Io(String),

    #[error("Dead letter channel failed: {0}")]
    DeadLetterChannelFailed(String),

    #[error("Circuit breaker open: {0}")]
    CircuitOpen(String),

    #[error("HTTP {method} {url} failed: {status_code} {status_text}")]
    HttpOperationFailed {
        method: String,
        url: String,
        status_code: u16,
        status_text: String,
        response_body: Option<String>,
    },

    #[error("Exchange stopped by Stop EIP")]
    Stopped,

    /// Producer's `poll_ready` returned a shutdown signal — the consumer/semaphore
    /// is closing and the producer cannot acquire a permit. Distinct from Stop EIP
    /// (which is successful control flow). Used by JMS/OpenSearch producers. See ADR-0024.
    #[error("Consumer stopping: semaphore closed during poll_ready")]
    ConsumerStopping,

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Body stream has already been consumed")]
    AlreadyConsumed,

    #[error("Stream size exceeded limit: {0}")]
    StreamLimitExceeded(usize),

    #[error("Unauthenticated: {0}")]
    Unauthenticated(String),

    #[error("Unauthorized: {0}")]
    Unauthorized(String),
}

impl CamelError {
    pub fn classify(&self) -> &'static str {
        #[allow(unreachable_patterns)]
        match self {
            Self::ComponentNotFound(_) => "component",
            Self::EndpointCreationFailed(_) | Self::InvalidUri(_) => "endpoint",
            Self::ProcessorError(_) | Self::ProcessorErrorWithSource(_, _) => "processor",
            Self::TypeConversionFailed(_) | Self::AlreadyConsumed => "type_conversion",
            Self::Io(_) => "io",
            Self::RouteError(_) => "route",
            Self::CircuitOpen(_) => "circuit_open",
            Self::HttpOperationFailed { .. } => "http",
            Self::Config(_) => "config",
            Self::DeadLetterChannelFailed(_) => "dead_letter",
            Self::Stopped => "stopped",
            Self::ConsumerStopping => "consumer_stop",
            Self::StreamLimitExceeded(_) => "stream",
            Self::ChannelClosed => "channel",
            Self::Unauthenticated(_) => "unauthenticated",
            Self::Unauthorized(_) => "unauthorized",
            _ => "unknown",
        }
    }

    /// Stable variant name used by `doTry` catch-by-variant matchers.
    ///
    /// `ProcessorErrorWithSource` aliases to `"ProcessorError"` — the two variants are
    /// not distinguishable by name in MVP (see spec §5.4).
    ///
    /// The enum is `#[non_exhaustive]`; this match lives in the defining crate (camel-api),
    /// so internal exhaustive matching is allowed. Adding a new variant without updating
    /// this method will fail to compile, surfaced by `variant_name_tests`.
    pub fn variant_name(&self) -> &'static str {
        match self {
            Self::ComponentNotFound(_) => "ComponentNotFound",
            Self::EndpointCreationFailed(_) => "EndpointCreationFailed",
            Self::ProcessorError(_) => "ProcessorError",
            Self::ProcessorErrorWithSource(_, _) => "ProcessorError",
            Self::TypeConversionFailed(_) => "TypeConversionFailed",
            Self::InvalidUri(_) => "InvalidUri",
            Self::ChannelClosed => "ChannelClosed",
            Self::RouteError(_) => "RouteError",
            Self::Io(_) => "Io",
            Self::DeadLetterChannelFailed(_) => "DeadLetterChannelFailed",
            Self::CircuitOpen(_) => "CircuitOpen",
            Self::HttpOperationFailed { .. } => "HttpOperationFailed",
            Self::Stopped => "Stopped",
            Self::ConsumerStopping => "ConsumerStopping",
            Self::Config(_) => "Config",
            Self::AlreadyConsumed => "AlreadyConsumed",
            Self::StreamLimitExceeded(_) => "StreamLimitExceeded",
            Self::Unauthenticated(_) => "Unauthenticated",
            Self::Unauthorized(_) => "Unauthorized",
        }
    }
}

impl From<std::io::Error> for CamelError {
    fn from(err: std::io::Error) -> Self {
        CamelError::Io(err.to_string())
    }
}

impl From<crate::template::TemplateError> for CamelError {
    fn from(err: crate::template::TemplateError) -> Self {
        CamelError::Config(err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all_error_samples() -> Vec<CamelError> {
        vec![
            CamelError::ComponentNotFound("x".to_string()),
            CamelError::EndpointCreationFailed("x".to_string()),
            CamelError::ProcessorError("x".to_string()),
            CamelError::ProcessorErrorWithSource(
                "x".to_string(),
                Arc::new(std::io::Error::new(std::io::ErrorKind::Other, "inner")),
            ),
            CamelError::TypeConversionFailed("x".to_string()),
            CamelError::InvalidUri("x".to_string()),
            CamelError::ChannelClosed,
            CamelError::RouteError("x".to_string()),
            CamelError::Io("x".to_string()),
            CamelError::DeadLetterChannelFailed("x".to_string()),
            CamelError::CircuitOpen("x".to_string()),
            CamelError::HttpOperationFailed {
                method: "GET".to_string(),
                url: "https://example.com".to_string(),
                status_code: 500,
                status_text: "Internal Server Error".to_string(),
                response_body: Some("error".to_string()),
            },
            CamelError::Stopped,
            CamelError::ConsumerStopping,
            CamelError::Config("x".to_string()),
            CamelError::AlreadyConsumed,
            CamelError::StreamLimitExceeded(42),
            CamelError::Unauthenticated("token expired".to_string()),
            CamelError::Unauthorized("missing admin role".to_string()),
        ]
    }

    #[test]
    fn test_http_operation_failed_display() {
        let err = CamelError::HttpOperationFailed {
            method: "GET".to_string(),
            url: "https://example.com/test".to_string(),
            status_code: 404,
            status_text: "Not Found".to_string(),
            response_body: Some("page not found".to_string()),
        };
        let msg = format!("{err}");
        assert!(msg.contains("404"));
        assert!(msg.contains("Not Found"));
    }

    #[test]
    fn test_http_operation_failed_clone() {
        let err = CamelError::HttpOperationFailed {
            method: "POST".to_string(),
            url: "https://api.example.com/users".to_string(),
            status_code: 500,
            status_text: "Internal Server Error".to_string(),
            response_body: None,
        };
        let cloned = err.clone();
        assert!(matches!(
            cloned,
            CamelError::HttpOperationFailed {
                status_code: 500,
                ..
            }
        ));
    }

    #[test]
    fn test_stopped_variant_exists_and_is_clone() {
        let err = CamelError::Stopped;
        let cloned = err.clone();
        assert!(matches!(cloned, CamelError::Stopped));
        assert_eq!(format!("{err}"), "Exchange stopped by Stop EIP");
    }

    #[test]
    fn test_classify_maps_all_variants() {
        assert_eq!(
            CamelError::ComponentNotFound("x".to_string()).classify(),
            "component"
        );
        assert_eq!(
            CamelError::EndpointCreationFailed("x".to_string()).classify(),
            "endpoint"
        );
        assert_eq!(
            CamelError::ProcessorError("x".to_string()).classify(),
            "processor"
        );
        assert_eq!(
            CamelError::TypeConversionFailed("x".to_string()).classify(),
            "type_conversion"
        );
        assert_eq!(
            CamelError::InvalidUri("x".to_string()).classify(),
            "endpoint"
        );
        assert_eq!(CamelError::ChannelClosed.classify(), "channel");
        assert_eq!(CamelError::RouteError("x".to_string()).classify(), "route");
        assert_eq!(CamelError::Io("x".to_string()).classify(), "io");
        assert_eq!(
            CamelError::DeadLetterChannelFailed("x".to_string()).classify(),
            "dead_letter"
        );
        assert_eq!(
            CamelError::CircuitOpen("x".to_string()).classify(),
            "circuit_open"
        );
        assert_eq!(
            CamelError::HttpOperationFailed {
                method: "GET".to_string(),
                url: "https://example.com".to_string(),
                status_code: 500,
                status_text: "Internal Server Error".to_string(),
                response_body: None,
            }
            .classify(),
            "http"
        );
        assert_eq!(CamelError::Stopped.classify(), "stopped");
        assert_eq!(CamelError::Config("x".to_string()).classify(), "config");
        assert_eq!(CamelError::AlreadyConsumed.classify(), "type_conversion");
        assert_eq!(CamelError::StreamLimitExceeded(42).classify(), "stream");
    }

    #[test]
    fn test_classify_output_is_ascii_and_short() {
        for error in all_error_samples() {
            let class = error.classify();
            assert!(class.is_ascii());
            assert!(class.len() <= 15, "class too long: {class}");
        }
    }

    #[test]
    fn test_auth_variants_classify() {
        assert_eq!(
            CamelError::Unauthenticated("x".to_string()).classify(),
            "unauthenticated"
        );
        assert_eq!(
            CamelError::Unauthorized("x".to_string()).classify(),
            "unauthorized"
        );
    }

    #[test]
    fn test_auth_variants_are_clone() {
        let err = CamelError::Unauthenticated("test".to_string());
        let cloned = err.clone();
        assert!(matches!(cloned, CamelError::Unauthenticated(_)));

        let err2 = CamelError::Unauthorized("test".to_string());
        let cloned2 = err2.clone();
        assert!(matches!(cloned2, CamelError::Unauthorized(_)));
    }
}

#[cfg(test)]
mod variant_name_tests {
    use super::CamelError;
    use std::sync::Arc;

    /// Representative value for each of the 18 enum variants. This test fails to compile
    /// when a new variant is added to CamelError without updating variant_name().
    /// The enum is `#[non_exhaustive]` but this match lives in the same crate, so internal
    /// exhaustive matching is allowed.
    #[test]
    fn variant_name_covers_all_variants() {
        let cases: Vec<(CamelError, &str)> = vec![
            (
                CamelError::ComponentNotFound("x".into()),
                "ComponentNotFound",
            ),
            (
                CamelError::EndpointCreationFailed("x".into()),
                "EndpointCreationFailed",
            ),
            (CamelError::ProcessorError("x".into()), "ProcessorError"),
            (
                CamelError::ProcessorErrorWithSource(
                    "x".into(),
                    Arc::new(std::io::Error::new(std::io::ErrorKind::Other, "y")),
                ),
                "ProcessorError", // aliased
            ),
            (
                CamelError::TypeConversionFailed("x".into()),
                "TypeConversionFailed",
            ),
            (CamelError::InvalidUri("x".into()), "InvalidUri"),
            (CamelError::ChannelClosed, "ChannelClosed"),
            (CamelError::RouteError("x".into()), "RouteError"),
            (CamelError::Io("x".into()), "Io"),
            (
                CamelError::DeadLetterChannelFailed("x".into()),
                "DeadLetterChannelFailed",
            ),
            (CamelError::CircuitOpen("x".into()), "CircuitOpen"),
            (
                CamelError::HttpOperationFailed {
                    method: "GET".into(),
                    url: "https://example.com".into(),
                    status_code: 500,
                    status_text: "Internal Server Error".into(),
                    response_body: None,
                },
                "HttpOperationFailed",
            ),
            (CamelError::Stopped, "Stopped"),
            (CamelError::Config("x".into()), "Config"),
            (CamelError::AlreadyConsumed, "AlreadyConsumed"),
            (CamelError::StreamLimitExceeded(42), "StreamLimitExceeded"),
            (CamelError::Unauthenticated("x".into()), "Unauthenticated"),
            (CamelError::Unauthorized("x".into()), "Unauthorized"),
        ];

        for (err, expected) in cases {
            assert_eq!(
                err.variant_name(),
                expected,
                "variant_name mismatch for {:?}",
                err
            );
        }
    }
}
