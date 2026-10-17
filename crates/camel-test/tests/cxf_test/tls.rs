//! Real-native producer transport TLS integration (Task 1.7).
//!
//! Every test drives the actual native cxf-bridge binary against a loopback
//! HTTPS SOAP server built in-process from ephemeral rcgen material. The server
//! records completed handshakes, failed handshakes, and the presented client
//! certificate, so the negative cases assert the specific TLS failure mode and
//! not merely "the call returned an error".
//!
//! This is a nested module of the `cxf_test` integration target (declared with
//! `#[path]`), so the `--test cxf_test` filter keeps working.

use std::net::{IpAddr, Ipv4Addr};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use camel_api::{Body, Exchange, Message};
use camel_component_cxf::config::{CxfSecurityFields, CxfTlsConfig};
use camel_component_cxf::{CxfBridgePool, CxfComponent, CxfPoolConfig, CxfProfileConfig};
use camel_dsl::parse_yaml;
use camel_test::CamelTestContext;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::support::cxf::{
    SOAP11_ENVELOPE_NS, TEST_TLS_MAX_BODY, TestClientAuth, TestTlsCa, TestTlsServerState,
    TestTlsStores, capture_boundary_crossing_probe, capture_probe_request, raw_tls_client,
    require_cxf_bridge_binary, spawn_soap_tls_server, spawn_soap_tls_server_with_response,
};
use crate::support::{bridge_bg_rt, install_crypto_provider, send_to_direct};
use crate::wire::{CapturedSoapStructure, parse_soap_structure};
use crate::{cxf_wsdl_path, init_tracing};

/// Bare `sayHello` operation payload the bridge wraps in a SOAP 1.1 envelope.
const SAY_HELLO_PAYLOAD: &str =
    r#"<hel:sayHello xmlns:hel="http://example.com/hello"><name>ping</name></hel:sayHello>"#;

/// Bound for the eagerly-spawned slot-initialisation join. The pool's own
/// `bridge_start_timeout_ms` is 30 s, so the join must outlast that startup.
const BRIDGE_INIT_BUDGET: Duration = Duration::from_secs(60);

fn loopback_san() -> Vec<rcgen::SanType> {
    vec![rcgen::SanType::IpAddress(IpAddr::V4(Ipv4Addr::LOCALHOST))]
}

/// A transport-TLS profile whose upstream endpoint is the loopback HTTPS server.
fn tls_profile(name: &str, port: u16, tls: Option<CxfTlsConfig>) -> CxfProfileConfig {
    CxfProfileConfig {
        name: name.to_string(),
        address: Some(format!("https://127.0.0.1:{port}/service")),
        wsdl_path: cxf_wsdl_path(),
        service_name: "{http://example.com/hello}HelloService".to_string(),
        port_name: "{http://example.com/hello}HelloPort".to_string(),
        security: CxfSecurityFields {
            tls,
            ..Default::default()
        },
    }
}

async fn tls_component(profiles: Vec<CxfProfileConfig>) -> CxfComponent {
    let pool = CxfBridgePool::from_config(CxfPoolConfig {
        profiles,
        max_bridges: 4,
        bridge_start_timeout_ms: 30_000,
        health_check_interval_ms: 5_000,
        bridge_cache_dir: None,
        version: camel_component_cxf::BRIDGE_VERSION.to_string(),
        bind_address: None,
        listener_tls: None,
        reconnect: Default::default(),
    })
    .expect("valid cxf pool config");
    // Eagerly create the bridge slot on the permanent background runtime so the
    // first producer invoke does not race the native-bridge startup (and the
    // `send_to_direct` retry budget) and the tonic dispatch task outlives the
    // individual test runtime.
    let pool = Arc::new(pool);
    let pool_init = Arc::clone(&pool);
    let mut pool_init_task = bridge_bg_rt().spawn(async move {
        pool_init
            .get_or_create_slot(&CxfBridgePool::slot_key())
            .await
    });
    if tokio::time::timeout(BRIDGE_INIT_BUDGET, &mut pool_init_task)
        .await
        .is_err()
    {
        pool_init_task.abort();
        panic!("bridge slot init must complete within {BRIDGE_INIT_BUDGET:?}");
    }
    CxfComponent::new(pool)
}

/// `CxfTlsConfig` is `#[non_exhaustive]`, so it is built by assignment rather
/// than a struct literal. PKCS12 is the default store type.
fn tls_config(
    stores: &TestTlsStores,
    keystore: Option<&std::path::Path>,
    truststore: &std::path::Path,
) -> CxfTlsConfig {
    let mut tls = CxfTlsConfig::default();
    if let Some(path) = keystore {
        tls.keystore_path = Some(path.to_string_lossy().to_string());
        tls.keystore_password = Some(stores.password.clone());
    }
    tls.truststore_path = Some(truststore.to_string_lossy().to_string());
    tls.truststore_password = Some(stores.password.clone());
    tls
}

fn route_yaml(route_id: &str, direct: &str, profile: &str, port: u16) -> String {
    let wsdl = cxf_wsdl_path();
    format!(
        "routes:\n  - id: {route_id}\n    from: {direct}\n    steps:\n      - to: \"cxf://https://127.0.0.1:{port}/service?wsdl={wsdl}&service={{http://example.com/hello}}HelloService&port={{http://example.com/hello}}HelloPort&operation=sayHello&profile={profile}\"\n      - to: \"mock:done\"\n"
    )
}

/// Builds a one-route context, invokes once, and returns either the producer
/// response exchange or the error string.
async fn invoke_single(
    component: CxfComponent,
    profile: &str,
    port: u16,
) -> Result<Exchange, String> {
    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .with_component(component)
        .build()
        .await;
    let yaml = route_yaml("cxf-tls-route", "direct:start", profile, port);
    for route in parse_yaml(&yaml).unwrap() {
        h.add_route(route).await.unwrap();
    }
    h.start().await;

    let exchange = Exchange::new(Message::new(Body::Text(SAY_HELLO_PAYLOAD.to_string())));
    let result = send_to_direct(&h, "direct:start", exchange).await;
    h.stop().await;
    result.map_err(|e| e.to_string())
}

fn response_body(exchange: &Exchange) -> String {
    exchange.body_as::<String>().unwrap_or_default()
}

fn errors_of(state: &crate::support::cxf::TestTlsServerState) -> Vec<String> {
    state.failures.lock().expect("tls failure lock").clone()
}

// ── Mission 363 one-way / empty-reply fixtures ──────────────────────────────

/// Namespace of the single Java `soap-oneway.wsdl` fixture.
const ONEWAY_NS: &str = "urn:oneway:363";

/// Bare input-only `ReceiveMessageOp` payload the bridge wraps in the request.
const RECEIVE_MESSAGE_PAYLOAD: &str =
    r#"<receiveMessage xmlns="urn:oneway:363">ping</receiveMessage>"#;

/// Bare request-reply `EchoOp` payload the bridge wraps in the request.
const ECHO_REQUEST_PAYLOAD: &str = r#"<echoRequest xmlns="urn:oneway:363">ping</echoRequest>"#;

/// Resolves the Java fixture from the workspace root (walking up from
/// `CARGO_MANIFEST_DIR`), matching how the other camel-test WSDL fixtures are
/// located.
fn oneway_wsdl_path() -> String {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root resolves from CARGO_MANIFEST_DIR");
    root.join("bridges/cxf/src/test/resources/soap-oneway.wsdl")
        .to_string_lossy()
        .to_string()
}

/// A transport-TLS profile selecting the one-way fixture's main binding.
fn oneway_profile(name: &str, port: u16, tls: Option<CxfTlsConfig>) -> CxfProfileConfig {
    CxfProfileConfig {
        name: name.to_string(),
        address: Some(format!("https://127.0.0.1:{port}/service")),
        wsdl_path: oneway_wsdl_path(),
        service_name: "{urn:oneway:363}OneWayService".to_string(),
        port_name: "{urn:oneway:363}MainPort".to_string(),
        security: CxfSecurityFields {
            tls,
            ..Default::default()
        },
    }
}

fn oneway_route_yaml(
    route_id: &str,
    direct: &str,
    profile: &str,
    port: u16,
    operation: &str,
) -> String {
    let wsdl = oneway_wsdl_path();
    format!(
        "routes:\n  - id: {route_id}\n    from: {direct}\n    steps:\n      - to: \"cxf://https://127.0.0.1:{port}/service?wsdl={wsdl}&service={{urn:oneway:363}}OneWayService&port={{urn:oneway:363}}MainPort&operation={operation}&profile={profile}\"\n"
    )
}

/// Builds a one-route context, invokes `operation` once, and returns either the
/// producer response exchange or the error string.
async fn invoke_oneway_operation(
    component: CxfComponent,
    profile: &str,
    port: u16,
    operation: &str,
    payload: &str,
    route_id: &str,
) -> Result<Exchange, String> {
    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .with_component(component)
        .build()
        .await;
    let yaml = oneway_route_yaml(route_id, "direct:start", profile, port, operation);
    for route in parse_yaml(&yaml).unwrap() {
        h.add_route(route).await.unwrap();
    }
    h.start().await;

    let exchange = Exchange::new(Message::new(Body::Text(payload.to_string())));
    let result = send_to_direct(&h, "direct:start", exchange).await;
    h.stop().await;
    result.map_err(|e| e.to_string())
}

/// Returns the single recorded POST's headers and body.
fn only_captured_request(state: &TestTlsServerState) -> (Vec<(String, String)>, Vec<u8>) {
    let headers = state.request_headers.lock().expect("request header lock");
    let bodies = state.request_bodies.lock().expect("request body lock");
    assert_eq!(
        bodies.len(),
        1,
        "exactly one complete POST body must be recorded"
    );
    (headers[0].clone(), bodies[0].clone())
}

/// Normalized (unquoted) SOAPAction of a captured POST.
fn captured_action(headers: &[(String, String)]) -> &str {
    let raw = headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("soapaction"))
        .map(|(_, value)| value.as_str())
        .unwrap_or_else(|| panic!("captured request must carry a SOAPAction header: {headers:?}"));
    raw.strip_prefix('"')
        .and_then(|action| action.strip_suffix('"'))
        .unwrap_or(raw)
}

/// Asserts the namespace-resolved request has exactly one SOAP 1.1 Envelope,
/// one Body, and `expected_body_child` as the only direct Body child.
fn assert_single_soap11_envelope(structure: &CapturedSoapStructure, expected_body_child: &str) {
    assert_eq!(
        structure.envelopes,
        vec![format!("{{{SOAP11_ENVELOPE_NS}}}Envelope")],
        "captured request must contain exactly one SOAP 1.1 Envelope, got {:?}",
        structure.envelopes
    );
    assert_eq!(
        structure.bodies,
        vec![format!("{{{SOAP11_ENVELOPE_NS}}}Body")],
        "captured request must contain exactly one SOAP 1.1 Body, got {:?}",
        structure.bodies
    );
    assert_eq!(
        structure.body_children,
        vec![expected_body_child.to_string()],
        "the operation element must be the only direct Body child, got {:?}",
        structure.body_children
    );
}

/// The capture layer rejects nonnumeric, negative, and over-cap
/// `Content-Length` values without allocating from them and without recording a
/// complete POST, then records a valid split-write body exactly once.
#[tokio::test(flavor = "multi_thread")]
async fn cxf_tls_capture_rejects_invalid_content_lengths() {
    init_tracing();
    install_crypto_provider();

    let ca = TestTlsCa::generate("cxf tls capture ca");
    let server = ca.sign_server(loopback_san());
    let client = ca.sign_client("cxf tls capture client");
    let (addr, state, handle) = spawn_soap_tls_server_with_response(
        &server.cert_der,
        &server.key_der,
        TestClientAuth::Required {
            roots_der: ca.der(),
        },
        200,
        Vec::new(),
    )
    .await;

    let over_cap = (TEST_TLS_MAX_BODY + 1).to_string();
    let mut expected_rejections = 0usize;
    for (label, content_length) in [
        ("nonnumeric", "not-a-number".to_string()),
        ("negative", "-1".to_string()),
        ("over-cap", over_cap),
    ] {
        let mut tls = raw_tls_client(addr, &ca, &client).await;
        tls.write_all(&capture_probe_request(&content_length))
            .await
            .expect("malformed probe request writes");
        tls.flush().await.expect("malformed probe request flushes");

        // The bounded read must complete: either EOF (`Ok(0)`) or a reset
        // (`Err`). A timeout would mean the server hung on malformed framing.
        let _observed = tokio::time::timeout(Duration::from_secs(5), tls.read(&mut [0u8; 1]))
            .await
            .unwrap_or_else(|_| {
                panic!("{label}: server must close the malformed connection, not hang")
            });

        expected_rejections += 1;
        assert_eq!(
            state.rejected.lock().expect("rejection lock").len(),
            expected_rejections,
            "{label}: the server must record exactly one framing rejection"
        );
        assert_eq!(
            state.requests.load(Ordering::SeqCst),
            0,
            "{label}: malformed framing must not be recorded as a complete POST"
        );
        assert!(
            state
                .request_bodies
                .lock()
                .expect("request body lock")
                .is_empty(),
            "{label}: no body may be recorded for a rejected request"
        );
    }

    // Header terminator crossing the 64 KiB boundary: the bytes before the
    // terminator fill exactly `cap - 3`, so its end position is `cap + 1`.
    // The capture layer must reject it without admitting an over-cap head.
    {
        let mut tls = raw_tls_client(addr, &ca, &client).await;
        tls.write_all(&capture_boundary_crossing_probe())
            .await
            .expect("boundary-crossing probe writes");
        tls.flush().await.expect("boundary-crossing probe flushes");
        let _observed = tokio::time::timeout(Duration::from_secs(5), tls.read(&mut [0u8; 1]))
            .await
            .expect("server must close the boundary-crossing connection, not hang");

        expected_rejections += 1;
        assert_eq!(
            state.rejected.lock().expect("rejection lock").len(),
            expected_rejections,
            "a terminator crossing the header cap must be rejected"
        );
        assert_eq!(
            state.requests.load(Ordering::SeqCst),
            0,
            "an over-cap head must not be recorded as a complete POST"
        );
        assert!(
            state
                .request_bodies
                .lock()
                .expect("request body lock")
                .is_empty(),
            "no body may be recorded for an over-cap head"
        );
    }

    let body = br#"<receiveMessage xmlns="urn:oneway:363">split-write</receiveMessage>"#;
    let head = format!(
        "POST /service HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: text/xml; charset=utf-8\r\nSOAPAction: \"urn:receive\"\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let split = body.len() / 2;
    let mut tls = raw_tls_client(addr, &ca, &client).await;
    tls.write_all(head.as_bytes())
        .await
        .expect("valid probe headers write");
    tls.write_all(&body[..split])
        .await
        .expect("first split body write");
    tls.flush().await.expect("first split body flush");
    tls.write_all(&body[split..])
        .await
        .expect("second split body write");
    tls.flush().await.expect("second split body flush");

    let mut response = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), tls.read_to_end(&mut response))
        .await
        .expect("valid split-write request must receive a bounded response")
        .expect("valid split-write response must be readable");
    assert!(
        response.starts_with(b"HTTP/1.1 200 OK\r\n"),
        "unexpected response: {:?}",
        String::from_utf8_lossy(&response)
    );

    assert_eq!(
        state.requests.load(Ordering::SeqCst),
        1,
        "the valid split-write request must be recorded as exactly one POST"
    );
    let captured_body = {
        let bodies = state.request_bodies.lock().expect("request body lock");
        assert_eq!(
            bodies.len(),
            1,
            "the split-write body must be recorded exactly once"
        );
        bodies[0].clone()
    };
    assert_eq!(
        captured_body.as_slice(),
        body,
        "the full split-write body must be captured"
    );

    handle.abort();
    let joined = tokio::time::timeout(Duration::from_secs(5), handle)
        .await
        .expect("fake TLS server task must stop promptly after abort");
    assert!(
        joined.is_err(),
        "aborted fake TLS server task must report cancellation"
    );
}

/// Exact-name acceptance: an input-only `ReceiveMessageOp` over mTLS returns
/// `Ok` with a zero-length body for both empty HTTP 200 and 202, sends exactly
/// one envelope-correct POST with `SOAPAction: urn:receive`, and accepts the
/// required client certificate.
///
/// Compiled here; runtime green requires the Task 2.8 native bridge rebuild.
#[tokio::test(flavor = "multi_thread")]
async fn cxf_oneway_empty_200_and_202_complete_ok_single_envelope_over_mtls() {
    init_tracing();
    install_crypto_provider();
    let _binary = require_cxf_bridge_binary();

    for status in [200u16, 202u16] {
        let ca = TestTlsCa::generate("cxf oneway tls ca");
        let server = ca.sign_server(loopback_san());
        let client = ca.sign_client("cxf oneway tls client");
        let stores = TestTlsStores::new();
        let keystore = stores.keystore(&client, &ca, &format!("oneway-client-{status}.p12"));
        let truststore = stores.truststore(&ca, &format!("oneway-trust-{status}.p12"));

        let (addr, state, handle) = spawn_soap_tls_server_with_response(
            &server.cert_der,
            &server.key_der,
            TestClientAuth::Required {
                roots_der: ca.der(),
            },
            status,
            Vec::new(),
        )
        .await;

        let profile = format!("oneway_{status}");
        let component = tls_component(vec![oneway_profile(
            &profile,
            addr.port(),
            Some(tls_config(&stores, Some(&keystore), &truststore)),
        )])
        .await;
        let result = invoke_oneway_operation(
            component,
            &profile,
            addr.port(),
            "ReceiveMessageOp",
            RECEIVE_MESSAGE_PAYLOAD,
            &format!("cxf-oneway-{status}"),
        )
        .await;
        handle.abort();
        let _ = handle.await;

        let exchange = result.unwrap_or_else(|err| {
            panic!("status {status}: one-way invoke must complete Ok, got: {err}")
        });
        match &exchange.input.body {
            Body::Bytes(bytes) => assert!(
                bytes.is_empty(),
                "status {status}: one-way output must be zero-length Body::Bytes, got {} bytes",
                bytes.len()
            ),
            other => {
                panic!("status {status}: one-way output must be Body::Bytes(empty), got {other:?}")
            }
        }

        assert_eq!(
            state.requests.load(Ordering::SeqCst),
            1,
            "status {status}: the operation must be sent exactly once"
        );
        assert_eq!(
            state.completed.load(Ordering::SeqCst),
            1,
            "status {status}: exactly one mTLS handshake must complete"
        );
        assert_eq!(
            state
                .client_cert_der
                .lock()
                .expect("client cert lock")
                .as_deref(),
            Some(client.cert_der.as_slice()),
            "status {status}: the server must accept the configured client certificate"
        );
        assert!(
            errors_of(&state).is_empty(),
            "status {status}: no handshake failure expected: {:?}",
            errors_of(&state)
        );

        let (headers, body) = only_captured_request(&state);
        assert_eq!(
            captured_action(&headers),
            "urn:receive",
            "status {status}: SOAPAction must be urn:receive"
        );
        let xml = std::str::from_utf8(&body).expect("captured request body must be UTF-8");
        let structure = parse_soap_structure(xml);
        assert_single_soap11_envelope(&structure, &format!("{{{ONEWAY_NS}}}receiveMessage"));
    }
}

/// Exact-name request-reply acceptance: an empty HTTP 200/202 for `EchoOp` is an
/// error, not a successful empty response, and is sent exactly once over mTLS.
///
/// Compiled here; runtime green requires the Task 2.8 native bridge rebuild.
#[tokio::test(flavor = "multi_thread")]
async fn cxf_request_reply_empty_200_and_202_fail_without_resend_over_mtls() {
    init_tracing();
    install_crypto_provider();
    let _binary = require_cxf_bridge_binary();

    for status in [200u16, 202u16] {
        let ca = TestTlsCa::generate("cxf echo tls ca");
        let server = ca.sign_server(loopback_san());
        let client = ca.sign_client("cxf echo tls client");
        let stores = TestTlsStores::new();
        let keystore = stores.keystore(&client, &ca, &format!("echo-client-{status}.p12"));
        let truststore = stores.truststore(&ca, &format!("echo-trust-{status}.p12"));

        let (addr, state, handle) = spawn_soap_tls_server_with_response(
            &server.cert_der,
            &server.key_der,
            TestClientAuth::Required {
                roots_der: ca.der(),
            },
            status,
            Vec::new(),
        )
        .await;

        let profile = format!("echo_{status}");
        let component = tls_component(vec![oneway_profile(
            &profile,
            addr.port(),
            Some(tls_config(&stores, Some(&keystore), &truststore)),
        )])
        .await;
        let result = invoke_oneway_operation(
            component,
            &profile,
            addr.port(),
            "EchoOp",
            ECHO_REQUEST_PAYLOAD,
            &format!("cxf-echo-{status}"),
        )
        .await;
        handle.abort();
        let _ = handle.await;

        assert!(
            result.is_err(),
            "status {status}: an empty request-reply response must fail the invoke, got Ok"
        );

        assert_eq!(
            state.requests.load(Ordering::SeqCst),
            1,
            "status {status}: the failed request-reply must not be resent"
        );
        assert_eq!(
            state.completed.load(Ordering::SeqCst),
            1,
            "status {status}: exactly one authenticated POST must reach the server"
        );
        assert_eq!(
            state
                .client_cert_der
                .lock()
                .expect("client cert lock")
                .as_deref(),
            Some(client.cert_der.as_slice()),
            "status {status}: the server must accept the configured client certificate"
        );

        let (headers, body) = only_captured_request(&state);
        assert_eq!(
            captured_action(&headers),
            "urn:echo",
            "status {status}: SOAPAction must be urn:echo"
        );
        let xml = std::str::from_utf8(&body).expect("captured request body must be UTF-8");
        let structure = parse_soap_structure(xml);
        assert_single_soap11_envelope(&structure, &format!("{{{ONEWAY_NS}}}echoRequest"));
    }
}

/// mTLS: the trusted server requires a client certificate and the profile
/// supplies one; the handshake completes with exactly that certificate.
#[tokio::test(flavor = "multi_thread")]
async fn cxf_producer_tls_presents_client_certificate() {
    init_tracing();
    install_crypto_provider();
    let _ = require_cxf_bridge_binary();

    let ca = TestTlsCa::generate("cxf tls test ca");
    let server = ca.sign_server(loopback_san());
    let client = ca.sign_client("cxf tls client");
    let stores = TestTlsStores::new();
    let keystore = stores.keystore(&client, &ca, "baleares-client.p12");
    let truststore = stores.truststore(&ca, "baleares-trust.p12");

    let (addr, state, handle) = spawn_soap_tls_server(
        &server.cert_der,
        &server.key_der,
        TestClientAuth::Required {
            roots_der: ca.der(),
        },
    )
    .await;

    let component = tls_component(vec![tls_profile(
        "baleares",
        addr.port(),
        Some(tls_config(&stores, Some(&keystore), &truststore)),
    )])
    .await;
    let result = invoke_single(component, "baleares", addr.port()).await;
    handle.abort();

    let exchange = result.expect("mTLS invoke must succeed");
    let body = response_body(&exchange);
    assert!(
        body.contains("sayHelloResponse"),
        "expected the WSDL operation response, got: {body}"
    );
    assert_eq!(
        state.completed.load(Ordering::SeqCst),
        1,
        "the server must complete exactly one mTLS handshake"
    );
    assert!(
        errors_of(&state).is_empty(),
        "no handshake failure expected: {:?}",
        errors_of(&state)
    );
    assert_eq!(
        state
            .client_cert_der
            .lock()
            .expect("client cert lock")
            .as_deref(),
        Some(client.cert_der.as_slice()),
        "the server must receive exactly the configured client certificate"
    );
}

/// Missing client certificate: a truststore-only profile against a server that
/// requires one. The handshake is rejected, not timed out.
#[tokio::test(flavor = "multi_thread")]
async fn cxf_producer_tls_no_client_cert_is_rejected() {
    init_tracing();
    install_crypto_provider();
    let _ = require_cxf_bridge_binary();

    let ca = TestTlsCa::generate("cxf tls test ca");
    let server = ca.sign_server(loopback_san());
    let stores = TestTlsStores::new();
    let truststore = stores.truststore(&ca, "cyl-trust.p12");

    let (addr, state, handle) = spawn_soap_tls_server(
        &server.cert_der,
        &server.key_der,
        TestClientAuth::Required {
            roots_der: ca.der(),
        },
    )
    .await;

    let component = tls_component(vec![tls_profile(
        "cyl",
        addr.port(),
        Some(tls_config(&stores, None, &truststore)),
    )])
    .await;
    let result = invoke_single(component, "cyl", addr.port()).await;
    handle.abort();

    assert!(
        result.is_err(),
        "a truststore-only profile must fail against a server requiring a client cert"
    );
    let failures = errors_of(&state);
    assert!(
        !failures.is_empty(),
        "the server must record a handshake failure, not a timeout"
    );
    assert!(
        failures
            .iter()
            .any(|f| f.to_lowercase().contains("no certificates")),
        "the server must reject the missing client certificate specifically: {failures:?}"
    );
    assert_eq!(
        state.completed.load(Ordering::SeqCst),
        0,
        "no handshake may complete without the required client certificate"
    );
    assert!(
        state
            .client_cert_der
            .lock()
            .expect("client cert lock")
            .is_none(),
        "the client must not present any certificate"
    );
}

/// Wrong CA: the server certificate chains to a CA the client does not trust.
#[tokio::test(flavor = "multi_thread")]
async fn cxf_producer_tls_wrong_ca_is_rejected() {
    init_tracing();
    install_crypto_provider();
    let _ = require_cxf_bridge_binary();

    let trusted_ca = TestTlsCa::generate("cxf trusted ca");
    let untrusted_ca = TestTlsCa::generate("cxf untrusted ca");
    let server = untrusted_ca.sign_server(loopback_san());
    let stores = TestTlsStores::new();
    let truststore = stores.truststore(&trusted_ca, "wrong-ca-trust.p12");

    let (addr, state, handle) =
        spawn_soap_tls_server(&server.cert_der, &server.key_der, TestClientAuth::None).await;

    let component = tls_component(vec![tls_profile(
        "baleares",
        addr.port(),
        Some(tls_config(&stores, None, &truststore)),
    )])
    .await;
    let result = invoke_single(component, "baleares", addr.port()).await;
    handle.abort();

    let err = result.expect_err("an untrusted server CA must fail the invoke");
    let lowered = err.to_lowercase();
    assert!(
        lowered.contains("pkix")
            || lowered.contains("certification path")
            || lowered.contains("certpath")
            || lowered.contains("unable to find valid certification path"),
        "expected a certificate-path/trust failure, got: {err}"
    );
    assert!(
        !lowered.contains("subject alternative") && !lowered.contains("hostname"),
        "wrong-CA must not surface as a hostname failure: {err}"
    );
    assert_eq!(
        state.completed.load(Ordering::SeqCst),
        0,
        "the handshake must not complete against an untrusted server"
    );
    assert!(
        !errors_of(&state).is_empty(),
        "the server must observe the aborted handshake"
    );
}

/// Wrong SAN: the server certificate chains to a trusted CA but its SAN does
/// not match the endpoint host. Hostname verification is not disabled.
#[tokio::test(flavor = "multi_thread")]
async fn cxf_producer_tls_wrong_san_is_rejected() {
    init_tracing();
    install_crypto_provider();
    let _ = require_cxf_bridge_binary();

    let ca = TestTlsCa::generate("cxf tls test ca");
    let server = ca.sign_server(vec![rcgen::SanType::DnsName(
        rcgen::string::Ia5String::try_from("wrong.example").expect("ia5 dns name"),
    )]);
    let stores = TestTlsStores::new();
    let truststore = stores.truststore(&ca, "wrong-san-trust.p12");

    let (addr, state, handle) =
        spawn_soap_tls_server(&server.cert_der, &server.key_der, TestClientAuth::None).await;

    let component = tls_component(vec![tls_profile(
        "baleares",
        addr.port(),
        Some(tls_config(&stores, None, &truststore)),
    )])
    .await;
    let result = invoke_single(component, "baleares", addr.port()).await;
    handle.abort();

    let err = result.expect_err("a SAN mismatch must fail the invoke");
    let lowered = err.to_lowercase();
    assert!(
        lowered.contains("alternative names")
            || lowered.contains("no name matching")
            || lowered.contains("hostname"),
        "expected a hostname-verification failure, got: {err}"
    );
    assert!(
        !lowered.contains("pkix") && !lowered.contains("certification path"),
        "a SAN mismatch must not surface as a trust-path failure: {err}"
    );
    assert_eq!(
        state.completed.load(Ordering::SeqCst),
        0,
        "the handshake must not complete with a mismatched SAN"
    );
    assert!(
        !errors_of(&state).is_empty(),
        "the server must observe the aborted handshake"
    );
}

/// Cross-profile isolation: `baleares` carries TLS material and presents a
/// certificate; `cyl` has no TLS block and fails the handshake against the same
/// server. One profile's TLS material must not leak into the other.
#[tokio::test(flavor = "multi_thread")]
async fn cxf_producer_tls_cross_profile_isolation() {
    init_tracing();
    install_crypto_provider();
    let _ = require_cxf_bridge_binary();

    let ca = TestTlsCa::generate("cxf tls test ca");
    let server = ca.sign_server(loopback_san());
    let client = ca.sign_client("cxf tls client");
    let stores = TestTlsStores::new();
    let keystore = stores.keystore(&client, &ca, "baleares-client.p12");
    let truststore = stores.truststore(&ca, "shared-trust.p12");

    let (addr, state, handle) = spawn_soap_tls_server(
        &server.cert_der,
        &server.key_der,
        TestClientAuth::Required {
            roots_der: ca.der(),
        },
    )
    .await;

    let component = tls_component(vec![
        tls_profile(
            "baleares",
            addr.port(),
            Some(tls_config(&stores, Some(&keystore), &truststore)),
        ),
        tls_profile("cyl", addr.port(), None),
    ])
    .await;

    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .with_component(component)
        .build()
        .await;
    for route in parse_yaml(&route_yaml(
        "cxf-tls-baleares",
        "direct:baleares",
        "baleares",
        addr.port(),
    ))
    .unwrap()
    {
        h.add_route(route).await.unwrap();
    }
    for route in parse_yaml(&route_yaml("cxf-tls-cyl", "direct:cyl", "cyl", addr.port())).unwrap() {
        h.add_route(route).await.unwrap();
    }
    h.start().await;

    let baleares = send_to_direct(
        &h,
        "direct:baleares",
        Exchange::new(Message::new(Body::Text(SAY_HELLO_PAYLOAD.to_string()))),
    )
    .await;
    let cyl = send_to_direct(
        &h,
        "direct:cyl",
        Exchange::new(Message::new(Body::Text(SAY_HELLO_PAYLOAD.to_string()))),
    )
    .await;
    h.stop().await;
    handle.abort();

    let baleares_exchange = baleares.expect("baleares must present its client certificate");
    assert!(
        response_body(&baleares_exchange).contains("sayHelloResponse"),
        "baleares must receive the SOAP response"
    );
    assert!(
        cyl.is_err(),
        "cyl has no TLS material and must fail the mTLS handshake"
    );
    assert_eq!(
        state.completed.load(Ordering::SeqCst),
        1,
        "exactly one handshake (baleares) may complete"
    );
    assert_eq!(
        state
            .client_cert_der
            .lock()
            .expect("client cert lock")
            .as_deref(),
        Some(client.cert_der.as_slice()),
        "baleares must present the configured client certificate"
    );
    assert!(
        !errors_of(&state).is_empty(),
        "cyl's failed handshake must be observed by the server"
    );
}

/// Truststore-only: the profile validates the server certificate and presents
/// no client certificate, against a server that requests one optionally.
#[tokio::test(flavor = "multi_thread")]
async fn cxf_producer_truststore_only_validates_server_without_client_cert() {
    init_tracing();
    install_crypto_provider();
    let _ = require_cxf_bridge_binary();

    let ca = TestTlsCa::generate("cxf tls test ca");
    let server = ca.sign_server(loopback_san());
    let stores = TestTlsStores::new();
    let truststore = stores.truststore(&ca, "trust-only-trust.p12");

    let (addr, state, handle) = spawn_soap_tls_server(
        &server.cert_der,
        &server.key_der,
        TestClientAuth::Optional {
            roots_der: ca.der(),
        },
    )
    .await;

    let component = tls_component(vec![tls_profile(
        "baleares",
        addr.port(),
        Some(tls_config(&stores, None, &truststore)),
    )])
    .await;
    let result = invoke_single(component, "baleares", addr.port()).await;
    handle.abort();

    let exchange = result.expect("truststore-only must validate the server and succeed");
    assert!(
        response_body(&exchange).contains("sayHelloResponse"),
        "the truststore-only profile must receive the SOAP response"
    );
    assert_eq!(
        state.completed.load(Ordering::SeqCst),
        1,
        "the server must complete one handshake and validate its certificate"
    );
    assert!(
        errors_of(&state).is_empty(),
        "no handshake failure expected: {:?}",
        errors_of(&state)
    );
    assert!(
        state
            .client_cert_der
            .lock()
            .expect("client cert lock")
            .is_none(),
        "a truststore-only profile must not present a client certificate"
    );
    assert_eq!(
        state.requests.load(Ordering::SeqCst),
        1,
        "the SOAP request must reach the server once"
    );
}
