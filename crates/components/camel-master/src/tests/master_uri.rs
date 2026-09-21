//! master:// URI parsing and delegate-scheme resolution tests. Split from tests.rs by concern (bd rc-ubk1v).
//! Shared mocks/helpers live in the tests module root (tests.rs).

use super::*;

#[test]
fn parse_master_uri_valid() {
    let cfg = MasterUriConfig::parse("master:mylock:timer:tick?period=250").unwrap();
    assert_eq!(cfg.lock_name, "mylock");
    assert_eq!(cfg.delegate_uri, "timer:tick?period=250");
}

#[test]
fn parse_master_uri_missing_lockname() {
    let err = MasterUriConfig::parse("master::timer:tick").unwrap_err();
    assert!(matches!(err, CamelError::InvalidUri(_)));
}

#[test]
fn parse_master_uri_missing_delegate() {
    let err = MasterUriConfig::parse("master:mylock:").unwrap_err();
    assert!(matches!(err, CamelError::InvalidUri(_)));
}

#[test]
fn endpoint_fails_when_delegate_component_missing() {
    let master = MasterComponent::default();
    let result = master.create_endpoint("master:lock-1:missing:delegate", &NoOpComponentContext);
    assert!(matches!(result, Err(CamelError::ComponentNotFound(_))));
}

#[test]
fn delegate_scheme_is_parsed_from_delegate_uri() {
    let seen_scheme = Arc::new(AtomicBool::new(false));

    struct SchemeAwareContext {
        delegate: Arc<dyn Component>,
        seen_scheme: Arc<AtomicBool>,
    }

    impl ComponentContext for SchemeAwareContext {
        fn resolve_component(&self, scheme: &str) -> Option<Arc<dyn Component>> {
            if scheme == "mock" {
                self.seen_scheme.store(true, Ordering::SeqCst);
                Some(Arc::clone(&self.delegate))
            } else {
                None
            }
        }

        fn resolve_language(&self, _name: &str) -> Option<Arc<dyn Language>> {
            None
        }

        fn metrics(&self) -> Arc<dyn MetricsCollector> {
            Arc::new(NoOpMetrics)
        }

        fn platform_service(&self) -> Arc<dyn PlatformService> {
            Arc::new(NoopPlatformService::default())
        }

        fn register_route_health_check(
            &self,
            _route_id: &str,
            _check: Arc<dyn camel_api::AsyncHealthCheck>,
        ) {
        }

        fn unregister_route_health_check(&self, _route_id: &str) {}
    }

    struct MockDelegateComponent;

    impl Component for MockDelegateComponent {
        fn scheme(&self) -> &str {
            "mock"
        }

        fn create_endpoint(
            &self,
            _uri: &str,
            _ctx: &dyn ComponentContext,
        ) -> Result<Box<dyn Endpoint>, CamelError> {
            Ok(Box::new(MockDelegateEndpoint))
        }
    }

    struct MockDelegateEndpoint;

    impl Endpoint for MockDelegateEndpoint {
        fn uri(&self) -> &str {
            "mock:delegate"
        }

        fn create_consumer(
            &self,
            _rt: std::sync::Arc<dyn camel_component_api::RuntimeObservability>,
        ) -> Result<Box<dyn Consumer>, CamelError> {
            Err(CamelError::EndpointCreationFailed("not used".to_string()))
        }

        fn create_producer(
            &self,
            _rt: std::sync::Arc<dyn camel_component_api::RuntimeObservability>,
            _ctx: &ProducerContext,
        ) -> Result<BoxProcessor, CamelError> {
            Err(CamelError::EndpointCreationFailed("not used".to_string()))
        }
    }

    let delegate = Arc::new(MockDelegateComponent);
    let ctx = SchemeAwareContext {
        delegate,
        seen_scheme: Arc::clone(&seen_scheme),
    };

    let master = MasterComponent::default();
    let endpoint = master
        .create_endpoint("master:mylock:mock:delegate?x=1", &ctx)
        .unwrap();

    assert_eq!(endpoint.uri(), "master:mylock:mock:delegate?x=1");
    assert!(seen_scheme.load(Ordering::SeqCst));
}
