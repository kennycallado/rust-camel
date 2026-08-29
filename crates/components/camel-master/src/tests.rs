use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

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
    /// Leader epoch published to the supervision loop (shared through
    /// `LeadershipHandle::new`). Bumped from tests to simulate a coalesced
    /// takeover flap while the delegate stays Active.
    leader_epoch: Arc<AtomicU64>,
    initial: Option<LeadershipEvent>,
}

impl FakeLeadershipService {
    fn new(initial: Option<LeadershipEvent>) -> Self {
        let starts_as_leader = matches!(initial, Some(LeadershipEvent::StartedLeading));
        Self {
            tx: Mutex::new(None),
            is_leader: Arc::new(AtomicBool::new(starts_as_leader)),
            leader_epoch: Arc::new(AtomicU64::new(1)),
            initial,
        }
    }

    /// Shared handle to the epoch counter for bump-from-test access.
    fn leader_epoch(&self) -> Arc<AtomicU64> {
        Arc::clone(&self.leader_epoch)
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
            Arc::clone(&self.leader_epoch),
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
    let ctx = ConsumerContext::new(tx, cancel.clone(), "master-test-route".to_string());

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
    let ctx = ConsumerContext::new(tx, cancel.clone(), "master-test-route".to_string());

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
    let ctx = ConsumerContext::new(tx, cancel.clone(), "master-test-route".to_string());

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
    /// One-shot exit signal handed to the FIRST created consumer (create
    /// ordinal 1) only: the consumer exits on its own when the signal
    /// fires, so the supervision tick observes a finished handle without
    /// any teardown. `None` in every test that does not need the knob.
    first_exit_signal: Arc<Mutex<Option<watch::Receiver<()>>>>,
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
            first_exit_signal: Arc::clone(&self.first_exit_signal),
        }))
    }
}

struct ErrorDelegateEndpoint {
    create_consumer_calls: Arc<AtomicUsize>,
    consumer_error_after: usize,
    consumer_error: Option<CamelError>,
    first_exit_signal: Arc<Mutex<Option<watch::Receiver<()>>>>,
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
        // The one-shot exit signal is scoped to create ordinal 1: the
        // first consumer self-exits on the signal, later consumers run
        // until cancelled.
        let exit_signal = if call_idx == 1 {
            self.first_exit_signal
                .lock()
                .expect("mutex poisoned: delegate exit signal")
                .take()
        } else {
            None
        };
        Ok(Box::new(SuccessDelegateConsumer { exit_signal }))
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
/// With a one-shot `exit_signal` (dead-delegate test), it exits on its
/// own when the signal fires instead — the task handle finishes without
/// any teardown, so the supervision tick observes a dead Active delegate.
struct SuccessDelegateConsumer {
    exit_signal: Option<watch::Receiver<()>>,
}

#[async_trait]
impl Consumer for SuccessDelegateConsumer {
    async fn start(&mut self, context: ConsumerContext) -> Result<(), CamelError> {
        context.send(Exchange::new(Message::new("ok"))).await?;
        match self.exit_signal.as_mut() {
            Some(exit) => {
                let _ = exit.changed().await;
            }
            None => context.cancelled().await,
        }
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
    build_error_delegate_master_with_metrics(
        platform_service,
        create_endpoint_calls,
        create_consumer_calls,
        endpoint_error,
        consumer_error_after,
        consumer_error,
        max_attempts,
        Arc::new(NoOpMetrics),
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
    let ctx = ConsumerContext::new(tx, cancel.clone(), "master-test-route".to_string());

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
    let ctx = ConsumerContext::new(tx, cancel.clone(), "master-test-route".to_string());

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

// ── MST-001 metrics wiring tests (master-metrics-wiring Task 1.2) ──

const METRICS_TEST_LOCK: &str = "lock-err";
const METRICS_TEST_ROUTE: &str = "master-test-route";

/// One recorded counter observation: (metric name, value, owned labels).
type RecordedCounter = (String, f64, Vec<(String, String)>);

/// Metrics collector that records every `record_counter` observation as an
/// owned `(name, value, labels)` tuple. The five classic methods are no-ops:
/// the master component only emits counters today.
struct RecordingMetricsCollector {
    events: Mutex<Vec<RecordedCounter>>,
}

impl RecordingMetricsCollector {
    /// Filtered, order-preserving view of all observations for one metric.
    fn counters_named(&self, name: &str) -> Vec<(f64, Vec<(String, String)>)> {
        self.events
            .lock()
            .expect("mutex poisoned: recording metrics collector")
            .iter()
            .filter(|(recorded, _, _)| recorded == name)
            .map(|(_, value, labels)| (*value, labels.clone()))
            .collect()
    }

    /// Position of the nth (0-based) observation of `name` in the GLOBAL
    /// insertion order (across all metric names). `None` when fewer than
    /// n+1 observations exist. Used to assert cross-metric emission
    /// ordering; `Option` forces callers to handle absence explicitly.
    fn nth_global_index_of(&self, name: &str, n: usize) -> Option<usize> {
        self.events
            .lock()
            .expect("mutex poisoned: recording metrics collector")
            .iter()
            .enumerate()
            .filter(|(_, (recorded, _, _))| recorded == name)
            .nth(n)
            .map(|(idx, _)| idx)
    }
}

impl MetricsCollector for RecordingMetricsCollector {
    fn record_exchange_duration(&self, _route_id: &str, _duration: Duration) {}
    fn increment_errors(&self, _route_id: &str, _error_type: &str) {}
    fn increment_exchanges(&self, _route_id: &str) {}
    fn set_queue_depth(&self, _route_id: &str, _depth: usize) {}
    fn record_circuit_breaker_change(&self, _route_id: &str, _from: &str, _to: &str) {}
    fn record_counter(&self, name: &str, value: f64, labels: &[(&str, &str)]) {
        self.events
            .lock()
            .expect("mutex poisoned: recording metrics collector")
            .push((
                name.to_string(),
                value,
                labels
                    .iter()
                    .map(|(key, label_value)| (key.to_string(), label_value.to_string()))
                    .collect(),
            ));
    }
}

/// Transient per `is_retryable_camel_error` (`Io(_)` is always retryable).
fn transient_io_error() -> CamelError {
    CamelError::Io("boom".to_string())
}

/// Expected complete label set of a `master_delegate_lifecycle_total`
/// observation for the lock/route pair used by the metrics tests.
fn expected_lifecycle_labels(event: &str, reason: &str) -> Vec<(String, String)> {
    vec![
        ("lock".to_string(), METRICS_TEST_LOCK.to_string()),
        ("route_id".to_string(), METRICS_TEST_ROUTE.to_string()),
        ("event".to_string(), event.to_string()),
        ("reason".to_string(), reason.to_string()),
    ]
}

/// Count of lifecycle observations whose labels match `event` with the
/// default reason (`"none"`) for the lock/route pair used by the metrics
/// tests. `create_error` observations never match (different reason).
fn lifecycle_events(metrics: &RecordingMetricsCollector, event: &str) -> usize {
    metrics
        .counters_named("master_delegate_lifecycle_total")
        .iter()
        .filter(|(_, labels)| *labels == expected_lifecycle_labels(event, "none"))
        .count()
}

/// Expected complete label set of a `master_leadership_transitions_total`
/// observation for the lock/route pair used by the metrics tests.
fn expected_transition_labels(event: &str) -> Vec<(String, String)> {
    vec![
        ("lock".to_string(), METRICS_TEST_LOCK.to_string()),
        ("route_id".to_string(), METRICS_TEST_ROUTE.to_string()),
        ("event".to_string(), event.to_string()),
    ]
}

/// Sibling of [`build_error_delegate_master`] that injects a metrics
/// collector so tests observe every counter the supervision loop emits.
/// Recording-collector access is always through this builder.
#[allow(clippy::too_many_arguments)] // harness signature mandated by Task 1.2
fn build_error_delegate_master_with_metrics(
    platform_service: Arc<dyn PlatformService>,
    create_endpoint_calls: Arc<AtomicUsize>,
    create_consumer_calls: Arc<AtomicUsize>,
    endpoint_error: Option<CamelError>,
    consumer_error_after: usize,
    consumer_error: Option<CamelError>,
    max_attempts: u32,
    metrics: Arc<dyn MetricsCollector>,
) -> MasterConsumer {
    let reconnect = NetworkRetryPolicy {
        max_attempts,
        initial_delay: Duration::from_millis(1),
        max_delay: Duration::from_millis(5),
        multiplier: 1.0,
        ..NetworkRetryPolicy::default()
    };
    MasterConsumer::new(
        METRICS_TEST_LOCK.to_string(),
        "errdelegate:delegate".to_string(),
        Arc::new(ErrorDelegateComponent {
            create_endpoint_calls,
            create_consumer_calls,
            endpoint_error,
            consumer_error_after,
            consumer_error,
            first_exit_signal: Arc::new(Mutex::new(None)),
        }),
        metrics,
        platform_service,
        Duration::from_millis(500),
        reconnect,
        Arc::new(PanicRuntimeObservability) as Arc<dyn camel_component_api::RuntimeObservability>,
    )
}

/// Poll the recording collector until `name` has at least `count`
/// observations. Retries advance on the 200 ms `DELEGATE_RETRY_INTERVAL`
/// tick, so the 5 s bound covers every retry-consuming config below
/// (worst case 4 attempts x 200 ms = 800 ms).
async fn await_counter_observations(
    metrics: &RecordingMetricsCollector,
    name: &str,
    count: usize,
) -> bool {
    timeout(Duration::from_secs(5), async {
        loop {
            if metrics.counters_named(name).len() >= count {
                break;
            }
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .is_ok()
}

/// Poll the leadership task for completion (task failure or
/// budget-exhaustion shutdown) using the existing 5 ms poll pattern.
async fn await_leadership_task_exit(master: &MasterConsumer) -> bool {
    timeout(Duration::from_secs(5), async {
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
    .await
    .is_ok()
}

#[tokio::test]
async fn lifecycle_started_emitted_on_acquisition() {
    let leadership = Arc::new(FakeLeadershipService::new(Some(
        LeadershipEvent::StartedLeading,
    )));
    let platform_service = Arc::new(FakePlatformService::new(leadership));
    let create_endpoint_calls = Arc::new(AtomicUsize::new(0));
    let create_consumer_calls = Arc::new(AtomicUsize::new(0));
    let metrics = Arc::new(RecordingMetricsCollector {
        events: Mutex::new(Vec::new()),
    });

    let mut master = build_error_delegate_master_with_metrics(
        platform_service,
        Arc::clone(&create_endpoint_calls),
        Arc::clone(&create_consumer_calls),
        None,
        0,
        None,
        30,
        Arc::clone(&metrics) as Arc<dyn MetricsCollector>,
    );

    let (tx, mut rx) = tokio::sync::mpsc::channel(16);
    let cancel = CancellationToken::new();
    let ctx = ConsumerContext::new(tx, cancel.clone(), METRICS_TEST_ROUTE.to_string());

    master.start(ctx).await.unwrap();

    // Run to success: the delegate's first exchange arrives via the bridge.
    let first = timeout(Duration::from_secs(5), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first.exchange.input.body.as_text(), Some("ok"));

    // The "started" observation races the spawned delegate's first send, so
    // poll the collector rather than assuming ordering.
    assert!(
        await_counter_observations(&metrics, "master_delegate_lifecycle_total", 1).await,
        "started observation should be recorded within 5s"
    );

    let lifecycle = metrics.counters_named("master_delegate_lifecycle_total");
    assert_eq!(lifecycle.len(), 1);
    assert_eq!(
        lifecycle[0],
        (1.0, expected_lifecycle_labels("started", "none"))
    );

    cancel.cancel();
    master.stop().await.unwrap();
}

#[tokio::test]
async fn lifecycle_stopped_emitted_after_active_drain() {
    let leadership = Arc::new(FakeLeadershipService::new(Some(
        LeadershipEvent::StartedLeading,
    )));
    let platform_service = Arc::new(FakePlatformService::new(leadership.clone()));
    let create_endpoint_calls = Arc::new(AtomicUsize::new(0));
    let create_consumer_calls = Arc::new(AtomicUsize::new(0));
    let metrics = Arc::new(RecordingMetricsCollector {
        events: Mutex::new(Vec::new()),
    });

    let mut master = build_error_delegate_master_with_metrics(
        platform_service,
        Arc::clone(&create_endpoint_calls),
        Arc::clone(&create_consumer_calls),
        None,
        0,
        None,
        30,
        Arc::clone(&metrics) as Arc<dyn MetricsCollector>,
    );

    let (tx, mut rx) = tokio::sync::mpsc::channel(16);
    let cancel = CancellationToken::new();
    let ctx = ConsumerContext::new(tx, cancel.clone(), METRICS_TEST_ROUTE.to_string());

    master.start(ctx).await.unwrap();

    let first = timeout(Duration::from_secs(5), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first.exchange.input.body.as_text(), Some("ok"));
    assert!(
        await_counter_observations(&metrics, "master_delegate_lifecycle_total", 1).await,
        "started observation should be recorded within 5s"
    );

    leadership.emit(LeadershipEvent::StoppedLeading).await;

    // The drain is bounded by stop_delegate's drain timeout; poll until the
    // "stopped" observation lands (started + stopped).
    assert!(
        await_counter_observations(&metrics, "master_delegate_lifecycle_total", 2).await,
        "stopped observation should be recorded after the active drain"
    );

    let lifecycle = metrics.counters_named("master_delegate_lifecycle_total");
    assert_eq!(lifecycle.len(), 2);
    assert_eq!(
        lifecycle[0],
        (1.0, expected_lifecycle_labels("started", "none"))
    );
    assert_eq!(
        lifecycle[1],
        (1.0, expected_lifecycle_labels("stopped", "none"))
    );

    cancel.cancel();
    master.stop().await.unwrap();
}

#[tokio::test]
async fn inactive_stop_emits_nothing() {
    let leadership = Arc::new(FakeLeadershipService::new(None));
    let platform_service = Arc::new(FakePlatformService::new(leadership));
    let create_endpoint_calls = Arc::new(AtomicUsize::new(0));
    let create_consumer_calls = Arc::new(AtomicUsize::new(0));
    let metrics = Arc::new(RecordingMetricsCollector {
        events: Mutex::new(Vec::new()),
    });

    let mut master = build_error_delegate_master_with_metrics(
        platform_service,
        Arc::clone(&create_endpoint_calls),
        Arc::clone(&create_consumer_calls),
        None,
        0,
        None,
        30,
        Arc::clone(&metrics) as Arc<dyn MetricsCollector>,
    );

    let (tx, _rx) = tokio::sync::mpsc::channel(16);
    let cancel = CancellationToken::new();
    let ctx = ConsumerContext::new(tx, cancel.clone(), METRICS_TEST_ROUTE.to_string());

    master.start(ctx).await.unwrap();

    // Never leading: no initial snapshot event and no watch deliveries. Let
    // the supervision loop idle across a few retry ticks.
    sleep(Duration::from_millis(100)).await;

    cancel.cancel();
    master.stop().await.unwrap();

    assert!(
        metrics
            .counters_named("master_delegate_lifecycle_total")
            .is_empty(),
        "inactive leadership must not emit lifecycle observations"
    );
    assert_eq!(create_endpoint_calls.load(Ordering::SeqCst), 0);
    assert_eq!(create_consumer_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn create_error_endpoint_transient() {
    let leadership = Arc::new(FakeLeadershipService::new(Some(
        LeadershipEvent::StartedLeading,
    )));
    let platform_service = Arc::new(FakePlatformService::new(leadership));
    let create_endpoint_calls = Arc::new(AtomicUsize::new(0));
    let create_consumer_calls = Arc::new(AtomicUsize::new(0));
    let metrics = Arc::new(RecordingMetricsCollector {
        events: Mutex::new(Vec::new()),
    });

    // max_attempts = 1: the initial snapshot consults should_retry(0)
    // (allowed) and counts itself, so exactly one create is attempted; the
    // next tick consults should_retry(1) → refused, and budget exhaustion
    // stops the consumer. (The previous two-observation assertion ratified
    // the N+1 snapshot quirk; in-arm counting fixes it.)
    let mut master = build_error_delegate_master_with_metrics(
        platform_service,
        Arc::clone(&create_endpoint_calls),
        Arc::clone(&create_consumer_calls),
        Some(transient_io_error()),
        0,
        None,
        1,
        Arc::clone(&metrics) as Arc<dyn MetricsCollector>,
    );

    let (tx, _rx) = tokio::sync::mpsc::channel(16);
    let cancel = CancellationToken::new();
    let ctx = ConsumerContext::new(tx, cancel.clone(), METRICS_TEST_ROUTE.to_string());

    master.start(ctx).await.unwrap();

    // Budget exhaustion terminates the leadership task (clean exit).
    assert!(
        await_leadership_task_exit(&master).await,
        "budget-exhaustion shutdown should finish the leadership task within 5s"
    );

    let lifecycle = metrics.counters_named("master_delegate_lifecycle_total");
    assert_eq!(lifecycle.len(), 1);
    assert_eq!(
        lifecycle[0],
        (1.0, expected_lifecycle_labels("create_error", "transient"))
    );
    assert_eq!(
        create_endpoint_calls.load(Ordering::SeqCst),
        1,
        "exact budget: one counted attempt at max_attempts = 1"
    );

    cancel.cancel();
    let _ = master.stop().await;
}

#[tokio::test]
async fn create_error_endpoint_permanent() {
    let leadership = Arc::new(FakeLeadershipService::new(Some(
        LeadershipEvent::StartedLeading,
    )));
    let platform_service = Arc::new(FakePlatformService::new(leadership));
    let create_endpoint_calls = Arc::new(AtomicUsize::new(0));
    let create_consumer_calls = Arc::new(AtomicUsize::new(0));
    let metrics = Arc::new(RecordingMetricsCollector {
        events: Mutex::new(Vec::new()),
    });

    let mut master = build_error_delegate_master_with_metrics(
        platform_service,
        Arc::clone(&create_endpoint_calls),
        Arc::clone(&create_consumer_calls),
        Some(CamelError::EndpointCreationFailed("permanent".to_string())),
        0,
        None,
        30,
        Arc::clone(&metrics) as Arc<dyn MetricsCollector>,
    );

    let (tx, _rx) = tokio::sync::mpsc::channel(16);
    let cancel = CancellationToken::new();
    let ctx = ConsumerContext::new(tx, cancel.clone(), METRICS_TEST_ROUTE.to_string());

    master.start(ctx).await.unwrap();

    // Permanent errors fail fast: the leadership task terminates on the
    // first attempt, not via budget exhaustion.
    assert!(
        await_leadership_task_exit(&master).await,
        "permanent error must terminate the leadership task within 5s"
    );

    let lifecycle = metrics.counters_named("master_delegate_lifecycle_total");
    assert_eq!(lifecycle.len(), 1);
    assert_eq!(
        lifecycle[0],
        (1.0, expected_lifecycle_labels("create_error", "permanent"))
    );
    assert_eq!(
        create_endpoint_calls.load(Ordering::SeqCst),
        1,
        "permanent error must fail fast after exactly 1 invocation"
    );

    cancel.cancel();
    let _ = master.stop().await;
}

#[tokio::test]
async fn create_error_consumer_transient() {
    let leadership = Arc::new(FakeLeadershipService::new(Some(
        LeadershipEvent::StartedLeading,
    )));
    let platform_service = Arc::new(FakePlatformService::new(leadership));
    let create_endpoint_calls = Arc::new(AtomicUsize::new(0));
    let create_consumer_calls = Arc::new(AtomicUsize::new(0));
    let metrics = Arc::new(RecordingMetricsCollector {
        events: Mutex::new(Vec::new()),
    });

    // First 2 create_consumer calls fail transiently, the 3rd succeeds.
    let mut master = build_error_delegate_master_with_metrics(
        platform_service,
        Arc::clone(&create_endpoint_calls),
        Arc::clone(&create_consumer_calls),
        None,
        2,
        Some(transient_io_error()),
        3,
        Arc::clone(&metrics) as Arc<dyn MetricsCollector>,
    );

    let (tx, mut rx) = tokio::sync::mpsc::channel(16);
    let cancel = CancellationToken::new();
    let ctx = ConsumerContext::new(tx, cancel.clone(), METRICS_TEST_ROUTE.to_string());

    master.start(ctx).await.unwrap();

    // Run to success: two transient failures, then the delegate starts.
    let first = timeout(Duration::from_secs(5), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first.exchange.input.body.as_text(), Some("ok"));
    assert!(
        await_counter_observations(&metrics, "master_delegate_lifecycle_total", 3).await,
        "two transient create_error observations plus started should be recorded"
    );

    let lifecycle = metrics.counters_named("master_delegate_lifecycle_total");
    assert_eq!(lifecycle.len(), 3);
    assert_eq!(
        lifecycle[0],
        (1.0, expected_lifecycle_labels("create_error", "transient"))
    );
    assert_eq!(
        lifecycle[1],
        (1.0, expected_lifecycle_labels("create_error", "transient"))
    );
    assert_eq!(
        lifecycle[2],
        (1.0, expected_lifecycle_labels("started", "none"))
    );
    assert_eq!(create_consumer_calls.load(Ordering::SeqCst), 3);

    cancel.cancel();
    master.stop().await.unwrap();
}

#[tokio::test]
async fn create_error_consumer_permanent() {
    let leadership = Arc::new(FakeLeadershipService::new(Some(
        LeadershipEvent::StartedLeading,
    )));
    let platform_service = Arc::new(FakePlatformService::new(leadership));
    let create_endpoint_calls = Arc::new(AtomicUsize::new(0));
    let create_consumer_calls = Arc::new(AtomicUsize::new(0));
    let metrics = Arc::new(RecordingMetricsCollector {
        events: Mutex::new(Vec::new()),
    });

    let mut master = build_error_delegate_master_with_metrics(
        platform_service,
        Arc::clone(&create_endpoint_calls),
        Arc::clone(&create_consumer_calls),
        None,
        1,
        Some(CamelError::ProcessorError("permanent".to_string())),
        30,
        Arc::clone(&metrics) as Arc<dyn MetricsCollector>,
    );

    let (tx, _rx) = tokio::sync::mpsc::channel(16);
    let cancel = CancellationToken::new();
    let ctx = ConsumerContext::new(tx, cancel.clone(), METRICS_TEST_ROUTE.to_string());

    master.start(ctx).await.unwrap();

    assert!(
        await_leadership_task_exit(&master).await,
        "permanent consumer error must terminate the leadership task within 5s"
    );

    let lifecycle = metrics.counters_named("master_delegate_lifecycle_total");
    assert_eq!(lifecycle.len(), 1);
    assert_eq!(
        lifecycle[0],
        (1.0, expected_lifecycle_labels("create_error", "permanent"))
    );
    assert_eq!(
        create_consumer_calls.load(Ordering::SeqCst),
        1,
        "permanent error must fail fast after exactly 1 invocation"
    );

    cancel.cancel();
    let _ = master.stop().await;
}

#[tokio::test]
async fn retry_accumulation_one_transition_n_create_errors() {
    let leadership = Arc::new(FakeLeadershipService::new(Some(
        LeadershipEvent::StartedLeading,
    )));
    let platform_service = Arc::new(FakePlatformService::new(leadership));
    let create_endpoint_calls = Arc::new(AtomicUsize::new(0));
    let create_consumer_calls = Arc::new(AtomicUsize::new(0));
    let metrics = Arc::new(RecordingMetricsCollector {
        events: Mutex::new(Vec::new()),
    });

    // 3 transient failures then success: the retry tick re-dispatches
    // synthetic StartedLeading, which must not re-emit the transition.
    let mut master = build_error_delegate_master_with_metrics(
        platform_service,
        Arc::clone(&create_endpoint_calls),
        Arc::clone(&create_consumer_calls),
        None,
        3,
        Some(transient_io_error()),
        4,
        Arc::clone(&metrics) as Arc<dyn MetricsCollector>,
    );

    let (tx, mut rx) = tokio::sync::mpsc::channel(16);
    let cancel = CancellationToken::new();
    let ctx = ConsumerContext::new(tx, cancel.clone(), METRICS_TEST_ROUTE.to_string());

    master.start(ctx).await.unwrap();

    // Run to success: three transient failures, then the delegate starts
    // (3 create_error + 1 started lifecycle observations in total).
    let first = timeout(Duration::from_secs(5), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first.exchange.input.body.as_text(), Some("ok"));
    assert!(
        await_counter_observations(&metrics, "master_delegate_lifecycle_total", 4).await,
        "three create_error observations plus started should be recorded"
    );

    let transitions = metrics.counters_named("master_leadership_transitions_total");
    assert_eq!(transitions.len(), 1);
    assert_eq!(
        transitions[0],
        (1.0, expected_transition_labels("acquired"))
    );

    let lifecycle = metrics.counters_named("master_delegate_lifecycle_total");
    assert_eq!(
        lifecycle.len(),
        4,
        "three transient create_error observations plus one started"
    );
    let transient_create_errors = lifecycle
        .iter()
        .filter(|(_, labels)| *labels == expected_lifecycle_labels("create_error", "transient"))
        .count();
    assert_eq!(transient_create_errors, 3);
    assert_eq!(
        lifecycle[3],
        (1.0, expected_lifecycle_labels("started", "none"))
    );
    assert_eq!(create_consumer_calls.load(Ordering::SeqCst), 4);

    cancel.cancel();
    master.stop().await.unwrap();
}

// ── Exact acquisition budget tests (camel-master-reconcile-hygiene
// Task 1.2) ──────────────────────────────────────────────────────────

#[tokio::test]
async fn term_bump_at_exhausted_budget_reacquires_fresh() {
    let leadership = Arc::new(FakeLeadershipService::new(Some(
        LeadershipEvent::StartedLeading,
    )));
    let platform_service = Arc::new(FakePlatformService::new(leadership.clone()));
    let create_endpoint_calls = Arc::new(AtomicUsize::new(0));
    let create_consumer_calls = Arc::new(AtomicUsize::new(0));
    let metrics = Arc::new(RecordingMetricsCollector {
        events: Mutex::new(Vec::new()),
    });

    // max_attempts = 1 with a healthy delegate: the initial snapshot spends
    // the whole budget. A guard-detected term bump on the live delegate is a
    // new acquisition epoch: it must reset the budget and recreate — never
    // leave a zombie delegate stamped at the old epoch.
    let mut master = build_error_delegate_master_with_metrics(
        platform_service,
        Arc::clone(&create_endpoint_calls),
        Arc::clone(&create_consumer_calls),
        None,
        0,
        None,
        1,
        Arc::clone(&metrics) as Arc<dyn MetricsCollector>,
    );

    let (tx, mut rx) = tokio::sync::mpsc::channel(16);
    let cancel = CancellationToken::new();
    let ctx = ConsumerContext::new(tx, cancel.clone(), METRICS_TEST_ROUTE.to_string());

    master.start(ctx).await.unwrap();

    // Baseline barrier: the first exchange (stamped epoch 1) plus the
    // started lifecycle observation confirm the initial acquisition landed.
    let first = timeout(Duration::from_secs(5), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first.exchange.input.body.as_text(), Some("ok"));
    let first_epoch = first
        .exchange
        .properties
        .get(crate::leadership::LEADER_EPOCH_PROPERTY)
        .and_then(|v| v.as_str());
    assert_eq!(first_epoch, Some("1"), "baseline bridge stamps epoch 1");
    assert!(
        await_counter_observations(&metrics, "master_delegate_lifecycle_total", 1).await,
        "started observation should be recorded within 5s"
    );

    // Delivery-driven term bump on a live delegate with an exhausted budget.
    leadership.leader_epoch().store(2, Ordering::Release);
    leadership.emit(LeadershipEvent::StartedLeading).await;

    let recreated = timeout(Duration::from_secs(5), async {
        loop {
            if create_consumer_calls.load(Ordering::SeqCst) >= 2 {
                break;
            }
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .is_ok();
    assert!(
        recreated,
        "term bump must reset the exhausted budget and recreate the delegate within 5s"
    );
    assert!(
        !master
            .leadership_task
            .as_ref()
            .is_some_and(|h| h.is_finished()),
        "a fresh acquisition epoch must keep the leadership task alive"
    );

    // The next envelope comes from the recreated delegate, restamped at the
    // bumped epoch.
    let second = timeout(Duration::from_secs(5), rx.recv())
        .await
        .unwrap()
        .unwrap();
    let second_epoch = second
        .exchange
        .properties
        .get(crate::leadership::LEADER_EPOCH_PROPERTY)
        .expect("recreated bridge must stamp x-camel-leader-epoch");
    assert_eq!(
        second_epoch,
        &serde_json::Value::String("2".to_string()),
        "recreated bridge must carry the bumped epoch 2"
    );

    cancel.cancel();
    master.stop().await.unwrap();
}

#[tokio::test]
async fn persistent_transient_at_max_two_attempts_exactly_twice() {
    let leadership = Arc::new(FakeLeadershipService::new(Some(
        LeadershipEvent::StartedLeading,
    )));
    let platform_service = Arc::new(FakePlatformService::new(leadership));
    let create_endpoint_calls = Arc::new(AtomicUsize::new(0));
    let create_consumer_calls = Arc::new(AtomicUsize::new(0));
    let metrics = Arc::new(RecordingMetricsCollector {
        events: Mutex::new(Vec::new()),
    });

    // max_attempts = 2 with persistent transient endpoint failure: exactly
    // two counted attempts, then the budget refuses and the consumer stops.
    let mut master = build_error_delegate_master_with_metrics(
        platform_service,
        Arc::clone(&create_endpoint_calls),
        Arc::clone(&create_consumer_calls),
        Some(transient_io_error()),
        0,
        None,
        2,
        Arc::clone(&metrics) as Arc<dyn MetricsCollector>,
    );

    let (tx, _rx) = tokio::sync::mpsc::channel(16);
    let cancel = CancellationToken::new();
    let ctx = ConsumerContext::new(tx, cancel.clone(), METRICS_TEST_ROUTE.to_string());

    master.start(ctx).await.unwrap();

    assert!(
        await_leadership_task_exit(&master).await,
        "budget-exhaustion shutdown should finish the leadership task within 5s"
    );

    let lifecycle = metrics.counters_named("master_delegate_lifecycle_total");
    assert_eq!(lifecycle.len(), 2, "exactly two create_error observations");
    assert_eq!(
        lifecycle[0],
        (1.0, expected_lifecycle_labels("create_error", "transient"))
    );
    assert_eq!(
        lifecycle[1],
        (1.0, expected_lifecycle_labels("create_error", "transient"))
    );
    assert_eq!(
        create_endpoint_calls.load(Ordering::SeqCst),
        2,
        "exact budget: two counted attempts at max_attempts = 2"
    );

    cancel.cancel();
    let _ = master.stop().await;
}

#[tokio::test]
async fn exhausted_budget_refuses_duplicate_delivery() {
    let leadership = Arc::new(FakeLeadershipService::new(Some(
        LeadershipEvent::StartedLeading,
    )));
    let platform_service = Arc::new(FakePlatformService::new(leadership.clone()));
    let create_endpoint_calls = Arc::new(AtomicUsize::new(0));
    let create_consumer_calls = Arc::new(AtomicUsize::new(0));
    let metrics = Arc::new(RecordingMetricsCollector {
        events: Mutex::new(Vec::new()),
    });

    // max_attempts = 1, transient endpoint error. The duplicate
    // StartedLeading delivered right after start (delegate Inactive after
    // the failed create) counts against the same acquisition epoch but
    // performs no create.
    let mut master = build_error_delegate_master_with_metrics(
        platform_service,
        Arc::clone(&create_endpoint_calls),
        Arc::clone(&create_consumer_calls),
        Some(transient_io_error()),
        0,
        None,
        1,
        Arc::clone(&metrics) as Arc<dyn MetricsCollector>,
    );

    let (tx, _rx) = tokio::sync::mpsc::channel(16);
    let cancel = CancellationToken::new();
    let ctx = ConsumerContext::new(tx, cancel.clone(), METRICS_TEST_ROUTE.to_string());

    master.start(ctx).await.unwrap();

    leadership.emit(LeadershipEvent::StartedLeading).await;

    assert!(
        await_leadership_task_exit(&master).await,
        "budget-exhaustion shutdown should finish the leadership task within 5s"
    );

    let lifecycle = metrics.counters_named("master_delegate_lifecycle_total");
    assert_eq!(lifecycle.len(), 1);
    assert_eq!(
        lifecycle[0],
        (1.0, expected_lifecycle_labels("create_error", "transient"))
    );
    assert_eq!(
        create_endpoint_calls.load(Ordering::SeqCst),
        1,
        "the duplicate delivery counted but performed no create"
    );

    cancel.cancel();
    let _ = master.stop().await;
}

#[tokio::test]
async fn disabled_policy_creates_nothing() {
    let leadership = Arc::new(FakeLeadershipService::new(Some(
        LeadershipEvent::StartedLeading,
    )));
    let platform_service = Arc::new(FakePlatformService::new(leadership));
    let create_endpoint_calls = Arc::new(AtomicUsize::new(0));
    let create_consumer_calls = Arc::new(AtomicUsize::new(0));
    let metrics = Arc::new(RecordingMetricsCollector {
        events: Mutex::new(Vec::new()),
    });

    // Constructed directly (not via the metrics builder): a disabled policy
    // is a deliberate non-default configuration. The in-arm consult refuses
    // every delivery — zero creates, consumer stops at the first tick.
    let mut master = MasterConsumer::new(
        METRICS_TEST_LOCK.to_string(),
        "errdelegate:delegate".to_string(),
        Arc::new(ErrorDelegateComponent {
            create_endpoint_calls: Arc::clone(&create_endpoint_calls),
            create_consumer_calls: Arc::clone(&create_consumer_calls),
            endpoint_error: None,
            consumer_error_after: 0,
            consumer_error: None,
            first_exit_signal: Arc::new(Mutex::new(None)),
        }),
        Arc::clone(&metrics) as Arc<dyn MetricsCollector>,
        platform_service,
        Duration::from_millis(500),
        NetworkRetryPolicy::disabled(),
        Arc::new(PanicRuntimeObservability) as Arc<dyn camel_component_api::RuntimeObservability>,
    );

    let (tx, _rx) = tokio::sync::mpsc::channel(16);
    let cancel = CancellationToken::new();
    let ctx = ConsumerContext::new(tx, cancel.clone(), METRICS_TEST_ROUTE.to_string());

    master.start(ctx).await.unwrap();

    assert!(
        await_leadership_task_exit(&master).await,
        "a disabled policy must stop the consumer at the first retry tick"
    );

    assert_eq!(
        create_endpoint_calls.load(Ordering::SeqCst),
        0,
        "a disabled policy must perform no endpoint create"
    );
    assert_eq!(
        create_consumer_calls.load(Ordering::SeqCst),
        0,
        "a disabled policy must perform no consumer create"
    );
    assert!(
        metrics
            .counters_named("master_delegate_lifecycle_total")
            .is_empty(),
        "a disabled policy must emit no lifecycle observations"
    );

    cancel.cancel();
    let _ = master.stop().await;
}

#[tokio::test]
async fn unlimited_default_keeps_retrying() {
    let leadership = Arc::new(FakeLeadershipService::new(Some(
        LeadershipEvent::StartedLeading,
    )));
    let platform_service = Arc::new(FakePlatformService::new(leadership));
    let create_endpoint_calls = Arc::new(AtomicUsize::new(0));
    let create_consumer_calls = Arc::new(AtomicUsize::new(0));
    let metrics = Arc::new(RecordingMetricsCollector {
        events: Mutex::new(Vec::new()),
    });

    // max_attempts = 0 (default): unlimited — persistent transient failure
    // must never exhaust the budget or stop the consumer.
    let mut master = build_error_delegate_master_with_metrics(
        platform_service,
        Arc::clone(&create_endpoint_calls),
        Arc::clone(&create_consumer_calls),
        Some(transient_io_error()),
        0,
        None,
        0,
        Arc::clone(&metrics) as Arc<dyn MetricsCollector>,
    );

    let (tx, _rx) = tokio::sync::mpsc::channel(16);
    let cancel = CancellationToken::new();
    let ctx = ConsumerContext::new(tx, cancel.clone(), METRICS_TEST_ROUTE.to_string());

    master.start(ctx).await.unwrap();

    assert!(
        await_counter_observations(&metrics, "master_delegate_lifecycle_total", 3).await,
        "an unlimited policy should keep recording create_error observations"
    );

    let create_errors = metrics
        .counters_named("master_delegate_lifecycle_total")
        .iter()
        .filter(|(_, labels)| *labels == expected_lifecycle_labels("create_error", "transient"))
        .count();
    assert!(
        create_errors >= 3,
        "expected at least 3 create_error observations"
    );

    sleep(Duration::from_secs(2)).await;
    assert!(
        !master
            .leadership_task
            .as_ref()
            .is_some_and(|h| h.is_finished()),
        "max_attempts = 0 must never exhaust the budget"
    );

    cancel.cancel();
    let _ = master.stop().await;
}

// ── MST-001 leadership transition edge tests (master-metrics-wiring
// Task 1.3) ──────────────────────────────────────────────────────────

#[tokio::test]
async fn transition_acquired_on_initial_snapshot() {
    let leadership = Arc::new(FakeLeadershipService::new(Some(
        LeadershipEvent::StartedLeading,
    )));
    let platform_service = Arc::new(FakePlatformService::new(leadership));
    let create_endpoint_calls = Arc::new(AtomicUsize::new(0));
    let create_consumer_calls = Arc::new(AtomicUsize::new(0));
    let metrics = Arc::new(RecordingMetricsCollector {
        events: Mutex::new(Vec::new()),
    });

    let mut master = build_error_delegate_master_with_metrics(
        platform_service,
        Arc::clone(&create_endpoint_calls),
        Arc::clone(&create_consumer_calls),
        None,
        0,
        None,
        30,
        Arc::clone(&metrics) as Arc<dyn MetricsCollector>,
    );

    let (tx, mut rx) = tokio::sync::mpsc::channel(16);
    let cancel = CancellationToken::new();
    let ctx = ConsumerContext::new(tx, cancel.clone(), METRICS_TEST_ROUTE.to_string());

    master.start(ctx).await.unwrap();

    // Success signal: the delegate's first exchange arrives via the bridge.
    let first = timeout(Duration::from_secs(5), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first.exchange.input.body.as_text(), Some("ok"));

    assert!(
        await_counter_observations(&metrics, "master_leadership_transitions_total", 1).await,
        "initial-snapshot acquisition should be recorded within 5s"
    );

    let transitions = metrics.counters_named("master_leadership_transitions_total");
    assert_eq!(transitions.len(), 1);
    assert_eq!(
        transitions[0],
        (1.0, expected_transition_labels("acquired"))
    );

    cancel.cancel();
    master.stop().await.unwrap();
}

#[tokio::test]
async fn transition_lost_on_leading_edge() {
    let leadership = Arc::new(FakeLeadershipService::new(Some(
        LeadershipEvent::StartedLeading,
    )));
    let platform_service = Arc::new(FakePlatformService::new(leadership.clone()));
    let create_endpoint_calls = Arc::new(AtomicUsize::new(0));
    let create_consumer_calls = Arc::new(AtomicUsize::new(0));
    let metrics = Arc::new(RecordingMetricsCollector {
        events: Mutex::new(Vec::new()),
    });

    let mut master = build_error_delegate_master_with_metrics(
        platform_service,
        Arc::clone(&create_endpoint_calls),
        Arc::clone(&create_consumer_calls),
        None,
        0,
        None,
        30,
        Arc::clone(&metrics) as Arc<dyn MetricsCollector>,
    );

    let (tx, mut rx) = tokio::sync::mpsc::channel(16);
    let cancel = CancellationToken::new();
    let ctx = ConsumerContext::new(tx, cancel.clone(), METRICS_TEST_ROUTE.to_string());

    master.start(ctx).await.unwrap();

    let first = timeout(Duration::from_secs(5), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first.exchange.input.body.as_text(), Some("ok"));
    assert!(
        await_counter_observations(&metrics, "master_leadership_transitions_total", 1).await,
        "initial acquisition should be recorded within 5s"
    );

    leadership.emit(LeadershipEvent::StoppedLeading).await;

    // Bounded drain wait: the lost edge is emitted before reconcile_event
    // stops the delegate.
    assert!(
        await_counter_observations(&metrics, "master_leadership_transitions_total", 2).await,
        "lost transition should be recorded within 5s of the StoppedLeading delivery"
    );

    let transitions = metrics.counters_named("master_leadership_transitions_total");
    assert_eq!(transitions.len(), 2);
    assert_eq!(
        transitions[0],
        (1.0, expected_transition_labels("acquired"))
    );
    assert_eq!(transitions[1], (1.0, expected_transition_labels("lost")));

    // Spec clause: the lost transition is emitted BEFORE reconcile_event
    // processes the edge — i.e. before the delegate drain emits its
    // ("event","stopped") lifecycle observation. Await the stopped
    // observation (lifecycle count 2 = started + stopped), then compare
    // the global insertion position of the SECOND transition (lost)
    // against the SECOND lifecycle observation (stopped).
    assert!(
        await_counter_observations(&metrics, "master_delegate_lifecycle_total", 2).await,
        "stopped lifecycle observation should be recorded within 5s of the lost edge"
    );
    let lost_idx = metrics
        .nth_global_index_of("master_leadership_transitions_total", 1)
        .expect("two transition observations recorded");
    let stopped_idx = metrics
        .nth_global_index_of("master_delegate_lifecycle_total", 1)
        .expect("two lifecycle observations recorded");
    assert!(
        lost_idx < stopped_idx,
        "lost transition (global index {lost_idx}) must precede the stopped \
         lifecycle observation (global index {stopped_idx})"
    );

    cancel.cancel();
    master.stop().await.unwrap();
}

#[tokio::test]
async fn repeated_identical_delivery_does_not_reemit() {
    let leadership = Arc::new(FakeLeadershipService::new(Some(
        LeadershipEvent::StartedLeading,
    )));
    let platform_service = Arc::new(FakePlatformService::new(leadership.clone()));
    let create_endpoint_calls = Arc::new(AtomicUsize::new(0));
    let create_consumer_calls = Arc::new(AtomicUsize::new(0));
    let metrics = Arc::new(RecordingMetricsCollector {
        events: Mutex::new(Vec::new()),
    });

    let mut master = build_error_delegate_master_with_metrics(
        platform_service,
        Arc::clone(&create_endpoint_calls),
        Arc::clone(&create_consumer_calls),
        None,
        0,
        None,
        30,
        Arc::clone(&metrics) as Arc<dyn MetricsCollector>,
    );

    let (tx, mut rx) = tokio::sync::mpsc::channel(16);
    let cancel = CancellationToken::new();
    let ctx = ConsumerContext::new(tx, cancel.clone(), METRICS_TEST_ROUTE.to_string());

    master.start(ctx).await.unwrap();

    // Baseline: leading with an active delegate and exactly one acquired
    // transition from the initial snapshot.
    let first = timeout(Duration::from_secs(5), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first.exchange.input.body.as_text(), Some("ok"));
    assert!(
        await_counter_observations(&metrics, "master_leadership_transitions_total", 1).await,
        "initial acquisition should be recorded within 5s"
    );

    // Phase A: deliver StartedLeading again (true → true, no edge). An
    // identical delivery while Active at the published epoch must be an
    // epoch-idempotent no-op: the delegate is neither stopped nor
    // recreated. No counter exists for a no-op delivery, so settle 500 ms
    // (matches the file's sleep-settle precedent) and assert stability.
    let baseline_started = lifecycle_events(&metrics, "started");
    let baseline_stopped = lifecycle_events(&metrics, "stopped");
    assert_eq!(
        baseline_started, 1,
        "baseline delegate should have started exactly once"
    );
    assert_eq!(baseline_stopped, 0, "baseline should have no stops");

    leadership.emit(LeadershipEvent::StartedLeading).await;
    sleep(Duration::from_millis(500)).await;

    assert_eq!(
        create_consumer_calls.load(Ordering::SeqCst),
        1,
        "duplicate StartedLeading must not recreate the delegate"
    );
    assert_eq!(
        lifecycle_events(&metrics, "started"),
        baseline_started,
        "duplicate StartedLeading must not emit a started lifecycle observation"
    );
    assert_eq!(
        lifecycle_events(&metrics, "stopped"),
        baseline_stopped,
        "duplicate StartedLeading must not emit a stopped lifecycle observation"
    );

    let acquired_count = metrics
        .counters_named("master_leadership_transitions_total")
        .iter()
        .filter(|(_, labels)| *labels == expected_transition_labels("acquired"))
        .count();
    assert_eq!(
        acquired_count, 1,
        "true→true delivery must not re-emit the acquired transition"
    );

    // Establish the leading → not-leading edge so Phase B starts from
    // not-leading with exactly one lost transition recorded. The await is
    // the positive delivery-processing barrier for everything above: watch
    // deliveries are handled in order, so a late-processed duplicate with
    // a broken guard would bump create_consumer_calls before this edge is
    // recorded — caught by the asserts below.
    leadership.emit(LeadershipEvent::StoppedLeading).await;
    assert!(
        await_counter_observations(&metrics, "master_leadership_transitions_total", 2).await,
        "lost transition should be recorded within 5s of the edge"
    );
    // The lost transition is emitted before reconcile_event stops the
    // delegate, so await the stopped observation before asserting deltas.
    assert!(
        await_counter_observations(
            &metrics,
            "master_delegate_lifecycle_total",
            baseline_started + baseline_stopped + 1
        )
        .await,
        "delegate stop at the lost edge should be recorded within 5s"
    );
    assert_eq!(
        create_consumer_calls.load(Ordering::SeqCst),
        1,
        "no delivery up to the lost edge may recreate the delegate"
    );
    assert_eq!(
        lifecycle_events(&metrics, "started"),
        baseline_started,
        "duplicate StartedLeading must not emit a started lifecycle observation"
    );
    assert_eq!(
        lifecycle_events(&metrics, "stopped"),
        baseline_stopped + 1,
        "the lost edge must stop the delegate exactly once"
    );

    // Phase B: StoppedLeading twice in a row (false → false, no edge).
    // Neither delivery emits anything, so there is no counter to await
    // (no positive signal exists for a no-edge delivery); settle 500 ms
    // (matches the file's sleep-settle precedent). Back-to-back watch
    // emits may coalesce into one observation — either way, no re-emit
    // is expected.
    leadership.emit(LeadershipEvent::StoppedLeading).await;
    leadership.emit(LeadershipEvent::StoppedLeading).await;
    sleep(Duration::from_millis(500)).await;

    let lost_count = metrics
        .counters_named("master_leadership_transitions_total")
        .iter()
        .filter(|(_, labels)| *labels == expected_transition_labels("lost"))
        .count();
    assert_eq!(
        lost_count, 1,
        "false→false deliveries must not re-emit the lost transition"
    );

    cancel.cancel();
    master.stop().await.unwrap();
}

#[tokio::test]
async fn term_bump_while_active_reconciles_once() {
    let leadership = Arc::new(FakeLeadershipService::new(Some(
        LeadershipEvent::StartedLeading,
    )));
    let platform_service = Arc::new(FakePlatformService::new(leadership.clone()));
    let create_endpoint_calls = Arc::new(AtomicUsize::new(0));
    let create_consumer_calls = Arc::new(AtomicUsize::new(0));
    let metrics = Arc::new(RecordingMetricsCollector {
        events: Mutex::new(Vec::new()),
    });

    let mut master = build_error_delegate_master_with_metrics(
        platform_service,
        Arc::clone(&create_endpoint_calls),
        Arc::clone(&create_consumer_calls),
        None,
        0,
        None,
        30,
        Arc::clone(&metrics) as Arc<dyn MetricsCollector>,
    );

    let (tx, mut rx) = tokio::sync::mpsc::channel(16);
    let cancel = CancellationToken::new();
    let ctx = ConsumerContext::new(tx, cancel.clone(), METRICS_TEST_ROUTE.to_string());

    master.start(ctx).await.unwrap();

    // Baseline: Active delegate at epoch 1 with one acquisition transition
    // and one started lifecycle observation.
    let first = timeout(Duration::from_secs(5), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first.exchange.input.body.as_text(), Some("ok"));
    assert!(
        await_counter_observations(&metrics, "master_leadership_transitions_total", 1).await,
        "initial acquisition should be recorded within 5s"
    );
    let baseline_started = lifecycle_events(&metrics, "started");
    let baseline_stopped = lifecycle_events(&metrics, "stopped");
    assert_eq!(
        baseline_started, 1,
        "baseline delegate should have started exactly once"
    );
    assert_eq!(baseline_stopped, 0, "baseline should have no stops");
    assert_eq!(
        create_consumer_calls.load(Ordering::SeqCst),
        1,
        "baseline delegate should be created exactly once"
    );

    // Coalesced flap across a takeover: the published epoch advances while
    // the delegate stays Active. The duplicate StartedLeading delivery
    // must drain and recreate the delegate exactly once, restamping the
    // epoch bridge at the new epoch.
    leadership.leader_epoch().store(2, Ordering::Release);
    leadership.emit(LeadershipEvent::StartedLeading).await;
    let reconciled = timeout(Duration::from_secs(5), async {
        loop {
            if create_consumer_calls.load(Ordering::SeqCst) >= 2 {
                break;
            }
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .is_ok();
    assert!(
        reconciled,
        "term bump should force re-reconciliation within 5s"
    );
    // The recreation completes only when its "started" observation is
    // recorded (the create counter bumps slightly before the lifecycle
    // emit inside reconcile_event).
    assert!(
        await_counter_observations(
            &metrics,
            "master_delegate_lifecycle_total",
            baseline_started + baseline_stopped + 2
        )
        .await,
        "stopped+started lifecycle pair should be recorded within 5s"
    );

    // Exactly one stopped+started lifecycle pair added by the bump.
    assert_eq!(
        create_consumer_calls.load(Ordering::SeqCst),
        2,
        "term bump must recreate the delegate exactly once"
    );
    assert_eq!(
        lifecycle_events(&metrics, "started") - baseline_started,
        1,
        "term bump must emit exactly one started lifecycle observation"
    );
    assert_eq!(
        lifecycle_events(&metrics, "stopped") - baseline_stopped,
        1,
        "term bump must emit exactly one stopped lifecycle observation"
    );

    // The next envelope comes from the recreated delegate through the
    // bridge restamped at the bumped epoch.
    let second = timeout(Duration::from_secs(5), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(second.exchange.input.body.as_text(), Some("ok"));
    let epoch_prop = second
        .exchange
        .properties
        .get(crate::leadership::LEADER_EPOCH_PROPERTY)
        .expect("recreated bridge must stamp x-camel-leader-epoch");
    assert_eq!(
        epoch_prop,
        &serde_json::Value::String("2".to_string()),
        "recreated bridge must carry the bumped epoch 2"
    );

    cancel.cancel();
    master.stop().await.unwrap();
}

// ── Tick-driven stale-stamp detection tests (camel-master-reconcile-
// hygiene Task 1.3) ──────────────────────────────────────────────────

#[tokio::test]
async fn tick_renews_epoch_advance_restamps_without_delivery() {
    let leadership = Arc::new(FakeLeadershipService::new(Some(
        LeadershipEvent::StartedLeading,
    )));
    let platform_service = Arc::new(FakePlatformService::new(leadership.clone()));
    let create_endpoint_calls = Arc::new(AtomicUsize::new(0));
    let create_consumer_calls = Arc::new(AtomicUsize::new(0));
    let metrics = Arc::new(RecordingMetricsCollector {
        events: Mutex::new(Vec::new()),
    });

    let mut master = build_error_delegate_master_with_metrics(
        platform_service,
        Arc::clone(&create_endpoint_calls),
        Arc::clone(&create_consumer_calls),
        None,
        0,
        None,
        30,
        Arc::clone(&metrics) as Arc<dyn MetricsCollector>,
    );

    let (tx, mut rx) = tokio::sync::mpsc::channel(16);
    let cancel = CancellationToken::new();
    let ctx = ConsumerContext::new(tx, cancel.clone(), METRICS_TEST_ROUTE.to_string());

    master.start(ctx).await.unwrap();

    // Baseline: Active delegate at epoch 1, one create, one acquisition
    // transition, one started lifecycle observation.
    let first = timeout(Duration::from_secs(5), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first.exchange.input.body.as_text(), Some("ok"));
    let first_epoch = first
        .exchange
        .properties
        .get(crate::leadership::LEADER_EPOCH_PROPERTY)
        .and_then(|v| v.as_str());
    assert_eq!(first_epoch, Some("1"), "baseline bridge stamps epoch 1");
    assert!(
        await_counter_observations(&metrics, "master_leadership_transitions_total", 1).await,
        "initial acquisition should be recorded within 5s"
    );
    let baseline_started = lifecycle_events(&metrics, "started");
    let baseline_stopped = lifecycle_events(&metrics, "stopped");
    assert_eq!(
        baseline_started, 1,
        "baseline delegate should have started exactly once"
    );
    assert_eq!(baseline_stopped, 0, "baseline should have no stops");
    assert_eq!(
        create_consumer_calls.load(Ordering::SeqCst),
        1,
        "baseline delegate should be created exactly once"
    );

    // Renewal-path epoch advance with NO watch delivery (clamp adoption
    // of an out-of-band lease term): only the published epoch moves.
    // The retry tick must detect the stale stamp and re-reconcile.
    leadership.leader_epoch().store(2, Ordering::Release);

    let reconciled = timeout(Duration::from_secs(5), async {
        loop {
            if create_consumer_calls.load(Ordering::SeqCst) >= 2 {
                break;
            }
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .is_ok();
    assert!(
        reconciled,
        "stale-stamp tick must dispatch the reconciliation within 5s"
    );
    // The recreation completes only when its stopped+started pair is
    // recorded (the create counter bumps before the lifecycle emits).
    assert!(
        await_counter_observations(
            &metrics,
            "master_delegate_lifecycle_total",
            baseline_started + baseline_stopped + 2
        )
        .await,
        "stopped+started lifecycle pair should be recorded within 5s"
    );

    // Exactly one stopped+started pair added by the tick dispatch.
    assert_eq!(
        create_consumer_calls.load(Ordering::SeqCst),
        2,
        "stale-stamp tick must recreate the delegate exactly once"
    );
    assert_eq!(
        lifecycle_events(&metrics, "started") - baseline_started,
        1,
        "tick dispatch must emit exactly one started lifecycle observation"
    );
    assert_eq!(
        lifecycle_events(&metrics, "stopped") - baseline_stopped,
        1,
        "tick dispatch must emit exactly one stopped lifecycle observation"
    );

    // The next envelope comes from the recreated delegate through the
    // bridge restamped at the advanced epoch.
    let second = timeout(Duration::from_secs(5), rx.recv())
        .await
        .unwrap()
        .unwrap();
    let epoch_prop = second
        .exchange
        .properties
        .get(crate::leadership::LEADER_EPOCH_PROPERTY)
        .expect("recreated bridge must stamp x-camel-leader-epoch");
    assert_eq!(
        epoch_prop,
        &serde_json::Value::String("2".to_string()),
        "recreated bridge must carry the advanced epoch 2"
    );

    cancel.cancel();
    master.stop().await.unwrap();
}

#[tokio::test]
async fn dead_delegate_stale_stamp_resets_budget() {
    let leadership = Arc::new(FakeLeadershipService::new(Some(
        LeadershipEvent::StartedLeading,
    )));
    let platform_service = Arc::new(FakePlatformService::new(leadership.clone()));
    let create_endpoint_calls = Arc::new(AtomicUsize::new(0));
    let create_consumer_calls = Arc::new(AtomicUsize::new(0));
    let metrics = Arc::new(RecordingMetricsCollector {
        events: Mutex::new(Vec::new()),
    });

    // One-shot exit signal: the FIRST consumer self-exits when the signal
    // fires; delegate #2 keeps running until cancelled.
    let (exit_tx, exit_rx) = tokio::sync::watch::channel(());

    // max_attempts = 1 with a healthy delegate: the initial snapshot
    // spends the whole budget. Constructed directly (not via the metrics
    // builder) because the exit-signal knob must reach the component.
    let mut master = MasterConsumer::new(
        METRICS_TEST_LOCK.to_string(),
        "errdelegate:delegate".to_string(),
        Arc::new(ErrorDelegateComponent {
            create_endpoint_calls: Arc::clone(&create_endpoint_calls),
            create_consumer_calls: Arc::clone(&create_consumer_calls),
            endpoint_error: None,
            consumer_error_after: 0,
            consumer_error: None,
            first_exit_signal: Arc::new(Mutex::new(Some(exit_rx))),
        }),
        Arc::clone(&metrics) as Arc<dyn MetricsCollector>,
        platform_service,
        Duration::from_millis(500),
        NetworkRetryPolicy {
            max_attempts: 1,
            initial_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(5),
            multiplier: 1.0,
            ..NetworkRetryPolicy::default()
        },
        Arc::new(PanicRuntimeObservability) as Arc<dyn camel_component_api::RuntimeObservability>,
    );

    let (tx, mut rx) = tokio::sync::mpsc::channel(16);
    let cancel = CancellationToken::new();
    let ctx = ConsumerContext::new(tx, cancel.clone(), METRICS_TEST_ROUTE.to_string());

    master.start(ctx).await.unwrap();

    // Baseline: the single healthy create exhausts the budget at count 1.
    let first = timeout(Duration::from_secs(5), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first.exchange.input.body.as_text(), Some("ok"));
    let first_epoch = first
        .exchange
        .properties
        .get(crate::leadership::LEADER_EPOCH_PROPERTY)
        .and_then(|v| v.as_str());
    assert_eq!(first_epoch, Some("1"), "baseline bridge stamps epoch 1");
    assert_eq!(
        create_consumer_calls.load(Ordering::SeqCst),
        1,
        "budget must be exhausted by the single healthy create"
    );

    // Ordering matters (design §1): bump the published epoch BEFORE the
    // delegate dies, so the tick that finds the dead handle takes the
    // stale-stamp branch (budget reset) instead of the finished-handle
    // teardown (which would leave the stale exhausted budget in force
    // and stop the consumer).
    leadership.leader_epoch().store(2, Ordering::Release);
    let _ = exit_tx.send(());

    let recreated = timeout(Duration::from_secs(5), async {
        loop {
            if create_consumer_calls.load(Ordering::SeqCst) >= 2 {
                break;
            }
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .is_ok();
    assert!(
        recreated,
        "stale-stamp dispatch must reset the exhausted budget and recreate within 5s"
    );
    assert!(
        !master
            .leadership_task
            .as_ref()
            .is_some_and(|h| h.is_finished()),
        "the stale-stamp reset must keep the leadership task alive"
    );

    // The next envelope comes from delegate #2, restamped at the new epoch.
    let second = timeout(Duration::from_secs(5), rx.recv())
        .await
        .unwrap()
        .unwrap();
    let epoch_prop = second
        .exchange
        .properties
        .get(crate::leadership::LEADER_EPOCH_PROPERTY)
        .expect("recreated bridge must stamp x-camel-leader-epoch");
    assert_eq!(
        epoch_prop,
        &serde_json::Value::String("2".to_string()),
        "recreated bridge must carry the advanced epoch 2"
    );

    cancel.cancel();
    master.stop().await.unwrap();
}

#[tokio::test]
async fn synthetic_retry_does_not_reemit_transition() {
    let leadership = Arc::new(FakeLeadershipService::new(Some(
        LeadershipEvent::StartedLeading,
    )));
    let platform_service = Arc::new(FakePlatformService::new(leadership));
    let create_endpoint_calls = Arc::new(AtomicUsize::new(0));
    let create_consumer_calls = Arc::new(AtomicUsize::new(0));
    let metrics = Arc::new(RecordingMetricsCollector {
        events: Mutex::new(Vec::new()),
    });

    // Three transient consumer failures then success: the retry tick
    // re-dispatches synthetic StartedLeading three times, and none of
    // those re-dispatches may re-emit the acquired transition.
    let mut master = build_error_delegate_master_with_metrics(
        platform_service,
        Arc::clone(&create_endpoint_calls),
        Arc::clone(&create_consumer_calls),
        None,
        3,
        Some(transient_io_error()),
        4,
        Arc::clone(&metrics) as Arc<dyn MetricsCollector>,
    );

    let (tx, mut rx) = tokio::sync::mpsc::channel(16);
    let cancel = CancellationToken::new();
    let ctx = ConsumerContext::new(tx, cancel.clone(), METRICS_TEST_ROUTE.to_string());

    master.start(ctx).await.unwrap();

    // Run to success, then wait until every synthetic re-dispatch has
    // been processed (3 create_error + 1 started lifecycle observation).
    let first = timeout(Duration::from_secs(5), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first.exchange.input.body.as_text(), Some("ok"));
    assert!(
        await_counter_observations(&metrics, "master_delegate_lifecycle_total", 4).await,
        "three create_error observations plus started confirm all re-dispatches were processed"
    );

    let transitions = metrics.counters_named("master_leadership_transitions_total");
    assert_eq!(
        transitions.len(),
        1,
        "synthetic retry re-dispatches must not re-emit the acquired transition"
    );
    assert_eq!(
        transitions[0],
        (1.0, expected_transition_labels("acquired"))
    );

    cancel.cancel();
    master.stop().await.unwrap();
}

#[tokio::test]
async fn transition_counted_despite_permanent_endpoint_failure() {
    let leadership = Arc::new(FakeLeadershipService::new(Some(
        LeadershipEvent::StartedLeading,
    )));
    let platform_service = Arc::new(FakePlatformService::new(leadership));
    let create_endpoint_calls = Arc::new(AtomicUsize::new(0));
    let create_consumer_calls = Arc::new(AtomicUsize::new(0));
    let metrics = Arc::new(RecordingMetricsCollector {
        events: Mutex::new(Vec::new()),
    });

    let mut master = build_error_delegate_master_with_metrics(
        platform_service,
        Arc::clone(&create_endpoint_calls),
        Arc::clone(&create_consumer_calls),
        Some(CamelError::EndpointCreationFailed("permanent".to_string())),
        0,
        None,
        30,
        Arc::clone(&metrics) as Arc<dyn MetricsCollector>,
    );

    let (tx, _rx) = tokio::sync::mpsc::channel(16);
    let cancel = CancellationToken::new();
    let ctx = ConsumerContext::new(tx, cancel.clone(), METRICS_TEST_ROUTE.to_string());

    master.start(ctx).await.unwrap();

    // The permanent endpoint error propagates and terminates the task,
    // but the acquired transition must already have been recorded.
    assert!(
        await_leadership_task_exit(&master).await,
        "permanent endpoint failure must terminate the leadership task within 5s"
    );

    let transitions = metrics.counters_named("master_leadership_transitions_total");
    assert_eq!(
        transitions.len(),
        1,
        "acquired transition must be recorded exactly once before the failure"
    );
    assert_eq!(
        transitions[0],
        (1.0, expected_transition_labels("acquired"))
    );

    cancel.cancel();
    let _ = master.stop().await;
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
    let ctx = ConsumerContext::new(tx, cancel.clone(), "master-test-route".to_string());

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
    let ctx = ConsumerContext::new(tx, cancel.clone(), "master-test-route".to_string());

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
    let ctx = ConsumerContext::new(tx, cancel.clone(), "master-test-route".to_string());

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
