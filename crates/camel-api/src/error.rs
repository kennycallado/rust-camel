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
}

impl From<std::io::Error> for CamelError {
    fn from(err: std::io::Error) -> Self {
        CamelError::Io(err.to_string())
    }
}
