use async_trait::async_trait;
use std::sync::Arc;

use camel_api::security_policy::{AuthorizationDecision, Principal, SecurityPolicy};
use camel_api::{CamelError, Exchange};

use crate::token_authenticator::TokenAuthenticator;

/// Property key used to store the authenticated principal in the exchange.
pub const PRINCIPAL_KEY: &str = "camel.auth.principal";

/// Extracts and validates a Bearer token from the `Authorization` header.
///
/// If the header is present, validates it via the supplied [`TokenAuthenticator`] and stores
/// the resulting [`Principal`] in `PRINCIPAL_KEY` for downstream processors.
///
/// If no `Authorization` header is present, the behavior depends on
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
) -> Result<Principal, CamelError> {
    // Clone the token string so the borrow on exchange.input ends before the mut borrow for set_property.
    let token = exchange
        .input
        .header_ic("authorization")
        .and_then(|v| v.as_str())
        .and_then(|s| s.strip_prefix("Bearer "))
        .map(|s| s.to_string());

    if let Some(token) = token {
        let principal = authenticator.authenticate_bearer(&token).await?;
        // Store for downstream processors
        if let Ok(value) = serde_json::to_value(&principal) {
            exchange.set_property(PRINCIPAL_KEY, value);
        }
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
fn extract_principal_from_exchange(exchange: &Exchange) -> Result<Principal, CamelError> {
    exchange
        .property(PRINCIPAL_KEY)
        .and_then(|v| serde_json::from_value::<Principal>(v.clone()).ok())
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
}

impl RolePolicy {
    pub fn new(
        required_roles: Vec<String>,
        all_required: bool,
        trust_upstream_principal: bool,
        authenticator: Arc<dyn TokenAuthenticator>,
    ) -> Self {
        Self {
            required_roles,
            all_required,
            trust_upstream_principal,
            authenticator,
        }
    }
}

#[async_trait]
impl SecurityPolicy for RolePolicy {
    async fn evaluate(&self, exchange: &mut Exchange) -> Result<AuthorizationDecision, CamelError> {
        let principal = authenticate(
            exchange,
            &*self.authenticator,
            self.trust_upstream_principal,
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
}

impl ScopePolicy {
    pub fn new(
        required_scopes: Vec<String>,
        all_required: bool,
        trust_upstream_principal: bool,
        authenticator: Arc<dyn TokenAuthenticator>,
    ) -> Self {
        Self {
            required_scopes,
            all_required,
            trust_upstream_principal,
            authenticator,
        }
    }
}

#[async_trait]
impl SecurityPolicy for ScopePolicy {
    async fn evaluate(&self, exchange: &mut Exchange) -> Result<AuthorizationDecision, CamelError> {
        let principal = authenticate(
            exchange,
            &*self.authenticator,
            self.trust_upstream_principal,
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

    /// Build an exchange with a Bearer token in the Authorization header.
    fn exchange_with_bearer(principal: Principal) -> Exchange {
        let validator_principal = principal.clone();
        let mut msg = Message::default();
        msg.set_header(
            "Authorization",
            serde_json::Value::String("Bearer mock-token".into()),
        );
        // Also embed principal in exchange so fallback path is testable if needed
        let mut ex = Exchange::new(msg);
        let value = serde_json::to_value(&validator_principal).unwrap();
        ex.set_property(PRINCIPAL_KEY, value);
        ex
    }

    /// Build an exchange with the principal in the exchange property (no Bearer header).
    fn exchange_with_principal(principal: Principal) -> Exchange {
        let mut ex = Exchange::new(Message::default());
        let value = serde_json::to_value(&principal).unwrap();
        ex.set_property(PRINCIPAL_KEY, value);
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
        let policy = RolePolicy::new(vec!["admin".into()], true, false, Arc::new(FailValidator));
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
        );
        let mut ex = exchange_with_principal(principal);
        let result = policy.evaluate(&mut ex).await;
        assert!(matches!(result, Err(CamelError::Unauthenticated(_))));
    }
}
