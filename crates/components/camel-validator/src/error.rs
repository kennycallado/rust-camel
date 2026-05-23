use std::sync::Arc;

use thiserror::Error;

use crate::proto;

#[derive(Error, Debug, Clone)]
pub enum ValidatorError {
    #[error("{0}")]
    Endpoint(String),
    #[error("{0}")]
    Validation(String),
    #[error("{message}")]
    Transport {
        message: String,
        #[source]
        source: Option<Arc<dyn std::error::Error + Send + Sync>>,
    },
    #[error("{0}")]
    UnsupportedMode(&'static str),
    #[error("{message}")]
    CompilationFailed {
        message: String,
        #[source]
        source: Option<Arc<dyn std::error::Error + Send + Sync>>,
    },
    #[error("payload too large: {actual} bytes exceeds limit of {limit} bytes")]
    PayloadTooLarge { actual: usize, limit: usize },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_variant_preserves_source_chain() {
        let inner = std::io::Error::other("socket closed");
        let err = ValidatorError::transport_with_source("rpc failed", inner);
        let source = std::error::Error::source(&err);
        assert!(source.is_some());
        assert!(source.unwrap().to_string().contains("socket closed"));
    }
}

impl ValidatorError {
    pub fn endpoint(msg: impl Into<String>) -> Self {
        Self::Endpoint(msg.into())
    }

    pub fn validation(msg: impl Into<String>) -> Self {
        Self::Validation(msg.into())
    }

    pub fn transport_with_source(
        msg: impl Into<String>,
        source: impl std::error::Error + Send + Sync + 'static,
    ) -> Self {
        Self::Transport {
            message: msg.into(),
            source: Some(Arc::new(source)),
        }
    }

    pub fn from_bridge_error(err: &proto::BridgeError) -> Self {
        let msg = err.message.clone();
        match proto::bridge_error::Kind::try_from(err.kind)
            .unwrap_or(proto::bridge_error::Kind::Unknown)
        {
            proto::bridge_error::Kind::CompilationFailed => Self::CompilationFailed {
                message: msg,
                source: None,
            },
            proto::bridge_error::Kind::ValidationFailed => Self::Validation(msg),
            proto::bridge_error::Kind::Unknown
            | proto::bridge_error::Kind::InvalidInput
            | proto::bridge_error::Kind::TransformFailed
            | proto::bridge_error::Kind::ResourceNotFound
            | proto::bridge_error::Kind::SecurityViolation
            | proto::bridge_error::Kind::Internal => Self::Transport {
                message: msg,
                source: None,
            },
        }
    }

    pub fn to_endpoint_error(&self) -> camel_component_api::CamelError {
        camel_component_api::CamelError::EndpointCreationFailed(self.to_string())
    }

    pub fn to_processor_error(&self) -> camel_component_api::CamelError {
        camel_component_api::CamelError::ProcessorError(self.to_string())
    }
}
