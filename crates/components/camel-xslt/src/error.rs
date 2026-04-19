#[derive(Debug, thiserror::Error)]
pub enum XsltError {
    #[error("compile failed: {0}")]
    CompileFailed(String),
    #[error("transform failed: {0}")]
    TransformFailed(String),
    #[error("bridge error: {0}")]
    Bridge(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}
