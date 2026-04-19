use std::fmt::{Display, Formatter};

use crate::proto;

#[derive(Debug, Clone)]
pub enum ValidatorError {
    Endpoint(String),
    Validation(String),
    Transport(String),
    UnsupportedMode(&'static str),
    CompilationFailed(String),
}

impl ValidatorError {
    pub fn endpoint(msg: impl Into<String>) -> Self {
        Self::Endpoint(msg.into())
    }

    pub fn validation(msg: impl Into<String>) -> Self {
        Self::Validation(msg.into())
    }

    pub fn from_bridge_error(err: &proto::BridgeError) -> Self {
        let msg = err.message.clone();
        match proto::bridge_error::Kind::try_from(err.kind)
            .unwrap_or(proto::bridge_error::Kind::Unknown)
        {
            proto::bridge_error::Kind::CompilationFailed => Self::CompilationFailed(msg),
            proto::bridge_error::Kind::ValidationFailed => Self::Validation(msg),
            proto::bridge_error::Kind::Unknown
            | proto::bridge_error::Kind::InvalidInput
            | proto::bridge_error::Kind::TransformFailed
            | proto::bridge_error::Kind::ResourceNotFound
            | proto::bridge_error::Kind::SecurityViolation
            | proto::bridge_error::Kind::Internal => Self::Transport(msg),
        }
    }

    pub fn to_endpoint_error(&self) -> camel_component_api::CamelError {
        camel_component_api::CamelError::EndpointCreationFailed(self.to_string())
    }

    pub fn to_processor_error(&self) -> camel_component_api::CamelError {
        camel_component_api::CamelError::ProcessorError(self.to_string())
    }
}

impl Display for ValidatorError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Endpoint(msg) => write!(f, "{msg}"),
            Self::Validation(msg) => write!(f, "{msg}"),
            Self::Transport(msg) => write!(f, "{msg}"),
            Self::UnsupportedMode(msg) => write!(f, "{msg}"),
            Self::CompilationFailed(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for ValidatorError {}
