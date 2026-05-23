use std::sync::Arc;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum BeanError {
    #[error("Bean not found: {0}")]
    NotFound(String),

    #[error("Bean method not found: {0}")]
    MethodNotFound(String),

    #[error("Parameter binding failed: {0}")]
    BindingFailed(String),

    #[error("Handler execution failed: {0}")]
    ExecutionFailed(String),

    #[error("Bean name must not be empty or whitespace-only: '{0}'")]
    InvalidName(String),

    #[error("Bean already registered: {0}")]
    DuplicateName(String),
}

impl From<BeanError> for camel_api::CamelError {
    fn from(err: BeanError) -> Self {
        camel_api::CamelError::ProcessorErrorWithSource(err.to_string(), Arc::new(err))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bean_error_source_chain_preserved() {
        let bean_err = BeanError::NotFound("myBean".to_string());
        let camel_err: camel_api::CamelError = bean_err.into();

        // Verify source() returns Some (the original BeanError)
        let source = std::error::Error::source(&camel_err);
        assert!(
            source.is_some(),
            "CamelError::source() must return Some for BeanError conversion"
        );

        // Verify the source is indeed a BeanError::NotFound
        let source_ref = source.unwrap();
        let msg = source_ref.to_string();
        assert!(
            msg.contains("myBean"),
            "Source error message should contain 'myBean', got: {msg}"
        );
    }

    #[test]
    fn test_bean_error_to_camel_error_is_not_panic() {
        // Verify conversion produces an Err, not a panic
        let bean_err = BeanError::MethodNotFound("process".to_string());
        let camel_err: camel_api::CamelError = bean_err.into();
        assert!(matches!(
            camel_err,
            camel_api::CamelError::ProcessorErrorWithSource(_, _)
        ));
    }
}
