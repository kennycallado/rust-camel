//! MST-001 metrics wiring tests (master-metrics-wiring Task 1.2). Split from tests.rs by concern (bd rc-ubk1v).
//! Shared mocks/helpers live in the tests module root (tests.rs).

use super::*;

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
