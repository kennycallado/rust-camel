//! producer passthrough delegation tests and mock delegate infra. Split from tests.rs by concern (bd rc-ubk1v).
//! Shared mocks/helpers live in the tests module root (tests.rs).

use super::*;

struct MockDelegateContext {
    delegate: Arc<dyn Component>,
}

impl ComponentContext for MockDelegateContext {
    fn resolve_component(&self, scheme: &str) -> Option<Arc<dyn Component>> {
        if self.delegate.scheme() == scheme {
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

struct MockProducerDelegateComponent {
    create_endpoint_calls: Arc<AtomicUsize>,
    create_producer_calls: Arc<AtomicUsize>,
    fail_producer: bool,
}

impl Component for MockProducerDelegateComponent {
    fn scheme(&self) -> &str {
        "mock"
    }

    fn create_endpoint(
        &self,
        _uri: &str,
        _ctx: &dyn ComponentContext,
    ) -> Result<Box<dyn Endpoint>, CamelError> {
        self.create_endpoint_calls.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(MockProducerDelegateEndpoint {
            create_producer_calls: Arc::clone(&self.create_producer_calls),
            fail_producer: self.fail_producer,
        }))
    }
}

struct MockProducerDelegateEndpoint {
    create_producer_calls: Arc<AtomicUsize>,
    fail_producer: bool,
}

impl Endpoint for MockProducerDelegateEndpoint {
    fn uri(&self) -> &str {
        "mock:delegate"
    }

    fn create_consumer(
        &self,
        _rt: std::sync::Arc<dyn camel_component_api::RuntimeObservability>,
    ) -> Result<Box<dyn Consumer>, CamelError> {
        Err(CamelError::EndpointCreationFailed(
            "not used in test".to_string(),
        ))
    }

    fn create_producer(
        &self,
        _rt: std::sync::Arc<dyn camel_component_api::RuntimeObservability>,
        _ctx: &ProducerContext,
    ) -> Result<BoxProcessor, CamelError> {
        self.create_producer_calls.fetch_add(1, Ordering::SeqCst);
        if self.fail_producer {
            return Err(CamelError::ProcessorError(
                "delegate producer failed".to_string(),
            ));
        }
        Ok(BoxProcessor::from_fn(
            |exchange| async move { Ok(exchange) },
        ))
    }
}

#[tokio::test]
async fn producer_passthrough_delegates_and_produces() {
    let endpoint_calls = Arc::new(AtomicUsize::new(0));
    let producer_calls = Arc::new(AtomicUsize::new(0));
    let delegate = Arc::new(MockProducerDelegateComponent {
        create_endpoint_calls: Arc::clone(&endpoint_calls),
        create_producer_calls: Arc::clone(&producer_calls),
        fail_producer: false,
    });

    let ctx = MockDelegateContext {
        delegate: delegate.clone(),
    };

    let master = MasterComponent::default();
    let endpoint = master
        .create_endpoint("master:lock-1:mock:delegate", &ctx)
        .unwrap();
    let producer_ctx = ProducerContext::new();
    let producer = endpoint
        .create_producer(
            Arc::new(PanicRuntimeObservability)
                as Arc<dyn camel_component_api::RuntimeObservability>,
            &producer_ctx,
        )
        .unwrap();

    let exchange = Exchange::new(Message::new("ok"));
    let result = producer.oneshot(exchange).await.unwrap();

    assert_eq!(result.input.body.as_text(), Some("ok"));
    assert_eq!(endpoint_calls.load(Ordering::SeqCst), 1);
    assert_eq!(producer_calls.load(Ordering::SeqCst), 1);
}

#[test]
fn producer_passthrough_bubbles_delegate_errors() {
    let endpoint_calls = Arc::new(AtomicUsize::new(0));
    let producer_calls = Arc::new(AtomicUsize::new(0));
    let delegate = Arc::new(MockProducerDelegateComponent {
        create_endpoint_calls: Arc::clone(&endpoint_calls),
        create_producer_calls: Arc::clone(&producer_calls),
        fail_producer: true,
    });

    let ctx = MockDelegateContext {
        delegate: delegate.clone(),
    };

    let master = MasterComponent::default();
    let endpoint = master
        .create_endpoint("master:lock-1:mock:delegate", &ctx)
        .unwrap();
    let producer_ctx = ProducerContext::new();
    let err = endpoint
        .create_producer(
            Arc::new(PanicRuntimeObservability)
                as Arc<dyn camel_component_api::RuntimeObservability>,
            &producer_ctx,
        )
        .unwrap_err();

    assert!(matches!(err, CamelError::ProcessorError(_)));
    assert_eq!(endpoint_calls.load(Ordering::SeqCst), 1);
    assert_eq!(producer_calls.load(Ordering::SeqCst), 1);
}
