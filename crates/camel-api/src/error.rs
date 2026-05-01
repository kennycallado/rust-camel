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

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Body stream has already been consumed")]
    AlreadyConsumed,

    #[error("Stream size exceeded limit: {0}")]
    StreamLimitExceeded(usize),
}

impl CamelError {
    pub fn classify(&self) -> &'static str {
        #[allow(unreachable_patterns)]
        match self {
            Self::ComponentNotFound(_) => "component",
            Self::EndpointCreationFailed(_) | Self::InvalidUri(_) => "endpoint",
            Self::ProcessorError(_) => "processor",
            Self::TypeConversionFailed(_) | Self::AlreadyConsumed => "type_conversion",
            Self::Io(_) => "io",
            Self::RouteError(_) => "route",
            Self::CircuitOpen(_) => "circuit_open",
            Self::HttpOperationFailed { .. } => "http",
            Self::Config(_) => "config",
            Self::DeadLetterChannelFailed(_) => "dead_letter",
            Self::Stopped => "stopped",
            Self::StreamLimitExceeded(_) => "stream",
            Self::ChannelClosed => "channel",
            _ => "unknown",
        }
    }
}

impl From<std::io::Error> for CamelError {
    fn from(err: std::io::Error) -> Self {
        CamelError::Io(err.to_string())
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
            CamelError::Config("x".to_string()),
            CamelError::AlreadyConsumed,
            CamelError::StreamLimitExceeded(42),
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
}
