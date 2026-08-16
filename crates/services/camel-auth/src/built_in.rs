use async_trait::async_trait;
use std::sync::Arc;

use camel_api::security_policy::{
    AuthorizationDecision, CredentialSource, Principal, SecurityPolicy, principal_from_exchange,
    store_principal_properties,
};
use camel_api::{CamelError, Exchange};

use crate::credential_source::extract_token_from_exchange;
use crate::token_authenticator::TokenAuthenticator;

/// Property key used to store the authenticated principal in the exchange.
///
/// Re-exported from the camel-api contract so `camel_auth::built_in::PRINCIPAL_KEY`
/// and `camel_api::security_policy::PRINCIPAL_KEY` are the same constant.
pub use camel_api::security_policy::PRINCIPAL_KEY;

/// Extracts and validates a credential from the route-declared sources.
///
/// If a source yields a token, validates it via the supplied [`TokenAuthenticator`] and stores
/// the resulting [`Principal`] in `PRINCIPAL_KEY` for downstream processors.
///
/// If no source yields a token, the behavior depends on
/// `trust_upstream_principal`:
/// - `true`: falls back to an already-populated principal in the exchange
///   (e.g. set by an upstream authentication filter). **Spoofable** unless
///   the route topology guarantees property integrity.
/// - `false` (default): returns `Unauthenticated` — fail-closed. Use this
///   unless the deployment explicitly trusts an upstream producer to
///   authenticate and stamp the principal property.
async fn authenticate(
    exchange: &mut Exchange,
    authenticator: &dyn TokenAuthenticator,
    trust_upstream_principal: bool,
    sources: &[CredentialSource],
) -> Result<Principal, CamelError> {
    // Extraction returns an owned token, so the borrow on `exchange` ends before the mut borrow.
    let token = extract_token_from_exchange(exchange, sources).map(|extracted| extracted.token);

    if let Some(token) = token {
        let principal = authenticator.authenticate_bearer(&token).await?;
        // Store for downstream processors
        store_principal_properties(exchange, &principal);
        return Ok(principal);
    }

    if trust_upstream_principal {
        extract_principal_from_exchange(exchange)
    } else {
        Err(CamelError::Unauthenticated(
            "no Bearer token and trust_upstream_principal is false".into(),
        ))
    }
}

/// Extract a `Principal` from exchange properties, returning `Unauthenticated` if absent.
///
/// Delegates to the canonical `principal_from_exchange` reader so the trust
/// branch consumes the same JSON-string format that `store_principal_properties`
/// writes (a `from_value` read of a `to_string`-stored value silently missed the
/// principal and turned every `trust_upstream_principal` grant into a 500).
fn extract_principal_from_exchange(exchange: &Exchange) -> Result<Principal, CamelError> {
    principal_from_exchange(exchange)
        .ok_or_else(|| CamelError::Unauthenticated("no principal in exchange".into()))
}

/// Role-based access control policy.
///
/// Validates the incoming request via a token authenticator (Bearer token) and evaluates whether
/// the principal holds the required roles.
/// When `all_required` is true, every listed role must be present.
/// When `all_required` is false, at least one listed role must be present.
pub struct RolePolicy {
    required_roles: Vec<String>,
    all_required: bool,
    /// When `true`, fall back to the `camel.auth.principal` exchange property
    /// if no Bearer token is present. Default `false` (fail-closed) — see
    /// H1 in `docs/superpowers/specs/v1-sec-stabilization-spec.md`.
    trust_upstream_principal: bool,
    authenticator: Arc<dyn TokenAuthenticator>,
    credential_sources: Vec<CredentialSource>,
}

impl RolePolicy {
    pub fn new(
        required_roles: Vec<String>,
        all_required: bool,
        trust_upstream_principal: bool,
        authenticator: Arc<dyn TokenAuthenticator>,
        credential_sources: Vec<CredentialSource>,
    ) -> Self {
        Self {
            required_roles,
            all_required,
            trust_upstream_principal,
            authenticator,
            credential_sources,
        }
    }

    /// Credential sources the policy extracts tokens from, in declared order.
    pub fn credential_sources(&self) -> &[CredentialSource] {
        &self.credential_sources
    }
}

#[async_trait]
impl SecurityPolicy for RolePolicy {
    async fn evaluate(&self, exchange: &mut Exchange) -> Result<AuthorizationDecision, CamelError> {
        let principal = authenticate(
            exchange,
            &*self.authenticator,
            self.trust_upstream_principal,
            &self.credential_sources,
        )
        .await?;

        let missing: Vec<String> = self
            .required_roles
            .iter()
            .filter(|r| !principal.has_role(r))
            .cloned()
            .collect();

        let granted = if self.all_required {
            missing.is_empty()
        } else {
            self.required_roles.is_empty() || missing.len() < self.required_roles.len()
        };

        if granted {
            Ok(AuthorizationDecision::Granted { principal })
        } else {
            let actual = principal.roles.clone();
            Ok(AuthorizationDecision::Denied {
                reason: format!("missing required role(s): {}", missing.join(", ")), // allow-secret
                required: self.required_roles.clone(),
                actual,
            })
        }
    }
}

/// Scope-based access control policy.
///
/// Validates the incoming request via a token authenticator (Bearer token) and evaluates whether
/// the principal holds the required scopes.
/// When `all_required` is true, every listed scope must be present.
/// When `all_required` is false, at least one listed scope must be present.
pub struct ScopePolicy {
    required_scopes: Vec<String>,
    all_required: bool,
    /// When `true`, fall back to the `camel.auth.principal` exchange property
    /// if no Bearer token is present. Default `false` (fail-closed) — see
    /// H1 in `docs/superpowers/specs/v1-sec-stabilization-spec.md`.
    trust_upstream_principal: bool,
    authenticator: Arc<dyn TokenAuthenticator>,
    credential_sources: Vec<CredentialSource>,
}

impl ScopePolicy {
    pub fn new(
        required_scopes: Vec<String>,
        all_required: bool,
        trust_upstream_principal: bool,
        authenticator: Arc<dyn TokenAuthenticator>,
        credential_sources: Vec<CredentialSource>,
    ) -> Self {
        Self {
            required_scopes,
            all_required,
            trust_upstream_principal,
            authenticator,
            credential_sources,
        }
    }

    /// Credential sources the policy extracts tokens from, in declared order.
    pub fn credential_sources(&self) -> &[CredentialSource] {
        &self.credential_sources
    }
}

#[async_trait]
impl SecurityPolicy for ScopePolicy {
    async fn evaluate(&self, exchange: &mut Exchange) -> Result<AuthorizationDecision, CamelError> {
        let principal = authenticate(
            exchange,
            &*self.authenticator,
            self.trust_upstream_principal,
            &self.credential_sources,
        )
        .await?;

        let missing: Vec<String> = self
            .required_scopes
            .iter()
            .filter(|s| !principal.has_scope(s))
            .cloned()
            .collect();

        let granted = if self.all_required {
            missing.is_empty()
        } else {
            self.required_scopes.is_empty() || missing.len() < self.required_scopes.len()
        };

        if granted {
            Ok(AuthorizationDecision::Granted { principal })
        } else {
            let actual = principal.scopes.clone();
            Ok(AuthorizationDecision::Denied {
                reason: format!("missing required scope(s): {}", missing.join(", ")),
                required: self.required_scopes.clone(),
                actual,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jwt::JwtValidator;
    use crate::native_auth::{
        NativeCredential, NativeCredentialSecret, NativeCredentialStore, StaticTokenAuthenticator,
    };
    use crate::types::AuthError;
    use camel_api::Message;

    fn test_principal(roles: Vec<&str>, scopes: Vec<&str>) -> Principal {
        Principal {
            subject: "test-user".into(),
            issuer: "test".into(),
            audience: vec![],
            roles: roles.iter().map(|s| s.to_string()).collect(),
            scopes: scopes.iter().map(|s| s.to_string()).collect(),
            claims: serde_json::Value::Null,
        }
    }

    /// Mock validator that returns a fixed principal regardless of token content.
    struct MockJwtValidator {
        principal: Principal,
    }

    #[async_trait]
    impl JwtValidator for MockJwtValidator {
        async fn validate(&self, _token: &str) -> Result<Principal, AuthError> {
            Ok(self.principal.clone())
        }
    }

    fn mock_validator(principal: Principal) -> Arc<dyn TokenAuthenticator> {
        Arc::new(MockJwtValidator { principal })
    }

    /// Build a static authenticator over a native store seeded with `credential`.
    fn store_seeded_authenticator(
        credential: &str,
        principal: Principal,
    ) -> Arc<dyn TokenAuthenticator> {
        let store = NativeCredentialStore::try_new(vec![NativeCredential {
            secret: NativeCredentialSecret::Plaintext {
                value: zeroize::Zeroizing::new(credential.to_string()),
            },
            principal,
        }])
        .unwrap();
        Arc::new(StaticTokenAuthenticator::new(store))
    }

    /// Build an exchange with a Bearer token in the Authorization header.
    fn exchange_with_bearer(principal: Principal) -> Exchange {
        let mut msg = Message::default();
        msg.set_header(
            "Authorization",
            serde_json::Value::String("Bearer mock-token".into()),
        );
        // Also embed principal in exchange so fallback path is testable if needed.
        let mut ex = Exchange::new(msg);
        store_principal_properties(&mut ex, &principal);
        ex
    }

    /// Build an exchange with the principal in the exchange property (no Bearer header).
    ///
    /// Uses the canonical writer so the property format matches what the WS
    /// component produces and what the trust branch reads.
    fn exchange_with_principal(principal: Principal) -> Exchange {
        let mut ex = Exchange::new(Message::default());
        store_principal_properties(&mut ex, &principal);
        ex
    }

    #[tokio::test]
    async fn role_policy_grants_when_role_present() {
        let principal = test_principal(vec!["admin"], vec![]);
        let policy = RolePolicy::new(
            vec!["admin".into()],
            true,
            false,
            mock_validator(principal.clone()),
            vec![CredentialSource::AuthorizationHeader],
        );
        let mut ex = exchange_with_bearer(principal);
        let decision = policy.evaluate(&mut ex).await.unwrap();
        assert!(matches!(decision, AuthorizationDecision::Granted { .. }));
    }

    #[tokio::test]
    async fn role_policy_denies_when_role_missing() {
        let principal = test_principal(vec!["user"], vec![]);
        let policy = RolePolicy::new(
            vec!["admin".into()],
            true,
            false,
            mock_validator(principal.clone()),
            vec![CredentialSource::AuthorizationHeader],
        );
        let mut ex = exchange_with_bearer(principal);
        let decision = policy.evaluate(&mut ex).await.unwrap();
        assert!(matches!(decision, AuthorizationDecision::Denied { .. }));
    }

    #[tokio::test]
    async fn role_policy_any_required() {
        let principal = test_principal(vec!["user"], vec![]);
        let policy = RolePolicy::new(
            vec!["admin".into(), "user".into()],
            false,
            false,
            mock_validator(principal.clone()),
            vec![CredentialSource::AuthorizationHeader],
        );
        let mut ex = exchange_with_bearer(principal);
        let decision = policy.evaluate(&mut ex).await.unwrap();
        assert!(matches!(decision, AuthorizationDecision::Granted { .. }));
    }

    #[tokio::test]
    async fn scope_policy_grants() {
        let principal = test_principal(vec![], vec!["read"]);
        let policy = ScopePolicy::new(
            vec!["read".into()],
            true,
            false,
            mock_validator(principal.clone()),
            vec![CredentialSource::AuthorizationHeader],
        );
        let mut ex = exchange_with_bearer(principal);
        let decision = policy.evaluate(&mut ex).await.unwrap();
        assert!(matches!(decision, AuthorizationDecision::Granted { .. }));
    }

    #[tokio::test]
    async fn unauthenticated_when_no_principal_and_no_header() {
        // No Bearer header, no exchange property — validator never called
        struct FailValidator;
        #[async_trait]
        impl JwtValidator for FailValidator {
            async fn validate(&self, _token: &str) -> Result<Principal, AuthError> {
                panic!("should not be called")
            }
        }
        let policy = RolePolicy::new(
            vec!["admin".into()],
            true,
            false,
            Arc::new(FailValidator),
            vec![CredentialSource::AuthorizationHeader],
        );
        let mut ex = Exchange::new(Message::default());
        let result = policy.evaluate(&mut ex).await;
        assert!(matches!(result, Err(CamelError::Unauthenticated(_))));
    }

    #[tokio::test]
    async fn principal_fallback_denied_by_default() {
        // No Bearer header, but principal pre-populated (upstream filter scenario).
        // Without `trust_upstream_principal` opt-in, MUST be denied.
        let principal = test_principal(vec!["admin"], vec![]);
        let policy = RolePolicy::new(
            vec!["admin".into()],
            true,
            false, // trust_upstream_principal
            mock_validator(principal.clone()),
            vec![CredentialSource::AuthorizationHeader],
        );
        let mut ex = exchange_with_principal(principal); // no Authorization header
        let result = policy.evaluate(&mut ex).await;
        assert!(matches!(result, Err(CamelError::Unauthenticated(_))));
    }

    #[tokio::test]
    async fn principal_fallback_allowed_with_opt_in() {
        // Same setup but `trust_upstream_principal=true` allows upstream-set principal.
        let principal = test_principal(vec!["admin"], vec![]);
        let policy = RolePolicy::new(
            vec!["admin".into()],
            true,
            true, // trust_upstream_principal
            mock_validator(principal.clone()),
            vec![CredentialSource::AuthorizationHeader],
        );
        let mut ex = exchange_with_principal(principal); // no Authorization header
        let decision = policy.evaluate(&mut ex).await.unwrap();
        assert!(matches!(decision, AuthorizationDecision::Granted { .. }));
    }

    #[tokio::test]
    async fn scope_policy_fallback_denied_by_default() {
        // No Bearer header, principal pre-populated — Scopes policy also gates.
        let principal = test_principal(vec![], vec!["read"]);
        let policy = ScopePolicy::new(
            vec!["read".into()],
            true,
            false, // trust_upstream_principal
            mock_validator(principal.clone()),
            vec![CredentialSource::AuthorizationHeader],
        );
        let mut ex = exchange_with_principal(principal);
        let result = policy.evaluate(&mut ex).await;
        assert!(matches!(result, Err(CamelError::Unauthenticated(_))));
    }

    #[test]
    fn role_policy_constructor_accepts_sources() {
        let authenticator = mock_validator(test_principal(vec![], vec![]));
        let sources = vec![CredentialSource::Cookie { name: "s".into() }];
        let policy = RolePolicy::new(
            vec!["r".into()],
            true,
            false,
            authenticator,
            sources.clone(),
        );
        assert_eq!(policy.credential_sources(), sources.as_slice());
    }

    // --- Task 1.2: multi-source extraction over the Exchange ---

    #[tokio::test]
    async fn authenticate_default_equals_bearer_prefix_strip() {
        // The pre-change path stripped "Bearer " from the Authorization header.
        // A real store seeded with the bare token proves the strip: a no-strip
        // or double-strip regression would miss the lookup and fail here.
        let principal = test_principal(vec!["admin"], vec![]);
        let store = NativeCredentialStore::try_new(vec![NativeCredential {
            secret: NativeCredentialSecret::Plaintext {
                value: zeroize::Zeroizing::new("mock-token".to_string()),
            },
            principal: principal.clone(),
        }])
        .unwrap();
        let authenticator: Arc<dyn TokenAuthenticator> =
            Arc::new(StaticTokenAuthenticator::new(store));
        let policy = RolePolicy::new(
            vec!["admin".into()],
            true,
            false,
            authenticator,
            vec![CredentialSource::AuthorizationHeader],
        );
        let mut ex = exchange_with_bearer(principal.clone());
        let decision = policy.evaluate(&mut ex).await.unwrap();
        match decision {
            AuthorizationDecision::Granted { principal: granted } => {
                assert_eq!(granted.subject, principal.subject);
            }
            other => panic!("expected Granted, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn authenticate_header_source_reads_authorization() {
        let principal = test_principal(vec!["admin"], vec![]);

        // Header present -> authenticates.
        let policy = RolePolicy::new(
            vec!["admin".into()],
            true,
            false,
            mock_validator(principal.clone()),
            vec![CredentialSource::AuthorizationHeader],
        );
        let mut ex = exchange_with_bearer(principal.clone());
        let decision = policy.evaluate(&mut ex).await.unwrap();
        assert!(matches!(decision, AuthorizationDecision::Granted { .. }));

        // Header absent, no other source -> Unauthenticated.
        let policy = RolePolicy::new(
            vec!["admin".into()],
            true,
            false,
            mock_validator(principal),
            vec![CredentialSource::AuthorizationHeader],
        );
        let mut ex = Exchange::new(Message::default());
        let result = policy.evaluate(&mut ex).await;
        assert!(matches!(result, Err(CamelError::Unauthenticated(_))));
    }

    #[tokio::test]
    async fn cookie_parse_malformed_is_absent_not_error() {
        let principal = test_principal(vec!["admin"], vec![]);
        let policy = RolePolicy::new(
            vec!["admin".into()],
            true,
            false,
            mock_validator(principal),
            vec![CredentialSource::Cookie {
                name: "session".into(),
            }],
        );
        let mut ex = Exchange::new(Message::default());
        ex.input.set_header(
            "Cookie",
            serde_json::Value::String("garbage-no-equals".into()),
        );
        // Absent source -> Unauthenticated. Never a parse panic, never another error.
        let result = policy.evaluate(&mut ex).await;
        assert!(matches!(result, Err(CamelError::Unauthenticated(_))));
    }

    #[tokio::test]
    async fn query_source_reads_camel_http_query_header() {
        let principal = test_principal(vec!["admin"], vec![]);
        let store = NativeCredentialStore::try_new(vec![NativeCredential {
            secret: NativeCredentialSecret::Plaintext {
                value: zeroize::Zeroizing::new("TOK".to_string()),
            },
            principal: principal.clone(),
        }])
        .unwrap();
        let authenticator: Arc<dyn TokenAuthenticator> =
            Arc::new(StaticTokenAuthenticator::new(store));
        let policy = RolePolicy::new(
            vec!["admin".into()],
            true,
            false,
            authenticator,
            vec![CredentialSource::QueryParam {
                param: "token".into(),
            }],
        );
        let mut ex = Exchange::new(Message::default());
        ex.input.set_header(
            "CamelHttpQuery",
            serde_json::Value::String("token=TOK".into()),
        );
        let decision = policy.evaluate(&mut ex).await.unwrap();
        assert!(matches!(decision, AuthorizationDecision::Granted { .. }));
    }

    #[tokio::test]
    async fn trust_false_preloaded_principal_unauthenticated() {
        let principal = test_principal(vec!["admin"], vec![]);
        let policy = RolePolicy::new(
            vec!["admin".into()],
            true,
            false, // trust_upstream_principal
            mock_validator(principal.clone()),
            vec![CredentialSource::AuthorizationHeader],
        );
        let mut ex = exchange_with_principal(principal); // preloaded principal, no credential
        let result = policy.evaluate(&mut ex).await;
        assert!(matches!(result, Err(CamelError::Unauthenticated(_))));
    }

    #[tokio::test]
    async fn trust_true_preloaded_principal_fallback() {
        let principal = test_principal(vec!["admin"], vec![]);
        let policy = RolePolicy::new(
            vec!["admin".into()],
            true,
            true, // trust_upstream_principal
            mock_validator(principal.clone()),
            vec![CredentialSource::AuthorizationHeader],
        );
        let mut ex = exchange_with_principal(principal); // preloaded principal, no credential
        let decision = policy.evaluate(&mut ex).await.unwrap();
        assert!(matches!(decision, AuthorizationDecision::Granted { .. }));
    }

    #[tokio::test]
    async fn prefix_credential_unauthenticated() {
        // Store holds the full credential. A truncated prefix must never match
        // (constant-time exact compare in the shared store lookup).
        let principal = test_principal(vec!["admin"], vec![]);
        let store = NativeCredentialStore::try_new(vec![NativeCredential {
            secret: NativeCredentialSecret::Plaintext {
                value: zeroize::Zeroizing::new("SENTINEL_FULL_9kq2".to_string()),
            },
            principal: principal.clone(),
        }])
        .unwrap();
        let authenticator: Arc<dyn TokenAuthenticator> =
            Arc::new(StaticTokenAuthenticator::new(store));

        // Authorization header.
        let policy = RolePolicy::new(
            vec!["admin".into()],
            true,
            false,
            authenticator.clone(),
            vec![CredentialSource::AuthorizationHeader],
        );
        let mut ex = Exchange::new(Message::default());
        ex.input.set_header(
            "Authorization",
            serde_json::Value::String("Bearer SENTINEL_FULL".into()),
        );
        let result = policy.evaluate(&mut ex).await;
        assert!(matches!(result, Err(CamelError::Unauthenticated(_))));

        // Cookie via exchange headers.
        let policy = RolePolicy::new(
            vec!["admin".into()],
            true,
            false,
            authenticator.clone(),
            vec![CredentialSource::Cookie {
                name: "session".into(),
            }],
        );
        let mut ex = Exchange::new(Message::default());
        ex.input.set_header(
            "Cookie",
            serde_json::Value::String("session=SENTINEL_FULL".into()),
        );
        let result = policy.evaluate(&mut ex).await;
        assert!(matches!(result, Err(CamelError::Unauthenticated(_))));

        // Query via the CamelHttpQuery header.
        let policy = RolePolicy::new(
            vec!["admin".into()],
            true,
            false,
            authenticator,
            vec![CredentialSource::QueryParam {
                param: "token".into(),
            }],
        );
        let mut ex = Exchange::new(Message::default());
        ex.input.set_header(
            "CamelHttpQuery",
            serde_json::Value::String("token=SENTINEL_FULL".into()),
        );
        let result = policy.evaluate(&mut ex).await;
        assert!(matches!(result, Err(CamelError::Unauthenticated(_))));
    }

    // --- Task 4.1: Header source ---

    #[tokio::test]
    async fn header_source_authenticates_api_key() {
        let principal = test_principal(vec!["admin"], vec![]);
        let store = NativeCredentialStore::try_new(vec![NativeCredential {
            secret: NativeCredentialSecret::Plaintext {
                value: zeroize::Zeroizing::new("SENTINEL_KEY_1".to_string()),
            },
            principal: principal.clone(),
        }])
        .unwrap();
        let authenticator: Arc<dyn TokenAuthenticator> =
            Arc::new(StaticTokenAuthenticator::new(store));
        let policy = RolePolicy::new(
            vec!["admin".into()],
            true,
            false,
            authenticator,
            vec![CredentialSource::Header {
                name: "x-api-key".into(),
            }],
        );
        let mut ex = Exchange::new(Message::default());
        ex.input.set_header(
            "X-API-Key",
            serde_json::Value::String("SENTINEL_KEY_1".into()),
        );
        let decision = policy.evaluate(&mut ex).await.unwrap();
        match decision {
            AuthorizationDecision::Granted { principal: granted } => {
                assert_eq!(granted.subject, principal.subject);
            }
            other => panic!("expected Granted, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn header_source_miss_maps_401() {
        let principal = test_principal(vec!["admin"], vec![]);
        let policy = RolePolicy::new(
            vec!["admin".into()],
            true,
            false,
            mock_validator(principal),
            vec![CredentialSource::Header {
                name: "x-api-key".into(),
            }],
        );
        // No X-API-Key header, no other source -> Unauthenticated.
        let mut ex = Exchange::new(Message::default());
        let result = policy.evaluate(&mut ex).await;
        assert!(matches!(result, Err(CamelError::Unauthenticated(_))));
    }

    #[tokio::test]
    async fn header_lookup_case_insensitive() {
        let principal = test_principal(vec!["admin"], vec![]);
        let store = NativeCredentialStore::try_new(vec![NativeCredential {
            secret: NativeCredentialSecret::Plaintext {
                value: zeroize::Zeroizing::new("SENTINEL_KEY_1".to_string()),
            },
            principal: principal.clone(),
        }])
        .unwrap();
        let authenticator: Arc<dyn TokenAuthenticator> =
            Arc::new(StaticTokenAuthenticator::new(store));
        let policy = RolePolicy::new(
            vec!["admin".into()],
            true,
            false,
            authenticator,
            vec![CredentialSource::Header {
                name: "x-api-key".into(),
            }],
        );
        let mut ex = Exchange::new(Message::default());
        // Header key casing differs from the declared source name.
        ex.input.set_header(
            "X-API-KEY",
            serde_json::Value::String("SENTINEL_KEY_1".into()),
        );
        let decision = policy.evaluate(&mut ex).await.unwrap();
        assert!(matches!(decision, AuthorizationDecision::Granted { .. }));
    }

    // --- RFC 9110/7235 bearer scheme semantics on the unified default path ---

    #[tokio::test]
    async fn bearer_scheme_lowercase_grants() {
        let principal = test_principal(vec!["admin"], vec![]);
        let policy = RolePolicy::new(
            vec!["admin".into()],
            true,
            false,
            store_seeded_authenticator("TOK", principal),
            vec![CredentialSource::AuthorizationHeader],
        );
        let mut ex = Exchange::new(Message::default());
        ex.input.set_header(
            "Authorization",
            serde_json::Value::String("bearer TOK".into()),
        );
        let decision = policy.evaluate(&mut ex).await.unwrap();
        assert!(matches!(decision, AuthorizationDecision::Granted { .. }));
    }

    #[tokio::test]
    async fn bearer_scheme_uppercase_grants() {
        let principal = test_principal(vec!["admin"], vec![]);
        let policy = RolePolicy::new(
            vec!["admin".into()],
            true,
            false,
            store_seeded_authenticator("TOK", principal),
            vec![CredentialSource::AuthorizationHeader],
        );
        let mut ex = Exchange::new(Message::default());
        ex.input.set_header(
            "Authorization",
            serde_json::Value::String("BEARER TOK".into()),
        );
        let decision = policy.evaluate(&mut ex).await.unwrap();
        assert!(matches!(decision, AuthorizationDecision::Granted { .. }));
    }

    #[tokio::test]
    async fn bearer_leading_whitespace_grants() {
        let principal = test_principal(vec!["admin"], vec![]);
        let policy = RolePolicy::new(
            vec!["admin".into()],
            true,
            false,
            store_seeded_authenticator("TOK", principal),
            vec![CredentialSource::AuthorizationHeader],
        );
        let mut ex = Exchange::new(Message::default());
        ex.input.set_header(
            "Authorization",
            serde_json::Value::String(" Bearer TOK".into()),
        );
        let decision = policy.evaluate(&mut ex).await.unwrap();
        assert!(matches!(decision, AuthorizationDecision::Granted { .. }));
    }

    #[tokio::test]
    async fn bearer_double_space_yields_trimmed_token() {
        let principal = test_principal(vec!["admin"], vec![]);
        let policy = RolePolicy::new(
            vec!["admin".into()],
            true,
            false,
            store_seeded_authenticator("TOK", principal),
            vec![CredentialSource::AuthorizationHeader],
        );
        let mut ex = Exchange::new(Message::default());
        ex.input.set_header(
            "Authorization",
            serde_json::Value::String("Bearer  TOK".into()),
        );
        let decision = policy.evaluate(&mut ex).await.unwrap();
        assert!(matches!(decision, AuthorizationDecision::Granted { .. }));
    }

    #[tokio::test]
    async fn bearer_empty_token_falls_to_trust_branch() {
        let principal = test_principal(vec!["admin"], vec![]);

        // trust=false: empty token yields no extraction -> Unauthenticated.
        let policy = RolePolicy::new(
            vec!["admin".into()],
            true,
            false,
            mock_validator(principal.clone()),
            vec![CredentialSource::AuthorizationHeader],
        );
        let mut ex = exchange_with_principal(principal.clone());
        ex.input
            .set_header("Authorization", serde_json::Value::String("Bearer ".into()));
        let result = policy.evaluate(&mut ex).await;
        assert!(matches!(result, Err(CamelError::Unauthenticated(_))));

        // trust=true: empty token yields no extraction -> preloaded principal grants.
        let policy = RolePolicy::new(
            vec!["admin".into()],
            true,
            true,
            mock_validator(principal.clone()),
            vec![CredentialSource::AuthorizationHeader],
        );
        let mut ex = exchange_with_principal(principal);
        ex.input
            .set_header("Authorization", serde_json::Value::String("Bearer ".into()));
        let decision = policy.evaluate(&mut ex).await.unwrap();
        assert!(matches!(decision, AuthorizationDecision::Granted { .. }));
    }
}
