use async_trait::async_trait;
use camel_api::CamelError;
use camel_api::security_policy::{Principal, TransportId};

use crate::jwt::JwtValidator;

/// Per-request authentication context: the token plus the route/provider
/// audience and issuer constraints to enforce.
///
/// `audiences` and `accepted_issuers` are the request-scoped sets. When either
/// is non-empty the authenticator enforces it (REPLACEMENT semantics, bypassing
/// constructor-fixed checks); when both are empty the authenticator falls back
/// to its constructor-fixed behavior.
pub struct AuthnRequest<'a> {
    pub token: &'a str,
    pub audiences: &'a [String],
    pub accepted_issuers: &'a [String],
    pub transport: TransportId,
}

/// Separates authentication (token → Principal) from authorization (SecurityPolicy check).
///
/// Provides a blanket implementation for any [`JwtValidator`], converting
/// provider-specific [`AuthError`](crate::types::AuthError) variants into
/// domain-level [`CamelError`] variants.
#[async_trait]
pub trait TokenAuthenticator: Send + Sync {
    /// Authenticate a Bearer token and return the associated [`Principal`].
    async fn authenticate_bearer(&self, token: &str) -> Result<Principal, CamelError>;

    /// Authenticate a token against per-request audience/issuer constraints.
    ///
    /// The default delegates to [`authenticate_bearer`](Self::authenticate_bearer),
    /// preserving constructor-fixed behavior for implementors that do not
    /// distinguish request-scoped constraints.
    async fn authenticate(&self, req: AuthnRequest<'_>) -> Result<Principal, CamelError> {
        self.authenticate_bearer(req.token).await
    }
}

#[async_trait]
impl<T: JwtValidator> TokenAuthenticator for T {
    async fn authenticate_bearer(&self, token: &str) -> Result<Principal, CamelError> {
        self.validate(token).await.map_err(CamelError::from)
    }

    async fn authenticate(&self, req: AuthnRequest<'_>) -> Result<Principal, CamelError> {
        // No request-scoped constraints → constructor-fixed behavior via delegation.
        if req.audiences.is_empty() && req.accepted_issuers.is_empty() {
            return self.authenticate_bearer(req.token).await;
        }

        // REPLACEMENT semantics: signature-only verification, then enforce the
        // request's issuer/audience sets (constructor-fixed checks bypassed).
        let principal = self
            .validate_signature(req.token)
            .await
            .map_err(CamelError::from)?;

        if !req.accepted_issuers.is_empty()
            && !req.accepted_issuers.iter().any(|i| i == &principal.issuer)
        {
            return Err(CamelError::Unauthenticated(format!(
                "token issuer {:?} not in accepted set", // allow-secret
                principal.issuer
            )));
        }

        if !req.audiences.is_empty()
            && !principal.audience.iter().any(|a| req.audiences.contains(a))
        {
            return Err(CamelError::Unauthenticated(format!(
                "token audience {:?} not in accepted set", // allow-secret
                principal.audience
            )));
        }

        Ok(principal)
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

    // --- Task 3.1: per-request audience/issuer enforcement ---

    use crate::claims::{ClaimPaths, JsonPointerClaimsMapper};
    use crate::jwks::{Jwk, JwksProvider};
    use crate::jwt::LocalJwtValidator;
    use std::sync::Arc;

    static TEST_RSA_PRIVATE_PEM: &[u8] = include_bytes!("../tests/fixtures/test_rsa_private.pem");
    static TEST_RSA_PUBLIC_PEM: &[u8] = include_bytes!("../tests/fixtures/test_rsa_public.pem");

    struct MockJwks {
        kid: String,
        public_pem: &'static [u8],
    }

    #[async_trait]
    impl JwksProvider for MockJwks {
        async fn get_signing_keys(&self) -> Result<Vec<Jwk>, AuthError> {
            Ok(vec![Jwk {
                kid: self.kid.clone(),
                kty: "RSA".into(),
                alg: Some("RS256".into()),
                r#use: None,
                n: String::from_utf8_lossy(self.public_pem).into_owned(),
                e: "AQAB".into(),
            }])
        }

        async fn refresh(&self) -> Result<(), AuthError> {
            Ok(())
        }
    }

    fn jwt_validator(audience: Vec<&str>, issuer: &str) -> LocalJwtValidator {
        let mapper = Arc::new(JsonPointerClaimsMapper::new(ClaimPaths {
            subject: "/sub".into(),
            roles: vec!["/groups".into()],
            scopes: Some("/scope".into()),
        }));
        LocalJwtValidator::new(
            audience.iter().map(|s| s.to_string()).collect(),
            issuer.to_string(),
            Arc::new(MockJwks {
                kid: "test-key".into(),
                public_pem: TEST_RSA_PUBLIC_PEM,
            }),
            mapper,
        )
    }

    fn make_token(kid: &str, claims: &serde_json::Value) -> String {
        let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
        header.kid = Some(kid.to_string());
        let encoding_key = jsonwebtoken::EncodingKey::from_rsa_pem(TEST_RSA_PRIVATE_PEM).unwrap();
        jsonwebtoken::encode(&header, claims, &encoding_key).unwrap()
    }

    fn valid_claims(iss: &str, aud: &str) -> serde_json::Value {
        let now = chrono::Utc::now().timestamp() as u64;
        json!({
            "sub": "user-1",
            "iss": iss,
            "aud": aud,
            "exp": now + 3600,
            "iat": now,
        })
    }

    fn req<'a>(token: &'a str, audiences: &'a [String], issuers: &'a [String]) -> AuthnRequest<'a> {
        AuthnRequest {
            token,
            audiences,
            accepted_issuers: issuers,
            transport: TransportId::Http,
        }
    }

    #[tokio::test]
    async fn issuer_not_accepted_rejects() {
        let v = jwt_validator(vec!["api"], "https://a");
        let token = make_token("test-key", &valid_claims("https://b", "api"));
        let err = v
            .authenticate(req(
                &token,
                &["api".to_string()],
                &["https://a".to_string()],
            ))
            .await
            .unwrap_err();
        assert!(matches!(err, CamelError::Unauthenticated(_)));
    }

    #[tokio::test]
    async fn audience_mismatch_rejects() {
        let v = jwt_validator(vec!["api"], "https://a");
        let token = make_token("test-key", &valid_claims("https://a", "api-b"));
        let err = v
            .authenticate(req(
                &token,
                &["api-a".to_string(), "api-c".to_string()],
                &["https://a".to_string()],
            ))
            .await
            .unwrap_err();
        assert!(matches!(err, CamelError::Unauthenticated(_)));
    }

    #[tokio::test]
    async fn request_audience_overrides_constructor_default() {
        // Constructor-fixed audience ["api"]; request audiences ["api-2"]; token aud "api-2".
        let v = jwt_validator(vec!["api"], "https://a");
        let token = make_token("test-key", &valid_claims("https://a", "api-2"));

        // Request-scoped check active → fixed check bypassed → OK.
        let principal = v
            .authenticate(req(
                &token,
                &["api-2".to_string()],
                &["https://a".to_string()],
            ))
            .await
            .unwrap();
        assert_eq!(principal.subject, "user-1");

        // Empty request audiences → constructor behavior via delegation (fixed
        // check rejects aud "api-2" against constructor audience ["api"]).
        let err = v.authenticate(req(&token, &[], &[])).await.unwrap_err();
        assert!(matches!(err, CamelError::Unauthenticated(_)));
    }

    #[tokio::test]
    async fn default_delegation_backcompat() {
        struct OnlyBearer;
        #[async_trait]
        impl TokenAuthenticator for OnlyBearer {
            async fn authenticate_bearer(&self, _token: &str) -> Result<Principal, CamelError> {
                Ok(test_principal())
            }
        }
        let principal = OnlyBearer
            .authenticate(req("tok", &["api".to_string()], &["https://a".to_string()]))
            .await
            .unwrap();
        assert_eq!(principal.subject, "user1");
    }

    #[tokio::test]
    async fn empty_issuers_accepts_any() {
        let v = jwt_validator(vec!["api"], "https://a");
        let token = make_token("test-key", &valid_claims("https://anything", "api"));
        let principal = v
            .authenticate(req(&token, &["api".to_string()], &[]))
            .await
            .unwrap();
        assert_eq!(principal.subject, "user-1");
    }

    #[tokio::test]
    async fn empty_constructor_audience_fails_closed() {
        // Constructor audience empty; empty request sets → delegation path
        // (fixed-claims check). Empty constructor audience must reject
        // (fail-closed), not authenticate with zero audience scoping.
        let v = jwt_validator(vec![], "https://a");
        let token = make_token("test-key", &valid_claims("https://a", "api"));
        let err = v.authenticate(req(&token, &[], &[])).await.unwrap_err();
        assert!(matches!(err, CamelError::Unauthenticated(_)));
    }

    #[tokio::test]
    async fn empty_constructor_issuer_fails_closed() {
        // Issuer direction of the same invariant: empty constructor issuer
        // on the delegation path must reject, not accept any issuer.
        let v = jwt_validator(vec!["api"], "");
        let token = make_token("test-key", &valid_claims("https://a", "api"));
        let err = v.authenticate(req(&token, &[], &[])).await.unwrap_err();
        assert!(matches!(err, CamelError::Unauthenticated(_)));
    }

    #[tokio::test]
    async fn empty_constructor_audience_replaced_by_request_audiences() {
        // Constructor audience empty; non-empty request audiences → REPLACEMENT
        // path (signature-only + request checks). The request set replaces the
        // constructor-fixed check, so a matching request audience grants.
        let v = jwt_validator(vec![], "https://a");
        let token = make_token("test-key", &valid_claims("https://a", "api"));
        let principal = v
            .authenticate(req(
                &token,
                &["api".to_string()],
                &["https://a".to_string()],
            ))
            .await
            .unwrap();
        assert_eq!(principal.subject, "user-1");
    }
}
