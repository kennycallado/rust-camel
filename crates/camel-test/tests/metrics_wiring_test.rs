//! End-to-end metrics wiring tests (metrics-handle-late-binding T1.4).
//!
//! Exercises the one shared late-bound metrics handle through the three
//! service modes a context can be in:
//!
//! 1. **prom-only** — `[observability.prometheus]` enabled: one exchange
//!    through a failing route must surface BOTH the pipeline families
//!    (`camel_exchanges_total`, from the tracer adapter's per-exchange
//!    recording) AND the component error family (`camel_errors_total`,
//!    from the direct consumer's b-prime increment) in the scraped body.
//!    This is the rc-685y end-to-end: before the effective-tracer-config
//!    fix, prom-only mode left the pipeline disabled and the scrape showed
//!    no exchange families at all.
//! 2. **otel stand-in** — `[observability.otel]` enabled (which forces the
//!    tracer pipeline on) but no real exporter: a `RecordingLifecycle`
//!    registered post-build mirrors how the real OtelService registers
//!    (inside `configure_context`, after the context is built).
//! 3. **both-registered composition** — prometheus via `configure_context`
//!    plus the recording lifecycle after it: registration composes, so both
//!    collectors observe the same exchange.
//! 4. **late registration** — no observability at all: routes added first,
//!    then the recording lifecycle registered on the BUILT context. With
//!    the pipeline disabled, the observable emission is the component path
//!    (the direct consumer's error increment through the runtime handle).
//!
//! Harness idioms mirror `otel_direct_hop_regression.rs` (route building,
//! direct-producer drive with startup-race retries, startup-wait polling)
//! and `camel-function/tests/protocol.rs` (poll-GET against a locally
//! bound server). The prometheus port is pre-allocated with a
//! bind/read/drop `std::net::TcpListener` because the service is
//! constructed inside `configure_context`, leaving the ephemeral port
//! undiscoverable otherwise. The error leg carries
//! `failIfNoConsumers=false` so the failure lands inside the traced step
//! call (readiness-time failures bypass the tracer adapter entirely — see
//! `add_failing_route`).

use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use camel_api::{CamelError, Exchange, Lifecycle, Message, MetricsCollector, RuntimeCommand};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_api::{NoOpComponentContext, RuntimeObservability};
use camel_component_direct::DirectComponent;
use camel_config::config::CamelConfig;
use camel_core::CamelContext;
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// Recording collector + lifecycle stand-in
// ---------------------------------------------------------------------------

/// Collects every `MetricsCollector` observation as `method:route` strings.
struct RecordingCollector {
    calls: Arc<Mutex<Vec<String>>>,
}

impl RecordingCollector {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            calls: Arc::new(Mutex::new(Vec::new())),
        })
    }

    fn push(&self, method: &str, key: &str) {
        self.calls
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(format!("{method}:{key}"));
    }

    fn snapshot(&self) -> Vec<String> {
        self.calls.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

impl MetricsCollector for RecordingCollector {
    fn record_exchange_duration(&self, route_id: &str, _duration: Duration) {
        self.push("record_exchange_duration", route_id);
    }

    fn increment_errors(&self, route_id: &str, _error_type: &str) {
        self.push("increment_errors", route_id);
    }

    fn increment_exchanges(&self, route_id: &str) {
        self.push("increment_exchanges", route_id);
    }

    fn set_queue_depth(&self, route_id: &str, _depth: usize) {
        self.push("set_queue_depth", route_id);
    }

    fn record_circuit_breaker_change(&self, route_id: &str, _from: &str, _to: &str) {
        self.push("record_circuit_breaker_change", route_id);
    }

    fn record_histogram(&self, name: &str, _value: f64, _labels: &[(&str, &str)]) {
        self.push("record_histogram", name);
    }

    fn record_counter(&self, name: &str, _value: f64, _labels: &[(&str, &str)]) {
        self.push("record_counter", name);
    }
}

/// Mirrors how `PrometheusService`/`OtelService` expose their collector:
/// a `Lifecycle` whose `as_metrics_collector` returns the recording one.
struct RecordingLifecycle {
    collector: Arc<RecordingCollector>,
}

#[async_trait]
impl Lifecycle for RecordingLifecycle {
    fn name(&self) -> &str {
        "recording-metrics-stand-in"
    }

    async fn start(&mut self) -> Result<(), CamelError> {
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), CamelError> {
        Ok(())
    }

    fn as_metrics_collector(&self) -> Option<Arc<dyn MetricsCollector>> {
        Some(Arc::clone(&self.collector) as Arc<dyn MetricsCollector>)
    }
}

// ---------------------------------------------------------------------------
// Harness helpers
// ---------------------------------------------------------------------------

fn test_rt() -> Arc<dyn RuntimeObservability> {
    Arc::new(NoOpComponentContext)
}

/// Pre-allocate an ephemeral port: bind, read, drop. The prometheus service
/// (constructed inside `configure_context`) re-binds it on `ctx.start()`.
fn prealloc_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind 127.0.0.1:0");
    listener
        .local_addr()
        .expect("read pre-allocated local addr")
        .port()
}

async fn context_from_toml(toml: &str) -> CamelContext {
    let config: CamelConfig = toml::from_str(toml).expect("test TOML parses into CamelConfig");
    let mut ctx = CamelConfig::configure_context(&config)
        .await
        .expect("configure_context succeeds");
    // configure_context registers no components by default; the direct
    // component backs both the route entry and the error leg.
    ctx.register_component(DirectComponent::new());
    ctx
}

/// `direct:entry → to:direct:missing` — the proven error path. The
/// `failIfNoConsumers=false` URI param is REQUIRED for pipeline-metrics
/// coverage: with the default `true`, `DirectProducer::poll_ready` fails
/// fast ("direct endpoint 'missing' not registered") and the exchange
/// errors at readiness — BEFORE the traced step call, so the tracer
/// adapter records nothing. With `false`, the failure moves to call time
/// ("no consumer registered for direct:missing", camel-direct lib.rs:465)
/// INSIDE the traced wrapper: the pipeline path records
/// duration/exchanges/errors, and the direct consumer still reports the
/// b-prime increment on the failed `send_and_wait`.
async fn add_failing_route(ctx: &CamelContext) {
    let route = RouteBuilder::from("direct:entry")
        .route_id("entry")
        .to("direct:missing?failIfNoConsumers=false")
        .build()
        .expect("failing route builds");
    ctx.add_route_definition(route)
        .await
        .expect("failing route registers");
}

async fn route_started(ctx: &CamelContext, route_id: &str) -> bool {
    matches!(
        ctx.runtime_route_status(route_id).await,
        Ok(Some(status)) if status == "Started"
    )
}

async fn wait_for_started(ctx: &CamelContext, route_ids: &[&str]) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let mut all_started = true;
        for id in route_ids {
            if !route_started(ctx, id).await {
                all_started = false;
                break;
            }
        }
        if all_started {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "routes {route_ids:?} did not reach Started within 5s"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Drive one InOut exchange through `direct:entry` via a direct producer
/// (fresh producer per attempt, retried on fast errors so startup
/// registration races cannot masquerade as route failures).
async fn drive_exchange(
    ctx: &CamelContext,
    retry_window: Duration,
) -> Result<Exchange, CamelError> {
    drive_exchange_to(ctx, "direct:entry", retry_window).await
}

/// Drive one InOut exchange through the given `direct:<name>` entry via a
/// fresh direct producer. `retry_window` of `Duration::ZERO` means
/// SINGLE-SHOT: the first Err returns immediately, so exactly-once
/// assertions count real dispatch invocations, not retry artifacts
/// (the default failing-exchange driver retries every Err for a second,
/// which would yield dozens of b′ emissions and make exactly-once
/// unassertable).
async fn drive_exchange_to(
    ctx: &CamelContext,
    endpoint: &str,
    retry_window: Duration,
) -> Result<Exchange, CamelError> {
    let deadline = tokio::time::Instant::now() + retry_window;
    loop {
        let producer = {
            let producer_ctx = ctx.producer_context();
            let registry = ctx.registry();
            let component = registry
                .get("direct")
                .expect("direct component not registered");
            let endpoint = component
                .create_endpoint(endpoint, ctx)
                .expect("failed to create direct endpoint");
            endpoint
                .create_producer(test_rt(), &producer_ctx)
                .expect("failed to create direct producer")
        };
        match producer
            .oneshot(Exchange::new_in_out(Message::new("metrics-probe")))
            .await
        {
            Ok(reply) => return Ok(reply),
            Err(_) if tokio::time::Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Err(e) => return Err(e),
        }
    }
}

/// Run one exchange through the failing route and assert it failed.
async fn run_one_failing_exchange(ctx: &CamelContext) {
    let outcome = tokio::time::timeout(
        Duration::from_secs(5),
        drive_exchange(ctx, Duration::from_secs(1)),
    )
    .await
    .expect("exchange through failing route completes within 5s");
    assert!(
        outcome.is_err(),
        "to:direct:missing must fail the exchange (no consumer registered)"
    );
}

/// Poll until the recording collector observed a call for every prefix.
async fn wait_for_calls(collector: &RecordingCollector, prefixes: &[&str]) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let calls = collector.snapshot();
        if prefixes
            .iter()
            .all(|p| calls.iter().any(|c| c.starts_with(p)))
        {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "recording collector never observed {prefixes:?}; got {calls:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Poll `GET /metrics` until the exchange-disposition family appears — a
/// non-empty body alone can precede our first exchange (process-level
/// families render immediately), so the poll waits for evidence the
/// exchange actually landed.
async fn poll_metrics_body(port: u16) -> String {
    let url = format!("http://127.0.0.1:{port}/metrics");
    let client = reqwest::Client::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(resp) = client.get(&url).send().await
            && resp.status().is_success()
            && let Ok(body) = resp.text().await
            && body.contains("camel_exchanges_total")
        {
            return body;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "prometheus /metrics never exposed camel_exchanges_total at {url}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prometheus_only_emits_pipeline_and_component_metrics() {
    let port = prealloc_port();
    let toml_cfg = format!(
        r#"[observability.prometheus]
enabled = true
host = "127.0.0.1"
port = {port}
"#
    );

    let mut ctx = context_from_toml(&toml_cfg).await;
    add_failing_route(&ctx).await;
    ctx.start().await.expect("context starts");
    wait_for_started(&ctx, &["entry"]).await;

    run_one_failing_exchange(&ctx).await;

    let body = poll_metrics_body(port).await;
    assert!(
        body.contains("camel_exchanges_total"),
        "pipeline exchange-disposition family missing from /metrics:\n{body}"
    );
    assert!(
        body.contains("camel_exchange_duration_seconds"),
        "pipeline duration family missing from /metrics:\n{body}"
    );
    assert!(
        body.contains("camel_errors_total"),
        "component error family missing from /metrics:\n{body}"
    );

    ctx.stop().await.expect("context stops");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn otel_stand_in_emits_pipeline_and_component_metrics() {
    // Otel enabled forces the tracer pipeline on (effective_tracer_config);
    // camel-test builds camel-config without its `otel` feature, so no real
    // OtelService exists — the RecordingLifecycle registered post-build is
    // the faithful mirror of the real service's registration point.
    let toml_cfg = r#"[observability.otel]
enabled = true
"#;

    let mut ctx = context_from_toml(toml_cfg).await;
    let collector = RecordingCollector::new();
    ctx = ctx.with_lifecycle(RecordingLifecycle {
        collector: Arc::clone(&collector),
    });
    add_failing_route(&ctx).await;
    ctx.start().await.expect("context starts");
    wait_for_started(&ctx, &["entry"]).await;

    run_one_failing_exchange(&ctx).await;

    wait_for_calls(
        &collector,
        &[
            "increment_exchanges:",
            "record_exchange_duration:",
            "increment_errors:",
        ],
    )
    .await;

    ctx.stop().await.expect("context stops");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn both_registered_composes() {
    let port = prealloc_port();
    let toml_cfg = format!(
        r#"[observability.prometheus]
enabled = true
host = "127.0.0.1"
port = {port}
"#
    );

    // configure_context registers PrometheusService first; the recording
    // lifecycle goes in after it — registration composes, never replaces.
    let mut ctx = context_from_toml(&toml_cfg).await;
    let collector = RecordingCollector::new();
    ctx = ctx.with_lifecycle(RecordingLifecycle {
        collector: Arc::clone(&collector),
    });
    add_failing_route(&ctx).await;
    ctx.start().await.expect("context starts");
    wait_for_started(&ctx, &["entry"]).await;

    run_one_failing_exchange(&ctx).await;

    // Composition is order-agnostic (the handle composes whichever
    // registration arrives second); this test deliberately registers the
    // recording stand-in AFTER prometheus — the reverse of the spec
    // scenario's otel-first wording — to prove the same property from the
    // other side.
    wait_for_calls(
        &collector,
        &[
            "increment_exchanges:",
            "record_exchange_duration:",
            "increment_errors:",
        ],
    )
    .await;
    let body = poll_metrics_body(port).await;
    assert!(
        !body.is_empty()
            && body.contains("camel_exchanges_total")
            && body.contains("camel_exchange_duration_seconds")
            && body.contains("camel_errors_total"),
        "prometheus collector stopped observing after the second registration:\n{body}"
    );

    ctx.stop().await.expect("context stops");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn late_registration_after_routes_observed() {
    // No observability at all: routes go in first, the recording lifecycle
    // is registered on the BUILT context afterwards.
    let mut ctx = context_from_toml("").await;
    add_failing_route(&ctx).await;
    let collector = RecordingCollector::new();
    ctx = ctx.with_lifecycle(RecordingLifecycle {
        collector: Arc::clone(&collector),
    });
    ctx.start().await.expect("context starts");
    wait_for_started(&ctx, &["entry"]).await;

    run_one_failing_exchange(&ctx).await;

    // The pipeline is disabled here, so the observable emission is the
    // component path: the direct consumer's error increment through the
    // runtime handle the context threaded into the controller.
    wait_for_calls(&collector, &["increment_errors:"]).await;

    ctx.stop().await.expect("context stops");
}

// ---------------------------------------------------------------------------
// b′ failure-signal contract for direct dispatches (direct-inline-fixes
// Fix B, rc-y5nn): every unhandled direct-dispatch failure emits
// `increment_errors` exactly once per failing dispatch invocation through
// the producer's context-threaded handle. Unhandled covers the initial
// registry lookup failure, admission failure, in-pipeline errors, and the
// dispatch timeout; `ConsumerStopping` surrenders emit nothing. All
// exactly-once cases drive SINGLE-SHOT (`Duration::ZERO` retry window)
// after `wait_for_started` so retries cannot multiply emissions.
// ---------------------------------------------------------------------------

/// The recording lifecycle is registered BEFORE `start()` compiles routes,
/// so the producer's controller-threaded `self.runtime.metrics()` reaches
/// the collector even though the producing route's pipeline tracing is
/// unwired (no observability config → tracer pipeline stays off).
async fn unwired_context_with_collector(toml: &str) -> (CamelContext, Arc<RecordingCollector>) {
    let mut ctx = context_from_toml(toml).await;
    let collector = RecordingCollector::new();
    ctx = ctx.with_lifecycle(RecordingLifecycle {
        collector: Arc::clone(&collector),
    });
    (ctx, collector)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn direct_lookup_failure_emits_b_prime_once() {
    let (mut ctx, collector) = unwired_context_with_collector("").await;
    let route = RouteBuilder::from("direct:entry2")
        .route_id("entry2")
        .to("direct:missing2?failIfNoConsumers=false")
        .build()
        .expect("producing route builds");
    ctx.add_route_definition(route)
        .await
        .expect("producing route registers");
    ctx.start().await.expect("context starts");
    wait_for_started(&ctx, &["entry2"]).await;

    let outcome = tokio::time::timeout(
        Duration::from_secs(5),
        drive_exchange_to(&ctx, "direct:entry2", Duration::ZERO),
    )
    .await
    .expect("single-shot exchange completes within 5s");
    assert!(
        outcome.is_err(),
        "dispatch to direct:missing2 must fail at registry lookup"
    );

    // The no-entry failure emits through the endpoint-derived attribution
    // `direct:missing2` — distinguishing the component signal from the
    // producing route's traced wrapper.
    let calls = collector.snapshot();
    let component = calls
        .iter()
        .filter(|c| **c == "increment_errors:direct:missing2")
        .count();
    assert_eq!(
        component, 1,
        "lookup failure must emit exactly one component b′ signal; got {calls:?}"
    );

    ctx.stop().await.expect("context stops");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn direct_pipeline_error_emits_b_prime_once() {
    let (mut ctx, collector) = unwired_context_with_collector("").await;
    let boom = RouteBuilder::from("direct:boom")
        .route_id("boom")
        .process(|_| async {
            Err::<Exchange, CamelError>(CamelError::ProcessorError("bench boom".into()))
        })
        .build()
        .expect("failing consumer route builds");
    ctx.add_route_definition(boom)
        .await
        .expect("failing consumer route registers");
    let entry = RouteBuilder::from("direct:entry3")
        .route_id("entry3")
        .to("direct:boom")
        .build()
        .expect("producing route builds");
    ctx.add_route_definition(entry)
        .await
        .expect("producing route registers");
    ctx.start().await.expect("context starts");
    wait_for_started(&ctx, &["boom", "entry3"]).await;

    let outcome = tokio::time::timeout(
        Duration::from_secs(5),
        drive_exchange_to(&ctx, "direct:entry3", Duration::ZERO),
    )
    .await
    .expect("single-shot exchange completes within 5s");
    assert!(
        matches!(outcome, Err(CamelError::ProcessorError(_))),
        "dispatch to direct:boom must surface the unhandled pipeline error"
    );

    // Entry-present failures attribute to the consumer entry's route id.
    let calls = collector.snapshot();
    let component = calls
        .iter()
        .filter(|c| **c == "increment_errors:boom")
        .count();
    assert_eq!(
        component, 1,
        "in-pipeline error must emit exactly one b′ signal; got {calls:?}"
    );

    ctx.stop().await.expect("context stops");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn direct_timeout_error_emits_b_prime_once() {
    let (mut ctx, collector) = unwired_context_with_collector("").await;
    let slow = RouteBuilder::from("direct:slow")
        .route_id("slow")
        .process(|ex| async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            Ok(ex)
        })
        .build()
        .expect("slow consumer route builds");
    ctx.add_route_definition(slow)
        .await
        .expect("slow consumer route registers");
    // timeout_ms is the snake_case URI param DirectConfig::from_uri reads;
    // the single timed section (lookup + admission + execution) expires at
    // 1ms while the consumer pipeline sleeps for 50ms.
    let entry = RouteBuilder::from("direct:entry4")
        .route_id("entry4")
        .to("direct:slow?timeout_ms=1")
        .build()
        .expect("producing route builds");
    ctx.add_route_definition(entry)
        .await
        .expect("producing route registers");
    ctx.start().await.expect("context starts");
    wait_for_started(&ctx, &["slow", "entry4"]).await;

    let outcome = tokio::time::timeout(
        Duration::from_secs(5),
        drive_exchange_to(&ctx, "direct:entry4", Duration::ZERO),
    )
    .await
    .expect("single-shot exchange completes within 5s");
    assert!(
        outcome
            .as_ref()
            .err()
            .is_some_and(|e| e.to_string().contains("timed out")),
        "dispatch to direct:slow must fail with the dispatch timeout, got {outcome:?}"
    );

    // The timeout branch (outer timeout wrapper — tokio drops the inner
    // future, so the inner emission site never ran) emits through the same
    // handle with the endpoint-derived attribution.
    let calls = collector.snapshot();
    let component = calls
        .iter()
        .filter(|c| **c == "increment_errors:direct:slow")
        .count();
    assert_eq!(
        component, 1,
        "timeout failure must emit exactly one b′ signal; got {calls:?}"
    );

    ctx.stop().await.expect("context stops");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn direct_consumer_stopping_no_emit() {
    let (mut ctx, collector) = unwired_context_with_collector("").await;
    // Stop-race shape (mirrors camel-core inline_dispatcher_tests.rs
    // `inline_consumer_stop_yields_consumer_stopping`): an inline dispatch
    // parks inside the target consumer's pipeline, the target route stops
    // mid-flight, and the dispatch fails with the ConsumerStopping
    // surrender. The drain grace (~5s) elapses before the pipeline token
    // cancels, so the surrender surfaces only after the stop's grace.
    let entered = Arc::new(tokio::sync::Notify::new());
    let probe = Arc::clone(&entered);
    let target = RouteBuilder::from("direct:target5")
        .route_id("target5")
        .process(move |ex| {
            let entered = Arc::clone(&probe);
            async move {
                entered.notify_one();
                std::future::pending::<()>().await;
                Ok(ex)
            }
        })
        .build()
        .expect("parking consumer route builds");
    ctx.add_route_definition(target)
        .await
        .expect("parking consumer route registers");
    let entry = RouteBuilder::from("direct:entry5")
        .route_id("entry5")
        .to("direct:target5")
        .build()
        .expect("producing route builds");
    ctx.add_route_definition(entry)
        .await
        .expect("producing route registers");
    ctx.start().await.expect("context starts");
    wait_for_started(&ctx, &["target5", "entry5"]).await;

    let runtime = ctx.runtime();
    let stopper = async move {
        entered.notified().await;
        runtime
            .execute(RuntimeCommand::StopRoute {
                route_id: "target5".to_string(),
                command_id: "test:stop:target5".to_string(),
                causation_id: None,
            })
            .await
            .expect("stopping target5 succeeds");
    };
    let drive = drive_exchange_to(&ctx, "direct:entry5", Duration::ZERO);
    let (outcome, _) = tokio::join!(drive, stopper);
    let err = outcome.expect_err("parked dispatch must fail when the consumer stops");
    assert!(
        matches!(err, CamelError::ConsumerStopping),
        "expected the consumer-stop surrender, got {err:?}"
    );

    // The surrender is not an operator-visible failure: zero b′ emissions
    // are attributable to it. The emission (had it fired) would precede
    // the error surfacing, so the snapshot is final here.
    let calls = collector.snapshot();
    assert!(
        calls.iter().all(|c| !c.starts_with("increment_errors:")),
        "ConsumerStopping surrender must not emit b′; got {calls:?}"
    );

    ctx.stop().await.expect("context stops");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn direct_wired_route_no_double_count() {
    // Producing route WITH pipeline tracing wired (otel enabled forces the
    // tracer pipeline on; the recording lifecycle registered post-build
    // mirrors the real OtelService registration point).
    let (mut ctx, collector) =
        unwired_context_with_collector("[observability.otel]\nenabled = true\n").await;
    let route = RouteBuilder::from("direct:entry6")
        .route_id("entry6")
        .to("direct:missing3?failIfNoConsumers=false")
        .build()
        .expect("wired producing route builds");
    ctx.add_route_definition(route)
        .await
        .expect("wired producing route registers");
    ctx.start().await.expect("context starts");
    wait_for_started(&ctx, &["entry6"]).await;

    let outcome = tokio::time::timeout(
        Duration::from_secs(5),
        drive_exchange_to(&ctx, "direct:entry6", Duration::ZERO),
    )
    .await
    .expect("single-shot exchange completes within 5s");
    assert!(
        outcome.is_err(),
        "dispatch to direct:missing3 must fail at registry lookup"
    );

    // The component b′ signal is counted by its endpoint-derived
    // attribution — exactly once. The traced wrapper's own pipeline-error
    // recording (attributed to the producing route) is separate, additive
    // telemetry: it neither suppresses nor duplicates the component signal.
    let calls = collector.snapshot();
    let component = calls
        .iter()
        .filter(|c| **c == "increment_errors:direct:missing3")
        .count();
    assert_eq!(
        component, 1,
        "wired route must record the component b′ signal exactly once; got {calls:?}"
    );
    assert!(
        calls.iter().any(|c| c.starts_with("increment_errors:")
            && c.as_str() != "increment_errors:direct:missing3"),
        "the traced wrapper's own recording must be present as additive telemetry; got {calls:?}"
    );

    ctx.stop().await.expect("context stops");
}
