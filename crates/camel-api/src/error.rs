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

    #[error("HTTP operation failed: {status_code} {status_text}")]
    HttpOperationFailed {
        status_code: u16,
        status_text: String,
        response_body: Option<String>,
    },

    #[error("Exchange stopped by Stop EIP")]
    Stopped,
}

impl From<std::io::Error> for CamelError {
    fn from(err: std::io::Error) -> Self {
        CamelError::Io(err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_http_operation_failed_display() {
        let err = CamelError::HttpOperationFailed {
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
}
