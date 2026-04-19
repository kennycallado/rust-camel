use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use camel_api::{
    CamelError, Exchange, LeaderElector, LeadershipEvent, LeadershipHandle, Message, NoOpMetrics,
    NoopLeaderElector, PlatformError, PlatformIdentity,
};
use camel_component_api::{
    BoxProcessor, Component, ComponentContext, Consumer, ConsumerContext, Endpoint, ProducerContext,
};
use camel_language_api::Language;
use camel_master::MasterComponent;
use tokio::sync::{oneshot, watch};
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

struct TestComponentContext {
    delegate: Arc<dyn Component>,
    elector: Arc<dyn LeaderElector>,
}

impl ComponentContext for TestComponentContext {
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

    fn metrics(&self) -> Arc<dyn camel_api::MetricsCollector> {
        Arc::new(NoOpMetrics)
    }

    fn leader_elector(&self) -> Arc<dyn LeaderElector> {
        Arc::clone(&self.elector)
    }
}

struct TestDelegateComponent;

impl Component for TestDelegateComponent {
    fn scheme(&self) -> &str {
        "test"
    }

    fn create_endpoint(
        &self,
        uri: &str,
        _ctx: &dyn ComponentContext,
    ) -> Result<Box<dyn Endpoint>, CamelError> {
        Ok(Box::new(TestDelegateEndpoint {
            uri: uri.to_string(),
        }))
    }
}

struct TestDelegateEndpoint {
    uri: String,
}

impl Endpoint for TestDelegateEndpoint {
    fn uri(&self) -> &str {
        &self.uri
    }

    fn create_consumer(&self) -> Result<Box<dyn Consumer>, CamelError> {
        Ok(Box::new(TestDelegateConsumer))
    }

    fn create_producer(&self, _ctx: &ProducerContext) -> Result<BoxProcessor, CamelError> {
        Err(CamelError::EndpointCreationFailed("not used".to_string()))
    }
}

struct TestDelegateConsumer;

#[async_trait]
impl Consumer for TestDelegateConsumer {
    async fn start(&mut self, context: ConsumerContext) -> Result<(), CamelError> {
        context
            .send(Exchange::new(Message::new("delegate-ok")))
            .await?;
        context.cancelled().await;
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), CamelError> {
        Ok(())
    }
}

struct FakeLeaderElector {
    seen_node_id: Arc<Mutex<Option<String>>>,
}

impl FakeLeaderElector {
    fn new() -> Self {
        Self {
            seen_node_id: Arc::new(Mutex::new(None)),
        }
    }

    fn seen_node_id(&self) -> Option<String> {
        self.seen_node_id
            .lock()
            .expect("fake elector mutex poisoned")
            .clone()
    }
}

#[async_trait]
impl LeaderElector for FakeLeaderElector {
    async fn start(&self, identity: PlatformIdentity) -> Result<LeadershipHandle, PlatformError> {
        *self
            .seen_node_id
            .lock()
            .expect("fake elector mutex poisoned") = Some(identity.node_id);

        let (tx, rx) = watch::channel(Some(LeadershipEvent::StartedLeading));
        drop(tx);
        let (_term_tx, term_rx) = oneshot::channel();

        Ok(LeadershipHandle::new(
            rx,
            Arc::new(AtomicBool::new(true)),
            CancellationToken::new(),
            term_rx,
        ))
    }
}

#[tokio::test]
async fn works_with_noop_leader_elector() {
    let delegate = Arc::new(TestDelegateComponent);
    let ctx = TestComponentContext {
        delegate,
        elector: Arc::new(NoopLeaderElector),
    };

    let component = MasterComponent::default();
    let endpoint = component
        .create_endpoint("master:leader-lock:test:delegate", &ctx)
        .unwrap();
    let mut consumer = endpoint.create_consumer().unwrap();

    let (tx, mut rx) = tokio::sync::mpsc::channel(8);
    let cancel = CancellationToken::new();
    let consumer_ctx = ConsumerContext::new(tx, cancel.clone());

    consumer.start(consumer_ctx).await.unwrap();

    let first = timeout(Duration::from_millis(300), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first.exchange.input.body.as_text(), Some("delegate-ok"));

    cancel.cancel();
    consumer.stop().await.unwrap();
}

#[tokio::test]
async fn lock_name_maps_to_lease_name() {
    let delegate = Arc::new(TestDelegateComponent);
    let fake = Arc::new(FakeLeaderElector::new());
    let ctx = TestComponentContext {
        delegate,
        elector: fake.clone(),
    };

    let component = MasterComponent::default();
    let endpoint = component
        .create_endpoint("master:lease-name:test:delegate", &ctx)
        .unwrap();
    let mut consumer = endpoint.create_consumer().unwrap();

    let (tx, _rx) = tokio::sync::mpsc::channel(8);
    let cancel = CancellationToken::new();
    let consumer_ctx = ConsumerContext::new(tx, cancel.clone());

    consumer.start(consumer_ctx).await.unwrap();

    assert_eq!(fake.seen_node_id().as_deref(), Some("lease-name"));

    cancel.cancel();
    consumer.stop().await.unwrap();
}
