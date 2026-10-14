//! rc-w02ns: Explicit-mode delegates (start() returns after spawning a
//! background task; supervision via `background_task_handle()`) must stay
//! running until leadership is lost. Shared mocks live in tests.rs.

use super::*;
use tokio::task::JoinHandle;

struct ExplicitDelegateComponent {
    start_calls: Arc<AtomicUsize>,
    stop_calls: Arc<AtomicUsize>,
    /// When `Some`, the background task fails with this message after its
    /// first heartbeat.
    fail_with: Option<&'static str>,
}

impl Component for ExplicitDelegateComponent {
    fn scheme(&self) -> &str {
        "explicit"
    }

    fn create_endpoint(
        &self,
        _uri: &str,
        _ctx: &dyn ComponentContext,
    ) -> Result<Box<dyn Endpoint>, CamelError> {
        Ok(Box::new(ExplicitDelegateEndpoint {
            start_calls: Arc::clone(&self.start_calls),
            stop_calls: Arc::clone(&self.stop_calls),
            fail_with: self.fail_with,
        }))
    }
}

struct ExplicitDelegateEndpoint {
    start_calls: Arc<AtomicUsize>,
    stop_calls: Arc<AtomicUsize>,
    fail_with: Option<&'static str>,
}

impl Endpoint for ExplicitDelegateEndpoint {
    fn uri(&self) -> &str {
        "explicit:delegate"
    }

    fn create_consumer(
        &self,
        _rt: Arc<dyn camel_component_api::RuntimeObservability>,
    ) -> Result<Box<dyn Consumer>, CamelError> {
        Ok(Box::new(ExplicitDelegateConsumer {
            start_calls: Arc::clone(&self.start_calls),
            stop_calls: Arc::clone(&self.stop_calls),
            fail_with: self.fail_with,
            own_cancel: CancellationToken::new(),
            handle: None,
        }))
    }

    fn create_producer(
        &self,
        _rt: Arc<dyn camel_component_api::RuntimeObservability>,
        _ctx: &ProducerContext,
    ) -> Result<BoxProcessor, CamelError> {
        Err(CamelError::EndpointCreationFailed("not used".to_string()))
    }
}

struct ExplicitDelegateConsumer {
    start_calls: Arc<AtomicUsize>,
    stop_calls: Arc<AtomicUsize>,
    fail_with: Option<&'static str>,
    own_cancel: CancellationToken,
    handle: Option<JoinHandle<Result<(), CamelError>>>,
}

#[async_trait]
impl Consumer for ExplicitDelegateConsumer {
    async fn start(&mut self, context: ConsumerContext) -> Result<(), CamelError> {
        self.start_calls.fetch_add(1, Ordering::SeqCst);
        let own_cancel = self.own_cancel.clone();
        let fail_with = self.fail_with;
        // Returns at once, like camel-jms / camel-kafka: the work lives in a
        // background task whose handle is exposed for supervision.
        self.handle = Some(tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = context.cancelled() => return Ok(()),
                    _ = own_cancel.cancelled() => return Ok(()),
                    _ = sleep(Duration::from_millis(10)) => {
                        context
                            .send(Exchange::new(Message::new("heartbeat")))
                            .await?;
                        if let Some(msg) = fail_with {
                            return Err(CamelError::ProcessorError(msg.to_string()));
                        }
                    }
                }
            }
        }));
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), CamelError> {
        self.stop_calls.fetch_add(1, Ordering::SeqCst);
        self.own_cancel.cancel();
        Ok(())
    }

    fn background_task_handle(&mut self) -> Option<JoinHandle<Result<(), CamelError>>> {
        self.handle.take()
    }
}

fn build_explicit_master(
    platform_service: Arc<dyn PlatformService>,
    start_calls: Arc<AtomicUsize>,
    stop_calls: Arc<AtomicUsize>,
    fail_with: Option<&'static str>,
) -> MasterConsumer {
    MasterConsumer::new(
        "lock-explicit".to_string(),
        "explicit:delegate".to_string(),
        Arc::new(ExplicitDelegateComponent {
            start_calls,
            stop_calls,
            fail_with,
        }),
        Arc::new(NoOpMetrics),
        platform_service,
        Duration::from_millis(500),
        NetworkRetryPolicy {
            max_attempts: 30,
            ..NetworkRetryPolicy::default()
        },
        Arc::new(PanicRuntimeObservability) as Arc<dyn camel_component_api::RuntimeObservability>,
    )
}

#[tokio::test]
async fn explicit_delegate_stays_running_while_leader() {
    let leadership = Arc::new(FakeLeadershipService::new(None));
    let platform_service = Arc::new(FakePlatformService::new(leadership.clone()));
    let start_calls = Arc::new(AtomicUsize::new(0));
    let stop_calls = Arc::new(AtomicUsize::new(0));
    let mut master = build_explicit_master(
        platform_service,
        Arc::clone(&start_calls),
        Arc::clone(&stop_calls),
        None,
    );

    let (tx, mut rx) = tokio::sync::mpsc::channel(64);
    let cancel = CancellationToken::new();
    let ctx = ConsumerContext::new(tx, cancel.clone(), "master-test-route".to_string());
    master.start(ctx).await.unwrap();
    leadership.emit(LeadershipEvent::StartedLeading).await;

    // Heartbeats keep flowing only while the delegate's background task is
    // alive; the pre-fix code called stop() right after start() returned.
    for _ in 0..10 {
        timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("delegate stopped delivering: stopped right after start")
            .expect("channel closed");
    }
    assert_eq!(start_calls.load(Ordering::SeqCst), 1, "no restart churn");
    assert_eq!(stop_calls.load(Ordering::SeqCst), 0, "no premature stop");

    // Leadership loss stops the delegate exactly once.
    leadership.emit(LeadershipEvent::StoppedLeading).await;
    timeout(Duration::from_secs(2), async {
        while stop_calls.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("delegate stop() must run on leadership loss");
    assert_eq!(stop_calls.load(Ordering::SeqCst), 1);

    cancel.cancel();
    master.stop().await.unwrap();
}

#[tokio::test]
async fn explicit_delegate_background_failure_propagates() {
    let leadership = Arc::new(FakeLeadershipService::new(None));
    let platform_service = Arc::new(FakePlatformService::new(leadership.clone()));
    let start_calls = Arc::new(AtomicUsize::new(0));
    let stop_calls = Arc::new(AtomicUsize::new(0));
    let mut master = build_explicit_master(
        platform_service,
        Arc::clone(&start_calls),
        Arc::clone(&stop_calls),
        Some("bg boom"),
    );

    let (tx, _rx) = tokio::sync::mpsc::channel(64);
    let cancel = CancellationToken::new();
    let ctx = ConsumerContext::new(tx, cancel.clone(), "master-test-route".to_string());
    master.start(ctx).await.unwrap();
    leadership.emit(LeadershipEvent::StartedLeading).await;

    // A bg-task error while still leader must surface through the delegate
    // task so supervision fails fast (same contract as a blocking start()).
    timeout(Duration::from_secs(5), async {
        while !master
            .leadership_task
            .as_ref()
            .is_some_and(tokio::task::JoinHandle::is_finished)
        {
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("bg failure must terminate the master task");
    assert_eq!(start_calls.load(Ordering::SeqCst), 1);

    let err = master.stop().await.expect_err("bg error must propagate");
    assert!(err.to_string().contains("bg boom"), "got: {err}");
    cancel.cancel();
}

/// rc-uszgx: the route controller aborts the handle from
/// `background_task_handle()` the moment the context is cancelled, then calls
/// `stop()`. The delegate's `stop()` must still run (graceful tail), exactly
/// once, and the lease must be stepped down.
#[tokio::test]
async fn controller_style_abort_does_not_cut_graceful_shutdown() {
    let leadership = Arc::new(FakeLeadershipService::new(None));
    let platform_service = Arc::new(FakePlatformService::new(leadership.clone()));
    let start_calls = Arc::new(AtomicUsize::new(0));
    let stop_calls = Arc::new(AtomicUsize::new(0));
    let mut master = build_explicit_master(
        platform_service,
        Arc::clone(&start_calls),
        Arc::clone(&stop_calls),
        None,
    );

    let (tx, mut rx) = tokio::sync::mpsc::channel(64);
    let cancel = CancellationToken::new();
    let ctx = ConsumerContext::new(tx, cancel.clone(), "master-test-route".to_string());
    master.start(ctx).await.unwrap();
    leadership.emit(LeadershipEvent::StartedLeading).await;
    timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("delegate delivers")
        .expect("channel open");

    // What the route controller does on shutdown.
    let bg = master.background_task_handle().expect("proxy handle");
    cancel.cancel();
    bg.abort();
    master.stop().await.unwrap();

    assert_eq!(stop_calls.load(Ordering::SeqCst), 1, "delegate stop ran");
    assert!(
        leadership.stepped_down(),
        "lease must be stepped down on graceful shutdown"
    );
}
