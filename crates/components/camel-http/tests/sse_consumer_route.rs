//! Route-level e2e: the `sse:` consumer driven through a real `CamelContext`.
//!
//! These tests cover the wiring the component-local suite cannot: `HttpBundle`
//! registers the `sse:` scheme, the context builds and starts a route whose
//! consumer is `Explicit` with a background task handle (the rc-2vgej startup
//! shape: the runtime keeps the consumer alive for the whole context
//! lifetime), and a consumer task error surfaces as an ADR-0007 route failure
//! while `stop()` still tears the context down.
//!
//! No infrastructure is involved: every peer is the in-process
//! [`HttpTestServer`](camel_component_api::test_support::http::HttpTestServer).

use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use camel_api::Body;
use camel_api::Exchange;
use camel_api::ExchangePattern;
use camel_builder::RouteBuilder;
use camel_builder::StepAccumulator;
use camel_component_api::ComponentBundle;
use camel_component_api::test_support::http::HttpTestServer;
use camel_component_api::test_support::http::Responder;
use camel_component_api::test_support::http::Response;
use camel_component_api::test_support::http::ResponseHeaders;
use camel_component_http::HttpBundle;
use camel_core::CamelContext;
use tokio::sync::mpsc;

/// Route id the tests query for lifecycle status.
const ROUTE_ID: &str = "sse-route-e2e";
/// Deadline for events to reach the sink and for lifecycle transitions.
const FLOW_BOUND: Duration = Duration::from_secs(15);
/// Deadline for a context stop() the task requires to be prompt.
const STOP_BOUND: Duration = Duration::from_secs(2);

/// Sink shared between the route's processor step and the test body.
type Sink = Arc<Mutex<Vec<Exchange>>>;

fn sse_response_headers() -> ResponseHeaders {
    ResponseHeaders::new(
        200,
        "OK",
        vec![("Content-Type".to_string(), "text/event-stream".to_string())],
    )
}

/// Builds and registers the route: bundle → `from("sse://…")` → processor sink.
///
/// The context is returned unstarted so each test shapes the peer and the
/// lifecycle assertions around it.
async fn build_sse_route(port: u16, query: &str) -> (CamelContext, Sink) {
    let sink: Sink = Arc::new(Mutex::new(Vec::new()));
    let mut ctx = CamelContext::builder().build().await.expect("context");

    let bundle = HttpBundle::from_toml(toml::Value::Table(toml::map::Map::new()))
        .expect("empty http bundle toml must use defaults");
    bundle.register_all(&mut ctx);

    let sink_for_step = Arc::clone(&sink);
    let route = RouteBuilder::from(&format!("sse://127.0.0.1:{port}/events?{query}"))
        .route_id(ROUTE_ID)
        .process(move |ex: Exchange| {
            let sink = Arc::clone(&sink_for_step);
            async move {
                // The sink is an observation step: keep a copy and let the
                // exchange continue through the (terminal) route.
                sink.lock().expect("sink lock").push(ex.clone());
                Ok(ex)
            }
        })
        .build()
        .expect("route");

    ctx.add_route_definition(route)
        .await
        .expect("add route definition");
    (ctx, sink)
}

/// Waits (bounded) until the harness recorded `count` requests, i.e. until the
/// consumer has connected at least that many times.
async fn await_requests(server: &HttpTestServer, count: usize) {
    let deadline = tokio::time::Instant::now() + FLOW_BOUND;
    while server.recorded().len() < count {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the server never recorded {count} request(s)"
        );
        tokio::task::yield_now().await;
    }
}

/// Waits (bounded) for at least `count` exchanges in the sink, then drains it.
async fn drain_sink(sink: &Sink, count: usize) -> Vec<Exchange> {
    let deadline = tokio::time::Instant::now() + FLOW_BOUND;
    loop {
        {
            let mut guard = sink.lock().expect("sink lock");
            if guard.len() >= count {
                return std::mem::take(&mut *guard);
            }
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the sink never accumulated {count} exchange(s)"
        );
        tokio::task::yield_now().await;
    }
}

/// Polls the runtime read model until the route reaches `expected` (ADR-0007
/// failure surfacing is asynchronous with respect to `start()`).
async fn await_route_status(ctx: &CamelContext, expected: &str) {
    let deadline = tokio::time::Instant::now() + FLOW_BOUND;
    loop {
        let status = ctx
            .runtime_route_status(ROUTE_ID)
            .await
            .expect("route status query must succeed");
        if status.as_deref() == Some(expected) {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "route '{ROUTE_ID}' never reached '{expected}', last observed: {status:?}"
        );
        tokio::task::yield_now().await;
    }
}

/// Asserts `stop()` completes within `bound` instead of hanging.
async fn stop_bounded(ctx: &mut CamelContext, bound: Duration) {
    tokio::time::timeout(bound, ctx.stop())
        .await
        .expect("context stop must complete within its bound")
        .expect("context stop must succeed");
}

#[tokio::test]
async fn sse_route_one_exchange_per_event_end_to_end() {
    let (chunk_tx, chunk_rx) = mpsc::channel(8);
    let server = HttpTestServer::start(Responder::Fixed(Response::staged(
        sse_response_headers(),
        chunk_rx,
    )))
    .await
    .expect("test server");
    for payload in ["one", "two", "three"] {
        chunk_tx
            .send(format!("data: {payload}\n\n").into_bytes())
            .await
            .expect("event chunk queued");
    }

    let (mut ctx, sink) =
        build_sse_route(server.port(), "allowInternal=true&retryInitialMs=1000").await;
    ctx.start().await.expect("start context");

    let exchanges = drain_sink(&sink, 3).await;
    assert_eq!(exchanges.len(), 3, "one exchange per event, no more");

    let bodies: Vec<Option<String>> = exchanges
        .iter()
        .map(|ex| match &ex.input.body {
            Body::Text(text) => Some(text.clone()),
            other => panic!("expected Body::Text from an SSE event, got {other:?}"),
        })
        .collect();
    assert_eq!(
        bodies,
        vec![
            Some("one".to_string()),
            Some("two".to_string()),
            Some("three".to_string())
        ],
        "the sink must observe the events in stream order"
    );
    for (index, exchange) in exchanges.iter().enumerate() {
        assert_eq!(
            exchange.pattern,
            ExchangePattern::InOnly,
            "exchange {index} must be dispatched InOnly"
        );
    }

    // The stream is still open: stopping must cancel the read, not wait for EOF.
    stop_bounded(&mut ctx, STOP_BOUND).await;
    drop(chunk_tx);
}

#[tokio::test]
async fn sse_route_204_terminal_fails_route_and_stops_cleanly() {
    let server = HttpTestServer::start(Responder::Fixed(Response::with_status(
        204,
        "No Content",
        Vec::new(),
    )))
    .await
    .expect("test server");

    let (mut ctx, sink) =
        build_sse_route(server.port(), "allowInternal=true&retryInitialMs=1000").await;
    ctx.start().await.expect("start context");

    // A 204 is terminal: the consumer task errors and ADR-0007 supervision
    // projects the route to Failed.
    await_route_status(&ctx, "Failed").await;
    assert!(
        sink.lock().expect("sink lock").is_empty(),
        "a 204 response must never dispatch an exchange"
    );

    stop_bounded(&mut ctx, STOP_BOUND).await;
}

#[tokio::test]
async fn sse_route_5xx_retried_events_flow() {
    let (chunk_tx, chunk_rx) = mpsc::channel(8);
    chunk_tx
        .send(b"data: after-retry\n\n".to_vec())
        .await
        .expect("event chunk queued");
    let stream_rx = Arc::new(Mutex::new(Some(chunk_rx)));
    let served_5xx = Arc::new(Mutex::new(false));
    let server = HttpTestServer::start(Responder::route({
        let stream_rx = Arc::clone(&stream_rx);
        let served_5xx = Arc::clone(&served_5xx);
        move |_request| {
            let mut served = served_5xx.lock().expect("5xx flag");
            if !*served {
                *served = true;
                return Response::with_status(503, "Service Unavailable", Vec::new());
            }
            match stream_rx.lock().expect("stream slot").take() {
                Some(rx) => Response::staged(sse_response_headers(), rx),
                None => Response::with_status(503, "Service Unavailable", Vec::new()),
            }
        }
    }))
    .await
    .expect("test server");

    let (mut ctx, sink) =
        build_sse_route(server.port(), "allowInternal=true&retryInitialMs=1000").await;
    ctx.start().await.expect("start context");

    let exchanges = drain_sink(&sink, 1).await;
    assert_eq!(exchanges.len(), 1, "exactly one event after the retry");
    assert_eq!(
        exchanges[0].input.body,
        Body::Text("after-retry".to_string()),
        "the post-retry stream must reach the sink"
    );
    assert_eq!(exchanges[0].pattern, ExchangePattern::InOnly);
    assert!(
        server.recorded().len() >= 2,
        "the consumer must reconnect after the 503"
    );

    stop_bounded(&mut ctx, STOP_BOUND).await;
    drop(chunk_tx);
}

#[tokio::test]
async fn sse_route_stop_cancels_mid_read_and_mid_sleep() {
    // (a) Mid-read: headers written, stream open with no further bytes. The
    // sender stays alive, so the consumer is parked inside a body read.
    let (chunk_tx, chunk_rx) = mpsc::channel(1);
    let server = HttpTestServer::start(Responder::Fixed(Response::staged(
        sse_response_headers(),
        chunk_rx,
    )))
    .await
    .expect("test server");

    let (mut ctx, _sink) =
        build_sse_route(server.port(), "allowInternal=true&retryInitialMs=1000").await;
    ctx.start().await.expect("start context");
    await_requests(&server, 1).await;

    stop_bounded(&mut ctx, STOP_BOUND).await;
    drop(chunk_tx);

    // (b) Mid-sleep: every request answers 503, so the resubscription loop is
    // parked in its 60 s backoff. Cancellation must fire during that sleep,
    // never after it.
    let server = HttpTestServer::start(Responder::Fixed(Response::with_status(
        503,
        "Service Unavailable",
        Vec::new(),
    )))
    .await
    .expect("test server");

    let (mut ctx, _sink) = build_sse_route(
        server.port(),
        "allowInternal=true&retryInitialMs=60000&retryMaxMs=60000",
    )
    .await;
    ctx.start().await.expect("start context");
    await_requests(&server, 1).await;
    // Let the consumer consume the 503 and schedule the 60 s backoff (the
    // second request can only follow that sleep, so any observation here is
    // inside the backoff window).
    for _ in 0..64 {
        tokio::task::yield_now().await;
    }
    assert_eq!(
        server.recorded().len(),
        1,
        "only the first attempt may have run before cancellation"
    );

    stop_bounded(&mut ctx, STOP_BOUND).await;
}
