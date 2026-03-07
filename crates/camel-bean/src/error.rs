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
}

impl From<BeanError> for camel_api::CamelError {
    fn from(err: BeanError) -> Self {
        camel_api::CamelError::ProcessorError(err.to_string())
    }
}
