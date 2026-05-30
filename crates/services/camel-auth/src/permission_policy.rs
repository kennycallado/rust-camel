//! Bridge between [`SecurityPolicy`] (Exchange-level) and [`PermissionEvaluator`] (permission-level).
//!
//! [`PermissionPolicy`] resolves resource and action from the exchange (literal, header, or property),
//! builds an evaluation context from configured headers/properties, and delegates to a
//! [`PermissionEvaluator`] implementation.

use std::sync::Arc;

use async_trait::async_trait;
use camel_api::security_policy::{AuthorizationDecision, SecurityPolicy, principal_from_exchange};
use camel_api::{CamelError, Exchange};

use crate::permission::{
    PermissionContextConfig, PermissionDecision, PermissionEvaluator, PermissionRequest,
    PermissionValueSource,
};

/// Where to read the resource/action label for error messages.
trait LabelSource {
    fn label(&self) -> String;
}

impl LabelSource for PermissionValueSource {
    fn label(&self) -> String {
        match self {
            PermissionValueSource::Literal(s) => s.clone(),
            PermissionValueSource::Header(name) => format!("header:{name}"),
            PermissionValueSource::Property(name) => format!("property:{name}"),
        }
    }
}

/// SecurityPolicy implementation that delegates to a [`PermissionEvaluator`].
///
/// Resolves resource and action from the exchange, builds an evaluation context
/// from configured headers and properties, and translates the evaluator's decision
/// into an [`AuthorizationDecision`].
pub struct PermissionPolicy {
    evaluator: Arc<dyn PermissionEvaluator>,
    resource: PermissionValueSource,
    action: PermissionValueSource,
    scopes: Vec<String>,
    context: PermissionContextConfig,
}

impl PermissionPolicy {
    pub fn new(
        evaluator: Arc<dyn PermissionEvaluator>,
        resource: PermissionValueSource,
        action: PermissionValueSource,
        scopes: Vec<String>,
        context: PermissionContextConfig,
    ) -> Self {
        Self {
            evaluator,
            resource,
            action,
            scopes,
            context,
        }
    }

    fn resolve_source(source: &PermissionValueSource, exchange: &Exchange) -> Option<String> {
        match source {
            PermissionValueSource::Literal(s) => Some(s.clone()),
            PermissionValueSource::Header(name) => exchange
                .input
                .header(name)
                .and_then(|v| v.as_str())
                .map(String::from),
            PermissionValueSource::Property(name) => exchange
                .property(name)
                .and_then(|v| v.as_str())
                .map(String::from),
        }
    }

    fn build_context(&self, exchange: &Exchange) -> serde_json::Value {
        let mut map = serde_json::Map::new();
        for name in &self.context.include_headers {
            if let Some(v) = exchange.input.header(name) {
                map.insert(name.clone(), v.clone());
            }
        }
        for name in &self.context.include_properties {
            if let Some(v) = exchange.property(name) {
                map.insert(name.clone(), v.clone());
            }
        }
        serde_json::Value::Object(map)
    }

    fn resource_label(&self) -> String {
        self.resource.label()
    }

    fn action_label(&self) -> String {
        self.action.label()
    }
}

#[async_trait]
impl SecurityPolicy for PermissionPolicy {
    async fn evaluate(&self, exchange: &mut Exchange) -> Result<AuthorizationDecision, CamelError> {
        let principal = principal_from_exchange(exchange)
            .ok_or_else(|| CamelError::Unauthenticated("no principal in exchange".into()))?;
        let resource = Self::resolve_source(&self.resource, exchange)
            .ok_or_else(|| CamelError::Unauthorized("cannot resolve permission resource".into()))?;
        let action = Self::resolve_source(&self.action, exchange)
            .ok_or_else(|| CamelError::Unauthorized("cannot resolve permission action".into()))?;
        let context = self.build_context(exchange);
        let request = PermissionRequest {
            principal: principal.clone(),
            resource,
            action,
            requested_scopes: self.scopes.clone(),
            context,
        };
        match self.evaluator.evaluate(request).await {
            Ok(PermissionDecision::Granted) => Ok(AuthorizationDecision::Granted { principal }),
            Ok(PermissionDecision::Denied { reason }) => Ok(AuthorizationDecision::Denied {
                reason,
                required: vec![format!("{}:{}", self.resource_label(), self.action_label())],
                actual: vec![],
            }),
            Err(e) => Err(e.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permission::{
        PermissionContextConfig, PermissionDecision, PermissionEvaluator, PermissionRequest,
        PermissionValueSource,
    };
    use crate::types::AuthError;
    use camel_api::Message;
    use camel_api::security_policy::{Principal, store_principal_properties};
    use serde_json::json;

    fn test_principal() -> Principal {
        Principal {
            subject: "alice".into(),
            issuer: "https://keycloak.example.com/realms/test".into(),
            audience: vec!["camel-api".into()],
            roles: vec!["admin".into()],
            scopes: vec!["read".into()],
            claims: json!({}),
        }
    }

    fn exchange_with_principal(principal: &Principal) -> Exchange {
        let mut ex = Exchange::new(Message::default());
        store_principal_properties(&mut ex, principal);
        ex
    }

    // --- Mock evaluators ---

    struct GrantEvaluator;

    #[async_trait]
    impl PermissionEvaluator for GrantEvaluator {
        async fn evaluate(
            &self,
            _request: PermissionRequest,
        ) -> Result<PermissionDecision, AuthError> {
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
        ) -> Result<PermissionDecision, AuthError> {
            Ok(PermissionDecision::Denied {
                reason: self.reason.clone(),
            })
        }
    }

    /// Evaluator that checks the resource field matches an expected value.
    struct CheckEvaluator {
        expected_resource: String,
    }

    #[async_trait]
    impl PermissionEvaluator for CheckEvaluator {
        async fn evaluate(
            &self,
            request: PermissionRequest,
        ) -> Result<PermissionDecision, AuthError> {
            if request.resource == self.expected_resource {
                Ok(PermissionDecision::Granted)
            } else {
                Ok(PermissionDecision::Denied {
                    reason: format!(
                        "expected resource '{}', got '{}'",
                        self.expected_resource, request.resource
                    ),
                })
            }
        }
    }

    /// Evaluator that asserts specific keys are present/absent in the context.
    struct ContextCheckEvaluator {
        must_have: String,
        must_not_have: String,
    }

    #[async_trait]
    impl PermissionEvaluator for ContextCheckEvaluator {
        async fn evaluate(
            &self,
            request: PermissionRequest,
        ) -> Result<PermissionDecision, AuthError> {
            let ctx = request
                .context
                .as_object()
                .expect("context should be an object");
            if !ctx.contains_key(&self.must_have) {
                return Ok(PermissionDecision::Denied {
                    reason: format!("context missing required key '{}'", self.must_have),
                });
            }
            if ctx.contains_key(&self.must_not_have) {
                return Ok(PermissionDecision::Denied {
                    reason: format!("context should not contain key '{}'", self.must_not_have),
                });
            }
            Ok(PermissionDecision::Granted)
        }
    }

    fn default_context_config() -> PermissionContextConfig {
        PermissionContextConfig::default()
    }

    // --- Tests ---

    #[tokio::test]
    async fn grants_when_evaluator_grants() {
        let principal = test_principal();
        let policy = PermissionPolicy::new(
            Arc::new(GrantEvaluator),
            PermissionValueSource::Literal("/orders".into()),
            PermissionValueSource::Literal("read".into()),
            vec![],
            default_context_config(),
        );
        let mut ex = exchange_with_principal(&principal);
        let decision = policy.evaluate(&mut ex).await.unwrap();
        match decision {
            AuthorizationDecision::Granted { principal: p } => {
                assert_eq!(p.subject, "alice");
            }
            AuthorizationDecision::Denied { .. } => panic!("expected Granted, got Denied"),
        }
    }

    #[tokio::test]
    async fn denies_when_evaluator_denies() {
        let principal = test_principal();
        let policy = PermissionPolicy::new(
            Arc::new(DenyEvaluator {
                reason: "insufficient scope".into(),
            }),
            PermissionValueSource::Literal("/orders".into()),
            PermissionValueSource::Literal("write".into()),
            vec![],
            default_context_config(),
        );
        let mut ex = exchange_with_principal(&principal);
        let decision = policy.evaluate(&mut ex).await.unwrap();
        match decision {
            AuthorizationDecision::Denied { reason, .. } => {
                assert_eq!(reason, "insufficient scope");
            }
            AuthorizationDecision::Granted { .. } => panic!("expected Denied, got Granted"),
        }
    }

    #[tokio::test]
    async fn unauthenticated_when_no_principal() {
        let policy = PermissionPolicy::new(
            Arc::new(GrantEvaluator),
            PermissionValueSource::Literal("/orders".into()),
            PermissionValueSource::Literal("read".into()),
            vec![],
            default_context_config(),
        );
        let mut ex = Exchange::new(Message::default());
        let result = policy.evaluate(&mut ex).await;
        assert!(
            matches!(result, Err(CamelError::Unauthenticated(ref msg)) if msg.contains("no principal")),
            "expected Unauthenticated error, got {:?}",
            result
        );
    }

    #[tokio::test]
    async fn resolves_header_source() {
        let principal = test_principal();
        let policy = PermissionPolicy::new(
            Arc::new(CheckEvaluator {
                expected_resource: "res-from-header".into(),
            }),
            PermissionValueSource::Header("X-Resource".into()),
            PermissionValueSource::Literal("read".into()),
            vec![],
            default_context_config(),
        );
        let mut ex = exchange_with_principal(&principal);
        ex.input.set_header("X-Resource", "res-from-header");
        let decision = policy.evaluate(&mut ex).await.unwrap();
        assert!(
            matches!(decision, AuthorizationDecision::Granted { .. }),
            "expected Granted, got {:?}",
            decision
        );
    }

    #[tokio::test]
    async fn unauthorized_when_resource_cannot_be_resolved() {
        let principal = test_principal();
        let policy = PermissionPolicy::new(
            Arc::new(GrantEvaluator),
            PermissionValueSource::Header("X-Resource".into()),
            PermissionValueSource::Literal("read".into()),
            vec![],
            default_context_config(),
        );
        let mut ex = exchange_with_principal(&principal);
        // X-Resource header NOT set → cannot resolve
        let result = policy.evaluate(&mut ex).await;
        assert!(
            matches!(result, Err(CamelError::Unauthorized(ref msg)) if msg.contains("cannot resolve permission resource")),
            "expected Unauthorized error for unresolved resource, got {:?}",
            result
        );
    }

    #[tokio::test]
    async fn context_includes_only_configured_fields() {
        let principal = test_principal();
        let context_config = PermissionContextConfig {
            include_headers: vec!["X-Tenant".into()],
            include_properties: vec![],
        };
        let policy = PermissionPolicy::new(
            Arc::new(ContextCheckEvaluator {
                must_have: "X-Tenant".into(),
                must_not_have: "X-Other".into(),
            }),
            PermissionValueSource::Literal("/orders".into()),
            PermissionValueSource::Literal("read".into()),
            vec![],
            context_config,
        );
        let mut ex = exchange_with_principal(&principal);
        ex.input.set_header("X-Tenant", "acme");
        ex.input.set_header("X-Other", "should-not-appear");
        let decision = policy.evaluate(&mut ex).await.unwrap();
        assert!(
            matches!(decision, AuthorizationDecision::Granted { .. }),
            "expected Granted, got {:?}",
            decision
        );
    }
}
