use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::{CamelError, Exchange};

/// Represents an authenticated principal extracted from token claims.
///
/// Provider-neutral: the `ClaimsMapper` trait in `camel-auth` is responsible
/// for mapping provider-specific claim shapes into this structure.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
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

// Manual Debug redacts untrusted `claims` (PII leak fix rc-yv1m).
// Do NOT add `Debug` to the #[derive(...)] above — it would reintroduce the leak.
impl std::fmt::Debug for Principal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Principal")
            .field("subject", &self.subject)
            .field("issuer", &self.issuer)
            .field("audience", &self.audience)
            .field("scopes", &self.scopes)
            .field("roles", &self.roles)
            .field("claims", &"[REDACTED]")
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum AuthorizationDecision {
    /// Access granted.
    ///
    /// `principal` is the policy's advisory asserted principal, stored to
    /// exchange properties for observability. It is NOT the authentication
    /// identity; that always comes from the [`AuthContext`] principal.
    Granted { principal: Principal },
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

/// Transport kinds that can host a server route.
///
/// Closed set: `http:`, `ws:`, `grpc:`, `mcp:`, and `wasm:` (source
/// routes hosting an http-listener) are the only transports that
/// terminate requests and run authorization.
///
/// exhaustive-by-contract: closed transport set; a new transport is an
/// architectural change, not additive growth.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TransportId {
    Http,
    Ws,
    Grpc,
    Mcp,
    Wasm,
}

/// Read-only view of the principal minted by the authentication path.
///
/// camel-api must never name a `camel-auth` type (no dependency cycle), so the
/// concrete `AuthenticatedPrincipal` lives in `camel-auth` and this crate
/// consumes it through this trait.
pub trait AuthPrincipal: Send + Sync {
    /// The authenticated principal.
    fn principal(&self) -> &Principal;
    /// The identifier of the provider that minted this principal.
    fn provider_id(&self) -> &str;
}

/// Authentication context handed to a [`SecurityPolicy`] during evaluation.
///
/// Carries the authenticated principal and the transport that carried the
/// request. The `AuthContext` principal is the authentication identity; a
/// policy's `Granted { principal }` value is advisory only.
pub struct AuthContext<'a> {
    pub principal: &'a dyn AuthPrincipal,
    pub transport: TransportId,
}

#[async_trait]
pub trait SecurityPolicy: Send + Sync {
    async fn evaluate(
        &self,
        exchange: &mut Exchange,
        auth: &AuthContext<'_>,
    ) -> Result<AuthorizationDecision, CamelError>;
}

/// Name of the input header camel-http uses to carry the raw HTTP query string.
///
/// camel-http stores the request query (the part after `?`) as a string header
/// under this exact name (`crates/components/camel-http/src/lib.rs`).
/// camel-auth cannot depend on camel-http, so this contract constant lives in
/// camel-api. `QueryParam` extraction reads the raw query from this header.
pub const CAMEL_HTTP_QUERY_HEADER: &str = "CamelHttpQuery";

/// Source from which a token can be extracted.
///
/// exhaustive-by-contract: closed source set; out-of-crate camel-auth
/// extraction matches all variants, so adding a source must update every
/// match site by review.
#[derive(Clone, PartialEq, Eq)]
pub enum CredentialSource {
    /// Extract from the `Authorization` header (Bearer scheme).
    AuthorizationHeader,
    /// Extract from a query parameter with the given name.
    QueryParam { param: String },
    /// Extract from a cookie with the given name.
    Cookie { name: String },
    /// Extract from a named request header (API-key style).
    ///
    /// The extracted value flows into the same constant-time
    /// `NativeCredentialStore::lookup` as every other source.
    Header { name: String },
}

impl CredentialSource {
    /// Returns the variant name without exposing sensitive values.
    pub fn variant_name(&self) -> &'static str {
        match self {
            CredentialSource::AuthorizationHeader => "AuthorizationHeader",
            CredentialSource::QueryParam { .. } => "QueryParam",
            CredentialSource::Cookie { .. } => "Cookie",
            CredentialSource::Header { .. } => "Header",
        }
    }
}

impl std::fmt::Debug for CredentialSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CredentialSource::AuthorizationHeader => f.write_str("AuthorizationHeader"),
            CredentialSource::QueryParam { param } => {
                write!(f, "QueryParam {{ param: {:?} }}", param) // allow-secret
            }
            CredentialSource::Cookie { name } => {
                write!(f, "Cookie {{ name: {:?} }}", name) // allow-secret
            }
            CredentialSource::Header { name } => {
                write!(f, "Header {{ name: {:?} }}", name) // allow-secret
            }
        }
    }
}

pub struct SecurityPolicyConfig {
    pub policy: Arc<dyn SecurityPolicy>,
    /// Extraction sources for the route's credential, in declared order.
    /// Defaults to header-only (fail-closed, ADR-0033).
    pub credential_sources: Vec<CredentialSource>,
}

impl SecurityPolicyConfig {
    pub fn new(policy: impl SecurityPolicy + 'static) -> Self {
        Self {
            policy: Arc::new(policy),
            credential_sources: vec![CredentialSource::AuthorizationHeader],
        }
    }

    pub fn from_arc(policy: Arc<dyn SecurityPolicy>) -> Self {
        Self {
            policy,
            credential_sources: vec![CredentialSource::AuthorizationHeader],
        }
    }

    pub fn with_credential_sources(mut self, sources: Vec<CredentialSource>) -> Self {
        self.credential_sources = sources;
        self
    }
}

impl Clone for SecurityPolicyConfig {
    fn clone(&self) -> Self {
        Self {
            policy: Arc::clone(&self.policy),
            credential_sources: self.credential_sources.clone(),
        }
    }
}

impl std::fmt::Debug for SecurityPolicyConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SecurityPolicyConfig")
            .field("policy", &"<SecurityPolicy>")
            .field("credential_sources", &self.credential_sources)
            .finish()
    }
}

/// Access classification for a compiled route.
///
/// `Public` needs no authentication. `Authenticated` requires a principal but
/// no authorization policy. `Authorized` runs a policy against the
/// authenticated principal.
///
/// exhaustive-by-contract: closed enforcement model; modes are kernel
/// semantics, additions are deliberate breaking changes.
pub enum AccessMode {
    Public,
    Authenticated,
    Authorized(Arc<dyn SecurityPolicy>),
}

impl Clone for AccessMode {
    fn clone(&self) -> Self {
        match self {
            Self::Public => Self::Public,
            Self::Authenticated => Self::Authenticated,
            Self::Authorized(policy) => Self::Authorized(Arc::clone(policy)),
        }
    }
}

impl std::fmt::Debug for AccessMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Public => f.write_str("Public"),
            Self::Authenticated => f.write_str("Authenticated"),
            Self::Authorized(_) => f.write_str("Authorized(<SecurityPolicy>)"),
        }
    }
}

/// Accepted issuer and audience set for a route's authentication.
///
/// Reserved in Phase 1; enforcement and cache-key participation arrive in
/// Phase 3. Every configured accepted audience is kept.
#[derive(Clone, Debug, PartialEq)]
pub struct AudienceBinding {
    pub issuers: Vec<String>,
    pub audiences: Vec<String>,
}

/// Compiled security plan for a single server route.
///
/// Produced at staging time, before any listener binds. `access_mode`
/// classifies the route; `provider_ref` is mandatory for
/// `Authenticated`/`Authorized` and absent for `Public`.
pub struct RouteSecurityPlan {
    pub access_mode: AccessMode,
    pub provider_ref: Option<String>,
    pub transport: TransportId,
    pub credential_sources: Vec<CredentialSource>,
    pub audience_binding: Option<AudienceBinding>,
}

impl Clone for RouteSecurityPlan {
    fn clone(&self) -> Self {
        Self {
            access_mode: self.access_mode.clone(),
            provider_ref: self.provider_ref.clone(),
            transport: self.transport,
            credential_sources: self.credential_sources.clone(),
            audience_binding: self.audience_binding.clone(),
        }
    }
}

impl std::fmt::Debug for RouteSecurityPlan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RouteSecurityPlan")
            .field("access_mode", &self.access_mode)
            .field("provider_ref", &self.provider_ref)
            .field("transport", &self.transport)
            .field("credential_sources", &self.credential_sources)
            .field("audience_binding", &self.audience_binding)
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

    /// A `SecurityPolicy` that always grants to `test_principal(vec![], vec![])`.
    fn grant_policy() -> impl SecurityPolicy + 'static {
        struct GrantPolicy;

        #[async_trait]
        impl SecurityPolicy for GrantPolicy {
            async fn evaluate(
                &self,
                _exchange: &mut Exchange,
                _auth: &AuthContext<'_>,
            ) -> Result<AuthorizationDecision, CamelError> {
                Ok(AuthorizationDecision::Granted {
                    principal: test_principal(vec![], vec![]),
                })
            }
        }

        GrantPolicy
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
        let config = SecurityPolicyConfig::new(grant_policy());
        let debug = format!("{config:?}");
        assert!(debug.contains("SecurityPolicyConfig"));
        assert!(debug.contains("<SecurityPolicy>"));
    }

    #[test]
    fn security_policy_config_new_is_header_only() {
        let config = SecurityPolicyConfig::new(grant_policy());
        assert_eq!(
            config.credential_sources,
            vec![CredentialSource::AuthorizationHeader]
        );
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
        let config = SecurityPolicyConfig::new(grant_policy());
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

    #[test]
    fn principal_debug_redacts_claims_compact() {
        let principal = Principal {
            subject: "subj-1".into(),
            issuer: "iss".into(),
            audience: vec!["a1".into()],
            scopes: vec!["s1".into()],
            roles: vec!["r1".into()],
            claims: serde_json::json!({"piid": "SENTINEL_CLAIM_VALUE_9kq2"}),
        };
        let s = format!("{principal:?}");
        assert!(
            s.contains("claims: \"[REDACTED]\""),
            "compact debug should show [REDACTED] for claims"
        );
        assert!(
            !s.contains("SENTINEL_CLAIM_VALUE_9kq2"),
            "compact debug should NOT contain raw claim value"
        );
        assert!(s.contains("subj-1"), "compact debug should contain subject");
        assert!(s.contains("iss"), "compact debug should contain issuer");
        assert!(s.contains("a1"), "compact debug should contain audience");
        assert!(s.contains("s1"), "compact debug should contain scopes");
        assert!(s.contains("r1"), "compact debug should contain roles");
    }

    #[test]
    fn principal_debug_redacts_claims_pretty() {
        let principal = Principal {
            subject: "subj-1".into(),
            issuer: "iss".into(),
            audience: vec!["a1".into()],
            scopes: vec!["s1".into()],
            roles: vec!["r1".into()],
            claims: serde_json::json!({"piid": "SENTINEL_CLAIM_VALUE_9kq2"}),
        };
        let s = format!("{principal:#?}");
        assert!(
            s.contains("[REDACTED]"),
            "pretty debug should show [REDACTED]"
        );
        assert!(
            !s.contains("SENTINEL_CLAIM_VALUE_9kq2"),
            "pretty debug should NOT contain raw claim value"
        );
    }

    #[test]
    fn principal_serialize_preserves_claims() {
        let principal = Principal {
            subject: "subj-1".into(),
            issuer: "iss".into(),
            audience: vec!["a1".into()],
            scopes: vec!["s1".into()],
            roles: vec!["r1".into()],
            claims: serde_json::json!({"piid": "SENTINEL_CLAIM_VALUE_9kq2"}),
        };
        let s = serde_json::to_string(&principal).unwrap();
        assert!(
            s.contains("SENTINEL_CLAIM_VALUE_9kq2"),
            "serialization should preserve raw claim value"
        );
    }

    #[test]
    fn access_mode_debug_redacts_policy() {
        let mode = AccessMode::Authorized(Arc::new(grant_policy()));
        let debug = format!("{mode:?}");
        assert!(debug.contains("<SecurityPolicy>"));
        assert!(!debug.contains("GrantPolicy"));
    }

    #[test]
    fn transport_id_derives_all() {
        fn name(t: TransportId) -> &'static str {
            match t {
                TransportId::Http => "http",
                TransportId::Ws => "ws",
                TransportId::Grpc => "grpc",
                TransportId::Mcp => "mcp",
                TransportId::Wasm => "wasm",
            }
        }
        assert_eq!(name(TransportId::Http), "http");
        assert_eq!(name(TransportId::Ws), "ws");
        assert_eq!(name(TransportId::Grpc), "grpc");
        assert_eq!(name(TransportId::Mcp), "mcp");
        assert_eq!(name(TransportId::Wasm), "wasm");
    }

    #[test]
    fn route_security_plan_clone_debug() {
        let plan = RouteSecurityPlan {
            access_mode: AccessMode::Authenticated,
            provider_ref: Some("idp-a".to_string()),
            transport: TransportId::Http,
            credential_sources: vec![CredentialSource::AuthorizationHeader],
            audience_binding: None,
        };
        let cloned = plan.clone();
        assert_eq!(cloned.provider_ref, plan.provider_ref);
        assert_eq!(cloned.transport, plan.transport);
        assert_eq!(cloned.credential_sources, plan.credential_sources);
        assert!(matches!(cloned.access_mode, AccessMode::Authenticated));
        assert!(cloned.audience_binding.is_none());

        let debug = format!("{plan:?}");
        assert!(debug.contains(r#"provider_ref: Some("idp-a")"#));
        assert!(!debug.contains("GrantPolicy"));
    }
}
