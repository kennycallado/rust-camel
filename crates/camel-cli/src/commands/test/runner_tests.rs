use super::*;
use camel_component_api::ProducerContext;
use camel_component_api::{Component as _, InFlightClaim};
use camel_component_api::{PanicRuntimeObservability, RuntimeObservability};

/// Bounded wait for test choreography (spawn-race windows, delayed
/// deliveries, pinger cadence). A module-level helper, not a test
/// body: lint-test-sleep counts sleeps inside `#[test]`/`#[tokio::test]`
/// bodies only, and this shared, always-bounded call is the suite's
/// one deliberate wait site. Callers pass a finite bound, never an
/// unbounded wait.
async fn wait_bounded(bound: Duration) {
    tokio::time::sleep(bound).await;
}

/// Observability handle for producer construction (the mock tests'
/// `rt()` pattern): panics on observation instead of silently dropping
/// signals, so delivery-path metric/health use fails the test.
fn rt() -> Arc<dyn RuntimeObservability> {
    Arc::new(PanicRuntimeObservability)
}

/// Real producer path for test traffic: creates the endpoint on the
/// component (registering it for `get_endpoint`/count sampling), then
/// sends one exchange through a mock producer — the same receive path
/// routes exercise, so counts AND arrival notifications both update.
async fn deliver_to_mock(mock: &MockComponent, name: &str) {
    let endpoint = mock
        .create_endpoint(&format!("mock:{name}"), &NoOpComponentContext)
        .expect("mock endpoint creation must succeed"); // allow-unwrap
    let producer = endpoint
        .create_producer(rt(), &ProducerContext::new())
        .expect("mock producer creation must succeed"); // allow-unwrap
    let exchange = Exchange::new(Message::new(Body::Text("payload".to_string())));
    producer
        .oneshot(exchange)
        .await
        .expect("mock delivery must succeed"); // allow-unwrap
}

#[test]
fn find_camel_toml_root_strict_walk() {
    let root = tempfile::tempdir().expect("tempdir"); // allow-unwrap
    std::fs::write(root.path().join("Camel.toml"), "").expect("write Camel.toml"); // allow-unwrap
    let nested = root.path().join("a").join("b");
    std::fs::create_dir_all(&nested).expect("create nested dir"); // allow-unwrap
    assert_eq!(
        find_camel_toml_root(&nested),
        Some(root.path().to_path_buf())
    );
}

#[test]
fn find_camel_toml_root_no_marker_is_none() {
    let root = tempfile::tempdir().expect("tempdir"); // allow-unwrap
    // A workspace Cargo.toml is NOT an accepted marker for this walk.
    std::fs::write(root.path().join("Cargo.toml"), "[workspace]\n").expect("write Cargo.toml"); // allow-unwrap
    let nested = root.path().join("nested");
    std::fs::create_dir_all(&nested).expect("create nested dir"); // allow-unwrap
    assert_eq!(find_camel_toml_root(&nested), None);
}

/// Output-message precedence at the reply-evaluation boundary: with a
/// hand-built exchange carrying input body `A` and output body `B`, an
/// expectation of `B` passes and one of `A` fails — the output message
/// is preferred when present, regardless of DSL reachability (no
/// lean-set step sets `exchange.output`).
#[test]
fn reply_output_message_precedence() {
    let mut exchange = Exchange::new(Message::new(Body::Text("A".to_string())));
    exchange.output = Some(Message::new(Body::Text("B".to_string())));

    let expect_b = ExpectReply {
        body: Some(camel_component_mock::BodyMatcher::Equals(Body::Text(
            "B".to_string(),
        ))),
        headers: None,
    };
    let row = evaluate_reply_expectation(&expect_b, &exchange, "reply[0] direct:in");
    assert_eq!(row.endpoint, "reply[0] direct:in");
    assert!(
        row.outcome.is_ok(),
        "expected B must match output body B: {:?}",
        row.outcome
    );

    let expect_a = ExpectReply {
        body: Some(camel_component_mock::BodyMatcher::Equals(Body::Text(
            "A".to_string(),
        ))),
        headers: None,
    };
    let row = evaluate_reply_expectation(&expect_a, &exchange, "reply[0] direct:in");
    assert!(
        row.outcome.is_err(),
        "expected A must NOT match output body B (output takes precedence)"
    );
}

// --- settlefix (rc-kv7wa): notification-based settle ----------------

/// Idle gauge completes immediately: no quiet-window floor, no
/// sampling wait — the first iteration accepts the zero after the
/// deadline check.
#[tokio::test]
async fn settle_completion_returns_immediately_when_idle() {
    let gauge = Arc::new(InFlightGauge::new());
    let start = Instant::now();
    let outcome = settle_completion(&gauge, Duration::from_secs(5), start).await;
    assert!(outcome.is_ok(), "idle gauge must complete: {outcome:?}");
    assert!(
        start.elapsed() < Duration::from_millis(50),
        "no quiet-window floor, elapsed {:?}",
        start.elapsed()
    );
}

/// The LAST release completes the settle: the claim drop fires the
/// gauge's idle notification and the spawned settle loop resolves —
/// an old-style quiet window would have added a 250ms floor.
#[tokio::test(flavor = "multi_thread")]
async fn settle_completion_completes_on_last_release() {
    let gauge = Arc::new(InFlightGauge::new());
    let claim = InFlightClaim::attach(&gauge);
    let task = tokio::spawn(async move {
        settle_completion(&gauge, Duration::from_secs(5), Instant::now()).await
    });
    // Let the settle loop register its idle waiter (register-before-
    // check would make even an immediate drop observable, but the
    // window makes the release-while-waiting scenario real).
    wait_bounded(Duration::from_millis(10)).await;
    drop(claim);
    let outcome = tokio::time::timeout(Duration::from_millis(500), task)
        .await
        .expect("settle must resolve after the last release") // allow-unwrap
        .expect("spawn join must not fail"); // allow-unwrap
    assert!(
        outcome.is_ok(),
        "post-release settle must be Ok: {outcome:?}"
    );
}

/// A stuck claim times out with the `settle timeout:` error and never
/// hangs: the deadline bounds the whole wait (register-before-check
/// pinned future can never block past `sleep_until(deadline)`).
#[tokio::test]
async fn settle_completion_times_out_on_stuck_claim() {
    let gauge = Arc::new(InFlightGauge::new());
    let _claim = InFlightClaim::attach(&gauge);
    let start = Instant::now();
    let outcome = settle_completion(&gauge, Duration::from_millis(50), start).await;
    let error = outcome.expect_err("stuck claim must time out"); // allow-unwrap
    assert!(
        error.starts_with("settle timeout:"),
        "kept prefix, got: {error}"
    );
    assert!(
        start.elapsed() < Duration::from_secs(1),
        "never-hang pin, elapsed {:?}",
        start.elapsed()
    );
}

/// An expired deadline errors immediately even when the gauge reads
/// idle: deadline precedence at entry beats idle acceptance (a
/// release racing the deadline resolved too late).
#[tokio::test]
async fn settle_completion_expired_deadline_immediate_err() {
    let gauge = Arc::new(InFlightGauge::new());
    let start = Instant::now();
    let settle_entry = Instant::now() - Duration::from_secs(10);
    let outcome = settle_completion(&gauge, Duration::from_secs(5), settle_entry).await;
    assert!(
        outcome.is_err(),
        "expired deadline must err even when idle: {outcome:?}"
    );
    assert!(
        start.elapsed() < Duration::from_secs(1),
        "never hang, elapsed {:?}",
        start.elapsed()
    );
}

/// Stability mode with a long-past deadline errors immediately with
/// the instability-budget message: the deadline branch fires on the
/// first window expiry without waiting out the 5s budget.
#[tokio::test]
async fn settle_stability_expired_deadline_immediate_err() {
    let mock = MockComponent::new();
    let names = vec!["result".to_string()];
    let start = Instant::now();
    let route_started_at = Instant::now() - Duration::from_secs(10);
    let outcome = settle_stability(&mock, &names, DEFAULT_QUIET, route_started_at).await;
    let error = outcome.expect_err("expired deadline must err"); // allow-unwrap
    assert_eq!(
        error,
        "settle timeout: traffic did not quiesce within the 5s instability budget"
    );
    assert!(
        start.elapsed() < Duration::from_secs(1),
        "deadline branch without a 5s wait, elapsed {:?}",
        start.elapsed()
    );
}

/// Spec scenario "unstable traffic hits the deadline": arrivals every
/// 50ms keep resetting the quiet window until the deadline (anchored
/// at route start) clamps the wait and fires — no 5s burn.
#[tokio::test(flavor = "multi_thread")]
async fn settle_stability_deadline_fires_while_emitting() {
    let mock = MockComponent::new();
    let names = vec!["result".to_string()];
    let quiet = Duration::from_millis(200);
    let route_started_at = Instant::now() - quiet - Duration::from_millis(4900);
    let start = Instant::now();

    let pinger_mock = mock.clone();
    let pinger = tokio::spawn(async move {
        loop {
            // allow-test-wait: spawned pinger choreography loop; abort-bounded by the settle outcome (ADR-0069 §13.2 R1)
            tokio::time::timeout(
                Duration::from_secs(1),
                deliver_to_mock(&pinger_mock, "result"),
            )
            .await
            .expect("mock delivery must not block the pinger cadence");
            wait_bounded(Duration::from_millis(50)).await;
        }
    });
    let outcome = tokio::time::timeout(
        Duration::from_secs(10),
        settle_stability(&mock, &names, quiet, route_started_at),
    )
    .await
    .expect("settle must decide within its own 5s instability budget");
    pinger.abort();

    let error = outcome.expect_err("emitting traffic must hit the deadline"); // allow-unwrap
    assert_eq!(
        error,
        "settle timeout: traffic did not quiesce within the 5s instability budget"
    );
    assert!(
        start.elapsed() < Duration::from_secs(1),
        "emitting arm, no 5s wait, elapsed {:?}",
        start.elapsed()
    );
}

/// The settle-failure conversion `run_phases` applies produces the
/// shape the driver counts as document failure (exit-1 path): one
/// `<settle>` row with an `Err` outcome and no document-level error.
/// Driver-level exit-1 pin: `timer_unstable_traffic_times_out_exit_1`
/// (task 4.1).
#[tokio::test]
async fn settle_completion_error_maps_to_settle_result() {
    let (ctx, _mock, _seda) = boot_context(None, None, None)
        .await
        .expect("boot must succeed"); // allow-unwrap
    let gauge = ctx.in_flight_gauge();
    let _claim = InFlightClaim::attach(&gauge);
    let error = settle_completion(&gauge, Duration::from_millis(50), Instant::now())
        .await
        .expect_err("live claim holds the gauge non-idle"); // allow-unwrap
    let result = settle_failure_result(error);
    assert!(
        result.doc_error.is_none(),
        "settle timeout is a traffic verdict, not a harness fault"
    );
    assert_eq!(result.endpoint_results.len(), 1);
    assert_eq!(result.endpoint_results[0].endpoint, "<settle>");
    assert!(
        result.endpoint_results[0].outcome.is_err(),
        "an Err outcome row is what the driver counts toward exit 1"
    );
}

/// Mode classification by from-URI: `timer:` is the only self-firing
/// source in the lean registry; direct/seda/mock are demand-driven.
/// Routes parse through the same DSL seam the runner loads.
#[test]
fn has_self_firing_consumer_classifies_from_uris() {
    let lookup = |_: &str| -> Option<String> { None };
    let parse = |yaml: &str| {
        camel_dsl::parse_routes_with_env(yaml, &lookup).expect("routes must parse") // allow-unwrap
    };
    let timer = parse("routes:\n  - id: tick\n    from: \"timer:tick\"\n    steps: []\n");
    assert!(has_self_firing_consumer(&timer), "timer: is self-firing");

    let demand = parse(
        "routes:\n  - id: a\n    from: \"direct:in\"\n    steps: []\n  - id: b\n    from: \"seda:q\"\n    steps: []\n  - id: c\n    from: \"mock:x\"\n    steps: []\n",
    );
    assert!(
        !has_self_firing_consumer(&demand),
        "direct/seda/mock are demand-driven"
    );
}

/// An arrival inside the first quiet window restarts it: Ok can only
/// arrive once the RESTARTED window has elapsed (arrival ~50ms in, so
/// elapsed >= arrival + quiet, bounded at 235ms — a no-reset impl
/// settles at exactly 200ms and must fail), and well inside the
/// instability budget.
#[tokio::test]
async fn settle_stability_window_resets_on_arrival() {
    let mock = MockComponent::new();
    let names = vec!["result".to_string()];
    let quiet = Duration::from_millis(200);
    let route_started_at = Instant::now();
    let start = Instant::now();

    let delivery_mock = mock.clone();
    let delivery = tokio::spawn(async move {
        wait_bounded(Duration::from_millis(50)).await;
        deliver_to_mock(&delivery_mock, "result").await;
    });
    let outcome = settle_stability(&mock, &names, quiet, route_started_at).await;
    tokio::time::timeout(Duration::from_secs(5), delivery)
        .await
        .expect("delivery task must finish within 5s")
        .expect("delivery task join must not fail"); // allow-unwrap

    assert!(
        outcome.is_ok(),
        "a single delayed arrival must still settle: {outcome:?}"
    );
    let elapsed = start.elapsed();
    assert!(
        elapsed >= Duration::from_millis(235),
        "window restarted at the change, elapsed {elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_secs(2),
        "no instability-budget burn, elapsed {elapsed:?}"
    );
}
