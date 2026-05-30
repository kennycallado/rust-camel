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
