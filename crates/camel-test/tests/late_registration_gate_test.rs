//! Late-registration exposure gate E2E (`finish-auth-flip`, Task 1.4).
//!
//! The ADR-0061 late-registration arm of the per-bind exposure gate
//! (`DefaultRouteController::add_route` → `enforce_late_registration_gate`):
//! once a listener for a bind is RUNNING, registering another route onto
//! the same bind re-aggregates the complete sibling plan set and refuses
//! any Public route on a non-loopback address without operator
//! acknowledgement. Both arms drive real sockets end-to-end through
//! `CamelTestContext` + `HttpComponent` (no timer-fixture `from_uri`
//! swaps):
//!
//! * Loopback: a late Public route onto a running `127.0.0.1` bind
//!   registers Ok (loopback needs no acknowledgement) and serves 200
//!   through the already-running shared listener.
//! * Non-loopback: an Authenticated route on `0.0.0.0` (no ack needed —
//!   no Public sibling), readiness PROVEN by a valid-credential 200
//!   through `127.0.0.1` BEFORE the late registration; a late Public
//!   route onto that bind is refused with an error naming the bind
//!   address and the route id, and its path answers 404 — the listener
//!   is alive, so a connection refusal could not distinguish a refused
//!   gate from a dead listener.
//!
//! Requires `integration-tests` feature to compile and run.

#![cfg(feature = "integration-tests")]

mod support;
use support::find_free_port;
use support::install_crypto_provider;

use std::sync::Arc;
use std::time::Duration;

use camel_api::RuntimeCommand;
use camel_api::Value;
use camel_api::security_policy::{CredentialSource, SecurityPolicyConfig};
use camel_auth::{RolePolicy, TokenAuthenticator};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_http::HttpComponent;
use camel_test::{CamelTestContext, SecurityConfigFixture};

/// Plain Public HTTP route: body `"<body>"` → 200, then `mock:<sink>`.
fn public_route(
    from: String,
    route_id: &str,
    body: &str,
    sink: &str,
) -> camel_core::route::RouteDefinition {
    RouteBuilder::from(&from)
        .route_id(route_id)
        .set_body(Value::String(body.to_string()))
        .set_header("CamelHttpResponseCode", Value::Number(200.into()))
        .to(sink.to_string())
        .build()
        .unwrap()
}

/// Step 1: a late Public route onto a RUNNING loopback bind registers Ok
/// (no acknowledgement required on loopback) and serves a real request
/// with HTTP 200 through the already-running shared listener.
#[tokio::test(flavor = "multi_thread")]
async fn late_public_route_real_loopback_served_e2e() {
    install_crypto_provider();
    let port = find_free_port();

    let h = CamelTestContext::builder()
        .with_component(HttpComponent::new())
        .with_mock()
        .build()
        .await;

    // First route brings the 127.0.0.1 bind up (running shared listener).
    let first = public_route(
        format!("http://127.0.0.1:{port}/first"),
        "late-gate-loopback-first",
        "first-ok",
        "mock:first",
    );
    h.add_route(first).await.unwrap();
    h.start().await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Late registration of a Public route onto the same RUNNING bind.
    // Loopback binds need no exposure acknowledgement: registration Ok.
    let late = public_route(
        format!("http://127.0.0.1:{port}/late"),
        "late-gate-loopback-late",
        "late-ok",
        "mock:late",
    );
    h.add_route(late)
        .await
        .expect("late Public registration on a running loopback bind must be accepted");

    // Start ONLY the late route on the running context (controlbus
    // pattern): its consumer registers `/late` on the already-running
    // shared (host, port) server. The startup handshake signals AFTER
    // path registration, so the request below is dispatchable — no extra
    // sleep needed. A full second `h.start()` is not viable: the runtime
    // command bus refuses StartRoute for routes already Started.
    let runtime = {
        let ctx = h.ctx().lock().await;
        ctx.runtime()
    };
    runtime
        .execute(RuntimeCommand::StartRoute {
            route_id: "late-gate-loopback-late".to_string(),
            command_id: "test:start:late-gate-loopback-late".to_string(),
            causation_id: None,
        })
        .await
        .expect("late route must start on the running context");

    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://127.0.0.1:{port}/late"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        200,
        "late Public route on a running loopback bind must serve"
    );
    // The 200 came from the late route's own pipeline, not a fallback
    // (body is the JSON-serialized mock payload).
    assert_eq!(resp.text().await.unwrap(), "\"late-ok\"");
}

/// Step 2: an Authenticated route runs on `0.0.0.0` without any exposure
/// acknowledgement (authenticated routes need none). After readiness is
/// PROVEN by a valid-credential 200 served through `127.0.0.1`, a late
/// Public route onto the same bind is refused — the error names both the
/// bind address and the route id — and the refused route's path answers
/// 404 from the still-alive listener (a connection refusal could not be
/// accepted as proof: it would not distinguish a refused gate from a
/// dead listener).
#[tokio::test(flavor = "multi_thread")]
async fn late_public_route_real_nonloopback_refused_e2e() {
    install_crypto_provider();
    let port = find_free_port();

    let fixture = SecurityConfigFixture::single_static_provider("idp-e2e");
    let provider_registry = Arc::new(fixture.providers());
    let entry = provider_registry
        .resolve("idp-e2e")
        .expect("fixture provider"); // allow-unwrap
    let authenticator: Arc<dyn TokenAuthenticator> = Arc::clone(&entry.authenticator);

    let h = CamelTestContext::builder()
        .with_component(HttpComponent::new())
        .with_mock()
        .build()
        .await;

    let policy = RolePolicy::new(vec!["test-role".to_string()], true);
    let config = SecurityPolicyConfig::new(policy)
        .with_credential_sources(vec![CredentialSource::AuthorizationHeader]);

    let secured = RouteBuilder::from(&format!("http://0.0.0.0:{port}/secured"))
        .route_id("late-gate-nonloopback-secured")
        .security_policy(config)
        .security_authenticator(authenticator)
        .provider_registry(provider_registry)
        .set_body(Value::String("ok".into()))
        .set_header("CamelHttpResponseCode", Value::Number(200.into()))
        .to("mock:result".to_string())
        .build()
        .unwrap();
    h.add_route(secured).await.unwrap();
    h.start().await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Readiness proof BEFORE the late registration: a valid fixture
    // credential served through 127.0.0.1 must get 200. A refusal here
    // (401/connection error) could not prove the listener is genuinely
    // up, and the 404-equivalent check below depends on a live listener.
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://127.0.0.1:{port}/secured"))
        .header("Authorization", "Bearer test-token-idp-e2e")
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        200,
        "valid credential must be served through 127.0.0.1 — listener readiness proof"
    );

    // Late Public registration onto the RUNNING non-loopback bind without
    // acknowledgement: refused, and the error must name both the bind
    // address and the late route id.
    let late = public_route(
        format!("http://0.0.0.0:{port}/late-open"),
        "late-gate-nonloopback-late",
        "late-open",
        "mock:late-open",
    );
    let err = h
        .add_route(late)
        .await
        .expect_err("late Public registration on a running non-loopback bind must be refused");
    let msg = err.to_string();
    let bind = format!("0.0.0.0:{port}");
    assert!(
        msg.contains(&bind),
        "refusal must name the bind address {bind}: {msg}"
    );
    assert!(
        msg.contains("late-gate-nonloopback-late"),
        "refusal must name the late route id: {msg}"
    );

    // 404-equivalent from the STILL-ALIVE listener: the send itself must
    // succeed (a connection refusal would not distinguish a refused gate
    // from a dead listener) and the unregistered path must answer 404.
    let resp = client
        .get(format!("http://127.0.0.1:{port}/late-open"))
        .send()
        .await
        .expect("listener must still be alive after the refused registration");
    assert_eq!(
        resp.status(),
        404,
        "the refused route's path must not be registered on the running listener"
    );
}
