use crate::authn_cache::AuthnCache;
use crate::permission::PermissionEvaluator;
use crate::token_authenticator::TokenAuthenticator;
use camel_api::security_policy::SecurityPolicy;
use dashmap::DashMap;
use std::sync::Arc;

pub struct NamedRegistry<T: ?Sized> {
    entries: DashMap<String, Arc<T>>,
}

impl<T: ?Sized> Default for NamedRegistry<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: ?Sized> NamedRegistry<T> {
    pub fn new() -> Self {
        Self {
            entries: DashMap::new(),
        }
    }

    pub fn register(&self, name: impl Into<String>, entry: Arc<T>) {
        self.entries.insert(name.into(), entry);
    }

    pub fn get(&self, name: &str) -> Option<Arc<T>> {
        self.entries.get(name).map(|e| Arc::clone(&*e))
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn entries(&self) -> Vec<(String, Arc<T>)> {
        self.entries
            .iter()
            .map(|entry| (entry.key().clone(), Arc::clone(entry.value())))
            .collect()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

pub type SecurityPolicyRegistry = NamedRegistry<dyn SecurityPolicy>;
pub type PermissionEvaluatorRegistry = NamedRegistry<dyn PermissionEvaluator>;

/// A single named authentication provider: its token authenticator plus the
/// (reserved) audience binding. `audience_binding` is `None` in Phase 1 and is
/// populated in Task 1.6.
///
/// Invariant: JWT-backed providers MUST populate `issuers` in their binding. A
/// binding with `audiences` but empty `issuers` silently drops the validator's
/// constructor-fixed issuer check on the kernel path (the request's non-empty
/// audience set bypasses both constructor checks via REPLACEMENT semantics).
pub struct ProviderEntry {
    pub authenticator: Arc<dyn TokenAuthenticator>,
    pub audience_binding: Option<camel_api::security_policy::AudienceBinding>,
}

/// Named registry of authentication providers.
///
/// Entries are stored as `Arc<ProviderEntry>`; [`ProviderRegistry::resolve`]
/// clones the `Arc` out of the DashMap so callers hold their own strong
/// reference independent of the map guard's lifetime.
pub struct ProviderRegistry {
    inner: NamedRegistry<ProviderEntry>,
    authn_cache: Option<Arc<AuthnCache>>,
}

impl Default for ProviderRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ProviderRegistry {
    pub fn new() -> Self {
        Self {
            inner: NamedRegistry::new(),
            authn_cache: None,
        }
    }

    /// Attach the authn result cache (Task 3.2). [`kernel_authenticate`]
    /// consults it via [`Self::authn_cache`] before calling a provider.
    ///
    /// [`kernel_authenticate`]: crate::kernel::kernel_authenticate
    pub fn with_authn_cache(mut self, cache: Arc<AuthnCache>) -> Self {
        self.authn_cache = Some(cache);
        self
    }

    /// The attached authn result cache, when present.
    pub fn authn_cache(&self) -> Option<&Arc<AuthnCache>> {
        self.authn_cache.as_ref()
    }

    pub fn register(&self, name: impl Into<String>, entry: ProviderEntry) {
        self.inner.register(name, Arc::new(entry));
    }

    pub fn resolve(&self, name: &str) -> Option<Arc<ProviderEntry>> {
        self.inner.get(name)
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    pub fn names(&self) -> Vec<String> {
        self.inner
            .entries()
            .into_iter()
            .map(|(name, _)| name)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use camel_api::security_policy::{
        AuthContext, AuthPrincipal, AuthorizationDecision, Principal, TransportId,
    };
    use camel_api::{CamelError, Exchange, Message};

    struct AllowPolicy;
    struct DenyPolicy;

    #[async_trait]
    impl SecurityPolicy for AllowPolicy {
        async fn evaluate(
            &self,
            _exchange: &mut Exchange,
            _auth: &AuthContext<'_>,
        ) -> Result<AuthorizationDecision, CamelError> {
            Ok(AuthorizationDecision::Granted {
                principal: Principal {
                    subject: "allow-user".into(),
                    issuer: "test".into(),
                    audience: vec![],
                    scopes: vec![],
                    roles: vec![],
                    claims: serde_json::Value::Null,
                },
            })
        }
    }

    #[async_trait]
    impl SecurityPolicy for DenyPolicy {
        async fn evaluate(
            &self,
            _exchange: &mut Exchange,
            _auth: &AuthContext<'_>,
        ) -> Result<AuthorizationDecision, CamelError> {
            Ok(AuthorizationDecision::Denied {
                reason: "deny".into(),
                required: vec![],
                actual: vec![],
            })
        }
    }

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

    fn test_principal() -> TestPrincipal {
        TestPrincipal(Principal {
            subject: "allow-user".into(),
            issuer: "test".into(),
            audience: vec![],
            scopes: vec![],
            roles: vec![],
            claims: serde_json::Value::Null,
        })
    }

    #[test]
    fn register_and_get() {
        let registry = SecurityPolicyRegistry::new();
        registry.register("admin-policy", Arc::new(AllowPolicy));
        let policy = registry.get("admin-policy");
        assert!(policy.is_some());
    }

    #[test]
    fn get_missing_returns_none() {
        let registry = SecurityPolicyRegistry::new();
        assert!(registry.get("nonexistent").is_none());
    }

    #[tokio::test]
    async fn register_replaces_existing() {
        let registry = SecurityPolicyRegistry::new();
        registry.register("my-policy", Arc::new(AllowPolicy));
        registry.register("my-policy", Arc::new(DenyPolicy));
        let policy = registry.get("my-policy").unwrap();
        let mut ex = Exchange::new(Message::default());
        let principal = test_principal();
        let auth = auth_ctx(&principal);
        // DenyPolicy was registered last — must be returned
        let decision = policy.evaluate(&mut ex, &auth).await.unwrap();
        assert!(matches!(decision, AuthorizationDecision::Denied { .. }));
    }

    // --- PermissionEvaluatorRegistry tests ---

    use crate::permission::{PermissionDecision, PermissionRequest};

    struct GrantEvaluator;

    #[async_trait]
    impl PermissionEvaluator for GrantEvaluator {
        async fn evaluate(
            &self,
            _request: PermissionRequest,
        ) -> Result<PermissionDecision, crate::types::AuthError> {
            Ok(PermissionDecision::Granted)
        }
    }

    struct DenyEvaluator {
        reason: String,
    }

    #[async_trait]
    impl PermissionEvaluator for DenyEvaluator {
        async fn evaluate(
            &self,
            _request: PermissionRequest,
        ) -> Result<PermissionDecision, crate::types::AuthError> {
            Ok(PermissionDecision::Denied {
                reason: self.reason.clone(),
            })
        }
    }

    #[test]
    fn evaluator_register_and_get() {
        let registry = PermissionEvaluatorRegistry::new();
        registry.register("keycloak-uma", Arc::new(GrantEvaluator));
        let evaluator = registry.get("keycloak-uma");
        assert!(evaluator.is_some());
    }

    #[test]
    fn evaluator_get_missing_returns_none() {
        let registry = PermissionEvaluatorRegistry::new();
        assert!(registry.get("nonexistent").is_none());
    }

    #[test]
    fn entries_returns_registered_items() {
        let registry = SecurityPolicyRegistry::new();
        registry.register("admin-policy", Arc::new(AllowPolicy));
        let entries = registry.entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].0, "admin-policy");
    }

    #[test]
    fn is_empty_returns_true_when_no_entries() {
        let registry = SecurityPolicyRegistry::new();
        assert!(registry.is_empty());
    }

    #[test]
    fn is_empty_returns_false_when_entries_exist() {
        let registry = SecurityPolicyRegistry::new();
        registry.register("p1", Arc::new(AllowPolicy));
        assert!(!registry.is_empty());
    }

    #[tokio::test]
    async fn evaluator_register_replaces_existing() {
        let registry = PermissionEvaluatorRegistry::new();
        registry.register("my-evaluator", Arc::new(GrantEvaluator));
        registry.register(
            "my-evaluator",
            Arc::new(DenyEvaluator {
                reason: "replaced".into(),
            }),
        );
        let evaluator = registry.get("my-evaluator").unwrap();
        let request = PermissionRequest {
            principal: Principal {
                subject: "test".into(),
                issuer: "test".into(),
                audience: vec![],
                scopes: vec![],
                roles: vec![],
                claims: serde_json::Value::Null,
            },
            resource: "/test".into(),
            action: "read".into(),
            requested_scopes: vec![],
            context: serde_json::Value::Null,
        };
        let decision = evaluator.evaluate(request).await.unwrap();
        assert!(
            matches!(decision, PermissionDecision::Denied { .. }),
            "expected Denied, got {decision:?}"
        );
    }

    // --- ProviderRegistry tests ---

    struct StaticAuth;

    #[async_trait]
    impl TokenAuthenticator for StaticAuth {
        async fn authenticate_bearer(&self, _token: &str) -> Result<Principal, CamelError> {
            Ok(Principal {
                subject: "static-user".into(),
                issuer: "test".into(),
                audience: vec![],
                scopes: vec![],
                roles: vec![],
                claims: serde_json::Value::Null,
            })
        }
    }

    fn provider_entry() -> ProviderEntry {
        ProviderEntry {
            authenticator: Arc::new(StaticAuth),
            audience_binding: None,
        }
    }

    #[test]
    fn provider_registry_registers_and_resolves() {
        let registry = ProviderRegistry::new();
        registry.register("idp-a", provider_entry());
        assert!(registry.resolve("idp-a").is_some());
        assert!(registry.resolve("ghost").is_none());
    }

    #[test]
    fn sole_and_multiple_provider_counts() {
        let registry = ProviderRegistry::new();
        registry.register("idp-a", provider_entry());
        assert_eq!(registry.len(), 1);
        registry.register("idp-b", provider_entry());
        assert_eq!(registry.len(), 2);
    }
}
