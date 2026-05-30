use camel_api::CamelError;

#[derive(Debug, Clone, thiserror::Error)]
pub enum AuthError {
    #[error("Unauthenticated: {0}")]
    Unauthenticated(String),

    #[error("Unauthorized: {0}")]
    Unauthorized(String),

    #[error("Token expired")]
    TokenExpired,

    #[error("Token invalid: {0}")]
    TokenInvalid(String),

    #[error("Config error: {0}")]
    ConfigError(String),

    #[error("authentication failed: {0}")]
    AuthenticationFailed(String),

    #[error("authorization denied: {0}")]
    AuthorizationDenied(String),

    #[error("configuration error: {0}")]
    Config(String),

    #[error("Auth provider unavailable: {0}")]
    ProviderUnavailable(String),
}

impl From<AuthError> for CamelError {
    fn from(e: AuthError) -> Self {
        match e {
            AuthError::Unauthenticated(s) => CamelError::Unauthenticated(s),
            AuthError::TokenExpired => CamelError::Unauthenticated("token expired".into()),
            AuthError::TokenInvalid(s) => CamelError::Unauthenticated(s),
            AuthError::Unauthorized(s) => CamelError::Unauthorized(s),
            AuthError::AuthenticationFailed(s) => CamelError::Unauthenticated(s),
            AuthError::AuthorizationDenied(s) => CamelError::Unauthorized(s),
            AuthError::ProviderUnavailable(s) => {
                CamelError::ProcessorError(format!("auth provider unavailable: {s}"))
            }
            AuthError::ConfigError(s) => CamelError::Config(s),
            AuthError::Config(s) => CamelError::Config(s),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_error_maps_unauthenticated() {
        let err = AuthError::Unauthenticated("bad token".into());
        let camel_err: CamelError = err.into();
        assert!(matches!(camel_err, CamelError::Unauthenticated(s) if s.contains("bad token")));
    }

    #[test]
    fn auth_error_maps_token_expired() {
        let err = AuthError::TokenExpired;
        let camel_err: CamelError = err.into();
        assert!(matches!(camel_err, CamelError::Unauthenticated(_)));
    }

    #[test]
    fn auth_error_maps_token_invalid() {
        let err = AuthError::TokenInvalid("bad sig".into());
        let camel_err: CamelError = err.into();
        assert!(matches!(camel_err, CamelError::Unauthenticated(s) if s.contains("bad sig")));
    }

    #[test]
    fn auth_error_maps_unauthorized() {
        let err = AuthError::Unauthorized("no admin".into());
        let camel_err: CamelError = err.into();
        assert!(matches!(camel_err, CamelError::Unauthorized(s) if s.contains("no admin")));
    }

    #[test]
    fn auth_error_maps_provider_unavailable() {
        let err = AuthError::ProviderUnavailable("jwks down".into());
        let camel_err: CamelError = err.into();
        assert!(matches!(camel_err, CamelError::ProcessorError(s) if s.contains("jwks down")));
    }

    #[test]
    fn auth_error_maps_config_error() {
        let err = AuthError::ConfigError("bad config".into());
        let camel_err: CamelError = err.into();
        assert!(matches!(camel_err, CamelError::Config(s) if s.contains("bad config")));
    }
}
