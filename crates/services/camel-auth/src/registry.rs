use crate::permission::PermissionEvaluator;
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

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use camel_api::security_policy::{AuthorizationDecision, Principal};
    use camel_api::{CamelError, Exchange, Message};

    struct AllowPolicy;
    struct DenyPolicy;

    #[async_trait]
    impl SecurityPolicy for AllowPolicy {
        async fn evaluate(
            &self,
            _exchange: &mut Exchange,
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
        ) -> Result<AuthorizationDecision, CamelError> {
            Ok(AuthorizationDecision::Denied {
                reason: "deny".into(),
                required: vec![],
                actual: vec![],
            })
        }
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
        // DenyPolicy was registered last — must be returned
        let decision = policy.evaluate(&mut ex).await.unwrap();
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
}
