//! exact acquisition budget tests (camel-master-reconcile-hygiene Task 1.2). Split from tests.rs by concern (bd rc-ubk1v).
//! Shared mocks/helpers live in the tests module root (tests.rs).

use super::*;

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
