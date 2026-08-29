//! Component-emission audit for the dead-observability sweep
//! (dashboard-observability Task 4.2).
//!
//! The five swept components (wasm, opensearch, seda, surrealdb, cxf)
//! now emit through the `ComponentMetrics` facade
//! (`RuntimeObservability::component_metrics()`) at their principal
//! operation boundaries. `AUDIT` below is the authoritative table.
//!
//! Double-count notes (ADR-0066 contract): retained specific labels
//! (b-prime:*) differ from the facade's e:{component}:{operation}, so
//! no single series double-counts; family-wide sums count each
//! failure twice (uniform + specific) — intended. LATENT: retry_async
//! with metrics=Some emits the same e:{scheme}:{operation} shape —
//! a component wiring BOTH the facade and metrics=Some at one boundary
//! would true-double-count that series; the ADR forbids the pair.
//!
//! # Double-count contract (design D5, binding)
//!
//! On failure the facade increments the error family as
//! `increment_errors(component, "e:{component}:{operation}")`. Retained
//! ADR-0012-specific labels (`b-prime:surrealdb:notification`,
//! `b-prime:cxf:response-marshalling`) stay; they NEVER equal the facade
//! label, so a single failure can land on TWO error-family series.
//! Dashboards summing the whole family count such failures twice —
//! intended per design D5, not a bug.
//!
//! # Drivable vs deferred
//!
//! - **Driven here** (default feature set): seda — `camel-test` drives
//!   SEDA routes through the real route compiler, so the producers and
//!   consumers receive the controller `RuntimeObservability` with the
//!   lever snapshot and the recording collector.
//! - **Driven here** (Task 4.3 legs): http (default feature set,
//!   raw-TCP server), and — under `integration-tests`, via the shared
//!   testcontainers backends of `redis_test.rs` / `kafka_test.rs` (CI
//!   runs this target with the feature, ci.yml component-emission step)
//!   — redis and kafka. Success legs run with the lever ON and assert
//!   `{component}:{op}:success` on the component-ops family; kafka's
//!   failure leg runs with the lever OFF and asserts `e:kafka:produce`
//!   on the never-gated error family. Kafka topic/group names are
//!   unique per run (`std::process::id`) so a stale broker state can
//!   never satisfy the consume leg.
//! - **Failure legs run, success legs need backends** (compiled under
//!   `integration-tests`, no `#[ignore]`): opensearch and surrealdb —
//!   the failure legs (refused port / dead datasource) are deterministic
//!   without Docker and execute in every integration run; their SUCCESS
//!   legs need live backends (testcontainers/Docker) and are not
//!   written here — CI covers the failure legs via the
//!   component-emission step.
//! - **No honest harness exists without external artifacts** (documented,
//!   not faked): wasm — endpoint creation canonicalizes the guest module
//!   path, so even the failure leg needs a compiled `.wasm` guest
//!   (fixture lives in `camel-component-wasm`); cxf — every consumer
//!   start requires the `cxf-bridge` binary
//!   (`support::cxf::require_cxf_bridge_binary`). Both stay in `AUDIT`
//!   and are verified by their crates' integration suites plus CI.

// Shared test-support module; only the crypto/redis helpers are used
// here, so dead-code from the unused (feature-gated) siblings is allowed.
#[allow(dead_code)]
mod support;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use camel_api::{CamelError, Exchange, Lifecycle, Message, MetricsCollector};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_api::{NoOpComponentContext, RuntimeObservability};
use camel_component_direct::DirectComponent;
use camel_component_seda::SedaComponent;
use camel_config::config::CamelConfig;
use camel_core::CamelContext;
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// Audit table
// ---------------------------------------------------------------------------

/// Every swept component's principal operation (Task 4.2 audit table).
/// Kept as data so `audit_table_complete` can lock the exact set and
/// cross-check it against what the driven subset actually emitted.
///
/// Double-count note: on failure each entry lands on the error family as
/// `e:{component}:{operation}`; retained `b-prime:*` labels coexist and
/// differ — family-wide sums count those failures twice (intended, D5).
const AUDIT: &[(&str, &str)] = &[
    ("wasm", "invoke"),
    ("opensearch", "execute"),
    ("seda", "consume"),
    ("seda", "produce"),
    ("surrealdb", "query"),
    ("cxf", "consume"),
];

/// Subset drivable with the default `camel-test` feature set (no Docker,
/// no guest modules, no bridge binaries). Scoped to AUDIT entries — the
/// Task 4.3 components (kafka/redis/http) sit outside the swept set and
/// are driven by their own legs (http default features; redis/kafka
/// under `integration-tests`) — see the module header.
const DRIVABLE: &[(&str, &str)] = &[("seda", "consume"), ("seda", "produce")];

// ---------------------------------------------------------------------------
// Recording collector (records full component/operation/outcome labels)
// ---------------------------------------------------------------------------

struct EmissionRecorder {
    ops: Mutex<Vec<String>>,
    errors: Mutex<Vec<String>>,
}

impl EmissionRecorder {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            ops: Mutex::new(Vec::new()),
            errors: Mutex::new(Vec::new()),
        })
    }

    fn ops(&self) -> Vec<String> {
        self.ops.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn errors(&self) -> Vec<String> {
        self.errors
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

impl MetricsCollector for EmissionRecorder {
    fn record_exchange_duration(&self, _route_id: &str, _duration: Duration) {}
    fn increment_errors(&self, component: &str, error_type: &str) {
        self.errors
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(format!("{component}:{error_type}"));
    }
    fn increment_exchanges(&self, _route_id: &str) {}
    fn set_queue_depth(&self, _queue: &str, _depth: usize) {}
    fn record_circuit_breaker_change(&self, _route: &str, _from: &str, _to: &str) {}
    fn record_component_operation(&self, component: &str, operation: &str, outcome: &str) {
        self.ops
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(format!("{component}:{operation}:{outcome}"));
    }
}

struct RecordingLifecycle {
    collector: Arc<EmissionRecorder>,
}

#[async_trait]
impl Lifecycle for RecordingLifecycle {
    fn name(&self) -> &str {
        "component-emission-recorder"
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
// Harness (idioms mirror metrics_wiring_test.rs)
// ---------------------------------------------------------------------------

fn test_rt() -> Arc<dyn RuntimeObservability> {
    Arc::new(NoOpComponentContext)
}

async fn context_from_toml(toml: &str) -> CamelContext {
    let config: CamelConfig = toml::from_str(toml).expect("test TOML parses into CamelConfig");
    let mut ctx = CamelConfig::configure_context(&config)
        .await
        .expect("configure_context succeeds");
    ctx.register_component(DirectComponent::new());
    ctx.register_component(SedaComponent::new());
    ctx
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
/// (fresh producer per attempt, retried on fast errors so startup races
/// cannot masquerade as route failures).
async fn drive_entry(ctx: &CamelContext, retry_window: Duration) -> Result<Exchange, CamelError> {
    drive_direct(ctx, "direct:entry", retry_window).await
}

/// Same as [`drive_entry`] but for an arbitrary `direct:{name}` entry
/// endpoint (dashboard-observability Task 4.3 legs use per-outcome routes).
async fn drive_direct(
    ctx: &CamelContext,
    entry_uri: &str,
    retry_window: Duration,
) -> Result<Exchange, CamelError> {
    let deadline = tokio::time::Instant::now() + retry_window;
    loop {
        let producer = {
            let producer_ctx = ctx.producer_context();
            let registry = ctx.registry();
            let component = registry.get("direct").expect("direct component registered");
            let endpoint = component
                .create_endpoint(entry_uri, ctx)
                .expect("failed to create direct endpoint");
            endpoint
                .create_producer(test_rt(), &producer_ctx)
                .expect("failed to create direct producer")
        };
        match producer
            .oneshot(Exchange::new_in_out(Message::new("emission-probe")))
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

async fn wait_for<F>(mut probe: F)
where
    F: FnMut() -> bool,
{
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if probe() {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "condition not met within 5s"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

// ---------------------------------------------------------------------------
// Driven cases: seda
// ---------------------------------------------------------------------------

/// Success leg (lever ON): `direct:entry → to(seda:work)` with a live
/// worker `from(seda:work)`. The route-compiled seda producer and the
/// worker's forwarder observe through the controller observability
/// handle, so `seda:produce:success` and `seda:consume:success` must both
/// land on the recording collector. Returns the emitted op labels.
async fn drive_seda_success(recorder: &Arc<EmissionRecorder>) -> Vec<String> {
    let mut ctx = context_from_toml(
        r#"[observability.metrics]
enabled = true
components = true
"#,
    )
    .await;
    ctx = ctx.with_lifecycle(RecordingLifecycle {
        collector: Arc::clone(recorder),
    });

    let worker = RouteBuilder::from("seda:work")
        .route_id("seda-worker")
        .process(|ex| async move { Ok(ex) })
        .build()
        .expect("seda worker route builds");
    ctx.add_route_definition(worker)
        .await
        .expect("worker route registers");

    let entry = RouteBuilder::from("direct:entry")
        .route_id("entry")
        .to("seda:work?waitForTaskToComplete=Always")
        .build()
        .expect("entry route builds");
    ctx.add_route_definition(entry)
        .await
        .expect("entry route registers");

    ctx.start().await.expect("context starts");
    wait_for_started(&ctx, &["seda-worker", "entry"]).await;

    let reply = tokio::time::timeout(
        Duration::from_secs(5),
        drive_entry(&ctx, Duration::from_secs(1)),
    )
    .await
    .expect("exchange through seda route completes within 5s");
    assert!(
        reply.is_ok(),
        "seda produce/consume must succeed, got {reply:?}"
    );

    wait_for(|| {
        let ops = recorder.ops();
        ops.iter().any(|o| o == "seda:produce:success")
            && ops.iter().any(|o| o == "seda:consume:success")
    })
    .await;

    ctx.stop().await.expect("context stops");
    recorder.ops()
}

/// Failure leg (lever OFF, the default): `direct:entry → to(seda:missing)`
/// with no consumer registered — produce fails and the error family must
/// see `seda:e:seda:produce` while the component-ops family stays empty.
async fn drive_seda_failure(recorder: &Arc<EmissionRecorder>) {
    let mut ctx = context_from_toml("").await;
    ctx = ctx.with_lifecycle(RecordingLifecycle {
        collector: Arc::clone(recorder),
    });

    let entry = RouteBuilder::from("direct:entry")
        .route_id("entry")
        .to("seda:missing")
        .build()
        .expect("entry route builds");
    ctx.add_route_definition(entry)
        .await
        .expect("entry route registers");

    ctx.start().await.expect("context starts");
    wait_for_started(&ctx, &["entry"]).await;

    let outcome = tokio::time::timeout(
        Duration::from_secs(5),
        drive_entry(&ctx, Duration::from_secs(1)),
    )
    .await
    .expect("exchange through failing seda route completes within 5s");
    assert!(
        outcome.is_err(),
        "to:seda:missing must fail the exchange (no consumer registered)"
    );

    wait_for(|| recorder.errors().iter().any(|e| e == "seda:e:seda:produce")).await;
    assert!(
        !recorder.ops().iter().any(|o| o.starts_with("seda:")),
        "lever OFF must suppress the component-ops family, got {:?}",
        recorder.ops()
    );

    ctx.stop().await.expect("context stops");
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dead_components_now_emit() {
    // Success (lever ON).
    let success_recorder = EmissionRecorder::new();
    drive_seda_success(&success_recorder).await;

    // Failure (lever OFF): error family present, family suppressed.
    let failure_recorder = EmissionRecorder::new();
    drive_seda_failure(&failure_recorder).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn audit_table_complete() {
    // The audit table is the exact swept set — locked so any addition or
    // removal is a deliberate plan change, not drift.
    assert_eq!(
        AUDIT,
        &[
            ("wasm", "invoke"),
            ("opensearch", "execute"),
            ("seda", "consume"),
            ("seda", "produce"),
            ("surrealdb", "query"),
            ("cxf", "consume"),
        ],
        "AUDIT must list exactly the Task 4.2 swept component operations"
    );

    // Cross-check: driving the mock-backed subset emits exactly the
    // AUDIT entries it declares (and nothing outside the table).
    let recorder = EmissionRecorder::new();
    let emitted = drive_seda_success(&recorder).await;
    let seda_ops: Vec<(&str, &str)> = emitted
        .iter()
        .map(|e| {
            let (component, rest) = e.split_once(':').expect("op label has component prefix");
            let (operation, outcome) = rest.split_once(':').expect("op label has outcome suffix");
            assert_eq!(outcome, "success", "unexpected outcome in {e}");
            (component, operation)
        })
        .collect();
    for op in &seda_ops {
        assert!(
            AUDIT.contains(op),
            "emitted {op:?} is not in the audit table"
        );
    }
    for entry in DRIVABLE {
        assert!(
            seda_ops.contains(entry),
            "drivable entry {entry:?} was not emitted: {emitted:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Deferred entries: integration-verification-deferred-to-CI
// ---------------------------------------------------------------------------
// Success paths need live backends; the failure legs below are
// deterministic without Docker and hold the wiring honest.

#[cfg(feature = "integration-tests")]
mod deferred {
    use super::*;
    use camel_component_opensearch::OpenSearchComponent;
    use camel_component_surrealdb::SurrealDbComponent;

    /// OpenSearch failure leg: refused loopback port, retries disabled —
    /// `e:opensearch:execute` must land on the error family with the
    /// lever off. Success path (real index round-trip) needs the
    /// testcontainers backend of `opensearch_test.rs`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn opensearch_execute_emits() {
        let recorder = EmissionRecorder::new();
        let mut ctx = context_from_toml("").await;
        ctx.register_component(OpenSearchComponent::new());
        ctx = ctx.with_lifecycle(RecordingLifecycle {
            collector: Arc::clone(&recorder),
        });

        let entry = RouteBuilder::from("direct:entry")
            .route_id("entry")
            .to("opensearch://127.0.0.1:1/idx?operation=PING&retryEnabled=false&timeout=500")
            .build()
            .expect("entry route builds");
        ctx.add_route_definition(entry)
            .await
            .expect("entry route registers");

        ctx.start().await.expect("context starts");
        wait_for_started(&ctx, &["entry"]).await;

        let outcome = tokio::time::timeout(
            Duration::from_secs(10),
            drive_entry(&ctx, Duration::from_secs(1)),
        )
        .await
        .expect("exchange through dead opensearch port completes within 10s");
        assert!(
            outcome.is_err(),
            "dead opensearch port must fail the exchange"
        );

        wait_for(|| {
            recorder
                .errors()
                .iter()
                .any(|e| e == "opensearch:e:opensearch:execute")
        })
        .await;

        ctx.stop().await.expect("context stops");
    }

    /// SurrealDB failure leg: a catalog whose datasource points at a dead
    /// WebSocket port — the live-query establishment (datasource resolve +
    /// LIVE SELECT bind) fails in the consumer's `start()` and
    /// `e:surrealdb:query` must land on the error family with the lever
    /// off. Success path (LIVE SELECT notifications) needs the
    /// testcontainers backend of `surrealdb_test.rs`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn surrealdb_query_emits() {
        use camel_api::datasource::DatasourceCatalog;
        use camel_component_surrealdb::pool_factory::SurrealDbPoolFactory;
        use camel_core::datasource::RuntimeDatasourceCatalog;

        let recorder = EmissionRecorder::new();
        let mut ctx = context_from_toml("").await;

        // Catalog with a datasource on a refused loopback port: endpoint
        // creation passes (ws:// scheme, datasource resolvable), pushing
        // the failure into the consumer start establishment.
        let mut configs = std::collections::HashMap::new();
        configs.insert(
            "dead".to_string(),
            camel_api::datasource::DatasourceConfig {
                db_url: "ws://127.0.0.1:1/dead".to_string(),
                provider: Some("surrealdb".to_string()),
                max_connections: None,
                min_connections: None,
                idle_timeout_secs: None,
                max_lifetime_secs: None,
                ssl_mode: None,
                ssl_root_cert: None,
                ssl_cert: None,
                ssl_key: None,
                extra: std::collections::HashMap::new(),
            },
        );
        let catalog = RuntimeDatasourceCatalog::new(configs);
        catalog
            .register_factory("surrealdb", Arc::new(SurrealDbPoolFactory))
            .expect("register factory");
        ctx.register_component(SurrealDbComponent::with_catalog(Arc::new(catalog)));
        ctx = ctx.with_lifecycle(RecordingLifecycle {
            collector: Arc::clone(&recorder),
        });

        let worker = RouteBuilder::from("surrealdb:live?datasource=dead&table=any")
            .route_id("surreal-worker")
            .process(|ex| async move { Ok(ex) })
            .build()
            .expect("surreal worker route builds");
        ctx.add_route_definition(worker)
            .await
            .expect("worker route registers");

        // Route start fails against the dead port — context start reports
        // the route error; the emission is observable either way.
        let _ = ctx.start().await;

        // The route never reaches Started — poll the collector for the
        // error family instead.
        wait_for(|| {
            recorder
                .errors()
                .iter()
                .any(|e| e == "surrealdb:e:surrealdb:query")
        })
        .await;

        ctx.stop().await.expect("context stops");
    }
}

// ---------------------------------------------------------------------------
// Task 4.3: kafka/redis/http success paths + scheme alignment
// ---------------------------------------------------------------------------

/// Component operation vocabularies — the Task 4.3 const table.
///
/// The first entries of each set are the facade principal operations
/// (`camel_component_operations_total` + `e:{component}:{operation}`
/// error labels); the remaining entries are the retry-helper (Task 1.2)
/// operation literals the component passes to `retry_async` /
/// `retry_async_cancelable` with `metrics=None` (labels only surface if a
/// future caller wires `metrics=Some` — the double-count contract forbids
/// that at a facade-owned boundary).
///
/// kafka note: `"recv"` is a consume-leg transport op kept for the
/// retained Phase-1 exhaustion label `e:kafka:recv-exhaustion` (manual
/// loop — kafka passes NO retry literals). It is NOT renamed to
/// `"consume"`: the facade observes per-message dispatch, the exhaustion
/// label covers transport starvation — different failure classes, no
/// same-series double-count (family-wide sums may count one failure on
/// two series; intended per D5).
const COMPONENT_OP_SETS: &[(&str, &[&str])] = &[
    ("kafka", &["consume", "produce", "recv"]),
    ("redis", &["command"]),
    ("http", &["request"]),
    ("seda", &["consume", "produce"]),
    ("opensearch", &["execute"]),
    ("surrealdb", &["query", "connect", "setup"]),
    ("cxf", &["consume"]),
    ("wasm", &["invoke"]),
    ("sql", &["consumer-pool-init", "producer-pool-init"]),
    ("container", &["events-connect", "logs-connect"]),
    ("ws", &["connect"]),
    ("grpc", &["rpc"]),
];

/// Every scheme/operation literal pair passed to `retry_async` /
/// `retry_async_cancelable` in production code (tests use "t"/"op").
/// Mirrors the Task 1.2 sweep: surrealdb pool_factory, container
/// reconnect, sql pool inits, ws reconnect, grpc rpc retry, opensearch
/// execute dispatch.
const RETRY_LABELS: &[(&str, &str)] = &[
    ("surrealdb", "connect"),
    ("surrealdb", "setup"),
    ("container", "events-connect"),
    ("container", "logs-connect"),
    ("sql", "consumer-pool-init"),
    ("sql", "producer-pool-init"),
    ("ws", "connect"),
    ("grpc", "rpc"),
    ("opensearch", "execute"),
];

/// Unit: the scheme/operation literal sets used in retry calls are
/// members of the component op sets (const-vs-const comparison — the
/// table is the Task 4.3 alignment contract, not runtime-observed data).
#[test]
fn retry_scheme_matches_operation_set() {
    for (scheme, operation) in RETRY_LABELS {
        let Some((_, ops)) = COMPONENT_OP_SETS.iter().find(|(s, _)| s == scheme) else {
            panic!("retry scheme {scheme:?} has no component op set entry");
        };
        assert!(
            ops.contains(operation),
            "retry operation {operation:?} is not in {scheme}'s op set {ops:?}"
        );
    }
    // kafka pins its retained Phase-1 exhaustion label: "recv" stays a
    // consume-leg op in the table (never folded into "consume").
    let kafka = COMPONENT_OP_SETS
        .iter()
        .find(|(s, _)| *s == "kafka")
        .expect("kafka op set present");
    assert!(kafka.1.contains(&"recv"));
}

/// HTTP round-trip: 200 → `http:request:success`, 5xx (with
/// throwExceptionOnFailure default true) → failed exchange +
/// `http:request:failure` on the component-ops family AND
/// `http:e:http:request` on the never-gated error family. Lever ON for
/// both legs so the outcome series are recorded.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn http_roundtrip_observed() {
    // In-repo raw-TCP test server (http_test.rs pattern): loop-accept,
    // route by path — /fail answers 500, everything else 200.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("test server binds");
    let port = listener.local_addr().expect("local addr").port();
    let server = tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let mut buf = vec![0u8; 4096];
                let n = stream.read(&mut buf).await.unwrap_or(0);
                let request = String::from_utf8_lossy(&buf[..n]);
                // Match on the request target (method-agnostic: the probe
                // exchange carries a body, so the producer sends POST).
                let target = request.lines().next().and_then(|l| l.split(' ').nth(1));
                let (status_line, body) = if target == Some("/fail") {
                    ("500 Internal Server Error", "boom")
                } else {
                    ("200 OK", "ok")
                };
                let response = format!(
                    "HTTP/1.1 {status_line}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes()).await;
            });
        }
    });

    let recorder = EmissionRecorder::new();
    let mut ctx = context_from_toml(
        r#"[observability.metrics]
enabled = true
components = true
"#,
    )
    .await;
    ctx.register_component(camel_component_http::HttpComponent::new());
    ctx = ctx.with_lifecycle(RecordingLifecycle {
        collector: Arc::clone(&recorder),
    });

    let ok_route = RouteBuilder::from("direct:ok")
        .route_id("http-ok")
        .to(format!("http://127.0.0.1:{port}/ok?allowInternal=true"))
        .build()
        .expect("ok route builds");
    ctx.add_route_definition(ok_route)
        .await
        .expect("ok route registers");

    let fail_route = RouteBuilder::from("direct:fail")
        .route_id("http-fail")
        .to(format!("http://127.0.0.1:{port}/fail?allowInternal=true"))
        .build()
        .expect("fail route builds");
    ctx.add_route_definition(fail_route)
        .await
        .expect("fail route registers");

    ctx.start().await.expect("context starts");
    wait_for_started(&ctx, &["http-ok", "http-fail"]).await;

    let ok = tokio::time::timeout(
        Duration::from_secs(5),
        drive_direct(&ctx, "direct:ok", Duration::from_secs(1)),
    )
    .await
    .expect("200 request completes within 5s");
    assert!(ok.is_ok(), "200 request must succeed, got {ok:?}");

    let fail = tokio::time::timeout(
        Duration::from_secs(5),
        drive_direct(&ctx, "direct:fail", Duration::from_secs(1)),
    )
    .await
    .expect("5xx request completes within 5s");
    assert!(fail.is_err(), "5xx must fail the exchange, got {fail:?}");

    wait_for(|| {
        let ops = recorder.ops();
        ops.iter().any(|o| o == "http:request:success")
            && ops.iter().any(|o| o == "http:request:failure")
    })
    .await;
    wait_for(|| recorder.errors().iter().any(|e| e == "http:e:http:request")).await;

    ctx.stop().await.expect("context stops");
    server.abort();
}

/// Redis command legs against the shared testcontainers backend
/// (`support::redis`, the repo's existing redis harness — runs under the
/// `integration-tests` feature alongside redis_test.rs, not #[ignore]d;
/// CI runs this test target explicitly (ci.yml component-emission step).
/// Success: SET → `redis:command:success`. Failure: INCR on a key
/// holding a string → WRONGTYPE (non-transient, non-idempotent → single
/// attempt, immediate Err) → `redis:command:failure` AND
/// `redis:e:redis:command`.
#[cfg(feature = "integration-tests")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn redis_success_and_failure_observed() {
    support::install_crypto_provider();
    let conn = support::redis::shared_redis().await.to_string();

    let recorder = EmissionRecorder::new();
    let mut ctx = context_from_toml(
        r#"[observability.metrics]
enabled = true
components = true
"#,
    )
    .await;
    ctx.register_component(camel_component_redis::RedisComponent::new());
    ctx = ctx.with_lifecycle(RecordingLifecycle {
        collector: Arc::clone(&recorder),
    });

    // Seed route: SET a string value (also the success leg).
    let seed = RouteBuilder::from("direct:seed")
        .route_id("redis-seed")
        .set_header(
            "CamelRedis.Key",
            serde_json::Value::String("emission-key".into()),
        )
        .set_header(
            "CamelRedis.Value",
            serde_json::Value::String("not-a-number".into()),
        )
        .to(format!("redis://{conn}?command=SET"))
        .build()
        .expect("seed route builds");
    ctx.add_route_definition(seed)
        .await
        .expect("seed route registers");

    // Failure route: INCR the string key → WRONGTYPE.
    let incr = RouteBuilder::from("direct:incr")
        .route_id("redis-incr")
        .set_header(
            "CamelRedis.Key",
            serde_json::Value::String("emission-key".into()),
        )
        .to(format!("redis://{conn}?command=INCR"))
        .build()
        .expect("incr route builds");
    ctx.add_route_definition(incr)
        .await
        .expect("incr route registers");

    ctx.start().await.expect("context starts");
    wait_for_started(&ctx, &["redis-seed", "redis-incr"]).await;

    let seeded = tokio::time::timeout(
        Duration::from_secs(5),
        drive_direct(&ctx, "direct:seed", Duration::from_secs(1)),
    )
    .await
    .expect("SET completes within 5s");
    assert!(seeded.is_ok(), "SET must succeed, got {seeded:?}");

    let incr_result = tokio::time::timeout(
        Duration::from_secs(5),
        drive_direct(&ctx, "direct:incr", Duration::from_secs(1)),
    )
    .await
    .expect("INCR completes within 5s");
    assert!(
        incr_result.is_err(),
        "INCR on a string key must fail (WRONGTYPE), got {incr_result:?}"
    );

    wait_for(|| {
        let ops = recorder.ops();
        ops.iter().any(|o| o == "redis:command:success")
            && ops.iter().any(|o| o == "redis:command:failure")
    })
    .await;
    wait_for(|| {
        recorder
            .errors()
            .iter()
            .any(|e| e == "redis:e:redis:command")
    })
    .await;

    ctx.stop().await.expect("context stops");
}

/// Kafka produce/consume legs against the shared testcontainers backend
/// (`support::kafka`, the repo's existing kafka harness — runs under the
/// `integration-tests` feature alongside kafka_test.rs, not #[ignore]d;
/// CI runs this test target explicitly, ci.yml component-emission step).
/// Topic and group names are unique per run (`std::process::id`) and the
/// consumer uses a fresh group with `autoOffsetReset=earliest`, so the
/// produced message is read back regardless of group-join timing — no
/// delivery race.
///
/// Success (lever ON): producer route `direct:produce → kafka:{topic}`
/// (the facade-owning producer, KafkaProducer::call) and a consumer
/// route `from(kafka:{topic})` on the same topic → `kafka:produce:success`
/// AND `kafka:consume:success` on the component-ops family.
///
/// Failure (lever OFF): kafka_test.rs has no failure legs to copy, so
/// this mirrors the seda leg — produce to a dead bootstrap (refused
/// loopback port) with `requestTimeoutMs` pinned low (it also becomes
/// the producer's `message.timeout.ms`, bounding the delivery attempt)
/// → `kafka:e:kafka:produce` on the error family while the
/// component-ops family stays suppressed.
#[cfg(feature = "integration-tests")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn kafka_produce_consume_and_failure_observed() {
    support::install_crypto_provider();
    let (_, brokers) = support::kafka::shared_kafka().await;
    let topic = format!("emission-topic-{}", std::process::id());
    let group = format!("emission-group-{}", std::process::id());

    // --- Success (lever ON): round-trip produce + consume.
    let recorder = EmissionRecorder::new();
    let mut ctx = context_from_toml(
        r#"[observability.metrics]
enabled = true
components = true
"#,
    )
    .await;
    ctx.register_component(camel_component_kafka::KafkaComponent::new());
    ctx = ctx.with_lifecycle(RecordingLifecycle {
        collector: Arc::clone(&recorder),
    });

    let producer = RouteBuilder::from("direct:produce")
        .route_id("kafka-produce")
        .set_body(camel_api::Value::String("emission-payload".to_string()))
        .to(format!("kafka:{topic}?brokers={brokers}&acks=all"))
        .build()
        .expect("producer route builds");
    ctx.add_route_definition(producer)
        .await
        .expect("producer route registers");

    let consumer = RouteBuilder::from(&format!(
        "kafka:{topic}?brokers={brokers}&groupId={group}&autoOffsetReset=earliest"
    ))
    .route_id("kafka-consume")
    .process(|ex| async move { Ok(ex) })
    .build()
    .expect("consumer route builds");
    ctx.add_route_definition(consumer)
        .await
        .expect("consumer route registers");

    ctx.start().await.expect("context starts");
    wait_for_started(&ctx, &["kafka-produce", "kafka-consume"]).await;

    let sent = tokio::time::timeout(
        Duration::from_secs(10),
        drive_direct(&ctx, "direct:produce", Duration::from_secs(1)),
    )
    .await
    .expect("kafka produce completes within 10s");
    assert!(
        sent.is_ok(),
        "produce against the live broker must succeed, got {sent:?}"
    );

    wait_for(|| recorder.ops().iter().any(|o| o == "kafka:produce:success")).await;

    // Group join + partition assignment is asynchronous; the message is
    // retained in the log and the fresh group's earliest reset reads it
    // back — poll with a delivery-sized window (kafka_test.rs uses 20s).
    support::wait::wait_until(
        "kafka consume emission",
        Duration::from_secs(30),
        Duration::from_millis(200),
        || {
            let recorder = Arc::clone(&recorder);
            async move { Ok(recorder.ops().iter().any(|o| o == "kafka:consume:success")) }
        },
    )
    .await
    .expect("consume emission not observed within 30s");

    ctx.stop().await.expect("context stops");

    // --- Failure (lever OFF): dead bootstrap.
    let failure_recorder = EmissionRecorder::new();
    let mut fail_ctx = context_from_toml("").await;
    fail_ctx.register_component(camel_component_kafka::KafkaComponent::new());
    fail_ctx = fail_ctx.with_lifecycle(RecordingLifecycle {
        collector: Arc::clone(&failure_recorder),
    });

    let dead = RouteBuilder::from("direct:kafka-dead")
        .route_id("kafka-dead")
        .to(format!(
            "kafka:emission-dead-{}?brokers=127.0.0.1:1&requestTimeoutMs=1000",
            std::process::id()
        ))
        .build()
        .expect("dead-bootstrap route builds");
    fail_ctx
        .add_route_definition(dead)
        .await
        .expect("dead-bootstrap route registers");

    fail_ctx.start().await.expect("context starts");
    wait_for_started(&fail_ctx, &["kafka-dead"]).await;

    let outcome = tokio::time::timeout(
        Duration::from_secs(10),
        drive_direct(&fail_ctx, "direct:kafka-dead", Duration::from_secs(1)),
    )
    .await
    .expect("produce to dead bootstrap completes within 10s");
    assert!(
        outcome.is_err(),
        "produce to a dead bootstrap must fail, got {outcome:?}"
    );

    wait_for(|| {
        failure_recorder
            .errors()
            .iter()
            .any(|e| e == "kafka:e:kafka:produce")
    })
    .await;
    assert!(
        !failure_recorder
            .ops()
            .iter()
            .any(|o| o.starts_with("kafka:")),
        "lever OFF must suppress the component-ops family, got {:?}",
        failure_recorder.ops()
    );

    fail_ctx.stop().await.expect("context stops");
}
