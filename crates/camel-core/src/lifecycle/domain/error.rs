use std::fmt;

use camel_api::CamelError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DomainError {
    InvalidTransition { from: String, to: String },
    NotFound(String),
    AlreadyExists(String),
    InvalidState(String),
}

impl fmt::Display for DomainError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DomainError::InvalidTransition { from, to } => {
                write!(f, "invalid transition: {from} -> {to}")
            }
            DomainError::NotFound(id) => write!(f, "not found: {id}"),
            DomainError::AlreadyExists(id) => write!(f, "already exists: {id}"),
            DomainError::InvalidState(msg) => write!(f, "invalid state: {msg}"),
        }
    }
}

impl std::error::Error for DomainError {}

impl From<DomainError> for CamelError {
    fn from(e: DomainError) -> Self {
        CamelError::RouteError(e.to_string())
    }
}
