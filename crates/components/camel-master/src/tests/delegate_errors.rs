//! rc-i1z delegate error classification tests (permanent vs transient). Split from tests.rs by concern (bd rc-ubk1v).
//! Shared mocks/helpers live in the tests module root (tests.rs).

use super::*;

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

    // A permanent error must terminate the task in milliseconds via
    // fail-fast classification, NOT via retry-budget exhaustion.
    tokio::time::timeout(Duration::from_millis(750), async {
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
    .expect("master must terminate without retry within 750ms");

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

// ── rc-77wln: delegate task failures are classified ─────────────────

/// Start a master over `build_start_error_master` with a fresh leader fake.
fn start_error_fixture(
    fault: ConsumerFault,
    max_attempts: u32,
) -> (
    MasterConsumer,
    Arc<FakeLeadershipService>,
    Arc<RecordingMetricsCollector>,
    Arc<AtomicUsize>,
    Arc<AtomicUsize>,
) {
    let leadership = Arc::new(FakeLeadershipService::new(Some(
        LeadershipEvent::StartedLeading,
    )));
    let leadership_dyn: Arc<dyn LeadershipService> = leadership.clone();
    let platform_service = Arc::new(FakePlatformService::new(leadership_dyn));
    let create_endpoint_calls = Arc::new(AtomicUsize::new(0));
    let create_consumer_calls = Arc::new(AtomicUsize::new(0));
    let metrics = Arc::new(RecordingMetricsCollector {
        events: Mutex::new(Vec::new()),
    });
    let master = build_start_error_master(
        platform_service,
        Arc::clone(&create_endpoint_calls),
        Arc::clone(&create_consumer_calls),
        Some(fault),
        max_attempts,
        Arc::clone(&metrics) as Arc<dyn MetricsCollector>,
    );
    (
        master,
        leadership,
        metrics,
        create_endpoint_calls,
        create_consumer_calls,
    )
}

#[tokio::test]
async fn transient_delegate_start_error_is_retried() {
    let (mut master, _leadership, metrics, endpoint_calls, consumer_calls) =
        start_error_fixture(ConsumerFault::StartError(transient_io_error()), 0);
    let (tx, mut rx) = tokio::sync::mpsc::channel(16);
    let cancel = CancellationToken::new();
    let ctx = ConsumerContext::new(tx, cancel.clone(), METRICS_TEST_ROUTE.to_string());

    master.start(ctx).await.unwrap();

    // The second delegate is the first one that sends.
    let first = timeout(Duration::from_secs(5), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first.exchange.input.body.as_text(), Some("ok"));
    assert_eq!(endpoint_calls.load(Ordering::SeqCst), 2);
    assert_eq!(consumer_calls.load(Ordering::SeqCst), 2);
    assert_eq!(start_error_events(&metrics, "transient"), 1);
    assert_eq!(lifecycle_events(&metrics, "started"), 2);
    assert_eq!(lifecycle_events(&metrics, "stopped"), 1);
    assert!(!master.leadership_task.as_ref().unwrap().is_finished());

    master.stop().await.unwrap();
}

#[tokio::test]
async fn permanent_delegate_start_error_ends_task_with_error() {
    let (mut master, leadership, metrics, _endpoint_calls, _consumer_calls) = start_error_fixture(
        ConsumerFault::StartError(CamelError::Config("boom".into())),
        0,
    );
    let (tx, _rx) = tokio::sync::mpsc::channel(16);
    let cancel = CancellationToken::new();
    let ctx = ConsumerContext::new(tx, cancel.clone(), METRICS_TEST_ROUTE.to_string());

    master.start(ctx).await.unwrap();

    assert!(await_leadership_task_exit(&master).await);
    // `step_down` awaits the fake's termination signal, which is sent after
    // the flag is set, so the flag is deterministic once the task finished.
    assert!(leadership.stepped_down());
    assert_eq!(start_error_events(&metrics, "permanent"), 1);
    assert_eq!(start_error_events(&metrics, "transient"), 0);
    let err = master.stop().await.unwrap_err();
    assert!(err.to_string().contains("boom"), "unexpected error: {err}");
}

#[tokio::test]
async fn exhausted_budget_returns_last_transient_error() {
    let (mut master, _leadership, metrics, endpoint_calls, _consumer_calls) =
        start_error_fixture(ConsumerFault::StartError(transient_io_error()), 1);
    let (tx, _rx) = tokio::sync::mpsc::channel(16);
    let cancel = CancellationToken::new();
    let ctx = ConsumerContext::new(tx, cancel.clone(), METRICS_TEST_ROUTE.to_string());

    master.start(ctx).await.unwrap();

    assert!(await_leadership_task_exit(&master).await);
    assert_eq!(endpoint_calls.load(Ordering::SeqCst), 1);
    assert_eq!(start_error_events(&metrics, "transient"), 1);
    assert_eq!(lifecycle_events(&metrics, "started"), 1);
    let err = master.stop().await.unwrap_err();
    assert!(err.to_string().contains("boom"), "unexpected error: {err}");
}

#[tokio::test]
async fn delegate_error_during_shutdown_is_not_classified() {
    let (mut master, _leadership, metrics, endpoint_calls, _consumer_calls) =
        start_error_fixture(ConsumerFault::ErrorOnCancel(transient_io_error()), 0);
    let (tx, mut rx) = tokio::sync::mpsc::channel(16);
    let cancel = CancellationToken::new();
    let ctx = ConsumerContext::new(tx, cancel.clone(), METRICS_TEST_ROUTE.to_string());

    master.start(ctx).await.unwrap();
    let first = timeout(Duration::from_secs(5), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first.exchange.input.body.as_text(), Some("ok"));

    master.stop().await.unwrap();

    assert_eq!(start_error_events(&metrics, "transient"), 0);
    assert_eq!(start_error_events(&metrics, "permanent"), 0);
    assert_eq!(endpoint_calls.load(Ordering::SeqCst), 1);
}
