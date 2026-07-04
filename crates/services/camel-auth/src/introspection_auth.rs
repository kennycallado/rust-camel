use std::sync::Arc;

use async_trait::async_trait;
use camel_api::CamelError;
use camel_api::security_policy::Principal;

use crate::claims::ClaimsMapper;
use crate::introspection::TokenIntrospector;
use crate::token_authenticator::TokenAuthenticator;
use crate::types::AuthError;

pub struct IntrospectionAuthenticator {
    introspector: Arc<dyn TokenIntrospector>,
    claims_mapper: Arc<dyn ClaimsMapper>,
}

impl IntrospectionAuthenticator {
    pub fn new(
        introspector: Arc<dyn TokenIntrospector>,
        claims_mapper: Arc<dyn ClaimsMapper>,
    ) -> Self {
        Self {
            introspector,
            claims_mapper,
        }
    }
}

#[async_trait]
impl TokenAuthenticator for IntrospectionAuthenticator {
    async fn authenticate_bearer(&self, token: &str) -> Result<Principal, CamelError> {
        let result = self.introspector.introspect(token).await?;
        if !result.active {
            return Err(AuthError::TokenInvalid("token is not active".into()).into());
        }

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(u64::MAX);

        if let Some(exp) = result.exp
            && exp < now
        {
            return Err(AuthError::TokenExpired.into());
        }

        if let Some(nbf) = result.nbf
            && nbf > now
        {
            return Err(AuthError::TokenInvalid(
                "token not yet valid (introspection nbf check)".into(),
            )
            .into());
        }

        let claims = serde_json::to_value(&result).map_err(|e| {
            AuthError::ConfigError(format!("introspection result serialization failed: {e}"))
        })?;
        self.claims_mapper
            .to_principal(&claims)
            .map_err(CamelError::from)
    }
}

impl std::fmt::Debug for IntrospectionAuthenticator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IntrospectionAuthenticator")
            .field("introspector", &"<TokenIntrospector>")
            .field("claims_mapper", &"<ClaimsMapper>")
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::claims::{ClaimPaths, JsonPointerClaimsMapper};
    use crate::introspection::IntrospectionResult;
    use crate::types::AuthError;
    use serde_json::{Map, json};

    struct MockIntrospector {
        result: IntrospectionResult,
    }

    #[async_trait]
    impl TokenIntrospector for MockIntrospector {
        async fn introspect(&self, _token: &str) -> Result<IntrospectionResult, AuthError> {
            Ok(self.result.clone())
        }
    }

    fn keycloak_mapper() -> Arc<dyn ClaimsMapper> {
        let paths = ClaimPaths {
            subject: "/sub".into(),
            roles: vec![
                "/realm_access/roles".into(),
                "/resource_access/my-client/roles".into(),
            ],
            scopes: Some("/scope".into()),
        };
        Arc::new(JsonPointerClaimsMapper::new(paths))
    }

    #[tokio::test]
    async fn active_token_maps_to_principal() {
        let introspector = MockIntrospector {
            result: IntrospectionResult {
                active: true,
                sub: Some("user-1".into()),
                exp: None,
                iat: None,
                nbf: None,
                scope: Some("read write".into()),
                client_id: None,
                token_type: None,
                iss: Some("https://kc.example.com/realms/test".into()),
                aud: None,
                extra: {
                    let mut m = Map::new();
                    m.insert("realm_access".into(), json!({"roles": ["admin", "user"]}));
                    m.insert(
                        "resource_access".into(),
                        json!({"my-client": {"roles": ["client-role"]}}),
                    );
                    m
                },
            },
        };
        let auth = IntrospectionAuthenticator::new(Arc::new(introspector), keycloak_mapper());
        let principal = auth.authenticate_bearer("opaque-token").await.unwrap();
        assert_eq!(principal.subject, "user-1");
        assert!(principal.has_role("admin"));
        assert!(principal.has_role("client-role"));
        assert_eq!(principal.scopes, vec!["read", "write"]);
    }

    #[tokio::test]
    async fn inactive_token_returns_unauthenticated() {
        let introspector = MockIntrospector {
            result: IntrospectionResult {
                active: false,
                sub: None,
                exp: None,
                iat: None,
                nbf: None,
                scope: None,
                client_id: None,
                token_type: None,
                iss: None,
                aud: None,
                extra: Map::new(),
            },
        };
        let auth = IntrospectionAuthenticator::new(Arc::new(introspector), keycloak_mapper());
        let err = auth.authenticate_bearer("dead-token").await.unwrap_err();
        match err {
            CamelError::Unauthenticated(msg) => assert!(msg.contains("not active")),
            other => panic!("expected Unauthenticated, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn introspection_provider_error_propagates() {
        struct FailingIntrospector;
        #[async_trait]
        impl TokenIntrospector for FailingIntrospector {
            async fn introspect(&self, _token: &str) -> Result<IntrospectionResult, AuthError> {
                Err(AuthError::ProviderUnavailable("connection refused".into()))
            }
        }
        let auth =
            IntrospectionAuthenticator::new(Arc::new(FailingIntrospector), keycloak_mapper());
        let err = auth.authenticate_bearer("tok").await.unwrap_err();
        match err {
            CamelError::ProcessorError(msg) => {
                assert!(msg.contains("auth provider unavailable"));
            }
            other => panic!("expected ProcessorError, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn introspection_rejects_expired_token() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        struct ExpiredIntrospector(u64);
        #[async_trait]
        impl TokenIntrospector for ExpiredIntrospector {
            async fn introspect(&self, _token: &str) -> Result<IntrospectionResult, AuthError> {
                Ok(IntrospectionResult {
                    active: true,
                    sub: Some("user-1".into()),
                    exp: Some(self.0 - 3600), // expired 1h ago
                    iat: None,
                    nbf: None,
                    scope: Some("read".into()),
                    client_id: None,
                    token_type: None,
                    iss: None,
                    aud: None,
                    extra: Map::new(),
                })
            }
        }

        let auth =
            IntrospectionAuthenticator::new(Arc::new(ExpiredIntrospector(now)), keycloak_mapper());
        let err = auth.authenticate_bearer("test-token").await.unwrap_err();
        match err {
            CamelError::Unauthenticated(msg) => {
                assert!(
                    msg.contains("expired"),
                    "expected 'expired' in error, got: {msg}"
                )
            }
            other => panic!("expected Unauthenticated, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn introspection_rejects_not_yet_valid_token() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        struct FutureIntrospector(u64);
        #[async_trait]
        impl TokenIntrospector for FutureIntrospector {
            async fn introspect(&self, _token: &str) -> Result<IntrospectionResult, AuthError> {
                Ok(IntrospectionResult {
                    active: true,
                    sub: Some("user-1".into()),
                    exp: Some(self.0 + 7200), // valid for 2h
                    iat: None,
                    nbf: Some(self.0 + 3600), // not valid for 1h
                    scope: Some("read".into()),
                    client_id: None,
                    token_type: None,
                    iss: None,
                    aud: None,
                    extra: Map::new(),
                })
            }
        }

        let auth =
            IntrospectionAuthenticator::new(Arc::new(FutureIntrospector(now)), keycloak_mapper());
        let err = auth.authenticate_bearer("test-token").await.unwrap_err();
        match err {
            CamelError::Unauthenticated(msg) => {
                assert!(
                    msg.contains("not yet valid"),
                    "expected 'not yet valid' in error, got: {msg}"
                )
            }
            other => panic!("expected Unauthenticated, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn accepts_token_with_valid_exp_and_nbf() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        struct ValidIntrospector(u64);
        #[async_trait]
        impl TokenIntrospector for ValidIntrospector {
            async fn introspect(&self, _token: &str) -> Result<IntrospectionResult, AuthError> {
                Ok(IntrospectionResult {
                    active: true,
                    sub: Some("user-1".into()),
                    exp: Some(self.0 + 3600), // valid for 1h
                    iat: None,
                    nbf: Some(self.0 - 3600), // was valid 1h ago
                    scope: Some("read".into()),
                    client_id: None,
                    token_type: None,
                    iss: None,
                    aud: None,
                    extra: {
                        let mut m = Map::new();
                        m.insert("realm_access".into(), json!({"roles": ["user"]}));
                        m
                    },
                })
            }
        }

        let auth =
            IntrospectionAuthenticator::new(Arc::new(ValidIntrospector(now)), keycloak_mapper());
        let principal = auth.authenticate_bearer("test-token").await.unwrap();
        assert_eq!(principal.subject, "user-1");
        assert!(principal.has_role("user"));
    }

    #[tokio::test]
    async fn missing_subject_returns_token_invalid() {
        let introspector = MockIntrospector {
            result: IntrospectionResult {
                active: true,
                sub: None,
                exp: None,
                iat: None,
                nbf: None,
                scope: None,
                client_id: None,
                token_type: None,
                iss: None,
                aud: None,
                extra: Map::new(),
            },
        };
        let auth = IntrospectionAuthenticator::new(Arc::new(introspector), keycloak_mapper());
        let err = auth.authenticate_bearer("tok").await.unwrap_err();
        match err {
            CamelError::Unauthenticated(msg) => assert!(msg.contains("subject")),
            other => panic!("expected Unauthenticated, got: {other:?}"),
        }
    }
}
