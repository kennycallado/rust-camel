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
//! keeps the 128 default).

mod common;

use std::collections::HashMap;
use std::sync::Arc;

use camel_api::{Body, CamelError};
use camel_component_api::consumer::ExchangeEnvelope;
use camel_component_api::{
    Component, Consumer, ConsumerContext, NoOpComponentContext, NoopRuntimeObservability,
    RuntimeObservability,
};
use camel_component_mcp::McpServerRegistry;
use camel_component_mcp::component::McpComponent;
use camel_component_mcp::config::{McpGlobalConfig, McpServerConfig, McpTlsConfig};
use camel_component_mcp::types::McpToolInvocation;
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
