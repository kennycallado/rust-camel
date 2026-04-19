#[derive(Debug, thiserror::Error)]
pub enum XjError {
    #[error("xslt error: {0}")]
    Xslt(#[from] camel_xslt::XsltError),
    #[error("unsupported body type")]
    UnsupportedBody,
}
