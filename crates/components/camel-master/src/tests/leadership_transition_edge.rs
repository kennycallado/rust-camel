//! MST-001 leadership transition edge tests (master-metrics-wiring Task 1.3). Split from tests.rs by concern (bd rc-ubk1v).
//! Shared mocks/helpers live in the tests module root (tests.rs).

use super::*;

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
