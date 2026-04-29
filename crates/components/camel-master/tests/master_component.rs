use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use camel_api::{
    CamelError, Exchange, LeadershipEvent, LeadershipHandle, LeadershipService, Message,
    MetricsCollector, NoOpMetrics, NoopPlatformService, NoopReadinessGate, PlatformError,
    PlatformIdentity, PlatformService, ReadinessGate,
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
    platform_service: Arc<dyn PlatformService>,
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

    fn metrics(&self) -> Arc<dyn MetricsCollector> {
        Arc::new(NoOpMetrics)
    }

    fn platform_service(&self) -> Arc<dyn PlatformService> {
        Arc::clone(&self.platform_service)
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

struct FakeLeadershipService {
    seen_lock_name: Arc<Mutex<Option<String>>>,
    tx: Arc<Mutex<Option<watch::Sender<Option<LeadershipEvent>>>>>,
}

struct DropSensitiveLeadershipService {
    canceled: Arc<AtomicBool>,
}

impl DropSensitiveLeadershipService {
    fn new() -> Self {
        Self {
            canceled: Arc::new(AtomicBool::new(false)),
        }
    }

    fn was_canceled(&self) -> bool {
        self.canceled.load(Ordering::Acquire)
    }
}

#[async_trait]
impl LeadershipService for DropSensitiveLeadershipService {
    async fn start(&self, _lock_name: &str) -> Result<LeadershipHandle, PlatformError> {
        let (tx, rx) = watch::channel(Some(LeadershipEvent::StartedLeading));
        let cancel = CancellationToken::new();
        let cancel_wait = cancel.clone();
        let canceled = Arc::clone(&self.canceled);
        let (term_tx, term_rx) = oneshot::channel();

        tokio::spawn(async move {
            cancel_wait.cancelled().await;
            canceled.store(true, Ordering::Release);
            let _ = tx.send(Some(LeadershipEvent::StoppedLeading));
            let _ = term_tx.send(());
        });

        Ok(LeadershipHandle::new(
            rx,
            Arc::new(AtomicBool::new(true)),
            cancel,
            term_rx,
        ))
    }
}

impl FakeLeadershipService {
    fn new() -> Self {
        Self {
            seen_lock_name: Arc::new(Mutex::new(None)),
            tx: Arc::new(Mutex::new(None)),
        }
    }

    fn seen_lock_name(&self) -> Option<String> {
        self.seen_lock_name
            .lock()
            .expect("fake leadership mutex poisoned")
            .clone()
    }
}

#[async_trait]
impl LeadershipService for FakeLeadershipService {
    async fn start(&self, lock_name: &str) -> Result<LeadershipHandle, PlatformError> {
        *self
            .seen_lock_name
            .lock()
            .expect("fake leadership mutex poisoned") = Some(lock_name.to_string());

        let (tx, rx) = watch::channel(Some(LeadershipEvent::StartedLeading));
        *self.tx.lock().expect("fake leadership tx mutex poisoned") = Some(tx.clone());

        let cancel = CancellationToken::new();
        let cancel_wait = cancel.clone();
        let tx_for_task = tx;
        let tx_store = Arc::clone(&self.tx);
        let (term_tx, term_rx) = oneshot::channel();
        tokio::spawn(async move {
            cancel_wait.cancelled().await;
            let _ = tx_for_task.send(Some(LeadershipEvent::StoppedLeading));
            if let Ok(mut guard) = tx_store.lock() {
                let _ = guard.take();
            }
            let _ = term_tx.send(());
        });

        Ok(LeadershipHandle::new(
            rx,
            Arc::new(AtomicBool::new(true)),
            cancel,
            term_rx,
        ))
    }
}

struct FlakyDelegateComponent {
    failures_remaining: Arc<std::sync::atomic::AtomicUsize>,
}

impl Component for FlakyDelegateComponent {
    fn scheme(&self) -> &str {
        "test"
    }

    fn create_endpoint(
        &self,
        uri: &str,
        _ctx: &dyn ComponentContext,
    ) -> Result<Box<dyn Endpoint>, CamelError> {
        let prev = self
            .failures_remaining
            .fetch_update(
                std::sync::atomic::Ordering::SeqCst,
                std::sync::atomic::Ordering::SeqCst,
                |v| v.checked_sub(1),
            )
            .unwrap_or(0);

        if prev > 0 {
            return Err(CamelError::EndpointCreationFailed(
                "transient delegate endpoint error".to_string(),
            ));
        }

        Ok(Box::new(TestDelegateEndpoint {
            uri: uri.to_string(),
        }))
    }
}

struct FakePlatformService {
    identity: PlatformIdentity,
    readiness_gate: Arc<dyn ReadinessGate>,
    leadership: Arc<dyn LeadershipService>,
}

impl FakePlatformService {
    fn new(leadership: Arc<dyn LeadershipService>) -> Self {
        Self {
            identity: PlatformIdentity::local("test-node"),
            readiness_gate: Arc::new(NoopReadinessGate),
            leadership,
        }
    }
}

impl PlatformService for FakePlatformService {
    fn identity(&self) -> PlatformIdentity {
        self.identity.clone()
    }

    fn readiness_gate(&self) -> Arc<dyn ReadinessGate> {
        Arc::clone(&self.readiness_gate)
    }

    fn leadership(&self) -> Arc<dyn LeadershipService> {
        Arc::clone(&self.leadership)
    }
}

#[tokio::test]
async fn works_with_noop_platform_service() {
    let delegate = Arc::new(TestDelegateComponent);
    let ctx = TestComponentContext {
        delegate,
        platform_service: Arc::new(NoopPlatformService::default()),
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
async fn lock_name_is_forwarded_to_leadership_service() {
    let delegate = Arc::new(TestDelegateComponent);
    let fake = Arc::new(FakeLeadershipService::new());
    let ctx = TestComponentContext {
        delegate,
        platform_service: Arc::new(FakePlatformService::new(fake.clone())),
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

    assert_eq!(fake.seen_lock_name().as_deref(), Some("lease-name"));

    cancel.cancel();
    consumer.stop().await.unwrap();
}

#[tokio::test]
async fn retries_delegate_start_while_leadership_stays_started() {
    let delegate = Arc::new(FlakyDelegateComponent {
        failures_remaining: Arc::new(std::sync::atomic::AtomicUsize::new(1)),
    });
    let fake = Arc::new(FakeLeadershipService::new());
    let ctx = TestComponentContext {
        delegate,
        platform_service: Arc::new(FakePlatformService::new(fake)),
    };

    let component = MasterComponent::default();
    let endpoint = component
        .create_endpoint("master:retry-lock:test:delegate", &ctx)
        .unwrap();
    let mut consumer = endpoint.create_consumer().unwrap();

    let (tx, mut rx) = tokio::sync::mpsc::channel(8);
    let cancel = CancellationToken::new();
    let consumer_ctx = ConsumerContext::new(tx, cancel.clone());

    consumer.start(consumer_ctx).await.unwrap();

    let first = timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("delegate should eventually recover while leadership is sustained")
        .expect("channel should contain delegate message");
    assert_eq!(first.exchange.input.body.as_text(), Some("delegate-ok"));

    cancel.cancel();
    consumer.stop().await.unwrap();
}

#[tokio::test]
async fn dropping_consumer_after_start_does_not_step_down_leadership_immediately() {
    let delegate = Arc::new(TestDelegateComponent);
    let leadership = Arc::new(DropSensitiveLeadershipService::new());
    let ctx = TestComponentContext {
        delegate,
        platform_service: Arc::new(FakePlatformService::new(leadership.clone())),
    };

    let component = MasterComponent::default();
    let endpoint = component
        .create_endpoint("master:drop-lock:test:delegate", &ctx)
        .unwrap();
    let mut consumer = endpoint.create_consumer().unwrap();

    let (tx, _rx) = tokio::sync::mpsc::channel(8);
    let consumer_ctx = ConsumerContext::new(tx, CancellationToken::new());

    consumer.start(consumer_ctx).await.unwrap();
    drop(consumer);

    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!leadership.was_canceled());
}
