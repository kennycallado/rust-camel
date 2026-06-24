//! Integration tests for the doTry / doCatch / doFinally EIP.
//!
//! These tests exercise the full pipeline: CamelContext → Consumer → DoTryService → Mock,
//! verifying that exceptions thrown inside `doTry` are caught by `doCatch` clauses and
//! that `doFinally` runs unconditionally.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use camel_api::body::Body;
use camel_api::{BoxProcessor, BoxProcessorExt, CamelError, Exchange};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_dsl::parse_yaml;
use camel_test::CamelTestContext;
use tower::ServiceExt;

fn test_rt() -> Arc<dyn camel_component_api::RuntimeObservability> {
    Arc::new(camel_component_api::NoOpComponentContext)
}

/// Send an exchange to a `direct:` endpoint and expect success.
async fn send_to_direct(h: &CamelTestContext, endpoint_uri: &str, exchange: Exchange) {
    let producer = {
        let ctx = h.ctx().lock().await;
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
    };
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
