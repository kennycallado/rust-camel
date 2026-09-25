//! Integration tests for the doTry / doCatch / doFinally EIP.
//!
//! These tests exercise the full pipeline: CamelContext → Consumer → DoTryService → Mock,
//! verifying that exceptions thrown inside `doTry` are caught by `doCatch` clauses and
//! that `doFinally` runs unconditionally.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use camel_api::body::Body;
use camel_api::{BoxProcessor, BoxProcessorExt, CamelError, Exchange};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_api::test_support::acquire_deadline;
use camel_dsl::parse_yaml;
use camel_test::CamelTestContext;
use tower::ServiceExt;

fn test_rt() -> Arc<dyn camel_component_api::RuntimeObservability> {
    Arc::new(camel_component_api::NoOpComponentContext)
}

/// Build a producer for a `direct:` endpoint. The context-lock guard is
/// dropped when this returns, so callers can `.await` the oneshot send
/// without holding the lock (clippy::await_holding_lock).
async fn build_direct_producer(
    h: &CamelTestContext,
    endpoint_uri: &str,
    context_label: &str,
) -> BoxProcessor {
    let ctx = acquire_deadline(h.ctx(), context_label, Duration::from_secs(10)).await;
    let producer_ctx = ctx.producer_context();
    let registry = ctx.registry();
    let component = registry
        .get("direct")
        .expect("direct component not registered");
    let endpoint = component
        .create_endpoint(endpoint_uri, &*ctx)
        .expect("failed to create direct endpoint");
    endpoint
        .create_producer(test_rt(), &producer_ctx)
        .expect("failed to create direct producer")
}

/// Send an exchange to a `direct:` endpoint and expect success.
async fn send_to_direct(h: &CamelTestContext, endpoint_uri: &str, exchange: Exchange) {
    let producer = build_direct_producer(h, endpoint_uri, "camel context (send_to_direct)").await;
    producer
        .oneshot(exchange)
        .await
        .expect("failed to send exchange to direct endpoint");
}

/// Integration test: Handled disposition routes the recovered exchange downstream.
///
/// Pipeline: direct:handled → doTry(process(fail)) → doCatch(ProcessorError, Handled)
///           → process(set body "caught") → endDoCatch → endDoTry → mock:handled-result
///
/// Expected: mock:handled-result receives 1 exchange with body "caught".
#[tokio::test(flavor = "multi_thread")]
async fn do_try_handled_routes_recovered_exchange_downstream() {
    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .build()
        .await;

    let route = RouteBuilder::from("direct:handled")
        .route_id("do-try-handled")
        .do_try()
        .process(BoxProcessor::from_fn(|_ex: Exchange| {
            Box::pin(async { Err(CamelError::ProcessorError("boom".into())) })
        }))
        .do_catch_exception(&["ProcessorError"])
        .handled()
        .process(BoxProcessor::from_fn(|mut ex: Exchange| {
            Box::pin(async move {
                ex.input.body = Body::Text("caught".into());
                Ok(ex)
            })
        }))
        .end_do_catch()
        .end_do_try()
        .to("mock:handled-result")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;
    // Allow spawned consumer tasks to register their endpoints.
    // DirectConsumer::start() registers synchronously inside a tokio::spawn;
    // on slow CI runners the task may not be scheduled before send_to_direct.
    tokio::time::sleep(Duration::from_millis(50)).await;

    send_to_direct(&h, "direct:handled", Exchange::default()).await;
    tokio::time::sleep(Duration::from_millis(200)).await;

    let endpoint = h.mock().get_endpoint("handled-result").unwrap();
    endpoint.assert_exchange_count(1).await;
    let received = endpoint.get_received_exchanges().await;
    assert_eq!(received.len(), 1);
    assert_eq!(received[0].input.body.as_text(), Some("caught"));

    h.stop().await;
}

/// Integration test: doFinally runs even when catch recovers the exchange.
///
/// Pipeline: direct:finally → doTry(process(fail)) → doCatch(ProcessorError, Handled)
///           → process(set body "caught") → endDoCatch
///           → doFinally → process(increment counter) → endDoFinally → endDoTry
///           → mock:finally-result
///
/// Expected: mock:finally-result receives 1 exchange, finally counter == 1.
#[tokio::test(flavor = "multi_thread")]
async fn do_try_finally_runs_after_handled_catch() {
    let finally_counter = Arc::new(AtomicU32::new(0));
    let counter_clone = finally_counter.clone();

    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .build()
        .await;

    let route = RouteBuilder::from("direct:finally")
        .route_id("do-try-finally")
        .do_try()
        .process(BoxProcessor::from_fn(|_ex: Exchange| {
            Box::pin(async { Err(CamelError::ProcessorError("boom".into())) })
        }))
        .do_catch_exception(&["ProcessorError"])
        .handled()
        .process(BoxProcessor::from_fn(|mut ex: Exchange| {
            Box::pin(async move {
                ex.input.body = Body::Text("caught".into());
                Ok(ex)
            })
        }))
        .end_do_catch()
        .do_finally()
        .unwrap()
        .process(BoxProcessor::from_fn(move |ex: Exchange| {
            let c = counter_clone.clone();
            Box::pin(async move {
                c.fetch_add(1, Ordering::SeqCst);
                Ok(ex)
            })
        }))
        .end_do_finally()
        .end_do_try()
        .to("mock:finally-result")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    send_to_direct(&h, "direct:finally", Exchange::default()).await;
    tokio::time::sleep(Duration::from_millis(200)).await;

    assert_eq!(
        finally_counter.load(Ordering::SeqCst),
        1,
        "doFinally must run after Handled catch recovers the exchange"
    );

    let endpoint = h.mock().get_endpoint("finally-result").unwrap();
    endpoint.assert_exchange_count(1).await;
    let received = endpoint.get_received_exchanges().await;
    assert_eq!(received.len(), 1);
    assert_eq!(
        received[0].input.body.as_text(),
        Some("caught"),
        "catch branch must have set body to 'caught' before mock received"
    );

    h.stop().await;
}

/// Integration test: Propagate disposition rethrows after catch side-effects.
///
/// Pipeline: direct:propagate → doTry(process(fail)) → doCatch(ProcessorError, Propagate)
///           → process(set body "should-not-reach-downstream") → endDoCatch → endDoTry
///           → mock:propagate-result
///
/// Expected: mock:propagate-result receives 0 exchanges (error propagates, route aborts).
#[tokio::test(flavor = "multi_thread")]
async fn do_try_propagate_does_not_reach_downstream() {
    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .build()
        .await;

    let route = RouteBuilder::from("direct:propagate")
        .route_id("do-try-propagate")
        .do_try()
        .process(BoxProcessor::from_fn(|_ex: Exchange| {
            Box::pin(async { Err(CamelError::ProcessorError("boom".into())) })
        }))
        .do_catch_exception(&["ProcessorError"])
        .propagate()
        .process(BoxProcessor::from_fn(|mut ex: Exchange| {
            Box::pin(async move {
                ex.input.body = Body::Text("should-not-reach-downstream".into());
                Ok(ex)
            })
        }))
        .end_do_catch()
        .end_do_try()
        .to("mock:propagate-result")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Manually send via the producer — send_to_direct uses .expect() internally,
    // but the Propagate disposition rethrows the original error, so oneshot returns
    // an Err which must not panic.
    let producer = {
        let ctx = h.ctx().lock().await;
        let producer_ctx = ctx.producer_context();
        let registry = ctx.registry();
        let component = registry
            .get("direct")
            .expect("direct component not registered");
        let endpoint = component
            .create_endpoint("direct:propagate", &*ctx)
            .expect("failed to create direct endpoint");
        endpoint
            .create_producer(test_rt(), &producer_ctx)
            .expect("failed to create direct producer")
    };
    let _ = producer.oneshot(Exchange::default()).await;

    tokio::time::sleep(Duration::from_millis(200)).await;

    let endpoint = h.mock().get_endpoint("propagate-result").unwrap();
    // Propagate disposition rethrows the original error, so mock:propagate-result
    // should receive 0 exchanges (the route aborts at the doTry scope).
    endpoint.assert_exchange_count(0).await;

    h.stop().await;
}

/// Integration test: YAML doTry with `on_when` Simple predicate filters catch execution.
///
/// Verifies the full YAML→compile→runtime pipeline for `on_when`:
///   YAML text → RouteDslStep::DoTry → DeclarativeStep::DoTry → BuilderStep::DeclarativeDoTry
///   → camel-core resolves the Simple predicate via language registry → DoTryService
///   evaluates predicate at runtime to gate the catch clause.
///
/// Setup (2 routes):
///   - yaml-on-when-route (loaded from YAML): sets body to "fail", then doTry calls
///     direct:failing-step. Catch matches ProcessorError + `on_when: "${body} == 'fail'"`
///     → on match (this case), catch runs and routes to mock:yaml-result.
///   - failing-step (Rust builder): always throws ProcessorError. Required because YAML
///     has no inline processor step; the failure is injected via a real component call.
///
/// Expected: mock:yaml-result receives 1 exchange (predicate matched, catch ran).
#[tokio::test(flavor = "multi_thread")]
async fn do_try_yaml_on_when_predicate_filters_catch_e2e() {
    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .build()
        .await;

    // Failure-injection route: YAML has no inline processor step, so chain to a
    // direct endpoint backed by a Rust builder route that always throws.
    let failing_route = RouteBuilder::from("direct:failing-step")
        .route_id("failing-step")
        .process_fn(BoxProcessor::from_fn(|_ex: Exchange| {
            Box::pin(async { Err(CamelError::ProcessorError("boom".into())) })
        }))
        .build()
        .unwrap();
    h.add_route(failing_route).await.unwrap();

    let yaml = r#"
routes:
  - id: "yaml-on-when-route"
    from: "direct:yaml-on-when"
    steps:
      - set_body: "fail"
      - do_try:
          steps:
            - to: "direct:failing-step"
          catch:
            - exception: ["ProcessorError"]
              on_when: "${body} == 'fail'"
              disposition: handled
              steps:
                - to: "mock:yaml-result"
"#;
    let yaml_routes = parse_yaml(yaml).expect("YAML parse failed");
    assert_eq!(
        yaml_routes.len(),
        1,
        "YAML must produce exactly one route definition"
    );
    for route in yaml_routes {
        h.add_route(route).await.unwrap();
    }

    h.start().await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    send_to_direct(&h, "direct:yaml-on-when", Exchange::default()).await;
    tokio::time::sleep(Duration::from_millis(200)).await;

    let endpoint = h.mock().get_endpoint("yaml-result").unwrap();
    endpoint.assert_exchange_count(1).await;
    let received = endpoint.get_received_exchanges().await;
    assert_eq!(received.len(), 1);
    // Body is "fail" (set by set_body before doTry) — catch ran because predicate matched.
    assert_eq!(
        received[0].input.body.as_text(),
        Some("fail"),
        "predicate `${{body}} == 'fail'` matched → catch ran → body unchanged through to mock"
    );

    h.stop().await;
}

/// Integration test: doTry Propagate disposition routes error to route-level onException.
///
/// Verifies the interaction between doTry's Propagate disposition and a global
/// `on_exception` clause declared on the same route. When doTry catches and rethrows
/// (Propagate), the rethrown error must reach the route's onException handler rather
/// than escaping silently or looping back into doTry.
///
/// Pipeline:
///   direct:propagate-to-on-exception
///     → on_exception(ProcessorError).handled_by("mock:on-exception-caught")
///     → doTry(process(fail))
///       → doCatch(ProcessorError, Propagate).process(set body "should-not-reach")
///     → to("mock:after-do-try")
///
/// Expected:
///   - mock:after-do-try receives 0 (doTry propagated, main pipeline aborted)
///   - mock:on-exception-caught receives 1 (onException handler routed the failed exchange)
#[tokio::test(flavor = "multi_thread")]
async fn do_try_propagate_reaches_route_on_exception() {
    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .build()
        .await;

    let route = RouteBuilder::from("direct:propagate-to-on-exception")
        .route_id("propagate-to-on-exception")
        .on_exception(|e: &CamelError| matches!(e, CamelError::ProcessorError(_)))
        .handled_by("mock:on-exception-caught")
        .end_on_exception()
        .do_try()
        .process(BoxProcessor::from_fn(|_ex: Exchange| {
            Box::pin(async { Err(CamelError::ProcessorError("boom".into())) })
        }))
        .do_catch_exception(&["ProcessorError"])
        .propagate()
        .process(BoxProcessor::from_fn(|mut ex: Exchange| {
            Box::pin(async move {
                ex.input.body = Body::Text("should-not-reach-downstream".into());
                Ok(ex)
            })
        }))
        .end_do_catch()
        .end_do_try()
        .to("mock:after-do-try")
        .build()
        .unwrap();

    h.add_route(route).await.unwrap();
    h.start().await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Propagate disposition rethrows the original error; oneshot returns Err which is
    // caught by the route-level onException. Don't use send_to_direct (it .expect()s Ok).
    let producer = {
        let ctx = h.ctx().lock().await;
        let producer_ctx = ctx.producer_context();
        let registry = ctx.registry();
        let component = registry
            .get("direct")
            .expect("direct component not registered");
        let endpoint = component
            .create_endpoint("direct:propagate-to-on-exception", &*ctx)
            .expect("failed to create direct endpoint");
        endpoint
            .create_producer(test_rt(), &producer_ctx)
            .expect("failed to create direct producer")
    };
    let _ = producer.oneshot(Exchange::default()).await;

    tokio::time::sleep(Duration::from_millis(200)).await;

    let after_do_try = h.mock().get_endpoint("after-do-try").unwrap();
    after_do_try.assert_exchange_count(0).await;

    let on_exception_caught = h.mock().get_endpoint("on-exception-caught").unwrap();
    on_exception_caught.assert_exchange_count(1).await;

    h.stop().await;
}

// ---------------------------------------------------------------------------
// doTry catch-block failure envelope (openspec/dotryorig task 1.3)
// ---------------------------------------------------------------------------

/// `true` when the error is the direct component's "consumer not yet
/// registered" rejection: the route consumer's registration task runs in a
/// `tokio::spawn` and may lag `start()` on slow CI runners.
fn direct_not_registered(err: &CamelError) -> bool {
    matches!(
        err,
        CamelError::EndpointCreationFailed(msg)
            if msg.starts_with("direct endpoint '") && msg.ends_with("' not registered")
    )
}

/// Send to a `direct:` endpoint, retrying while the route consumer's spawned
/// registration task has not yet registered the endpoint (the context lock is
/// released between attempts so registration can proceed). Returns the raw
/// oneshot result — unlike [`send_to_direct`] this never panics on a failed
/// pipeline, and it needs no post-start sleep (lint-test-sleep hygiene).
async fn send_when_registered(
    h: &CamelTestContext,
    endpoint_uri: &str,
    exchange: Exchange,
) -> Result<Exchange, CamelError> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        // build_direct_producer drops the context lock guard before
        // returning, so the oneshot await below never holds the lock
        // (clippy::await_holding_lock).
        // Bounded by the same 5s deadline: a hung producer build or
        // oneshot must not stall the retry loop forever
        // (lint-unbounded-wait).
        let attempt = match tokio::time::timeout_at(deadline, async {
            let producer =
                build_direct_producer(h, endpoint_uri, "camel context (send_when_registered)")
                    .await;
            producer.oneshot(exchange.clone()).await
        })
        .await
        {
            Ok(result) => result,
            Err(_elapsed) => {
                return Err(CamelError::ProcessorError(format!(
                    "direct endpoint '{endpoint_uri}': send_when_registered deadline elapsed"
                )));
            }
        };
        match attempt {
            Err(err) if direct_not_registered(&err) && tokio::time::Instant::now() < deadline => {
                tokio::task::yield_now().await;
            }
            other => return other,
        }
    }
}

/// Shared in-memory sink for the JSON log-capture subscriber used by the
/// compensation test: `tracing-subscriber`'s fmt layer writes each record as
/// one JSON line into this buffer.
#[derive(Clone)]
struct JsonLogBuffer(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for JsonLogBuffer {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for JsonLogBuffer {
    type Writer = JsonLogBuffer;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// Integration test (dotryorig task 1.3): a failing doTry catch body
/// (exception translation) routes by the CATCH error kind at the route-level
/// error handler.
///
/// Pipeline:
///   direct:translation (YAML, compiled route)
///     → error_handler.on_exceptions:
///         ProcessorError → direct:domain-shaper → mock:domain-caught
///         Io             → direct:io-shaper      → mock:io-caught
///     → doTry(to: direct:io-failing-step)  — body fails Io("orig-io")
///       → doCatch(["Io"], handled, to: direct:translating-step)
///         — catch body fails ProcessorError("translated-domain")
///     → to: mock:after-do-try
///
/// Expected (catch error supersedes the original as the main error):
///   - mock:domain-caught receives 1 (the ProcessorError clause fired)
///   - mock:io-caught receives 0 (the Io clause must NOT fire — kind
///     matching sees the catch error, not the original Io)
///   - mock:after-do-try receives 0 (handled_by absorbs; route aborts)
///
/// The oneshot result is intentionally NOT asserted on the kind: route-level
/// handlers absorb the error by design (see
/// `do_try_propagate_reaches_route_on_exception` for the same pattern).
#[tokio::test(flavor = "multi_thread")]
async fn do_try_catch_translation_route_matches_catch_kind() {
    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .build()
        .await;

    // Failure injection: YAML has no inline throw, so both the try body and
    // the translating catch body are real builder routes behind direct:.
    let io_failing = RouteBuilder::from("direct:io-failing-step")
        .route_id("io-failing-step")
        .process_fn(BoxProcessor::from_fn(|_ex: Exchange| {
            Box::pin(async { Err(CamelError::Io("orig-io".into())) })
        }))
        .build()
        .unwrap();
    h.add_route(io_failing).await.unwrap();

    let translating = RouteBuilder::from("direct:translating-step")
        .route_id("translating-step")
        .process_fn(BoxProcessor::from_fn(|_ex: Exchange| {
            Box::pin(async { Err(CamelError::ProcessorError("translated-domain".into())) })
        }))
        .build()
        .unwrap();
    h.add_route(translating).await.unwrap();

    // The two route-level shaper targets, each forwarding to a distinct mock.
    let domain_shaper = RouteBuilder::from("direct:domain-shaper")
        .route_id("domain-shaper")
        .to("mock:domain-caught")
        .build()
        .unwrap();
    h.add_route(domain_shaper).await.unwrap();

    let io_shaper = RouteBuilder::from("direct:io-shaper")
        .route_id("io-shaper")
        .to("mock:io-caught")
        .build()
        .unwrap();
    h.add_route(io_shaper).await.unwrap();

    let yaml = r#"
routes:
  - id: "translation"
    from: "direct:translation"
    error_handler:
      on_exceptions:
        - kind: "ProcessorError"
          handled: true
          handled_by: "direct:domain-shaper"
        - kind: "Io"
          handled: true
          handled_by: "direct:io-shaper"
    steps:
      - do_try:
          steps:
            - to: "direct:io-failing-step"
          catch:
            - exception: ["Io"]
              disposition: handled
              steps:
                - to: "direct:translating-step"
      - to: "mock:after-do-try"
"#;
    for route in parse_yaml(yaml).expect("YAML parse failed") {
        h.add_route(route).await.unwrap();
    }

    h.start().await;

    let outcome = send_when_registered(&h, "direct:translation", Exchange::default()).await;
    assert!(
        outcome.is_ok(),
        "route-level handlers absorb the catch error, got: {outcome:?}"
    );

    let domain_caught = h.mock().get_endpoint("domain-caught").unwrap();
    domain_caught
        .await_exchanges(1, Duration::from_secs(5))
        .await;
    domain_caught.assert_exchange_count(1).await;

    // The Io clause must NOT fire: kind matching sees the CATCH error.
    h.mock()
        .get_endpoint("io-caught")
        .unwrap()
        .assert_exchange_count(0)
        .await;
    // handled_by absorbs the error; the main pipeline never continues.
    h.mock()
        .get_endpoint("after-do-try")
        .unwrap()
        .assert_exchange_count(0)
        .await;

    h.stop().await;
}

/// Async body of the compensation-log test, split out so the sync test fn can
/// wrap the whole harness lifetime in the capturing subscriber's scope.
///
/// Builder route with NO route-level on_exception: the doTry body fails with
/// the original `Io("orig-io-lost")`, the matching catch body then fails with
/// `ProcessorError("compensation-down")` (Handled) — the catch error must be
/// the one propagated out of the route.
async fn run_compensation_route() -> Result<Exchange, CamelError> {
    let h = CamelTestContext::builder().with_direct().build().await;

    let route = RouteBuilder::from("direct:compensation")
        .route_id("compensation")
        .do_try()
        .process(BoxProcessor::from_fn(|_ex: Exchange| {
            Box::pin(async { Err(CamelError::Io("orig-io-lost".into())) })
        }))
        .do_catch_exception(&["Io"])
        .handled()
        .process(BoxProcessor::from_fn(|_ex: Exchange| {
            Box::pin(async { Err(CamelError::ProcessorError("compensation-down".into())) })
        }))
        .end_do_catch()
        .end_do_try()
        .build()
        .unwrap();
    h.add_route(route).await.unwrap();
    h.start().await;

    let result = send_when_registered(&h, "direct:compensation", Exchange::default()).await;
    h.stop().await;
    result
}

/// Integration test (dotryorig task 1.3): when the doTry catch body fails,
/// the CATCH error is the one returned AND the unconditional warn record
/// carries BOTH errors structured (`original_error` and `catch_error`).
///
/// Log capture mirrors camel-processor's
/// `delegate_failure_emits_system_broken_log_and_span_error`: a fmt-json
/// subscriber (WARN+) writing into a shared buffer, installed as a
/// thread-local scoped default around a current-thread-runtime block_on of
/// the whole route run.
#[test]
fn do_try_catch_failure_compensation_route_logs_original() {
    let buffer = JsonLogBuffer(Arc::new(Mutex::new(Vec::new())));
    let subscriber = tracing_subscriber::fmt()
        .json()
        .with_max_level(tracing_subscriber::filter::LevelFilter::WARN)
        .without_time()
        .with_writer(buffer.clone())
        .finish();

    let result = {
        use tracing_subscriber::util::SubscriberInitExt;
        // Thread-local scoped default (same semantics as
        // `tracing::subscriber::with_default`, RAII-guard form) — camel-test
        // has no direct `tracing` dependency to name the fn itself.
        let _guard = subscriber.set_default();
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("current-thread runtime")
            .block_on(run_compensation_route())
    };

    match &result {
        Err(CamelError::ProcessorError(m)) if m == "compensation-down" => {}
        other => panic!(
            "expected Err with the CATCH ProcessorError(\"compensation-down\"), got {other:?}"
        ),
    }

    let raw = buffer
        .0
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    assert!(!raw.is_empty(), "expected captured WARN+ log lines");
    let records: Vec<serde_json::Value> = raw
        .split(|&b| b == b'\n')
        .filter(|line| !line.iter().all(|b| b.is_ascii_whitespace()))
        .map(|line| {
            serde_json::from_slice(line).unwrap_or_else(|e| {
                panic!(
                    "captured log line is not JSON ({e}): {}",
                    String::from_utf8_lossy(line)
                )
            })
        })
        .collect();

    // fmt-json records may nest the event fields (including `message`)
    // under a top-level "fields" object; check the flat top-level key
    // first, then fall back to the nested "fields" object.
    fn field_str<'a>(record: &'a serde_json::Value, name: &str) -> Option<&'a str> {
        record.get(name).and_then(|v| v.as_str()).or_else(|| {
            record
                .get("fields")
                .and_then(|f| f.get(name))
                .and_then(|v| v.as_str())
        })
    }

    let record = records
        .iter()
        .find(|r| {
            field_str(r, "message")
                .is_some_and(|m| m.contains("do_try catch block failed"))
        })
        .unwrap_or_else(|| {
            panic!(
                "no record with message 'do_try catch block failed' among {} captured records: {records:?}",
                records.len()
            )
        });
    assert_eq!(
        record.get("level").and_then(|l| l.as_str()),
        Some("WARN"),
        "the catch-failure envelope record must be warn-level"
    );
    let original_error = field_str(record, "original_error")
        .unwrap_or_else(|| panic!("record missing original_error field: {record}"));
    let catch_error = field_str(record, "catch_error")
        .unwrap_or_else(|| panic!("record missing catch_error field: {record}"));
    assert!(
        original_error.contains("orig-io-lost"),
        "original_error must carry the ORIGINAL Io error, got: {original_error}"
    );
    assert!(
        catch_error.contains("compensation-down"),
        "catch_error must carry the CATCH error, got: {catch_error}"
    );
}
