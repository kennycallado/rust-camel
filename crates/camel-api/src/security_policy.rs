use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::{CamelError, Exchange};

/// Represents an authenticated principal extracted from token claims.
///
/// Provider-neutral: the `ClaimsMapper` trait in `camel-auth` is responsible
/// for mapping provider-specific claim shapes into this structure.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Principal {
    pub subject: String,
    #[serde(default)]
    pub issuer: String,
    #[serde(default)]
    pub audience: Vec<String>,
    pub scopes: Vec<String>,
    pub roles: Vec<String>,
    pub claims: serde_json::Value,
}

impl Principal {
    /// Check if the principal has a specific role.
    pub fn has_role(&self, role: &str) -> bool {
        self.roles.iter().any(|r| r == role)
    }

    /// Check if the principal has a specific scope.
    pub fn has_scope(&self, scope: &str) -> bool {
        self.scopes.iter().any(|s| s == scope)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum AuthorizationDecision {
    Granted {
        principal: Principal,
    },
    Denied {
        reason: String,
        required: Vec<String>,
        actual: Vec<String>,
    },
}

impl std::fmt::Display for AuthorizationDecision {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Granted { principal } => {
                write!(f, "Access granted for {}", principal.subject)
            }
            Self::Denied { reason, .. } => write!(f, "Access denied: {reason}"),
        }
    }
}

#[async_trait]
pub trait SecurityPolicy: Send + Sync {
    async fn evaluate(&self, exchange: &mut Exchange) -> Result<AuthorizationDecision, CamelError>;
}

pub struct SecurityPolicyConfig {
    pub policy: Arc<dyn SecurityPolicy>,
}

impl SecurityPolicyConfig {
    pub fn new(policy: impl SecurityPolicy + 'static) -> Self {
        Self {
            policy: Arc::new(policy),
        }
    }

    pub fn from_arc(policy: Arc<dyn SecurityPolicy>) -> Self {
        Self { policy }
    }
}

impl Clone for SecurityPolicyConfig {
    fn clone(&self) -> Self {
        Self {
            policy: Arc::clone(&self.policy),
        }
    }
}

impl std::fmt::Debug for SecurityPolicyConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SecurityPolicyConfig")
            .field("policy", &"<SecurityPolicy>")
            .finish()
    }
}

// --- Principal property storage helpers ---

/// Exchange property key for the principal's subject.
pub const PRINCIPAL_SUBJECT_KEY: &str = "camel.auth.subject";
/// Exchange property key for the principal's roles (JSON array).
pub const PRINCIPAL_ROLES_KEY: &str = "camel.auth.roles";
/// Exchange property key for the principal's scopes (JSON array).
pub const PRINCIPAL_SCOPES_KEY: &str = "camel.auth.scopes";
/// Exchange property key for the principal's issuer.
pub const PRINCIPAL_ISSUER_KEY: &str = "camel.auth.issuer";
/// Exchange property key for the principal's raw claims (JSON object).
pub const PRINCIPAL_CLAIMS_KEY: &str = "camel.auth.claims";
/// Exchange property key for the principal's audience (JSON array).
pub const PRINCIPAL_AUDIENCE_KEY: &str = "camel.auth.audience";
/// Exchange property key for the full serialized principal.
pub const PRINCIPAL_KEY: &str = "camel.auth.principal";

/// Store all principal properties as exchange properties under well-known keys.
pub fn store_principal_properties(exchange: &mut Exchange, principal: &Principal) {
    exchange.set_property(PRINCIPAL_SUBJECT_KEY, principal.subject.clone());
    exchange.set_property(
        PRINCIPAL_ROLES_KEY,
        serde_json::to_string(&principal.roles).unwrap_or_default(),
    );
    exchange.set_property(
        PRINCIPAL_SCOPES_KEY,
        serde_json::to_string(&principal.scopes).unwrap_or_default(),
    );
    exchange.set_property(PRINCIPAL_ISSUER_KEY, principal.issuer.clone());
    exchange.set_property(
        PRINCIPAL_CLAIMS_KEY,
        serde_json::to_string(&principal.claims).unwrap_or_default(),
    );
    exchange.set_property(
        PRINCIPAL_AUDIENCE_KEY,
        serde_json::to_string(&principal.audience).unwrap_or_default(),
    );
    exchange.set_property(
        PRINCIPAL_KEY,
        serde_json::to_string(principal).unwrap_or_default(),
    );
}

pub fn principal_from_exchange(exchange: &Exchange) -> Option<Principal> {
    exchange
        .property(PRINCIPAL_KEY)
        .and_then(|v| v.as_str())
        .and_then(|s| serde_json::from_str(s).ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Body;

    fn test_principal(roles: Vec<&str>, scopes: Vec<&str>) -> Principal {
        Principal {
            subject: "user1".into(),
            issuer: "test".into(),
            audience: vec![],
            scopes: scopes.into_iter().map(String::from).collect(),
            roles: roles.into_iter().map(String::from).collect(),
            claims: serde_json::Value::Null,
        }
    }

    #[test]
    fn principal_has_role_is_case_sensitive() {
        let p = test_principal(vec!["Admin", "User"], vec![]);
        assert!(!p.has_role("admin"));
        assert!(!p.has_role("ADMIN"));
        assert!(p.has_role("User"));
        assert!(!p.has_role("guest"));
    }

    #[test]
    fn principal_has_scope() {
        let p = test_principal(vec![], vec!["read", "write"]);
        assert!(p.has_scope("read"));
        assert!(!p.has_scope("delete"));
    }

    #[test]
    fn authorization_decision_granted_display() {
        let p = test_principal(vec![], vec![]);
        let d = AuthorizationDecision::Granted { principal: p };
        assert!(format!("{d}").contains("user1"));
    }

    #[test]
    fn authorization_decision_denied_display() {
        let d = AuthorizationDecision::Denied {
            reason: "missing role".into(),
            required: vec!["admin".into()],
            actual: vec![],
        };
        assert!(format!("{d}").contains("missing role"));
    }

    #[test]
    fn security_policy_config_debug_redacts_policy() {
        struct DummyPolicy;

        #[async_trait]
        impl SecurityPolicy for DummyPolicy {
            async fn evaluate(
                &self,
                _exchange: &mut Exchange,
            ) -> Result<AuthorizationDecision, CamelError> {
                Ok(AuthorizationDecision::Granted {
                    principal: test_principal(vec![], vec![]),
                })
            }
        }

        let config = SecurityPolicyConfig::new(DummyPolicy);
        let debug = format!("{config:?}");
        assert!(debug.contains("SecurityPolicyConfig"));
        assert!(debug.contains("<SecurityPolicy>"));
    }

    #[test]
    fn store_principal_properties_populates_all_keys() {
        let principal = Principal {
            subject: "alice".into(),
            issuer: "keycloak".into(),
            audience: vec!["api".into()],
            scopes: vec!["read".into(), "write".into()],
            roles: vec!["admin".into()],
            claims: serde_json::json!({"sub": "alice", "custom": true}),
        };
        let mut exchange = Exchange::new(crate::Message::new(Body::Empty));
        store_principal_properties(&mut exchange, &principal);

        assert_eq!(
            exchange.property(PRINCIPAL_SUBJECT_KEY).unwrap(),
            &serde_json::Value::String("alice".into())
        );
        assert_eq!(
            exchange.property(PRINCIPAL_ISSUER_KEY).unwrap(),
            &serde_json::Value::String("keycloak".into())
        );
        let roles: Vec<String> = serde_json::from_str(
            exchange
                .property(PRINCIPAL_ROLES_KEY)
                .unwrap()
                .as_str()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(roles, vec!["admin"]);
        let scopes: Vec<String> = serde_json::from_str(
            exchange
                .property(PRINCIPAL_SCOPES_KEY)
                .unwrap()
                .as_str()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(scopes, vec!["read", "write"]);
        let audience: Vec<String> = serde_json::from_str(
            exchange
                .property(PRINCIPAL_AUDIENCE_KEY)
                .unwrap()
                .as_str()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(audience, vec!["api"]);
        let claims: serde_json::Value = serde_json::from_str(
            exchange
                .property(PRINCIPAL_CLAIMS_KEY)
                .unwrap()
                .as_str()
                .unwrap(),
        )
        .unwrap();
        assert!(claims.as_object().unwrap().contains_key("custom"));
        let full: serde_json::Value =
            serde_json::from_str(exchange.property(PRINCIPAL_KEY).unwrap().as_str().unwrap())
                .unwrap();
        assert_eq!(full["subject"], "alice");
    }

    #[test]
    fn security_policy_config_clone() {
        struct DummyPolicy;

        #[async_trait]
        impl SecurityPolicy for DummyPolicy {
            async fn evaluate(
                &self,
                _exchange: &mut Exchange,
            ) -> Result<AuthorizationDecision, CamelError> {
                Ok(AuthorizationDecision::Granted {
                    principal: test_principal(vec![], vec![]),
                })
            }
        }

        let config = SecurityPolicyConfig::new(DummyPolicy);
        let cloned = config.clone();
        // Both point to same Arc
        assert!(Arc::ptr_eq(&config.policy, &cloned.policy));
    }

    #[test]
    fn test_principal_from_exchange_round_trip() {
        let principal = Principal {
            subject: "bob".into(),
            issuer: "keycloak".into(),
            audience: vec!["api".into()],
            scopes: vec!["read".into()],
            roles: vec!["user".into()],
            claims: serde_json::json!({"sub": "bob"}),
        };
        let mut exchange = Exchange::new(crate::Message::new(Body::Empty));
        store_principal_properties(&mut exchange, &principal);

        let recovered = principal_from_exchange(&exchange).expect("principal should be recovered");
        assert_eq!(recovered.subject, "bob");
        assert_eq!(recovered.issuer, "keycloak");
        assert_eq!(recovered.audience, vec!["api"]);
        assert_eq!(recovered.scopes, vec!["read"]);
        assert_eq!(recovered.roles, vec!["user"]);
    }
}
