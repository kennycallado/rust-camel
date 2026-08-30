//! Task 2.4 — server consumer endpoint: wiring registries to routes.
//!
//! Covers the consumer-side spec scenarios: bind refused without a security
//! policy (through the consumer), the per-bind exposure gate (Task 2.6:
//! non-loopback Public routes refuse without an operator ack and warn when
//! acked), tool invocations served through the route, stop unregisters,
//! resource URI registration, the default 128-tool cap rejecting the 129th
//! consumer start, and a raised cap allowing 150 tools on the shared
//! listener. The DSL listener-ownership tests (unify-transport-auth Task
//! 2.4) cover the `mcp.declared.*` channel: DSL TLS reaching the listener,
//! the TOML/DSL hard-conflict start failure, equal declarations proceeding,
//! TOML-only servers unchanged, and presence-based caps (a cap declared by
//! exactly one side is that side's runtime value; a cap declared by neither
//! keeps the 128 default). The crash regressions (fix-mcp-dead-registry-entry
//! Task 4) pin the crash windows: an aborted bridge plus a consumer dropped
//! without `stop()` must not wedge the shared listener — the same tool name
//! and resource URI restart, the dead entry vanishes from `tools/list`, and
//! a refused live duplicate leaves the incumbent's security plan intact.

mod common;

use std::collections::HashMap;
use std::sync::Arc;

use camel_api::security_policy::{AccessMode, RouteSecurityPlan, TransportId};
use camel_api::{Body, CamelError};
use camel_component_api::consumer::ExchangeEnvelope;
use camel_component_api::{
    Component, Consumer, ConsumerContext, NoOpComponentContext, NoopRuntimeObservability,
    RuntimeObservability, SecurityContext,
};
use camel_component_mcp::McpServerRegistry;
use camel_component_mcp::component::McpComponent;
use camel_component_mcp::config::{McpGlobalConfig, McpServerConfig, McpTlsConfig};
use camel_component_mcp::types::{McpResourceRead, McpToolInvocation};
use common::warn_capture;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

/// Trivial schema shared by the cap tests.
const TRIVIAL_SCHEMA: &str = r#"{"type":"object"}"#;

fn rt() -> Arc<dyn RuntimeObservability> {
    Arc::new(NoopRuntimeObservability)
}

/// URL-encode a value for a query parameter (the DSL lowering channel).
fn encoded(value: &str) -> String {
    percent_encoding::utf8_percent_encode(value, percent_encoding::NON_ALPHANUMERIC).to_string()
}

fn server_config(bind: &str, policy: bool, max_tools: Option<usize>) -> McpServerConfig {
    let mut json = serde_json::json!({ "bind": bind });
    if policy {
        json["security_policy"] = serde_json::json!({ "require": "auth" });
    }
    if let Some(max_tools) = max_tools {
        json["max_tools"] = serde_json::json!(max_tools);
    }
    serde_json::from_value(json).expect("valid server config")
}

fn component_with_server(name: &str, cfg: McpServerConfig) -> McpComponent {
    let mut servers = HashMap::new();
    servers.insert(name.to_string(), cfg);
    McpComponent::new(McpGlobalConfig {
        servers,
        remotes: HashMap::new(),
    })
}

/// Create the consumer for a consumer-shaped URI (endpoint + consumer).
fn consumer_for(component: &McpComponent, uri: &str) -> Box<dyn Consumer> {
    let endpoint = component
        .create_endpoint(uri, &NoOpComponentContext)
        .expect("endpoint creation must succeed");
    endpoint
        .create_consumer(rt())
        .expect("consumer creation must succeed")
}

/// A hand-owned route channel: the test acts as the route's pipeline.
fn test_context() -> (ConsumerContext, mpsc::Receiver<ExchangeEnvelope>) {
    let (tx, rx) = mpsc::channel::<ExchangeEnvelope>(16);
    let ctx = ConsumerContext::new(tx, CancellationToken::new(), "test-route".to_string());
    (ctx, rx)
}

#[tokio::test]
async fn consumer_start_without_security_policy_starts_on_loopback() {
    // Task 2.9 removed the component-local `security_policy` presence gate
    // (ADR-0061 Rule 9 supersedes ADR-0060 Rule 8): a policy-less server
    // classifies Public, and a loopback bind permits Public silently
    // (ADR-0061 Rule 4). The non-loopback refusal now comes from the kernel
    // exposure gate — see `mcp_bind_gate_refuses_public_without_ack` and
    // `mcp_old_bind_gate_removed` in server_config_test.rs.
    let component = component_with_server("nosec", server_config("127.0.0.10:0", false, None));
    let mut consumer = consumer_for(
        &component,
        &format!("mcp:nosec/tool/t?schema={}", encoded(TRIVIAL_SCHEMA)),
    );

    let (ctx, _route_rx) = test_context();
    consumer
        .start(ctx)
        .await
        .expect("loopback policy-less server must start (Public, gated per bind)");
    consumer.stop().await.expect("clean stop");
}

#[tokio::test]
async fn mcp_bind_gate_refuses_public_without_ack() {
    // Task 2.6: the ADR-0061 per-bind exposure gate replaces the former
    // warn-only non-loopback advisory. A route with no route-level security
    // registers a default Public plan (ADR-0061 Rule 4), so a non-loopback
    // bind refuses to start without the operator's ack; an acknowledged bind
    // starts and warns permanently (mirrors the camel-core gate tests).
    let bind = "0.0.0.0:0";
    let captures = warn_capture();
    let component = component_with_server("exposed", server_config(bind, true, None));
    let mut consumer = consumer_for(
        &component,
        &format!("mcp:exposed/tool/t?schema={}", encoded(TRIVIAL_SCHEMA)),
    );

    // No acknowledgement: refusal naming the bind and the route.
    let (ctx, _route_rx) = test_context();
    let err = consumer
        .start(ctx)
        .await
        .expect_err("non-loopback Public route without ack must refuse to start");
    assert!(
        matches!(err, CamelError::RouteError(ref message) if message.contains(bind)),
        "gate refusal must name the bind, got {err}"
    );
    assert!(
        matches!(err, CamelError::RouteError(ref message) if message.contains("test-route")),
        "gate refusal must name the route, got {err}"
    );
    assert_eq!(
        captures.warn_count_containing("public (unauthenticated) routes exposed"),
        0,
        "a refused start must not warn"
    );

    // Acknowledged: start proceeds and the gate warns (acknowledgement
    // never silences the warning, ADR-0052 rule 3).
    McpServerRegistry::global().set_bind_exposure_acks(HashMap::from([(bind.to_string(), true)]));
    let (ctx, _route_rx) = test_context();
    consumer
        .start(ctx)
        .await
        .expect("acknowledged non-loopback bind must start");
    assert_eq!(
        captures.warn_count_containing("public (unauthenticated) routes exposed"),
        1,
        "exactly one exposure warn on the acknowledged start"
    );
    consumer.stop().await.expect("clean stop");

    // Reset the process-global acks so sibling tests fail closed by default.
    McpServerRegistry::global().set_bind_exposure_acks(HashMap::new());
}

#[tokio::test]
async fn tool_consumer_serves_invocation() {
    let bind = "127.0.0.11:0";
    let cfg = server_config(bind, true, None);
    let component = component_with_server("crm", cfg.clone());

    let schema = r#"{"type":"object","properties":{"id":{"type":"string"}},"required":["id"]}"#;
    let mut consumer = consumer_for(
        &component,
        &format!("mcp:crm/tool/lookup?schema={}", encoded(schema)),
    );

    let (ctx, mut route_rx) = test_context();
    consumer.start(ctx).await.expect("tool consumer must start");

    // Resolve the registered sender and enqueue an invocation directly.
    let handle = McpServerRegistry::global()
        .get_or_spawn(bind, &cfg)
        .await
        .expect("shared listener");
    let entry = handle
        .tool_registry
        .resolve("lookup")
        .expect("tool registered");
    let (reply_tx, reply_rx) = oneshot::channel();
    entry
        .sender
        .send(McpToolInvocation {
            name: "lookup".to_string(),
            arguments: serde_json::json!({ "id": "42" }),
            headers: std::collections::HashMap::new(),
            principal: None,
            reply: reply_tx,
        })
        .await
        .expect("invocation enqueue");

    // The route: identity/set-body processor.
    let envelope = route_rx.recv().await.expect("route received the exchange");
    assert_eq!(
        envelope.exchange.input.body,
        Body::Json(serde_json::json!({ "id": "42" }))
    );
    let mut out = envelope.exchange;
    out.input.body = Body::Text("lookup-ok:42".to_string());
    envelope
        .reply_tx
        .expect("reply channel present")
        .send(Ok(out))
        .expect("route reply");

    let result = reply_rx.await.expect("tool result");
    assert_eq!(
        result.content,
        serde_json::Value::String("lookup-ok:42".to_string())
    );

    consumer.stop().await.expect("clean stop");
}

#[tokio::test]
async fn consumer_stop_unregisters() {
    let bind = "127.0.0.12:0";
    let cfg = server_config(bind, true, None);
    let component = component_with_server("crm", cfg.clone());

    let mut consumer = consumer_for(
        &component,
        &format!("mcp:crm/tool/lookup?schema={}", encoded(TRIVIAL_SCHEMA)),
    );
    let (ctx, _route_rx) = test_context();
    consumer.start(ctx).await.expect("tool consumer must start");
    consumer.stop().await.expect("clean stop");

    let handle = McpServerRegistry::global()
        .get_or_spawn(bind, &cfg)
        .await
        .expect("shared listener");
    assert!(
        handle.tool_registry.resolve("lookup").is_none(),
        "stop must unregister the tool"
    );
}

#[tokio::test]
async fn restart_after_clean_stop_succeeds() {
    // Owner-liveness wiring (fix-mcp-dead-registry-entry Task 1): a clean
    // stop unregisters the tool under the consumer's own owner token, so a
    // second consumer for the same tool name starts fresh on the same bind.
    let bind = "127.0.0.21:0";
    let cfg = server_config(bind, true, None);
    let component = component_with_server("crm", cfg);
    let uri = format!("mcp:crm/tool/t?schema={}", encoded(TRIVIAL_SCHEMA));

    let mut first = consumer_for(&component, &uri);
    let (ctx, _route_rx) = test_context();
    first.start(ctx).await.expect("first start must succeed");
    first.stop().await.expect("clean stop");

    let mut second = consumer_for(&component, &uri);
    let (ctx, _route_rx) = test_context();
    second
        .start(ctx)
        .await
        .expect("restart after a clean stop must succeed");
    second.stop().await.expect("clean stop");
}

#[tokio::test]
async fn resource_consumer_registers_uri() {
    let bind = "127.0.0.13:0";
    let cfg = server_config(bind, true, None);
    let component = component_with_server("crm", cfg.clone());

    let mut consumer = consumer_for(&component, "mcp:crm/resource/customers?uri=crm://customers");
    let (ctx, _route_rx) = test_context();
    consumer
        .start(ctx)
        .await
        .expect("resource consumer must start");

    let handle = McpServerRegistry::global()
        .get_or_spawn(bind, &cfg)
        .await
        .expect("shared listener");
    assert!(
        handle
            .resource_registry
            .list_ready()
            .contains(&"crm://customers".to_string()),
        "ready resource URIs must contain the declared URI, got {:?}",
        handle.resource_registry.list_ready()
    );

    consumer.stop().await.expect("clean stop");
}

#[tokio::test]
async fn consumer_start_rejects_129th_tool_with_camel_error() {
    let bind = "127.0.0.14:0";
    let cfg = server_config(bind, true, None);
    let component = component_with_server("crm", cfg.clone());
    let (ctx, _route_rx) = test_context();

    let mut consumers = Vec::new();
    for i in 0..128 {
        let uri = format!("mcp:crm/tool/t{i}?schema={}", encoded(TRIVIAL_SCHEMA));
        let mut consumer = consumer_for(&component, &uri);
        consumer
            .start(ctx.clone())
            .await
            .unwrap_or_else(|e| panic!("consumer {i} must start: {e}"));
        consumers.push(consumer);
    }

    let mut one_too_many = consumer_for(
        &component,
        &format!("mcp:crm/tool/t128?schema={}", encoded(TRIVIAL_SCHEMA)),
    );
    let err = one_too_many
        .start(ctx.clone())
        .await
        .expect_err("the 129th tool must exceed the default cap");
    assert!(
        matches!(err, CamelError::Config(ref message) if message.contains("cap exceeded") && message.contains("tools")),
        "expected Config(cap exceeded for tools), got {err}"
    );

    for mut consumer in consumers {
        consumer.stop().await.expect("clean stop");
    }
}

#[tokio::test]
async fn raised_cap_starts_150_tool_consumers() {
    let bind = "127.0.0.15:0";
    let cfg = server_config(bind, true, Some(200));
    let component = component_with_server("crm", cfg.clone());
    let (ctx, _route_rx) = test_context();

    let mut consumers = Vec::new();
    for i in 0..150 {
        let uri = format!("mcp:crm/tool/t{i}?schema={}", encoded(TRIVIAL_SCHEMA));
        let mut consumer = consumer_for(&component, &uri);
        consumer
            .start(ctx.clone())
            .await
            .unwrap_or_else(|e| panic!("consumer {i} must start: {e}"));
        consumers.push(consumer);
    }

    for mut consumer in consumers {
        consumer.stop().await.expect("clean stop");
    }
}

// ── DSL listener ownership (unify-transport-auth Task 2.4) ──

/// Build the `mcp.declared.*` query suffix a route lowered from a DSL `mcp:`
/// block carries on its from-URI (mirrors the presence-based lowering in
/// `camel-dsl`'s `mcp.rs`; values percent-encoded like every lowered
/// parameter). Caps and TLS ride the URI only when declared — `None` emits
/// no parameter, so "declared" stays distinguishable from "defaulted".
fn declared_params(
    bind: &str,
    max_tools: Option<usize>,
    max_resources: Option<usize>,
    tls: Option<(&str, &str)>,
) -> String {
    let mut out = format!("&mcp.declared.bind={}", encoded(bind));
    if let Some(max_tools) = max_tools {
        out.push_str(&format!("&mcp.declared.max_tools={max_tools}"));
    }
    if let Some(max_resources) = max_resources {
        out.push_str(&format!("&mcp.declared.max_resources={max_resources}"));
    }
    if let Some((cert_path, key_path)) = tls {
        out.push_str(&format!(
            "&mcp.declared.tls.cert_path={}&mcp.declared.tls.key_path={}",
            encoded(cert_path),
            encoded(key_path)
        ));
    }
    out
}

#[tokio::test]
async fn dsl_tls_reaches_listener() {
    use camel_component_api::test_support::tls as tls_support;

    let _ = rustls::crypto::ring::default_provider().install_default();

    // DSL tls paths: rcgen-generated CA + server cert in tempdir PEMs (the
    // repo's TLS test fixture channel).
    let (ca_pem, cert_pem, key_pem) = tls_support::gen_server_cert();
    let cert_path = tls_support::write_pem_tmp("mcp-dsl-cert.pem", &cert_pem);
    let key_path = tls_support::write_pem_tmp("mcp-dsl-key.pem", &key_pem);
    let cert_str = cert_path.to_str().expect("cert path is utf-8");
    let key_str = key_path.to_str().expect("key path is utf-8");

    let bind = "127.0.0.16:0";
    // TOML entry declares NO tls — the DSL declaration owns the listener's
    // TLS (spec scenario 'DSL TLS reaches the listener').
    let cfg = server_config(bind, true, None);
    let component = component_with_server("crm", cfg.clone());

    let mut consumer = consumer_for(
        &component,
        &format!(
            "mcp:crm/tool/t?schema={}{}",
            encoded(TRIVIAL_SCHEMA),
            declared_params(bind, Some(128), Some(128), Some((cert_str, key_str))),
        ),
    );
    let (ctx, _route_rx) = test_context();
    consumer
        .start(ctx)
        .await
        .expect("DSL-owned TLS listener must start");

    // Look the shared listener up with the EFFECTIVE config (TLS merged in
    // from the DSL) — a plain-config lookup trips the registry's own
    // tls conflict check against the running TLS listener.
    let mut effective = cfg;
    effective.tls = Some(McpTlsConfig {
        cert_path: cert_str.to_string(),
        key_path: key_str.to_string(),
    });
    let handle = McpServerRegistry::global()
        .get_or_spawn(bind, &effective)
        .await
        .expect("shared TLS listener");

    // TLS client handshake succeeds against the DSL-declared material
    // (CA-trusted; the generated server cert carries the localhost SAN).
    let mut roots = rustls::RootCertStore::empty();
    let ca_certs: Vec<_> = rustls_pemfile::certs(&mut std::io::BufReader::new(ca_pem.as_bytes()))
        .collect::<Result<_, _>>()
        .expect("parse generated CA pem");
    roots.add_parsable_certificates(ca_certs);
    let connector = tokio_rustls::TlsConnector::from(Arc::new(
        tokio_rustls::rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth(),
    ));
    let tcp = tokio::net::TcpStream::connect(handle.local_addr)
        .await
        .expect("tcp connect to the TLS listener");
    let server_name = tokio_rustls::rustls::pki_types::ServerName::try_from("localhost")
        .expect("localhost server name");
    let _tls_stream = connector
        .connect(server_name, tcp)
        .await
        .expect("TLS handshake against the DSL-declared cert must succeed");

    consumer.stop().await.expect("clean stop");
}

#[tokio::test]
async fn dsl_toml_bind_conflict_fails_startup() {
    // DSL declares 127.0.0.1:9100 while TOML declares 127.0.0.1:9200 for the
    // same server name — startup must fail naming BOTH sources (spec
    // scenario 'TOML/DSL conflict fails startup'). The conflict surfaces at
    // consumer start, BEFORE any bind, so the scenario's fixed ports never
    // collide with sibling listeners.
    let component = component_with_server("crm", server_config("127.0.0.1:9200", true, None));
    let mut consumer = consumer_for(
        &component,
        &format!(
            "mcp:crm/tool/t?schema={}{}",
            encoded(TRIVIAL_SCHEMA),
            declared_params("127.0.0.1:9100", Some(128), Some(128), None),
        ),
    );

    let (ctx, _route_rx) = test_context();
    let err = consumer
        .start(ctx)
        .await
        .expect_err("a bind declared with different values by DSL and TOML must refuse to start");
    assert!(
        matches!(&err, CamelError::Config(message)
            if message.contains("dsl")
                && message.contains("toml")
                && message.contains("127.0.0.1:9100")
                && message.contains("127.0.0.1:9200")),
        "expected a Config error naming both sources and both values, got {err}"
    );
}

#[tokio::test]
async fn dsl_toml_equal_values_proceed() {
    // The same bind declared by both sides, and max_tools declared EQUAL by
    // both sides (TOML 128 via `server_config(.., Some(128))`, DSL 128) —
    // no conflict, the server starts. max_resources is declared by the DSL
    // only (TOML silent) — one-sided declarations proceed too.
    let bind = "127.0.0.17:0";
    let cfg = server_config(bind, true, Some(128));
    let component = component_with_server("crm", cfg.clone());
    let mut consumer = consumer_for(
        &component,
        &format!(
            "mcp:crm/tool/t?schema={}{}",
            encoded(TRIVIAL_SCHEMA),
            declared_params(bind, Some(128), Some(128), None),
        ),
    );

    let (ctx, _route_rx) = test_context();
    consumer
        .start(ctx)
        .await
        .expect("equal DSL/TOML declarations must proceed");
    consumer.stop().await.expect("clean stop");
}

#[tokio::test]
async fn toml_only_server_unchanged() {
    // No DSL block (no mcp.declared.* parameters): the TOML entry drives the
    // listener exactly as before; undeclared caps materialize at their 128
    // defaults when the shared listener is spawned (regression).
    let bind = "127.0.0.18:0";
    let cfg = server_config(bind, true, None);
    let component = component_with_server("crm", cfg.clone());
    let mut consumer = consumer_for(
        &component,
        &format!("mcp:crm/tool/t?schema={}", encoded(TRIVIAL_SCHEMA)),
    );

    let (ctx, _route_rx) = test_context();
    consumer
        .start(ctx)
        .await
        .expect("TOML-only server must start as before");

    let handle = McpServerRegistry::global()
        .get_or_spawn(bind, &cfg)
        .await
        .expect("shared listener");
    assert_eq!(
        handle.cfg.max_tools,
        Some(128),
        "default tool cap regression"
    );
    assert_eq!(
        handle.cfg.max_resources,
        Some(128),
        "default resource cap regression"
    );
    assert!(handle.cfg.tls.is_none(), "no TLS without a declaration");

    consumer.stop().await.expect("clean stop");
}

#[tokio::test]
async fn dsl_cap_flows_when_toml_silent() {
    // The TOML entry omits the cap, the DSL block declares max_tools: 200 →
    // the runtime uses the DSL value with NO error (spec: DSL caps SHALL
    // flow). Under the old effective-value comparison this exact scenario
    // hard-errored "dsl: 200, toml: 128" — a TOML value nobody wrote.
    let bind = "127.0.0.19:0";
    let cfg = server_config(bind, true, None);
    let component = component_with_server("crm", cfg.clone());
    let mut consumer = consumer_for(
        &component,
        &format!(
            "mcp:crm/tool/t?schema={}{}",
            encoded(TRIVIAL_SCHEMA),
            declared_params(bind, Some(200), None, None),
        ),
    );

    let (ctx, _route_rx) = test_context();
    consumer
        .start(ctx)
        .await
        .expect("a DSL-declared cap with a silent TOML entry must start");

    // Runtime cap = the DSL value; the TOML-silent resource cap fell back
    // to the 128 default (lookup with the effective config — the
    // `dsl_tls_reaches_listener` precedent).
    let mut effective = cfg;
    effective.max_tools = Some(200);
    let handle = McpServerRegistry::global()
        .get_or_spawn(bind, &effective)
        .await
        .expect("shared listener");
    assert_eq!(
        handle.cfg.max_tools,
        Some(200),
        "the DSL-declared cap must be the runtime cap"
    );
    assert_eq!(
        handle.cfg.max_resources,
        Some(128),
        "a cap declared by neither side keeps the 128 default"
    );

    consumer.stop().await.expect("clean stop");
}

#[tokio::test]
async fn toml_cap_kept_when_dsl_silent() {
    // TOML declares max_tools = 200 explicitly; a DSL block added WITHOUT
    // repeating the cap → 200 is kept, no error. Under the old
    // effective-value comparison startup failed claiming "the DSL mcp:
    // block declares max_tools 128" — a value nobody wrote
    // (working-config regression).
    let bind = "127.0.0.20:0";
    let cfg = server_config(bind, true, Some(200));
    let component = component_with_server("crm", cfg.clone());
    let mut consumer = consumer_for(
        &component,
        &format!(
            "mcp:crm/tool/t?schema={}{}",
            encoded(TRIVIAL_SCHEMA),
            declared_params(bind, None, None, None),
        ),
    );

    let (ctx, _route_rx) = test_context();
    consumer
        .start(ctx)
        .await
        .expect("a DSL block silent on caps must keep the TOML cap");

    let handle = McpServerRegistry::global()
        .get_or_spawn(bind, &cfg)
        .await
        .expect("shared listener");
    assert_eq!(
        handle.cfg.max_tools,
        Some(200),
        "the TOML-declared cap must survive a cap-silent DSL block"
    );

    consumer.stop().await.expect("clean stop");
}

// ── Crash regressions (fix-mcp-dead-registry-entry Task 4, ADR-0068) ──

/// A distinct compiled route plan — the tests tell planA from planB by the
/// `provider_ref` field (same discriminator as the Task 3 registry tests).
fn security_plan(provider_ref: &str) -> RouteSecurityPlan {
    RouteSecurityPlan {
        access_mode: AccessMode::Authenticated,
        provider_ref: Some(provider_ref.to_string()),
        transport: TransportId::Mcp,
        credential_sources: vec![],
        audience_binding: None,
    }
}

#[tokio::test]
async fn aborted_bridge_and_dropped_consumer_same_name_restart_succeeds() {
    // Crash regression: aborting the bridge and dropping the consumer
    // WITHOUT `stop()` leaves the registry entry behind with a dead owner
    // (the leak window). Owner liveness must let fresh consumers take over
    // the tool name and the resource URI on the same bind, and dispatch
    // must reach the takeover routes. `Registration` is Tool XOR Resource
    // per consumer, so the takeover drives one of each.
    let bind = "127.0.0.22:0";
    let cfg = server_config(bind, true, None);
    let component = component_with_server("crm", cfg.clone());
    let tool_uri = format!("mcp:crm/tool/t?schema={}", encoded(TRIVIAL_SCHEMA));
    let resource_uri = "mcp:crm/resource/r?uri=crm://x";

    let mut tool_c1 = consumer_for(&component, &tool_uri);
    let (ctx, _route_rx) = test_context();
    tool_c1.start(ctx).await.expect("tool c1 must start");
    let bridge = tool_c1
        .background_task_handle()
        .expect("bridge handle present after start");
    bridge.abort();
    drop(tool_c1);

    let mut resource_c1 = consumer_for(&component, resource_uri);
    let (ctx, _route_rx) = test_context();
    resource_c1
        .start(ctx)
        .await
        .expect("resource c1 must start");
    let bridge = resource_c1
        .background_task_handle()
        .expect("bridge handle present after start");
    bridge.abort();
    drop(resource_c1);

    // Takeover: both dead entries are replaceable.
    let mut tool_c2 = consumer_for(&component, &tool_uri);
    let (ctx, mut tool_route_rx) = test_context();
    tool_c2
        .start(ctx)
        .await
        .expect("tool takeover after abort+drop must succeed");
    let mut resource_c2 = consumer_for(&component, resource_uri);
    let (ctx, mut resource_route_rx) = test_context();
    resource_c2
        .start(ctx)
        .await
        .expect("resource takeover after abort+drop must succeed");

    let handle = McpServerRegistry::global()
        .get_or_spawn(bind, &cfg)
        .await
        .expect("shared listener");

    // tools/call for "t" is answered by the takeover route.
    let entry = handle.tool_registry.resolve("t").expect("tool registered");
    let (reply_tx, reply_rx) = oneshot::channel();
    entry
        .sender
        .send(McpToolInvocation {
            name: "t".to_string(),
            arguments: serde_json::json!({ "id": "42" }),
            headers: std::collections::HashMap::new(),
            principal: None,
            reply: reply_tx,
        })
        .await
        .expect("invocation enqueue");
    let envelope = tool_route_rx
        .recv()
        .await
        .expect("route received the exchange");
    let mut out = envelope.exchange;
    out.input.body = Body::Text("takeover-ok".to_string());
    envelope
        .reply_tx
        .expect("reply channel present")
        .send(Ok(out))
        .expect("route reply");
    let result = reply_rx.await.expect("tool result");
    assert_eq!(
        result.content,
        serde_json::Value::String("takeover-ok".to_string()),
        "tools/call must be served by the takeover consumer"
    );

    // resources/read for crm://x dispatches to the takeover resource route.
    let entry = handle
        .resource_registry
        .resolve("crm://x")
        .expect("resource registered");
    let (reply_tx, reply_rx) = oneshot::channel();
    entry
        .sender
        .send(McpResourceRead {
            uri: "crm://x".to_string(),
            headers: std::collections::HashMap::new(),
            principal: None,
            reply: reply_tx,
        })
        .await
        .expect("read enqueue");
    let envelope = resource_route_rx
        .recv()
        .await
        .expect("route received the exchange");
    assert_eq!(
        envelope.exchange.input.body,
        Body::Text("crm://x".to_string()),
        "the resource bridge carries the requested URI as the route body"
    );
    let mut out = envelope.exchange;
    out.input.body = Body::Text("resource-ok".to_string());
    envelope
        .reply_tx
        .expect("reply channel present")
        .send(Ok(out))
        .expect("route reply");
    let resource = reply_rx.await.expect("resource result");
    assert_eq!(
        resource.content,
        b"resource-ok".to_vec(),
        "resources/read must be served by the takeover consumer"
    );

    tool_c2.stop().await.expect("clean stop");
    resource_c2.stop().await.expect("clean stop");
}

#[tokio::test]
async fn dropped_consumer_without_stop_releases_name() {
    // Crash regression: a consumer dropped without stop() — no abort either,
    // the bridge task stays detached — must not veto a legal restart.
    // Channel liveness alone would NOT fix this case: the dead entry's
    // sender side keeps the detached bridge parked on `recv`, so the name
    // is released through owner liveness, not channel death.
    let bind = "127.0.0.23:0";
    let cfg = server_config(bind, true, None);
    let component = component_with_server("crm", cfg.clone());
    let uri = format!("mcp:crm/tool/t?schema={}", encoded(TRIVIAL_SCHEMA));

    let mut first = consumer_for(&component, &uri);
    let (ctx, _route_rx) = test_context();
    first.start(ctx).await.expect("first start must succeed");
    drop(first);

    let mut second = consumer_for(&component, &uri);
    let (ctx, _route_rx) = test_context();
    second
        .start(ctx)
        .await
        .expect("the dropped consumer must have released the tool name");
    second.stop().await.expect("clean stop");
}

#[tokio::test]
async fn live_duplicate_consumer_rejected() {
    // Guard: the duplicate rejection itself predates the fix, but the
    // plan invariant is Task 3 — the refused start neither removed nor
    // overwrote the incumbent's bind security plan (planA stays, the
    // impostor's planB never lands).
    let bind = "127.0.0.24:0";
    let cfg = server_config(bind, true, None);
    let component = component_with_server("crm", cfg.clone());
    let uri = format!("mcp:crm/tool/t?schema={}", encoded(TRIVIAL_SCHEMA));

    let mut incumbent = consumer_for(&component, &uri);
    incumbent.set_security_context(SecurityContext::from_plan(security_plan("plan-a")));
    let (ctx, _route_rx) = test_context();
    incumbent
        .start(ctx)
        .await
        .expect("incumbent consumer must start");

    let mut duplicate = consumer_for(&component, &uri);
    duplicate.set_security_context(SecurityContext::from_plan(security_plan("plan-b")));
    let (ctx, _route_rx) = test_context();
    let err = duplicate
        .start(ctx)
        .await
        .expect_err("a live duplicate tool name must be refused");
    assert!(
        matches!(err, CamelError::EndpointCreationFailed(ref message)
            if message.contains("already registered")),
        "the duplicate refusal must name the existing registration, got {err}"
    );

    let handle = McpServerRegistry::global()
        .get_or_spawn(bind, &cfg)
        .await
        .expect("shared listener");
    assert_eq!(
        handle
            .security
            .plan_for("test-route")
            .expect("the incumbent's plan must survive the refused start")
            .provider_ref,
        Some("plan-a".to_string()),
        "the failed start must keep the incumbent's plan (planA), not the impostor's (planB)"
    );

    incumbent.stop().await.expect("clean stop");
}

#[tokio::test]
async fn dead_owner_tool_absent_from_list_ready_via_listener_handle() {
    // Crash regression (Task 1's lazy prune): after the consumer drops
    // without stop(), the dead owner's entry must vanish from the
    // listener's tools/list projection — the cloned handle keeps the
    // listener itself alive, so only owner liveness can hide the entry.
    let bind = "127.0.0.25:0";
    let cfg = server_config(bind, true, None);
    let component = component_with_server("crm", cfg.clone());

    let mut consumer = consumer_for(
        &component,
        &format!("mcp:crm/tool/t?schema={}", encoded(TRIVIAL_SCHEMA)),
    );
    let (ctx, _route_rx) = test_context();
    consumer.start(ctx).await.expect("tool consumer must start");

    let handle = McpServerRegistry::global()
        .get_or_spawn(bind, &cfg)
        .await
        .expect("shared listener");
    assert!(
        handle
            .tool_registry
            .list_ready()
            .iter()
            .any(|(name, _)| name == "t"),
        "the live consumer's tool must be listed, got {:?}",
        handle.tool_registry.list_ready()
    );

    drop(consumer);

    assert!(
        !handle
            .tool_registry
            .list_ready()
            .iter()
            .any(|(name, _)| name == "t"),
        "a dead owner's tool must vanish from tools/list, got {:?}",
        handle.tool_registry.list_ready()
    );
}
