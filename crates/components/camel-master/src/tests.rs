use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use camel_api::{
    BoxProcessorExt, Exchange, LeadershipEvent, LeadershipHandle, LeadershipService, Message,
    NoOpMetrics, NoopPlatformService, NoopReadinessGate, PlatformError, PlatformIdentity,
    PlatformService, ReadinessGate,
};
use camel_component_api::NoOpComponentContext;
use camel_component_api::test_support::PanicRuntimeObservability;
use std::time::Instant;
use tokio::sync::{oneshot, watch};
use tokio::time::{sleep, timeout};
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

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

struct FakeLeadershipService {
    tx: Mutex<Option<watch::Sender<Option<LeadershipEvent>>>>,
    is_leader: Arc<AtomicBool>,
    initial: Option<LeadershipEvent>,
}

impl FakeLeadershipService {
    fn new(initial: Option<LeadershipEvent>) -> Self {
        let starts_as_leader = matches!(initial, Some(LeadershipEvent::StartedLeading));
        Self {
            tx: Mutex::new(None),
            is_leader: Arc::new(AtomicBool::new(starts_as_leader)),
            initial,
        }
    }

    async fn emit(&self, event: LeadershipEvent) {
        self.is_leader.store(
            matches!(event, LeadershipEvent::StartedLeading),
            Ordering::Release,
        );
        if let Some(tx) = self
            .tx
            .lock()
            .expect("mutex poisoned: fake elector sender")
            .as_ref()
        {
            let _ = tx.send(Some(event));
        }
    }
}

#[async_trait]
impl LeadershipService for FakeLeadershipService {
    async fn start(&self, _lock_name: &str) -> Result<LeadershipHandle, PlatformError> {
        let (tx, rx) = watch::channel(self.initial.clone());
        *self.tx.lock().expect("mutex poisoned: fake elector sender") = Some(tx);

        let cancel = CancellationToken::new();
        let cancel_wait = cancel.clone();
        let (term_tx, term_rx) = oneshot::channel();
        tokio::spawn(async move {
            cancel_wait.cancelled().await;
            let _ = term_tx.send(());
        });

        Ok(LeadershipHandle::new(
            rx,
            Arc::clone(&self.is_leader),
            cancel,
            term_rx,
        ))
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
            identity: PlatformIdentity::local("master-tests"),
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

struct FakeDelegateComponent {
    create_consumer_calls: Arc<AtomicUsize>,
    start_calls: Arc<AtomicUsize>,
}

impl Component for FakeDelegateComponent {
    fn scheme(&self) -> &str {
        "fake"
    }

    fn create_endpoint(
        &self,
        _uri: &str,
        _ctx: &dyn ComponentContext,
    ) -> Result<Box<dyn Endpoint>, CamelError> {
        Ok(Box::new(FakeDelegateEndpoint {
            create_consumer_calls: Arc::clone(&self.create_consumer_calls),
            start_calls: Arc::clone(&self.start_calls),
        }))
    }
}

struct FakeDelegateEndpoint {
    create_consumer_calls: Arc<AtomicUsize>,
    start_calls: Arc<AtomicUsize>,
}

impl Endpoint for FakeDelegateEndpoint {
    fn uri(&self) -> &str {
        "fake:delegate"
    }

    fn create_consumer(
        &self,
        _rt: std::sync::Arc<dyn camel_component_api::RuntimeObservability>,
    ) -> Result<Box<dyn Consumer>, CamelError> {
        let epoch = self.create_consumer_calls.fetch_add(1, Ordering::SeqCst) + 1;
        Ok(Box::new(FakeDelegateConsumer {
            epoch,
            start_calls: Arc::clone(&self.start_calls),
        }))
    }

    fn create_producer(
        &self,
        _rt: std::sync::Arc<dyn camel_component_api::RuntimeObservability>,
        _ctx: &ProducerContext,
    ) -> Result<BoxProcessor, CamelError> {
        Err(CamelError::EndpointCreationFailed("not used".to_string()))
    }
}

struct FakeDelegateConsumer {
    epoch: usize,
    start_calls: Arc<AtomicUsize>,
}

struct FailingDelegateComponent {
    create_endpoint_calls: Arc<AtomicUsize>,
}

impl Component for FailingDelegateComponent {
    fn scheme(&self) -> &str {
        "failing"
    }

    fn create_endpoint(
        &self,
        _uri: &str,
        _ctx: &dyn ComponentContext,
    ) -> Result<Box<dyn Endpoint>, CamelError> {
        self.create_endpoint_calls.fetch_add(1, Ordering::SeqCst);
        Err(CamelError::EndpointCreationFailed(
            "delegate endpoint creation failed".to_string(),
        ))
    }
}

#[async_trait]
impl Consumer for FakeDelegateConsumer {
    async fn start(&mut self, context: ConsumerContext) -> Result<(), CamelError> {
        self.start_calls.fetch_add(1, Ordering::SeqCst);
        context
            .send(Exchange::new(Message::new(format!("epoch-{}", self.epoch))))
            .await?;

        loop {
            tokio::select! {
                _ = context.cancelled() => {
                    break;
                }
                _ = sleep(Duration::from_millis(20)) => {
                    context
                        .send(Exchange::new(Message::new(format!("epoch-{}", self.epoch))))
                        .await?;
                }
            }
        }

        Ok(())
    }

    async fn stop(&mut self) -> Result<(), CamelError> {
        Ok(())
    }
}

fn build_master_consumer(
    platform_service: Arc<dyn PlatformService>,
    create_consumer_calls: Arc<AtomicUsize>,
    start_calls: Arc<AtomicUsize>,
    delegate_retry_max_attempts: Option<u32>,
) -> MasterConsumer {
    let reconnect = match delegate_retry_max_attempts {
        Some(max) => NetworkRetryPolicy {
            max_attempts: max,
            ..NetworkRetryPolicy::default()
        },
        None => NetworkRetryPolicy {
            max_attempts: 0,
            ..NetworkRetryPolicy::default()
        },
    };
    MasterConsumer::new(
        "lock-a".to_string(),
        "fake:delegate".to_string(),
        Arc::new(FakeDelegateComponent {
            create_consumer_calls,
            start_calls,
        }),
        Arc::new(NoOpMetrics),
        platform_service,
        Duration::from_millis(500),
        reconnect,
        Arc::new(PanicRuntimeObservability) as Arc<dyn camel_component_api::RuntimeObservability>,
    )
}

#[tokio::test]
async fn starts_delegate_only_after_started_leading() {
    let leadership = Arc::new(FakeLeadershipService::new(None));
    let platform_service = Arc::new(FakePlatformService::new(leadership.clone()));
    let create_consumer_calls = Arc::new(AtomicUsize::new(0));
    let start_calls = Arc::new(AtomicUsize::new(0));
    let mut master = build_master_consumer(
        platform_service,
        Arc::clone(&create_consumer_calls),
        Arc::clone(&start_calls),
        Some(30),
    );

    let (tx, mut rx) = tokio::sync::mpsc::channel(16);
    let cancel = CancellationToken::new();
    let ctx = ConsumerContext::new(tx, cancel.clone());

    master.start(ctx).await.unwrap();

    sleep(Duration::from_millis(80)).await;
    assert!(rx.try_recv().is_err());
    assert_eq!(create_consumer_calls.load(Ordering::SeqCst), 0);

    leadership.emit(LeadershipEvent::StartedLeading).await;

    let first = timeout(Duration::from_millis(500), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first.exchange.input.body.as_text(), Some("epoch-1"));
    assert_eq!(create_consumer_calls.load(Ordering::SeqCst), 1);
    assert_eq!(start_calls.load(Ordering::SeqCst), 1);

    cancel.cancel();
    master.stop().await.unwrap();
}

#[tokio::test]
async fn stops_delegate_on_stopped_leading() {
    let leadership = Arc::new(FakeLeadershipService::new(None));
    let platform_service = Arc::new(FakePlatformService::new(leadership.clone()));
    let create_consumer_calls = Arc::new(AtomicUsize::new(0));
    let start_calls = Arc::new(AtomicUsize::new(0));
    let mut master = build_master_consumer(
        platform_service,
        Arc::clone(&create_consumer_calls),
        Arc::clone(&start_calls),
        Some(30),
    );

    let (tx, mut rx) = tokio::sync::mpsc::channel(16);
    let cancel = CancellationToken::new();
    let ctx = ConsumerContext::new(tx, cancel.clone());

    master.start(ctx).await.unwrap();
    leadership.emit(LeadershipEvent::StartedLeading).await;
    let _ = timeout(Duration::from_millis(500), rx.recv())
        .await
        .unwrap()
        .unwrap();

    leadership.emit(LeadershipEvent::StoppedLeading).await;
    sleep(Duration::from_millis(100)).await;
    while rx.try_recv().is_ok() {}
    assert!(
        timeout(Duration::from_millis(120), rx.recv())
            .await
            .is_err()
    );

    cancel.cancel();
    master.stop().await.unwrap();
}

#[tokio::test]
async fn recreates_delegate_on_new_leadership_epoch() {
    let leadership = Arc::new(FakeLeadershipService::new(None));
    let platform_service = Arc::new(FakePlatformService::new(leadership.clone()));
    let create_consumer_calls = Arc::new(AtomicUsize::new(0));
    let start_calls = Arc::new(AtomicUsize::new(0));
    let mut master = build_master_consumer(
        platform_service,
        Arc::clone(&create_consumer_calls),
        Arc::clone(&start_calls),
        Some(30),
    );

    let (tx, mut rx) = tokio::sync::mpsc::channel(16);
    let cancel = CancellationToken::new();
    let ctx = ConsumerContext::new(tx, cancel.clone());

    master.start(ctx).await.unwrap();

    leadership.emit(LeadershipEvent::StartedLeading).await;
    let first = timeout(Duration::from_millis(500), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first.exchange.input.body.as_text(), Some("epoch-1"));

    leadership.emit(LeadershipEvent::StoppedLeading).await;
    sleep(Duration::from_millis(120)).await;

    leadership.emit(LeadershipEvent::StartedLeading).await;
    let second = timeout(Duration::from_millis(500), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(second.exchange.input.body.as_text(), Some("epoch-2"));

    assert_eq!(create_consumer_calls.load(Ordering::SeqCst), 2);
    assert_eq!(start_calls.load(Ordering::SeqCst), 2);

    cancel.cancel();
    master.stop().await.unwrap();
}

// ── rc-i1z test infrastructure ──────────────────────────────────────

/// Delegate component that returns errors from create_endpoint or create_consumer.
/// Configurable: which error to return, and after how many successful calls
/// to stop failing.
struct ErrorDelegateComponent {
    create_endpoint_calls: Arc<AtomicUsize>,
    create_consumer_calls: Arc<AtomicUsize>,
    endpoint_error: Option<CamelError>,
    consumer_error_after: usize, // fail start() this many times, then succeed
    consumer_error: Option<CamelError>,
}

impl Component for ErrorDelegateComponent {
    fn scheme(&self) -> &str {
        "errdelegate"
    }

    fn create_endpoint(
        &self,
        _uri: &str,
        _ctx: &dyn ComponentContext,
    ) -> Result<Box<dyn Endpoint>, CamelError> {
        self.create_endpoint_calls.fetch_add(1, Ordering::SeqCst);
        if let Some(ref err) = self.endpoint_error {
            return Err(err.clone());
        }
        Ok(Box::new(ErrorDelegateEndpoint {
            create_consumer_calls: Arc::clone(&self.create_consumer_calls),
            consumer_error_after: self.consumer_error_after,
            consumer_error: self.consumer_error.clone(),
        }))
    }
}

struct ErrorDelegateEndpoint {
    create_consumer_calls: Arc<AtomicUsize>,
    consumer_error_after: usize,
    consumer_error: Option<CamelError>,
}

impl Endpoint for ErrorDelegateEndpoint {
    fn uri(&self) -> &str {
        "errdelegate:delegate"
    }

    fn create_consumer(
        &self,
        _rt: std::sync::Arc<dyn camel_component_api::RuntimeObservability>,
    ) -> Result<Box<dyn Consumer>, CamelError> {
        let call_idx = self.create_consumer_calls.fetch_add(1, Ordering::SeqCst) + 1;
        if call_idx <= self.consumer_error_after {
            return Err(self
                .consumer_error
                .clone()
                .unwrap_or_else(|| CamelError::ProcessorError("default error".to_string())));
        }
        Ok(Box::new(SuccessDelegateConsumer))
    }

    fn create_producer(
        &self,
        _rt: std::sync::Arc<dyn camel_component_api::RuntimeObservability>,
        _ctx: &ProducerContext,
    ) -> Result<BoxProcessor, CamelError> {
        Err(CamelError::EndpointCreationFailed("not used".to_string()))
    }
}

/// A delegate consumer that starts, sends one message, then cancels.
struct SuccessDelegateConsumer;

#[async_trait]
impl Consumer for SuccessDelegateConsumer {
    async fn start(&mut self, context: ConsumerContext) -> Result<(), CamelError> {
        context.send(Exchange::new(Message::new("ok"))).await?;
        context.cancelled().await;
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), CamelError> {
        Ok(())
    }
}

fn build_error_delegate_master(
    platform_service: Arc<dyn PlatformService>,
    create_endpoint_calls: Arc<AtomicUsize>,
    create_consumer_calls: Arc<AtomicUsize>,
    endpoint_error: Option<CamelError>,
    consumer_error_after: usize,
    consumer_error: Option<CamelError>,
    max_attempts: u32,
) -> MasterConsumer {
    let reconnect = NetworkRetryPolicy {
        max_attempts,
        initial_delay: Duration::from_millis(1),
        max_delay: Duration::from_millis(5),
        multiplier: 1.0,
        ..NetworkRetryPolicy::default()
    };
    MasterConsumer::new(
        "lock-err".to_string(),
        "errdelegate:delegate".to_string(),
        Arc::new(ErrorDelegateComponent {
            create_endpoint_calls,
            create_consumer_calls,
            endpoint_error,
            consumer_error_after,
            consumer_error,
        }),
        Arc::new(NoOpMetrics),
        platform_service,
        Duration::from_millis(500),
        reconnect,
        Arc::new(PanicRuntimeObservability) as Arc<dyn camel_component_api::RuntimeObservability>,
    )
}

#[tokio::test]
async fn delegate_permanent_error_terminates_master_without_retry() {
    let leadership = Arc::new(FakeLeadershipService::new(Some(
        LeadershipEvent::StartedLeading,
    )));
    let platform_service = Arc::new(FakePlatformService::new(leadership));
    let create_endpoint_calls = Arc::new(AtomicUsize::new(0));
    let create_consumer_calls = Arc::new(AtomicUsize::new(0));

    // Delegate that fails create_endpoint with a permanent error.
    // Use max_attempts=0 (unlimited) — without classification, this
    // would hang forever. With classification, the task must terminate
    // in milliseconds via fail-fast.
    let mut master = build_error_delegate_master(
        platform_service,
        Arc::clone(&create_endpoint_calls),
        Arc::clone(&create_consumer_calls),
        Some(CamelError::Config("permanent delegate error".to_string())),
        0, // consumer never succeeds (we never get there)
        None,
        0, // max_attempts=0 → unlimited — classification is the terminator
    );

    let (tx, _rx) = tokio::sync::mpsc::channel(16);
    let cancel = CancellationToken::new();
    let ctx = ConsumerContext::new(tx, cancel.clone());

    master.start(ctx).await.unwrap();

    // Poll for task completion with a short timeout. A permanent error
    // must terminate the task in milliseconds via fail-fast classification,
    // NOT via retry-budget exhaustion.
    let task_finished = timeout(Duration::from_millis(500), async {
        loop {
            if master
                .leadership_task
                .as_ref()
                .is_some_and(tokio::task::JoinHandle::is_finished)
            {
                break;
            }
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await;

    assert!(
        task_finished.is_ok(),
        "master should terminate within 500ms via fail-fast classification"
    );

    // Verify single invocation (true fail-fast, not budget exhaustion).
    assert_eq!(
        create_endpoint_calls.load(Ordering::SeqCst),
        1,
        "permanent error must terminate master after exactly 1 invocation"
    );

    // stop() propagates the delegate error; that's correct behavior
    let _ = master.stop().await;

    cancel.cancel();
}

#[tokio::test]
async fn delegate_transient_error_retries_and_eventually_succeeds() {
    let leadership = Arc::new(FakeLeadershipService::new(Some(
        LeadershipEvent::StartedLeading,
    )));
    let platform_service = Arc::new(FakePlatformService::new(leadership));
    let create_endpoint_calls = Arc::new(AtomicUsize::new(0));
    let create_consumer_calls = Arc::new(AtomicUsize::new(0));

    // Delegate that fails create_consumer with transient error for
    // the first 2 attempts, then succeeds on the 3rd.
    let mut master = build_error_delegate_master(
        platform_service,
        Arc::clone(&create_endpoint_calls),
        Arc::clone(&create_consumer_calls),
        None, // endpoint always succeeds
        2,    // fail first 2 create_consumer calls
        Some(CamelError::Io("connection refused".to_string())),
        5, // max_attempts
    );

    let (tx, mut rx) = tokio::sync::mpsc::channel(16);
    let cancel = CancellationToken::new();
    let ctx = ConsumerContext::new(tx, cancel.clone());

    master.start(ctx).await.unwrap();

    // Wait for the delegate to eventually succeed.
    let msg = timeout(Duration::from_secs(2), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(msg.exchange.input.body.as_text(), Some("ok"));

    // Endpoint created 3 times (initial event + 2 retry ticks), consumer
    // created 3 times (2 failures + 1 success).
    assert_eq!(create_endpoint_calls.load(Ordering::SeqCst), 3);
    assert_eq!(create_consumer_calls.load(Ordering::SeqCst), 3);

    cancel.cancel();
    master.stop().await.unwrap();
}

// ── Existing regression tests (rc-f9k) ──────────────────────────────

#[tokio::test]
async fn stops_retrying_delegate_start_after_max_attempts() {
    let leadership = Arc::new(FakeLeadershipService::new(Some(
        LeadershipEvent::StartedLeading,
    )));
    let platform_service = Arc::new(FakePlatformService::new(leadership));
    let create_endpoint_calls = Arc::new(AtomicUsize::new(0));

    let mut master = MasterConsumer::new(
        "lock-a".to_string(),
        "failing:delegate".to_string(),
        Arc::new(FailingDelegateComponent {
            create_endpoint_calls: Arc::clone(&create_endpoint_calls),
        }),
        Arc::new(NoOpMetrics),
        platform_service,
        Duration::from_millis(500),
        NetworkRetryPolicy {
            max_attempts: 1,
            ..NetworkRetryPolicy::default()
        },
        Arc::new(PanicRuntimeObservability) as Arc<dyn camel_component_api::RuntimeObservability>,
    );

    let (tx, _rx) = tokio::sync::mpsc::channel(16);
    let cancel = CancellationToken::new();
    let ctx = ConsumerContext::new(tx, cancel.clone());

    master.start(ctx).await.unwrap();
    sleep(Duration::from_millis(750)).await;

    // With error classification (rc-i1z), EndpointCreationFailed is
    // permanent → fail-fast after exactly 1 invocation. Previously
    // (pre-rc-i1z) this would have been 2 calls (initial + retry via
    // budget exhaustion).
    assert_eq!(create_endpoint_calls.load(Ordering::SeqCst), 1);

    cancel.cancel();
    let _ = master.stop().await;
}

/// Regression test for MST-002: stop() must abort the leadership JoinHandle
/// instead of just dropping it when the task is slow to drain.
/// Without the fix, stop() blocks for the full drain_timeout (~500 ms)
/// because the leadership task is stuck in stop_delegate awaiting a
/// slow delegate. With abort-first, stop() returns almost instantly.
#[tokio::test]
async fn stop_completes_quickly_when_leadership_task_is_slow() {
    // Delegate consumer that ignores its cancellation token and blocks.
    struct SlowStoppingConsumer;

    #[async_trait]
    impl Consumer for SlowStoppingConsumer {
        async fn start(&mut self, ctx: ConsumerContext) -> Result<(), CamelError> {
            ctx.send(Exchange::new(Message::new("slow-start")))
                .await
                .ok();
            // Ignore cancellation — sleep far beyond the drain timeout.
            sleep(Duration::from_secs(60)).await;
            Ok(())
        }

        async fn stop(&mut self) -> Result<(), CamelError> {
            Ok(())
        }
    }

    struct SlowStoppingComponent;

    impl Component for SlowStoppingComponent {
        fn scheme(&self) -> &str {
            "slow"
        }

        fn create_endpoint(
            &self,
            _uri: &str,
            _ctx: &dyn ComponentContext,
        ) -> Result<Box<dyn Endpoint>, CamelError> {
            Ok(Box::new(SlowStoppingEndpoint))
        }
    }

    struct SlowStoppingEndpoint;

    impl Endpoint for SlowStoppingEndpoint {
        fn uri(&self) -> &str {
            "slow:delegate"
        }

        fn create_consumer(
            &self,
            _rt: std::sync::Arc<dyn camel_component_api::RuntimeObservability>,
        ) -> Result<Box<dyn Consumer>, CamelError> {
            Ok(Box::new(SlowStoppingConsumer))
        }

        fn create_producer(
            &self,
            _rt: std::sync::Arc<dyn camel_component_api::RuntimeObservability>,
            _ctx: &ProducerContext,
        ) -> Result<BoxProcessor, CamelError> {
            Err(CamelError::EndpointCreationFailed("not used".into()))
        }
    }

    let leadership = Arc::new(FakeLeadershipService::new(Some(
        LeadershipEvent::StartedLeading,
    )));
    let platform_service = Arc::new(FakePlatformService::new(leadership));

    let mut master = MasterConsumer::new(
        "lock-slow".into(),
        "slow:delegate".into(),
        Arc::new(SlowStoppingComponent),
        Arc::new(NoOpMetrics),
        platform_service,
        Duration::from_millis(500), // drain_timeout
        NetworkRetryPolicy {
            max_attempts: 30,
            ..NetworkRetryPolicy::default()
        },
        Arc::new(PanicRuntimeObservability) as Arc<dyn camel_component_api::RuntimeObservability>,
    );

    let (tx, mut rx) = tokio::sync::mpsc::channel(16);
    let cancel = CancellationToken::new();
    let ctx = ConsumerContext::new(tx, cancel.clone());

    master.start(ctx).await.unwrap();

    // Wait for the delegate to actually start.
    let msg = timeout(Duration::from_secs(2), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(msg.exchange.input.body.as_text(), Some("slow-start"));

    // stop() must complete quickly because the leadership task is aborted,
    // not just timed-out and leaked.
    let start = Instant::now();
    master.stop().await.unwrap();
    let elapsed = start.elapsed();

    // With abort-first: ~0 ms. Without the fix: ~drain_timeout (500 ms).
    // Assert < 250 ms to reliably distinguish the two behaviours.
    assert!(
        elapsed < Duration::from_millis(250),
        "stop() took {:?}, expected < 250 ms (abort should be near-instant)",
        elapsed,
    );

    cancel.cancel();
}

#[tokio::test]
async fn stop_propagates_delegate_start_error() {
    struct FailingStartConsumer;

    #[async_trait]
    impl Consumer for FailingStartConsumer {
        async fn start(&mut self, _ctx: ConsumerContext) -> Result<(), CamelError> {
            Err(CamelError::ProcessorError(
                "delegate start failed".to_string(),
            ))
        }

        async fn stop(&mut self) -> Result<(), CamelError> {
            Ok(())
        }
    }

    struct FailingStartComponent;

    impl Component for FailingStartComponent {
        fn scheme(&self) -> &str {
            "failstart"
        }

        fn create_endpoint(
            &self,
            _uri: &str,
            _ctx: &dyn ComponentContext,
        ) -> Result<Box<dyn Endpoint>, CamelError> {
            Ok(Box::new(FailingStartEndpoint))
        }
    }

    struct FailingStartEndpoint;

    impl Endpoint for FailingStartEndpoint {
        fn uri(&self) -> &str {
            "failstart:delegate"
        }

        fn create_consumer(
            &self,
            _rt: std::sync::Arc<dyn camel_component_api::RuntimeObservability>,
        ) -> Result<Box<dyn Consumer>, CamelError> {
            Ok(Box::new(FailingStartConsumer))
        }

        fn create_producer(
            &self,
            _rt: std::sync::Arc<dyn camel_component_api::RuntimeObservability>,
            _ctx: &ProducerContext,
        ) -> Result<BoxProcessor, CamelError> {
            Err(CamelError::EndpointCreationFailed("not used".into()))
        }
    }

    let leadership = Arc::new(FakeLeadershipService::new(Some(
        LeadershipEvent::StartedLeading,
    )));
    let platform_service = Arc::new(FakePlatformService::new(leadership));

    let mut master = MasterConsumer::new(
        "lock-error".into(),
        "failstart:delegate".into(),
        Arc::new(FailingStartComponent),
        Arc::new(NoOpMetrics),
        platform_service,
        Duration::from_millis(500),
        NetworkRetryPolicy {
            max_attempts: 30,
            ..NetworkRetryPolicy::default()
        },
        Arc::new(PanicRuntimeObservability) as Arc<dyn camel_component_api::RuntimeObservability>,
    );

    let (tx, _rx) = tokio::sync::mpsc::channel(16);
    let cancel = CancellationToken::new();
    let ctx = ConsumerContext::new(tx, cancel.clone());

    master.start(ctx).await.unwrap();
    sleep(Duration::from_millis(250)).await;
    assert!(
        master
            .leadership_task
            .as_ref()
            .is_some_and(tokio::task::JoinHandle::is_finished),
        "leadership task should finish after delegate error"
    );
    let err = master.stop().await.expect_err("expected delegate error");
    assert!(err.to_string().contains("delegate start failed"));

    cancel.cancel();
}
