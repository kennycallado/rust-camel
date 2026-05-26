use thiserror::Error;

#[derive(Debug, Error)]
pub enum AuthError {
    #[error("token invalid: {0}")]
    TokenInvalid(String),

    #[error("authentication failed: {0}")]
    AuthenticationFailed(String),

    #[error("authorization denied: {0}")]
    AuthorizationDenied(String),

    #[error("configuration error: {0}")]
    Config(String),
}
