//! rc-02dx leadership state gauge tests. Split from tests.rs by concern (bd rc-ubk1v).
//! Shared mocks/helpers live in the tests module root (tests.rs).

use super::*;

// ── rc-02dx leadership state gauge tests ────────────────────────────

/// The `camel_master_is_leader` gauge reads 1 for the lock while
/// leadership is held. Steady-state readability is the gauge's purpose:
/// unlike the transition counters, it must NOT read 0 while steady.
#[tokio::test]
async fn is_leader_gauge_is_one_while_leadership_held() {
    // ARRANGE: leadership acquired for a named lock (same harness as
    // transition_acquired_on_initial_snapshot).
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

    let first = timeout(Duration::from_secs(5), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first.exchange.input.body.as_text(), Some("ok"));
    assert!(
        await_counter_observations(&metrics, "master_leadership_transitions_total", 1).await,
        "initial-snapshot acquisition should be recorded within 5s"
    );

    // ACT/ASSERT: the gauge reads 1 for the lock while leadership is held.
    assert!(
        await_leadership_gauge(&metrics, true).await,
        "leadership gauge should read 1 within 5s of the acquire edge"
    );
    assert_eq!(
        metrics.leadership_gauge(METRICS_TEST_LOCK),
        Some(1.0),
        "gauge must read 1 for the held lock"
    );

    cancel.cancel();
    master.stop().await.unwrap();
}

/// The `camel_master_is_leader` gauge reads 0 for the lock after
/// leadership is lost.
#[tokio::test]
async fn is_leader_gauge_is_zero_after_leadership_lost() {
    // ARRANGE: acquired, then lost (same harness as
    // transition_lost_on_leading_edge).
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
    assert!(
        await_counter_observations(&metrics, "master_leadership_transitions_total", 2).await,
        "lost transition should be recorded within 5s of the StoppedLeading delivery"
    );

    // ACT/ASSERT: the gauge reads 0 after the lose edge.
    assert!(
        await_leadership_gauge(&metrics, false).await,
        "leadership gauge should read 0 within 5s of the lose edge"
    );
    assert_eq!(
        metrics.leadership_gauge(METRICS_TEST_LOCK),
        Some(0.0),
        "gauge must read 0 after leadership is lost"
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
