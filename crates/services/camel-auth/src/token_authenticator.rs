use async_trait::async_trait;
use camel_api::CamelError;
use camel_api::security_policy::Principal;

use crate::jwt::JwtValidator;

/// Separates authentication (token → Principal) from authorization (SecurityPolicy check).
///
/// Provides a blanket implementation for any [`JwtValidator`], converting
/// provider-specific [`AuthError`](crate::types::AuthError) variants into
/// domain-level [`CamelError`] variants.
#[async_trait]
pub trait TokenAuthenticator: Send + Sync {
    /// Authenticate a Bearer token and return the associated [`Principal`].
    async fn authenticate_bearer(&self, token: &str) -> Result<Principal, CamelError>;
}

#[async_trait]
impl<T: JwtValidator> TokenAuthenticator for T {
    async fn authenticate_bearer(&self, token: &str) -> Result<Principal, CamelError> {
        self.validate(token).await.map_err(CamelError::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::AuthError;
    use serde_json::json;

    struct MockValidator {
        principal: Option<Principal>,
        should_fail: bool,
    }

    #[async_trait]
    impl JwtValidator for MockValidator {
        async fn validate(&self, _token: &str) -> Result<Principal, AuthError> {
            if self.should_fail {
                return Err(AuthError::TokenInvalid("bad token".into()));
            }
            self.principal
                .clone()
                .ok_or_else(|| AuthError::TokenInvalid("no principal".into()))
        }
    }

    fn test_principal() -> Principal {
        Principal {
            subject: "user1".into(),
            issuer: "test-issuer".into(),
            audience: vec!["api".into()],
            scopes: vec!["read".into()],
            roles: vec!["admin".into()],
            claims: json!({"sub": "user1"}),
        }
    }

    #[tokio::test]
    async fn test_authenticate_bearer_success() {
        let validator = MockValidator {
            principal: Some(test_principal()),
            should_fail: false,
        };
        let result = validator.authenticate_bearer("valid-token").await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().subject, "user1");
    }

    #[tokio::test]
    async fn test_authenticate_bearer_invalid_token() {
        let validator = MockValidator {
            principal: None,
            should_fail: true,
        };
        let err = validator.authenticate_bearer("bad").await.unwrap_err();
        match err {
            CamelError::Unauthenticated(msg) => assert!(msg.contains("bad token")),
            _ => panic!("expected Unauthenticated, got: {err:?}"),
        }
    }

    #[tokio::test]
    async fn test_authenticate_bearer_provider_unavailable() {
        struct UnavailableValidator;
        #[async_trait]
        impl JwtValidator for UnavailableValidator {
            async fn validate(&self, _token: &str) -> Result<Principal, AuthError> {
                Err(AuthError::ProviderUnavailable("connection refused".into()))
            }
        }
        let err = UnavailableValidator
            .authenticate_bearer("token")
            .await
            .unwrap_err();
        match err {
            CamelError::ProcessorError(msg) => assert!(msg.contains("auth provider unavailable")),
            _ => panic!("expected ProcessorError, got: {err:?}"),
        }
    }

    #[tokio::test]
    async fn test_authenticate_bearer_token_expired() {
        struct ExpiredValidator;
        #[async_trait]
        impl JwtValidator for ExpiredValidator {
            async fn validate(&self, _token: &str) -> Result<Principal, AuthError> {
                Err(AuthError::TokenExpired)
            }
        }
        let err = ExpiredValidator
            .authenticate_bearer("expired-token")
            .await
            .unwrap_err();
        match err {
            CamelError::Unauthenticated(msg) => assert!(msg.contains("token expired")),
            _ => panic!("expected Unauthenticated, got: {err:?}"),
        }
    }

    #[tokio::test]
    async fn test_authenticate_bearer_unauthorized() {
        struct UnauthorizedValidator;
        #[async_trait]
        impl JwtValidator for UnauthorizedValidator {
            async fn validate(&self, _token: &str) -> Result<Principal, AuthError> {
                Err(AuthError::Unauthorized("insufficient permissions".into()))
            }
        }
        let err = UnauthorizedValidator
            .authenticate_bearer("token")
            .await
            .unwrap_err();
        match err {
            CamelError::Unauthorized(msg) => assert!(msg.contains("insufficient permissions")),
            _ => panic!("expected Unauthorized, got: {err:?}"),
        }
    }

    #[tokio::test]
    async fn test_authenticate_bearer_config_error() {
        struct ConfigErrorValidator;
        #[async_trait]
        impl JwtValidator for ConfigErrorValidator {
            async fn validate(&self, _token: &str) -> Result<Principal, AuthError> {
                Err(AuthError::ConfigError("missing issuer".into()))
            }
        }
        let err = ConfigErrorValidator
            .authenticate_bearer("token")
            .await
            .unwrap_err();
        match err {
            CamelError::Config(msg) => assert!(msg.contains("missing issuer")),
            _ => panic!("expected Config, got: {err:?}"),
        }
    }
}
