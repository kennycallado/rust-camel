//! tick-driven stale-stamp detection tests (camel-master-reconcile-hygiene Task 1.3). Split from tests.rs by concern (bd rc-ubk1v).
//! Shared mocks/helpers live in the tests module root (tests.rs).

use super::*;

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
