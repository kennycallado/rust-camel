use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use tower::{Layer, Service};

use camel_api::security_policy::{
    AuthContext, AuthPrincipal, AuthorizationDecision, SecurityPolicy, TransportId,
    store_principal_properties,
};
use camel_api::{CamelError, Exchange};

/// Carrier-only authorization layer (ADR-0061 Task 2.9 strict mode).
///
/// The layer NEVER authenticates: transports mint the typed carrier at the
/// request boundary (`kernel_authenticate` + `install_carrier`) and the
/// pre-pipeline dispatch check rejects carrier-less Exchanges on non-Public
/// routes before the pipeline runs. The layer therefore sees an Exchange
/// that already carries the sealed [`AuthenticatedPrincipal`] and only
/// evaluates the route policy against it — no carrier, no authorization
/// path (fail closed). The Phase-1 legacy Bearer and anonymous-principal
/// branches were deleted here.
#[derive(Clone)]
pub struct SecurityPolicyLayer {
    policy: Arc<dyn SecurityPolicy>,
    transport: TransportId,
}

impl SecurityPolicyLayer {
    pub fn new(policy: Arc<dyn SecurityPolicy>, transport: TransportId) -> Self {
        Self { policy, transport }
    }
}

impl<S> Layer<S> for SecurityPolicyLayer {
    type Service = SecurityPolicyService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        SecurityPolicyService {
            inner,
            policy: Arc::clone(&self.policy),
            transport: self.transport,
        }
    }
}

pub struct SecurityPolicyService<S> {
    inner: S,
    policy: Arc<dyn SecurityPolicy>,
    transport: TransportId,
}

impl<S: Clone> Clone for SecurityPolicyService<S> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            policy: Arc::clone(&self.policy),
            transport: self.transport,
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

    fn call(&mut self, exchange: Exchange) -> Self::Future {
        let policy = Arc::clone(&self.policy);
        let transport = self.transport;
        let clone = self.inner.clone();
        let inner = std::mem::replace(&mut self.inner, clone);

        Box::pin(async move {
            // Strict mode (Task 2.9): the typed carrier is the ONLY
            // authentication evidence. `read_carrier` clones the
            // `AuthenticatedPrincipal` out, ending the extension borrow before
            // `evaluate(&mut exchange, ..)` (avoids E0502). No carrier → no
            // authorization path, fail closed.
            let principal = camel_auth::kernel::read_carrier(&exchange).ok_or_else(|| {
                CamelError::Unauthenticated("no authenticated principal present".to_string())
            })?;
            evaluate(policy, exchange, &principal, transport, inner).await
        })
    }
}

/// Evaluate the policy against `principal` and forward (or deny).
///
/// The principal is always the kernel-minted typed carrier (strict mode).
/// On `Granted`, stores the advisory principal properties and forwards to
/// the inner service.
async fn evaluate<S>(
    policy: Arc<dyn SecurityPolicy>,
    mut exchange: Exchange,
    principal: &dyn AuthPrincipal,
    transport: TransportId,
    mut inner: S,
) -> Result<Exchange, CamelError>
where
    S: Service<Exchange, Response = Exchange, Error = CamelError> + Send + 'static,
    S::Future: Send,
{
    let auth = AuthContext {
        principal,
        transport,
    };
    match policy.evaluate(&mut exchange, &auth).await {
        Ok(AuthorizationDecision::Granted { principal }) => {
            store_principal_properties(&mut exchange, &principal);
            inner.call(exchange).await
        }
        Ok(AuthorizationDecision::Denied {
            reason,
            required,
            actual,
        }) => {
            let msg =
                format!("Access denied: {reason}. Required: {required:?}, actual: {actual:?}");
            Err(CamelError::Unauthorized(msg))
        }
        Err(e) => Err(e),
        // Future AuthorizationDecision variants fail closed.
        _ => Err(CamelError::Unauthorized(
            "access denied by security policy".to_string(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use camel_api::security_policy::{
        AccessMode, CredentialSource, PRINCIPAL_AUDIENCE_KEY, PRINCIPAL_CLAIMS_KEY,
        PRINCIPAL_ISSUER_KEY, PRINCIPAL_KEY, PRINCIPAL_ROLES_KEY, PRINCIPAL_SCOPES_KEY,
        PRINCIPAL_SUBJECT_KEY, Principal, RouteSecurityPlan,
    };
    use camel_api::{BoxProcessor, BoxProcessorExt, Message};
    use camel_auth::TokenAuthenticator;
    use camel_auth::credential_source::ExtractedToken;
    use camel_auth::kernel::{KERNEL_PRINCIPAL_KEY, install_carrier, kernel_authenticate};
    use camel_auth::native_auth::{
        NativeCredential, NativeCredentialSecret, NativeCredentialStore,
    };
    use camel_auth::{ProviderEntry, ProviderRegistry, StaticTokenAuthenticator};
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU32, Ordering};
    use tower::ServiceExt;
    use zeroize::Zeroizing;

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

    /// Build a `StaticTokenAuthenticator` that accepts `token` and returns
    /// `test_principal()`.
    fn static_authenticator(token: &str) -> Arc<dyn TokenAuthenticator> {
        let store = NativeCredentialStore::try_new(vec![NativeCredential {
            secret: NativeCredentialSecret::Plaintext {
                value: Zeroizing::new(token.to_string()),
            },
            principal: test_principal(),
        }])
        .unwrap();
        Arc::new(StaticTokenAuthenticator::new(store))
    }

    /// A registry holding a single provider `id` whose token is `token`.
    fn provider_registry(id: &str, token: &str) -> ProviderRegistry {
        let registry = ProviderRegistry::new();
        registry.register(
            id,
            ProviderEntry {
                authenticator: static_authenticator(token),
                audience_binding: None,
            },
        );
        registry
    }

    fn authenticated_plan(provider_ref: &str) -> RouteSecurityPlan {
        RouteSecurityPlan {
            access_mode: AccessMode::Authenticated,
            provider_ref: Some(provider_ref.to_string()),
            transport: TransportId::Http,
            credential_sources: vec![CredentialSource::AuthorizationHeader],
            audience_binding: None,
        }
    }

    fn credentials(token: &str) -> ExtractedToken {
        ExtractedToken {
            token: token.to_string(),
            source: CredentialSource::AuthorizationHeader,
        }
    }

    /// Build a layer (policy-only route: no carrier minter in play).
    fn policy_only_layer(policy: Arc<dyn SecurityPolicy>) -> SecurityPolicyLayer {
        SecurityPolicyLayer::new(policy, TransportId::Http)
    }

    /// Mint a real carrier through the kernel (strict-mode input contract:
    /// the layer only authorizes carrier-carrying Exchanges).
    async fn minted_principal() -> camel_auth::AuthenticatedPrincipal {
        let providers = provider_registry("idp-a", "t-a");
        let plan = authenticated_plan("idp-a");
        kernel_authenticate(&plan, &providers, &credentials("t-a"))
            .await
            .expect("kernel mints test carrier")
    }

    /// An Exchange carrying the kernel-minted carrier.
    async fn carrier_exchange() -> Exchange {
        let principal = minted_principal().await;
        let mut exchange = make_exchange();
        install_carrier(&mut exchange, &principal);
        exchange
    }

    struct GrantPolicy;
    #[async_trait]
    impl SecurityPolicy for GrantPolicy {
        async fn evaluate(
            &self,
            _exchange: &mut Exchange,
            _auth: &AuthContext<'_>,
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
            _auth: &AuthContext<'_>,
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
            _auth: &AuthContext<'_>,
        ) -> Result<AuthorizationDecision, CamelError> {
            Err(CamelError::Unauthenticated("invalid token".into()))
        }
    }

    /// Records the `AuthContext` principal each call observes, then grants.
    struct RecordingGrantPolicy {
        count: AtomicU32,
        seen: Mutex<Vec<(String, String)>>, // (subject, provider_id)
    }

    #[async_trait]
    impl SecurityPolicy for RecordingGrantPolicy {
        async fn evaluate(
            &self,
            _exchange: &mut Exchange,
            auth: &AuthContext<'_>,
        ) -> Result<AuthorizationDecision, CamelError> {
            self.count.fetch_add(1, Ordering::SeqCst);
            self.seen.lock().unwrap().push((
                auth.principal.principal().subject.clone(),
                auth.principal.provider_id().to_string(),
            ));
            Ok(AuthorizationDecision::Granted {
                principal: test_principal(),
            })
        }
    }

    #[tokio::test]
    async fn test_granted_stores_properties() {
        let layer = policy_only_layer(Arc::new(GrantPolicy));
        let mut svc = layer.layer(ok_processor());
        let result = svc
            .ready()
            .await
            .unwrap()
            .call(carrier_exchange().await)
            .await;
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
        let layer = policy_only_layer(Arc::new(DenyPolicy));
        let mut svc = layer.layer(ok_processor());
        let result = svc
            .ready()
            .await
            .unwrap()
            .call(carrier_exchange().await)
            .await;
        assert!(result.is_err());
        match result.unwrap_err() {
            CamelError::Unauthorized(msg) => assert!(msg.contains("missing role")),
            other => panic!("expected Unauthorized, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_denied_error_contains_required_actual() {
        let layer = policy_only_layer(Arc::new(DenyPolicy));
        let mut svc = layer.layer(ok_processor());
        let result = svc
            .ready()
            .await
            .unwrap()
            .call(carrier_exchange().await)
            .await;
        let msg = match result.unwrap_err() {
            CamelError::Unauthorized(msg) => msg,
            other => panic!("expected Unauthorized, got: {other:?}"),
        };
        assert!(msg.contains("admin"));
        assert!(msg.contains("user"));
    }

    #[tokio::test]
    async fn test_evaluate_error_propagates() {
        let layer = policy_only_layer(Arc::new(FailPolicy));
        let mut svc = layer.layer(ok_processor());
        let result = svc
            .ready()
            .await
            .unwrap()
            .call(carrier_exchange().await)
            .await;
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
                _auth: &AuthContext<'_>,
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
        let layer = policy_only_layer(Arc::clone(&policy) as Arc<dyn SecurityPolicy>);
        let mut svc = layer.layer(ok_processor());
        for _ in 0..3 {
            let result = svc
                .ready()
                .await
                .unwrap()
                .call(carrier_exchange().await)
                .await;
            assert!(result.is_ok());
        }
        assert_eq!(count.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn test_granted_all_property_json_formats() {
        let layer = policy_only_layer(Arc::new(GrantPolicy));
        let mut svc = layer.layer(ok_processor());
        let result = svc
            .ready()
            .await
            .unwrap()
            .call(carrier_exchange().await)
            .await;
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
                _auth: &AuthContext<'_>,
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
        let layer = policy_only_layer(Arc::new(EmptyPrincipalPolicy));
        let mut svc = layer.layer(ok_processor());
        let result = svc
            .ready()
            .await
            .unwrap()
            .call(carrier_exchange().await)
            .await;
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
        let layer = policy_only_layer(Arc::new(GrantPolicy));
        let mut svc1 = layer.layer(ok_processor());
        let svc2 = svc1.clone();

        let r1 = svc1
            .ready()
            .await
            .unwrap()
            .call(carrier_exchange().await)
            .await;
        let mut svc2 = svc2;
        let r2 = svc2
            .ready()
            .await
            .unwrap()
            .call(carrier_exchange().await)
            .await;
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
                _auth: &AuthContext<'_>,
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
        let layer = policy_only_layer(Arc::new(GrantPolicy));
        let mut svc = layer.layer(ok_processor());
        let mut ex = carrier_exchange().await;
        ex.set_property("custom.key", "custom-value");
        let result = svc.ready().await.unwrap().call(ex).await;
        let ex = result.unwrap();
        assert_eq!(
            ex.property("custom.key"),
            Some(&serde_json::Value::String("custom-value".into()))
        );
        assert!(ex.property(PRINCIPAL_SUBJECT_KEY).is_some());
    }

    // ── Task 1.7 dual-read tests (strict-mode contract since Task 2.9) ──

    #[tokio::test]
    async fn layer_denies_without_typed_principal_or_token() {
        let policy = Arc::new(RecordingGrantPolicy {
            count: AtomicU32::new(0),
            seen: Mutex::new(Vec::new()),
        });
        let layer = SecurityPolicyLayer::new(
            Arc::clone(&policy) as Arc<dyn SecurityPolicy>,
            TransportId::Http,
        );
        let mut svc = layer.layer(ok_processor());
        let result = svc.ready().await.unwrap().call(make_exchange()).await;
        assert!(matches!(result, Err(CamelError::Unauthenticated(_))));
        assert_eq!(policy.count.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn layer_grants_with_typed_principal() {
        let policy = Arc::new(RecordingGrantPolicy {
            count: AtomicU32::new(0),
            seen: Mutex::new(Vec::new()),
        });
        let layer = SecurityPolicyLayer::new(
            Arc::clone(&policy) as Arc<dyn SecurityPolicy>,
            TransportId::Http,
        );

        // Mint a principal through the REAL path.
        let providers = provider_registry("idp-a", "t-a");
        let plan = authenticated_plan("idp-a");
        let principal = kernel_authenticate(&plan, &providers, &credentials("t-a"))
            .await
            .unwrap();
        let mut exchange = make_exchange();
        install_carrier(&mut exchange, &principal);

        let mut svc = layer.layer(ok_processor());
        let result = svc.ready().await.unwrap().call(exchange).await;
        assert!(result.is_ok());
        assert_eq!(policy.count.load(Ordering::SeqCst), 1);
        let seen = policy.seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].0, "user1");
        assert_eq!(seen[0].1, "idp-a");
    }

    #[tokio::test]
    async fn layer_bearer_token_without_carrier_denies() {
        // Strict mode (Task 2.9): a raw Bearer header is NOT authentication
        // evidence at the layer. Only the kernel-minted typed carrier
        // authorizes; the transport boundary mints it, never the layer.
        let policy = Arc::new(RecordingGrantPolicy {
            count: AtomicU32::new(0),
            seen: Mutex::new(Vec::new()),
        });
        let layer = SecurityPolicyLayer::new(
            Arc::clone(&policy) as Arc<dyn SecurityPolicy>,
            TransportId::Http,
        );
        let mut exchange = make_exchange();
        exchange.input.set_header("Authorization", "Bearer t-a");
        let mut svc = layer.layer(ok_processor());
        let result = svc.ready().await.unwrap().call(exchange).await;
        assert!(matches!(result, Err(CamelError::Unauthenticated(_))));
        assert_eq!(
            policy.count.load(Ordering::SeqCst),
            0,
            "the policy must never evaluate a carrier-less Exchange"
        );
    }

    #[tokio::test]
    async fn policy_only_route_without_carrier_denies() {
        // Strict mode (Task 2.9) removed the anonymous-principal evaluation:
        // a policy-only route (no carrier minter in play) fails closed. The
        // Phase-1 interim contract (anonymous evaluation) is gone.
        let policy = Arc::new(RecordingGrantPolicy {
            count: AtomicU32::new(0),
            seen: Mutex::new(Vec::new()),
        });
        let layer = SecurityPolicyLayer::new(
            Arc::clone(&policy) as Arc<dyn SecurityPolicy>,
            TransportId::Http,
        );
        let mut svc = layer.layer(ok_processor());
        let result = svc.ready().await.unwrap().call(make_exchange()).await;
        assert!(matches!(result, Err(CamelError::Unauthenticated(_))));
        assert_eq!(policy.count.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn spoofed_extension_value_does_not_authorize() {
        let layer = SecurityPolicyLayer::new(Arc::new(GrantPolicy), TransportId::Http);
        let mut exchange = make_exchange();
        // Wrong type under the carrier key: downcast fails → treated as
        // absent → deny.
        exchange.set_extension(KERNEL_PRINCIPAL_KEY, Arc::new("x".to_string()));
        let mut svc = layer.layer(ok_processor());
        let result = svc.ready().await.unwrap().call(exchange).await;
        assert!(matches!(result, Err(CamelError::Unauthenticated(_))));
    }

    #[tokio::test]
    async fn spoofed_legacy_property_without_token_denies() {
        let layer = SecurityPolicyLayer::new(Arc::new(GrantPolicy), TransportId::Http);
        let mut exchange = make_exchange();
        // Raw `camel.auth.principal` property (valid-format principal data) is
        // NOT carrier evidence — property evidence never authorizes.
        store_principal_properties(&mut exchange, &test_principal());
        let mut svc = layer.layer(ok_processor());
        let result = svc.ready().await.unwrap().call(exchange).await;
        assert!(matches!(result, Err(CamelError::Unauthenticated(_))));
    }
}
