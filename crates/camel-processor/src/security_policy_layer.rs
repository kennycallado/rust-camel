use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use tower::{Layer, Service};

use camel_api::security_policy::{
    AuthorizationDecision, SecurityPolicy, store_principal_properties,
};
use camel_api::{CamelError, Exchange};

#[derive(Clone)]
pub struct SecurityPolicyLayer {
    policy: Arc<dyn SecurityPolicy>,
}

impl SecurityPolicyLayer {
    pub fn new(policy: Arc<dyn SecurityPolicy>) -> Self {
        Self { policy }
    }
}

impl<S> Layer<S> for SecurityPolicyLayer {
    type Service = SecurityPolicyService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        SecurityPolicyService {
            inner,
            policy: Arc::clone(&self.policy),
        }
    }
}

pub struct SecurityPolicyService<S> {
    inner: S,
    policy: Arc<dyn SecurityPolicy>,
}

impl<S: Clone> Clone for SecurityPolicyService<S> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            policy: Arc::clone(&self.policy),
        }
    }
}

impl<S> Service<Exchange> for SecurityPolicyService<S>
where
    S: Service<Exchange, Response = Exchange, Error = CamelError> + Clone + Send + 'static,
    S::Future: Send,
{
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut exchange: Exchange) -> Self::Future {
        let policy = Arc::clone(&self.policy);
        let clone = self.inner.clone();
        let mut inner = std::mem::replace(&mut self.inner, clone);

        Box::pin(async move {
            match policy.evaluate(&mut exchange).await {
                Ok(AuthorizationDecision::Granted { principal }) => {
                    store_principal_properties(&mut exchange, &principal);
                    inner.call(exchange).await
                }
                Ok(AuthorizationDecision::Denied {
                    reason,
                    required,
                    actual,
                }) => {
                    let msg = format!(
                        "Access denied: {reason}. Required: {required:?}, actual: {actual:?}"
                    );
                    Err(CamelError::Unauthorized(msg))
                }
                Err(e) => Err(e),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use camel_api::security_policy::{
        PRINCIPAL_AUDIENCE_KEY, PRINCIPAL_CLAIMS_KEY, PRINCIPAL_ISSUER_KEY, PRINCIPAL_KEY,
        PRINCIPAL_ROLES_KEY, PRINCIPAL_SCOPES_KEY, PRINCIPAL_SUBJECT_KEY, Principal,
    };
    use camel_api::{BoxProcessor, BoxProcessorExt, Message};
    use std::sync::atomic::{AtomicU32, Ordering};
    use tower::ServiceExt;

    fn make_exchange() -> Exchange {
        Exchange::new(Message::new("test"))
    }

    fn ok_processor() -> BoxProcessor {
        BoxProcessor::from_fn(|ex| Box::pin(async move { Ok(ex) }))
    }

    fn test_principal() -> Principal {
        Principal {
            subject: "user1".into(),
            issuer: "test-issuer".into(),
            audience: vec!["api".into()],
            scopes: vec!["read".into()],
            roles: vec!["admin".into()],
            claims: serde_json::json!({"sub": "user1"}),
        }
    }

    struct GrantPolicy;
    #[async_trait]
    impl SecurityPolicy for GrantPolicy {
        async fn evaluate(
            &self,
            _exchange: &mut Exchange,
        ) -> Result<AuthorizationDecision, CamelError> {
            Ok(AuthorizationDecision::Granted {
                principal: test_principal(),
            })
        }
    }

    struct DenyPolicy;
    #[async_trait]
    impl SecurityPolicy for DenyPolicy {
        async fn evaluate(
            &self,
            _exchange: &mut Exchange,
        ) -> Result<AuthorizationDecision, CamelError> {
            Ok(AuthorizationDecision::Denied {
                reason: "missing role".into(),
                required: vec!["admin".into()],
                actual: vec!["user".into()],
            })
        }
    }

    struct FailPolicy;
    #[async_trait]
    impl SecurityPolicy for FailPolicy {
        async fn evaluate(
            &self,
            _exchange: &mut Exchange,
        ) -> Result<AuthorizationDecision, CamelError> {
            Err(CamelError::Unauthenticated("invalid token".into()))
        }
    }

    #[tokio::test]
    async fn test_granted_stores_properties() {
        let layer = SecurityPolicyLayer::new(Arc::new(GrantPolicy));
        let mut svc = layer.layer(ok_processor());
        let result = svc.ready().await.unwrap().call(make_exchange()).await;
        assert!(result.is_ok());
        let ex = result.unwrap();
        assert_eq!(
            ex.property(PRINCIPAL_SUBJECT_KEY),
            Some(&serde_json::Value::String("user1".into()))
        );
        assert_eq!(
            ex.property(PRINCIPAL_ISSUER_KEY),
            Some(&serde_json::Value::String("test-issuer".into()))
        );
        assert!(ex.property(PRINCIPAL_KEY).is_some());
    }

    #[tokio::test]
    async fn test_denied_returns_unauthorized_error() {
        let layer = SecurityPolicyLayer::new(Arc::new(DenyPolicy));
        let mut svc = layer.layer(ok_processor());
        let result = svc.ready().await.unwrap().call(make_exchange()).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            CamelError::Unauthorized(msg) => assert!(msg.contains("missing role")),
            other => panic!("expected Unauthorized, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_denied_error_contains_required_actual() {
        let layer = SecurityPolicyLayer::new(Arc::new(DenyPolicy));
        let mut svc = layer.layer(ok_processor());
        let result = svc.ready().await.unwrap().call(make_exchange()).await;
        let msg = match result.unwrap_err() {
            CamelError::Unauthorized(msg) => msg,
            other => panic!("expected Unauthorized, got: {other:?}"),
        };
        assert!(msg.contains("admin"));
        assert!(msg.contains("user"));
    }

    #[tokio::test]
    async fn test_evaluate_error_propagates() {
        let layer = SecurityPolicyLayer::new(Arc::new(FailPolicy));
        let mut svc = layer.layer(ok_processor());
        let result = svc.ready().await.unwrap().call(make_exchange()).await;
        match result.unwrap_err() {
            CamelError::Unauthenticated(msg) => assert!(msg.contains("invalid token")),
            other => panic!("expected Unauthenticated, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_multiple_calls_share_policy() {
        let count = Arc::new(AtomicU32::new(0));
        struct CountingPolicy {
            count: Arc<AtomicU32>,
        }
        #[async_trait]
        impl SecurityPolicy for CountingPolicy {
            async fn evaluate(
                &self,
                _exchange: &mut Exchange,
            ) -> Result<AuthorizationDecision, CamelError> {
                self.count.fetch_add(1, Ordering::SeqCst);
                Ok(AuthorizationDecision::Granted {
                    principal: Principal {
                        subject: "user1".into(),
                        issuer: "test".into(),
                        audience: vec![],
                        scopes: vec![],
                        roles: vec![],
                        claims: serde_json::Value::Null,
                    },
                })
            }
        }
        let policy = Arc::new(CountingPolicy {
            count: Arc::clone(&count),
        });
        let layer = SecurityPolicyLayer::new(Arc::clone(&policy) as Arc<dyn SecurityPolicy>);
        let mut svc = layer.layer(ok_processor());
        for _ in 0..3 {
            let result = svc.ready().await.unwrap().call(make_exchange()).await;
            assert!(result.is_ok());
        }
        assert_eq!(count.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn test_granted_all_property_json_formats() {
        let layer = SecurityPolicyLayer::new(Arc::new(GrantPolicy));
        let mut svc = layer.layer(ok_processor());
        let result = svc.ready().await.unwrap().call(make_exchange()).await;
        let ex = result.unwrap();

        let roles: Vec<String> =
            serde_json::from_str(ex.property(PRINCIPAL_ROLES_KEY).unwrap().as_str().unwrap())
                .unwrap();
        assert_eq!(roles, vec!["admin"]);

        let scopes: Vec<String> =
            serde_json::from_str(ex.property(PRINCIPAL_SCOPES_KEY).unwrap().as_str().unwrap())
                .unwrap();
        assert_eq!(scopes, vec!["read"]);

        let audience: Vec<String> = serde_json::from_str(
            ex.property(PRINCIPAL_AUDIENCE_KEY)
                .unwrap()
                .as_str()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(audience, vec!["api"]);

        let claims: serde_json::Value =
            serde_json::from_str(ex.property(PRINCIPAL_CLAIMS_KEY).unwrap().as_str().unwrap())
                .unwrap();
        assert_eq!(claims["sub"], "user1");
    }

    #[tokio::test]
    async fn test_granted_empty_principal_fields() {
        struct EmptyPrincipalPolicy;
        #[async_trait]
        impl SecurityPolicy for EmptyPrincipalPolicy {
            async fn evaluate(
                &self,
                _exchange: &mut Exchange,
            ) -> Result<AuthorizationDecision, CamelError> {
                Ok(AuthorizationDecision::Granted {
                    principal: Principal {
                        subject: "minimal".into(),
                        issuer: String::new(),
                        audience: vec![],
                        scopes: vec![],
                        roles: vec![],
                        claims: serde_json::Value::Null,
                    },
                })
            }
        }
        let layer = SecurityPolicyLayer::new(Arc::new(EmptyPrincipalPolicy));
        let mut svc = layer.layer(ok_processor());
        let result = svc.ready().await.unwrap().call(make_exchange()).await;
        let ex = result.unwrap();

        assert_eq!(
            ex.property(PRINCIPAL_SUBJECT_KEY),
            Some(&serde_json::Value::String("minimal".into()))
        );
        assert_eq!(
            ex.property(PRINCIPAL_ISSUER_KEY),
            Some(&serde_json::Value::String(String::new()))
        );
        let roles: Vec<String> =
            serde_json::from_str(ex.property(PRINCIPAL_ROLES_KEY).unwrap().as_str().unwrap())
                .unwrap();
        assert!(roles.is_empty());
    }

    #[tokio::test]
    async fn test_layer_clone_produces_working_service() {
        let layer = SecurityPolicyLayer::new(Arc::new(GrantPolicy));
        let mut svc1 = layer.layer(ok_processor());
        let svc2 = svc1.clone();

        let r1 = svc1.ready().await.unwrap().call(make_exchange()).await;
        let mut svc2 = svc2;
        let r2 = svc2.ready().await.unwrap().call(make_exchange()).await;
        assert!(r1.is_ok());
        assert!(r2.is_ok());
    }

    #[tokio::test]
    async fn test_granted_preserves_original_exchange_properties() {
        struct GrantPolicy;
        #[async_trait]
        impl SecurityPolicy for GrantPolicy {
            async fn evaluate(
                &self,
                _exchange: &mut Exchange,
            ) -> Result<AuthorizationDecision, CamelError> {
                Ok(AuthorizationDecision::Granted {
                    principal: Principal {
                        subject: "u".into(),
                        issuer: "i".into(),
                        audience: vec![],
                        scopes: vec![],
                        roles: vec![],
                        claims: serde_json::Value::Null,
                    },
                })
            }
        }
        let layer = SecurityPolicyLayer::new(Arc::new(GrantPolicy));
        let mut svc = layer.layer(ok_processor());
        let mut ex = make_exchange();
        ex.set_property("custom.key", "custom-value");
        let result = svc.ready().await.unwrap().call(ex).await;
        let ex = result.unwrap();
        assert_eq!(
            ex.property("custom.key"),
            Some(&serde_json::Value::String("custom-value".into()))
        );
        assert!(ex.property(PRINCIPAL_SUBJECT_KEY).is_some());
    }
}
