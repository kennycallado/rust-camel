//! Error-semantics e2e (dashboard-observability D2, spec
//! component-error-semantics "Circuit-breaker rejections are not errors").
//!
//! A route with a route-level circuit breaker (threshold 1) and a failing
//! downstream `to:direct:missing?failIfNoConsumers=false` leg: the first
//! exchange trips the breaker; subsequent exchanges are fast-failed at
//! readiness (poll_ready — the tracer adapter records nothing for them).
//! The scrape must show `camel_circuit_breaker_rejections_total{route}`
//! growing while `camel_errors_total` stays flat across the open-phase
//! sends.
//!
//! Harness idioms mirror `metrics_wiring_test.rs`: pre-allocated
//! prometheus port, direct-producer drive with startup-race retries,
//! startup-wait polling, poll-GET `/metrics`.

use std::net::TcpListener;
use std::time::Duration;

use camel_api::CircuitBreakerConfig;
use camel_api::{Exchange, Message};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_api::{NoOpComponentContext, RuntimeObservability};
use camel_component_direct::DirectComponent;
use camel_config::config::CamelConfig;
use camel_core::CamelContext;
use std::sync::Arc;
use tower::ServiceExt;

fn test_rt() -> Arc<dyn RuntimeObservability> {
    Arc::new(NoOpComponentContext)
}

/// Pre-allocate an ephemeral port (bind/read/drop) — the prometheus service
/// is constructed inside `configure_context`, so the port is otherwise
/// undiscoverable.
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
    ctx.register_component(DirectComponent::new());
    ctx
}

async fn route_started(ctx: &CamelContext, route_id: &str) -> bool {
    matches!(
        ctx.runtime_route_status(route_id).await,
        Ok(Some(status)) if status == "Started"
    )
}

async fn wait_for_started(ctx: &CamelContext, route_id: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !route_started(ctx, route_id).await {
        assert!(
            tokio::time::Instant::now() < deadline,
            "route {route_id} did not reach Started within 5s"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// One InOut send to `direct:cb-entry` (no retry: the route is Started, so
/// a reply — success or error — always arrives).
async fn send_one(ctx: &CamelContext) -> Result<Exchange, camel_api::CamelError> {
    let producer = {
        let producer_ctx = ctx.producer_context();
        let registry = ctx.registry();
        let component = registry
            .get("direct")
            .expect("direct component not registered");
        let endpoint = component
            .create_endpoint("direct:cb-entry", ctx)
            .expect("failed to create direct endpoint");
        endpoint
            .create_producer(test_rt(), &producer_ctx)
            .expect("failed to create direct producer")
    };
    producer
        .oneshot(Exchange::new_in_out(Message::new("cb-probe")))
        .await
}

/// Poll `GET /metrics` until the closed-phase exchange has landed (both the
/// exchange family and the genuine-failure error family present).
async fn poll_metrics_body(port: u16) -> String {
    let url = format!("http://127.0.0.1:{port}/metrics");
    let client = reqwest::Client::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(resp) = client.get(&url).send().await
            && resp.status().is_success()
            && let Ok(body) = resp.text().await
            && body.contains("camel_exchanges_total")
            && body.contains("camel_errors_total")
        {
            return body;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "prometheus /metrics never exposed exchange+error families at {url}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Poll `GET /metrics` until the rejection series for `route` has been
/// incremented at least once.
async fn poll_rejection_body(port: u16, route: &str) -> String {
    let url = format!("http://127.0.0.1:{port}/metrics");
    let client = reqwest::Client::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(resp) = client.get(&url).send().await
            && resp.status().is_success()
            && let Ok(body) = resp.text().await
            && rejection_value(&body, route).is_some_and(|v| v > 0)
        {
            return body;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "prometheus /metrics never exposed a positive rejection counter for {route} at {url}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Every rendered `camel_errors_total` series line (labels + value), sorted.
fn errors_series(body: &str) -> Vec<String> {
    let mut lines: Vec<String> = body
        .lines()
        .filter(|l| l.starts_with("camel_errors_total"))
        .map(str::to_string)
        .collect();
    lines.sort();
    lines
}

/// Value of `camel_circuit_breaker_rejections_total{route="<route>"}`, if
/// the series has been incremented yet.
fn rejection_value(body: &str, route: &str) -> Option<u64> {
    let needle = format!("camel_circuit_breaker_rejections_total{{route=\"{route}\"}} ");
    body.lines().find_map(|l| {
        l.strip_prefix(&needle)
            .and_then(|v| v.trim().parse::<u64>().ok())
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn open_breaker_end_to_end() {
    let port = prealloc_port();
    let toml_cfg = format!(
        r#"[observability.prometheus]
enabled = true
host = "127.0.0.1"
port = {port}
"#
    );

    let ctx = context_from_toml(&toml_cfg).await;
    // No error handler → old path: the breaker wraps the traced pipeline as
    // a Tower layer, so open-phase sends fast-fail in poll_ready.
    let route = RouteBuilder::from("direct:cb-entry")
        .route_id("cb-entry")
        .circuit_breaker(
            CircuitBreakerConfig::new()
                .failure_threshold(1)
                .open_duration(Duration::from_secs(60)),
        )
        .to("direct:missing?failIfNoConsumers=false")
        .build()
        .expect("breaker route builds");
    let mut ctx = ctx;
    ctx.add_route_definition(route)
        .await
        .expect("breaker route registers");
    ctx.start().await.expect("context starts");
    wait_for_started(&ctx, "cb-entry").await;

    // Closed phase: drive until the route reports the genuine downstream
    // failure (retries absorb startup races). Threshold 1 → the breaker
    // opens on this exchange.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(Err(_)) = tokio::time::timeout(Duration::from_secs(5), send_one(&ctx)).await {
            break; // genuine failure processed by the route
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "breaker route never reported the downstream failure"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // Baseline: the genuine failure is visible in errors_total.
    let baseline = poll_metrics_body(port).await;
    let errors_before = errors_series(&baseline);

    // Open phase: fire sends without waiting for replies. While the breaker
    // is open, `ready_with_backoff` treats the CircuitOpen readiness
    // rejection as retryable (1s backoff), so each send's FIRST poll
    // fast-fails in poll_ready and parks — the rejection is counted at that
    // fast-fail and the pipeline (tracer, downstream step) never runs, so
    // no error is recorded for the rejected exchange.
    for _ in 0..3 {
        let _ = tokio::time::timeout(Duration::from_millis(50), send_one(&ctx)).await;
    }

    let after = poll_rejection_body(port, "cb-entry").await;
    let rejections = rejection_value(&after, "cb-entry")
        .expect("camel_circuit_breaker_rejections_total{route=\"cb-entry\"} present");
    assert!(
        rejections > 0,
        "open-breaker sends must increment the rejection counter:\n{after}"
    );
    let errors_after = errors_series(&after);
    assert_eq!(
        errors_after, errors_before,
        "camel_errors_total must not grow during open-phase sends\nbefore:\n{errors_before:?}\nafter:\n{errors_after:?}"
    );

    ctx.stop().await.expect("context stops");
}
