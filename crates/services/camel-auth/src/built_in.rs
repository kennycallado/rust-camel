use async_trait::async_trait;

use camel_api::security_policy::{AuthContext, AuthorizationDecision, SecurityPolicy};
use camel_api::{CamelError, Exchange};

/// Property key used to store the authenticated principal in the exchange.
///
/// Re-exported from the camel-api contract so `camel_auth::built_in::PRINCIPAL_KEY`
/// and `camel_api::security_policy::PRINCIPAL_KEY` are the same constant.
pub use camel_api::security_policy::PRINCIPAL_KEY;

/// Role-based access control policy.
///
/// Reads the authenticated principal from the [`AuthContext`] (never from raw
/// Exchange properties) and evaluates whether it holds the required roles.
/// When `all_required` is true, every listed role must be present.
/// When `all_required` is false, at least one listed role must be present.
pub struct RolePolicy {
    required_roles: Vec<String>,
    all_required: bool,
}

impl RolePolicy {
    pub fn new(required_roles: Vec<String>, all_required: bool) -> Self {
        Self {
            required_roles,
            all_required,
        }
    }
}

#[async_trait]
impl SecurityPolicy for RolePolicy {
    async fn evaluate(
        &self,
        _exchange: &mut Exchange,
        auth: &AuthContext<'_>,
    ) -> Result<AuthorizationDecision, CamelError> {
        let principal = auth.principal.principal();

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
            Ok(AuthorizationDecision::Granted {
                principal: principal.clone(),
            })
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
/// Reads the authenticated principal from the [`AuthContext`] (never from raw
/// Exchange properties) and evaluates whether it holds the required scopes.
/// When `all_required` is true, every listed scope must be present.
/// When `all_required` is false, at least one listed scope must be present.
pub struct ScopePolicy {
    required_scopes: Vec<String>,
    all_required: bool,
}

impl ScopePolicy {
    pub fn new(required_scopes: Vec<String>, all_required: bool) -> Self {
        Self {
            required_scopes,
            all_required,
        }
    }
}

#[async_trait]
impl SecurityPolicy for ScopePolicy {
    async fn evaluate(
        &self,
        _exchange: &mut Exchange,
        auth: &AuthContext<'_>,
    ) -> Result<AuthorizationDecision, CamelError> {
        let principal = auth.principal.principal();

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
            Ok(AuthorizationDecision::Granted {
                principal: principal.clone(),
            })
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
    use camel_api::Message;
    use camel_api::security_policy::{
        AuthPrincipal, Principal, TransportId, store_principal_properties,
    };

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

    /// Throwaway `AuthPrincipal` for tests. The trait is open, so implementing
    /// it for a test stub is legal and grants NO minting power — only the
    /// concrete `AuthenticatedPrincipal` (camel-auth kernel) is unforgeable.
    struct TestPrincipal(Principal);

    impl AuthPrincipal for TestPrincipal {
        fn principal(&self) -> &Principal {
            &self.0
        }
        fn provider_id(&self) -> &str {
            "test"
        }
    }

    fn auth_ctx<'a>(principal: &'a TestPrincipal) -> AuthContext<'a> {
        AuthContext {
            principal,
            transport: TransportId::Http,
        }
    }

    fn empty_exchange() -> Exchange {
        Exchange::new(Message::default())
    }

    #[tokio::test]
    async fn role_policy_reads_typed_principal_roles() {
        let principal = TestPrincipal(test_principal(vec!["admin"], vec![]));
        let policy = RolePolicy::new(vec!["admin".into()], true);
        let mut ex = empty_exchange();
        let auth = auth_ctx(&principal);
        let decision = policy.evaluate(&mut ex, &auth).await.unwrap();
        assert!(matches!(decision, AuthorizationDecision::Granted { .. }));
    }

    #[tokio::test]
    async fn property_only_evidence_denies() {
        // Exchange carries a spoofed `camel.auth.principal` property with
        // valid-format principal data, but the typed `AuthContext` principal
        // lacks the required role — RolePolicy must deny (property evidence
        // never authorizes).
        let spoofed = test_principal(vec!["admin"], vec![]);
        let mut ex = empty_exchange();
        store_principal_properties(&mut ex, &spoofed);

        let typed = TestPrincipal(test_principal(vec!["user"], vec![]));
        let policy = RolePolicy::new(vec!["admin".into()], true);
        let auth = auth_ctx(&typed);
        let decision = policy.evaluate(&mut ex, &auth).await.unwrap();
        assert!(matches!(decision, AuthorizationDecision::Denied { .. }));
    }

    #[tokio::test]
    async fn role_policy_grants_when_role_present() {
        let principal = TestPrincipal(test_principal(vec!["admin"], vec![]));
        let policy = RolePolicy::new(vec!["admin".into()], true);
        let mut ex = empty_exchange();
        let auth = auth_ctx(&principal);
        let decision = policy.evaluate(&mut ex, &auth).await.unwrap();
        assert!(matches!(decision, AuthorizationDecision::Granted { .. }));
    }

    #[tokio::test]
    async fn role_policy_denies_when_role_missing() {
        let principal = TestPrincipal(test_principal(vec!["user"], vec![]));
        let policy = RolePolicy::new(vec!["admin".into()], true);
        let mut ex = empty_exchange();
        let auth = auth_ctx(&principal);
        let decision = policy.evaluate(&mut ex, &auth).await.unwrap();
        assert!(matches!(decision, AuthorizationDecision::Denied { .. }));
    }

    #[tokio::test]
    async fn role_policy_any_required() {
        let principal = TestPrincipal(test_principal(vec!["user"], vec![]));
        let policy = RolePolicy::new(vec!["admin".into(), "user".into()], false);
        let mut ex = empty_exchange();
        let auth = auth_ctx(&principal);
        let decision = policy.evaluate(&mut ex, &auth).await.unwrap();
        assert!(matches!(decision, AuthorizationDecision::Granted { .. }));
    }

    #[tokio::test]
    async fn scope_policy_grants() {
        let principal = TestPrincipal(test_principal(vec![], vec!["read"]));
        let policy = ScopePolicy::new(vec!["read".into()], true);
        let mut ex = empty_exchange();
        let auth = auth_ctx(&principal);
        let decision = policy.evaluate(&mut ex, &auth).await.unwrap();
        assert!(matches!(decision, AuthorizationDecision::Granted { .. }));
    }

    #[tokio::test]
    async fn scope_policy_denies_when_scope_missing() {
        let principal = TestPrincipal(test_principal(vec![], vec!["read"]));
        let policy = ScopePolicy::new(vec!["write".into()], true);
        let mut ex = empty_exchange();
        let auth = auth_ctx(&principal);
        let decision = policy.evaluate(&mut ex, &auth).await.unwrap();
        assert!(matches!(decision, AuthorizationDecision::Denied { .. }));
    }

    #[tokio::test]
    async fn scope_policy_any_required() {
        let principal = TestPrincipal(test_principal(vec![], vec!["read"]));
        let policy = ScopePolicy::new(vec!["write".into(), "read".into()], false);
        let mut ex = empty_exchange();
        let auth = auth_ctx(&principal);
        let decision = policy.evaluate(&mut ex, &auth).await.unwrap();
        assert!(matches!(decision, AuthorizationDecision::Granted { .. }));
    }
}
