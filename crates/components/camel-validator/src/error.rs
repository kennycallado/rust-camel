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

    /// Returns `true` exactly when the bridge processed the registration
    /// request and refused the schema set itself (`COMPILATION_FAILED`
    /// response kind via [`Self::from_bridge_error`]).
    ///
    /// Such a refusal is a function of the schema bytes and recurs
    /// deterministically against the same bridge process, so it can be
    /// cached negatively. All other variants — transport failures, other
    /// response kinds mapped to a source-less [`Self::Transport`],
    /// document-level [`Self::Validation`], [`Self::Endpoint`],
    /// [`Self::PayloadTooLarge`], and [`Self::UnsupportedMode`] — are not
    /// cacheable refusals.
    #[must_use]
    pub fn is_deterministic_refusal(&self) -> bool {
        matches!(self, Self::CompilationFailed { .. })
    }

    pub fn to_endpoint_error(&self) -> camel_component_api::CamelError {
        camel_component_api::CamelError::EndpointCreationFailed(self.to_string())
    }

    pub fn to_processor_error(&self) -> camel_component_api::CamelError {
        camel_component_api::CamelError::ProcessorError(self.to_string())
    }
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

    #[test]
    fn deterministic_refusal_classification_truth_table() {
        let compilation_failed = ValidatorError::CompilationFailed {
            message: "bad".to_string(),
            source: None,
        };
        let sourceless_transport = ValidatorError::Transport {
            message: "t".to_string(),
            source: None,
        };
        let tonic_transport = ValidatorError::transport_with_source(
            "t",
            tonic::Status::unavailable("tcp connect error"),
        );
        let validation = ValidatorError::Validation("v".into());
        let endpoint = ValidatorError::endpoint("e");
        let payload_too_large = ValidatorError::PayloadTooLarge {
            actual: 2,
            limit: 1,
        };
        let unsupported_mode = ValidatorError::UnsupportedMode("m");

        assert!(
            compilation_failed.is_deterministic_refusal(),
            "CompilationFailed must classify as a deterministic refusal"
        );
        assert!(
            !sourceless_transport.is_deterministic_refusal(),
            "source-less Transport must not classify as a deterministic refusal"
        );
        assert!(
            !tonic_transport.is_deterministic_refusal(),
            "tonic-backed Transport must not classify as a deterministic refusal"
        );
        assert!(
            !validation.is_deterministic_refusal(),
            "Validation must not classify as a deterministic refusal"
        );
        assert!(
            !endpoint.is_deterministic_refusal(),
            "Endpoint must not classify as a deterministic refusal"
        );
        assert!(
            !payload_too_large.is_deterministic_refusal(),
            "PayloadTooLarge must not classify as a deterministic refusal"
        );
        assert!(
            !unsupported_mode.is_deterministic_refusal(),
            "UnsupportedMode must not classify as a deterministic refusal"
        );
    }

    #[test]
    fn from_bridge_error_compilation_failed_is_deterministic_refusal() {
        let err = proto::BridgeError {
            kind: proto::bridge_error::Kind::CompilationFailed as i32,
            message: "SCHEMA_PARSE_ERROR: bad".to_string(),
            ..Default::default()
        };

        let mapped = ValidatorError::from_bridge_error(&err);

        assert!(matches!(&mapped, ValidatorError::CompilationFailed { .. }));
        assert!(mapped.is_deterministic_refusal());
        assert_eq!(mapped.to_string(), "SCHEMA_PARSE_ERROR: bad");
    }

    #[test]
    fn from_bridge_error_internal_is_transport_and_not_refusal() {
        let err = proto::BridgeError {
            kind: proto::bridge_error::Kind::Internal as i32,
            message: "upstream transport unavailable".to_string(),
            ..Default::default()
        };

        let mapped = ValidatorError::from_bridge_error(&err);

        assert!(matches!(
            &mapped,
            ValidatorError::Transport { source: None, .. }
        ));
        assert!(!mapped.is_deterministic_refusal());
    }
}
