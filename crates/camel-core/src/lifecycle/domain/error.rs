use std::fmt;

use thiserror::Error;

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

/// Error type for language registration operations on [`CamelContext`].
///
/// This is distinct from [`camel_language_api::error::LanguageError`], which
/// covers language-evaluation concerns (parse, eval, unknown variable, etc.).
/// Registration is a context-configuration invariant, not a language concern.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum LanguageRegistryError {
    #[error("language '{name}' is already registered")]
    AlreadyRegistered { name: String },
}
