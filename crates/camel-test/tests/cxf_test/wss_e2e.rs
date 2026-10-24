//! Native end-to-end WS-Security signed-call regression.
//!
//! A nested module of the `cxf_test` integration target (declared with
//! `#[path]`), so the `--test cxf_test` filter keeps working.
//!
//! **Guards the native SAAJ registration regression.** Without the native-image
//! registration that makes the SAAJ message factory resolvable, the native
//! cxf-bridge fails every outbound WS-Security signing before it sends with
//! `jakarta.xml.soap.SOAPException: Unable to create message factory for SOAP:
//! ... Provider com.sun.xml.messaging.saaj.soap.SAAJMetaFactoryImpl not found`.
//! LEG A (producer signing) therefore fails on the native binary before any
//! request reaches the capture service, and passes once SAAJ is registered for
//! the native image. LEG B (consumer verification) pins that a real signed
//! request verifies and exports the known `alice` signer descriptor, so the
//! profile shape exercised by LEG A is also proven on the inbound path. LEG C
//! pins the inbound algorithm allow-list boundary: a producer with no
//! signature-algorithm knobs emits WSS4J's outbound legacy default
//! (`rsa-sha1` + `sha1`), the default consumer profile rejects it as a WSS
//! fault, and a consumer profile that opts in via
//! `allowed_signature_algorithms`/`allowed_digest_algorithms` accepts it.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use camel_api::{Body, Exchange, Message};
use camel_component_cxf::config::CxfSecurityFields;
use camel_component_cxf::{CxfBridgePool, CxfComponent, CxfPoolConfig, CxfProfileConfig};
use camel_dsl::parse_yaml;
use camel_test::CamelTestContext;
use quick_xml::NsReader;
use quick_xml::events::{BytesStart, Event};
use reqwest::StatusCode;

use crate::signer::assert_alice_signer_properties;
use crate::support::cxf::{
    SoapVersion, require_cxf_bridge_binary, start_capturing_mock_soap_service,
};
use crate::support::wait::wait_until;
use crate::support::{bridge_bg_rt, install_crypto_provider, send_to_direct};
use crate::wire::{is_soap_envelope_ns, resolved_name};
use crate::{
    cxf_wsdl_path, cxf_wss_keystore_fixture, cxf_wss_signed_request_with_header_fixture,
    http_client, init_tracing, post_soap, reserve_local_port, wss_security,
    wss_single_profile_component,
};

/// WS-Security (SOAP message security) header namespace.
const WSSE_NS: &str =
    "http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-wssecurity-secext-1.0.xsd";
/// XML digital signature namespace.
const DS_NS: &str = "http://www.w3.org/2000/09/xmldsig#";

/// Legacy RSA-SHA1 signature URI. WSS4J emits this outbound when the profile
/// sets no `signature_algorithm` (LEG C producer).
const LEGACY_SIGNATURE_ALGORITHM: &str = "http://www.w3.org/2000/09/xmldsig#rsa-sha1";
/// Legacy SHA-1 digest URI. WSS4J emits this outbound when the profile sets no
/// `signature_digest_algorithm` (LEG C producer).
const LEGACY_DIGEST_ALGORITHM: &str = "http://www.w3.org/2000/09/xmldsig#sha1";

/// Bound for the eagerly-spawned producer bridge slot init. The pool's own
/// `bridge_start_timeout_ms` is 30 s, so the join must outlast that startup.
const BRIDGE_INIT_BUDGET: Duration = Duration::from_secs(60);

/// Bare `sayHello` operation payload the bridge wraps and signs. The Body
/// root is the WSDL's declared input element `sayHelloRequest` — the listener
/// resolves the payload QName against the selected binding and rejects the
/// bare operation local name.
const SAY_HELLO_PAYLOAD: &str = r#"<hel:sayHelloRequest xmlns:hel="http://example.com/hello"><name>signed</name></hel:sayHelloRequest>"#;

/// Namespace-resolved structure of a captured signed request.
#[derive(Debug, Default)]
struct SignedRequestStructure {
    /// Number of `wsse:Security` headers.
    security_headers: usize,
    /// Number of `ds:Signature` elements.
    signatures: usize,
    /// `URI` attribute values of every `ds:Reference`.
    reference_uris: Vec<String>,
    /// `Id` attribute values found on SOAP `Body` elements.
    body_ids: Vec<String>,
    /// `Algorithm` attribute values of every `ds:SignatureMethod`.
    signature_algorithms: Vec<String>,
    /// `Algorithm` attribute values of every `ds:DigestMethod`.
    digest_algorithms: Vec<String>,
}

/// Parses `xml` with a namespace-aware reader and records only resolved
/// `{namespace}local` element names plus the `URI`/`Id` attributes needed to
/// prove the signature reference covers the SOAP Body.
fn parse_signed_request(xml: &str) -> SignedRequestStructure {
    let mut reader = NsReader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut structure = SignedRequestStructure::default();

    loop {
        match reader.read_resolved_event_into(&mut buf) {
            Ok((resolve, Event::Start(start))) | Ok((resolve, Event::Empty(start))) => {
                let (ns, local) = resolved_name(&resolve, &start);
                if ns == WSSE_NS && local == "Security" {
                    structure.security_headers += 1;
                }
                if ns == DS_NS && local == "Signature" {
                    structure.signatures += 1;
                }
                if ns == DS_NS
                    && local == "Reference"
                    && let Some(uri) = attribute_by_local(&start, "URI")
                {
                    structure.reference_uris.push(uri);
                }
                if ns == DS_NS
                    && local == "SignatureMethod"
                    && let Some(algorithm) = attribute_by_local(&start, "Algorithm")
                {
                    structure.signature_algorithms.push(algorithm);
                }
                if ns == DS_NS
                    && local == "DigestMethod"
                    && let Some(algorithm) = attribute_by_local(&start, "Algorithm")
                {
                    structure.digest_algorithms.push(algorithm);
                }
                if is_soap_envelope_ns(&ns)
                    && local == "Body"
                    && let Some(id) = attribute_by_local(&start, "Id")
                {
                    structure.body_ids.push(id);
                }
            }
            Ok((_, Event::Eof)) => break,
            Ok(_) => {}
            Err(err) => panic!("captured signed request is not well-formed XML: {err}"),
        }
        buf.clear();
    }
    structure
}

/// Returns the value of the first attribute whose local name (prefix stripped)
/// matches `local`.
fn attribute_by_local(start: &BytesStart<'_>, local: &str) -> Option<String> {
    start.attributes().flatten().find_map(|attr| {
        let key = attr.key.as_ref();
        let key_local = key.rsplit(':').next().unwrap_or(key);
        (key_local == local).then(|| attr.value.to_string())
    })
}

/// A producer profile that signs every outbound request with the checked-in
/// `alice` JKS. Fields mirror the JVM signing configuration
/// (`SignedRequestFixtureGeneratorTest` / `WssSecurityProcessorTest`): keystore
/// and password `changeit`, signer alias `alice`, and SHA-256 signature/digest
/// algorithms. SHA-256 is not forced by WSS4J's outbound signer, which still
/// defaults to `rsa-sha1` + `sha1` (LEG C exercises that default). It is forced
/// here because the bridge's DEFAULT inbound allow-list accepts only SHA-2
/// unless the consumer profile opts in via
/// `allowed_signature_algorithms`/`allowed_digest_algorithms`.
/// `security_actions_out` is set explicitly to `Signature` (the same value
/// `SecurityProfile.resolveActionsOrDefault` returns when unset).
async fn signing_producer_component(addr: SocketAddr) -> CxfComponent {
    producer_component(
        addr,
        CxfSecurityFields {
            keystore_path: Some(cxf_wss_keystore_fixture()),
            keystore_password: Some("changeit".to_string()),
            sig_username: Some("alice".to_string()),
            sig_password: Some("changeit".to_string()),
            security_actions_out: Some("Signature".to_string()),
            signature_algorithm: Some(
                "http://www.w3.org/2001/04/xmldsig-more#rsa-sha256".to_string(),
            ),
            signature_digest_algorithm: Some("http://www.w3.org/2001/04/xmlenc#sha256".to_string()),
            ..Default::default()
        },
    )
    .await
}

/// A producer profile that leaves the signature algorithm knobs unset, so
/// WSS4J's outbound defaults (`rsa-sha1` + `sha1`) produce the legacy-signed
/// request LEG C posts to the consumer profiles.
async fn legacy_signing_producer_component(addr: SocketAddr) -> CxfComponent {
    producer_component(
        addr,
        CxfSecurityFields {
            keystore_path: Some(cxf_wss_keystore_fixture()),
            keystore_password: Some("changeit".to_string()),
            sig_username: Some("alice".to_string()),
            sig_password: Some("changeit".to_string()),
            security_actions_out: Some("Signature".to_string()),
            ..Default::default()
        },
    )
    .await
}

/// Shared producer pool setup for the signing profiles above.
async fn producer_component(addr: SocketAddr, security: CxfSecurityFields) -> CxfComponent {
    let pool = Arc::new(
        CxfBridgePool::from_config(CxfPoolConfig {
            profiles: vec![CxfProfileConfig {
                name: "wss_sign".to_string(),
                address: Some(format!("http://{addr}/service")),
                wsdl_path: cxf_wsdl_path(),
                service_name: "{http://example.com/hello}HelloService".to_string(),
                port_name: "{http://example.com/hello}HelloPort".to_string(),
                security,
            }],
            max_bridges: 4,
            bridge_start_timeout_ms: 30_000,
            health_check_interval_ms: 5_000,
            bridge_cache_dir: None,
            version: camel_component_cxf::BRIDGE_VERSION.to_string(),
            bind_address: None,
            listener_tls: None,
            reconnect: Default::default(),
        })
        .expect("valid cxf producer signing pool config"),
    );

    // Eagerly create the bridge slot on the permanent background runtime so the
    // first producer invoke does not race native-bridge startup (and the
    // `send_to_direct` retry budget) and the tonic dispatch task outlives this
    // test's runtime.
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
        panic!("producer signing bridge slot init must complete within {BRIDGE_INIT_BUDGET:?}");
    }

    CxfComponent::new(pool)
}

/// End-to-end native WS-Security closed loop: LEG A signs an outbound producer
/// call and asserts the captured wire carries a `wsse:Security` header with a
/// `ds:Signature` whose `ds:Reference` URI covers the SOAP `Body`; LEG B posts
/// those captured (native-signed) bytes to a WSS consumer and asserts they
/// verify and export the `alice` signer descriptor.
///
/// LEG A is the regression leg: on the native binary without the SAAJ native
/// registration it fails before the request is sent. LEG B closes the loop so
/// a structurally valid but cryptographically broken signature cannot pass.
#[tokio::test(flavor = "multi_thread")]
async fn cxf_native_wss_signed_call_and_verified_signer() {
    init_tracing();
    install_crypto_provider();
    let _binary = require_cxf_bridge_binary();
    let wsdl_path = cxf_wsdl_path();

    // --- LEG A: outbound signature on the native producer path ---
    let (addr, captured) = start_capturing_mock_soap_service(SoapVersion::Soap11).await;
    let h_producer = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .with_component(signing_producer_component(addr).await)
        .build()
        .await;

    let producer_yaml = format!(
        "routes:\n  - id: cxf-wss-signed-call\n    from: direct:start\n    steps:\n      - to: \"cxf://http://{addr}/service?wsdl={wsdl_path}&service={{http://example.com/hello}}HelloService&port={{http://example.com/hello}}HelloPort&operation=sayHello&profile=wss_sign\"\n      - to: \"mock:done\"\n"
    );
    for route in parse_yaml(&producer_yaml).unwrap() {
        h_producer.add_route(route).await.unwrap();
    }
    h_producer.start().await;

    let exchange = Exchange::new(Message::new(Body::Text(SAY_HELLO_PAYLOAD.to_string())));
    let response = send_to_direct(&h_producer, "direct:start", exchange)
        .await
        .expect(
            "outbound WS-Security signing must not fail before the request is sent \
             (regression: native SAAJMetaFactory/MessageFactory registration)",
        );
    let response_body = response.body_as::<String>().unwrap_or_default();
    assert!(
        response_body.contains("Hello World"),
        "the signed call must complete against the capturing service, got: {response_body}"
    );

    let endpoint = h_producer.mock().get_endpoint("done").unwrap();
    wait_until(
        "signed producer delivery to mock",
        Duration::from_secs(5),
        Duration::from_millis(200),
        || {
            let endpoint = endpoint.clone();
            async move { Ok(!endpoint.get_received_exchanges().await.is_empty()) }
        },
    )
    .await
    .unwrap();

    let captured_body = captured
        .lock()
        .expect("captured request lock")
        .first()
        .cloned()
        .expect("capturing service should have recorded one signed request")
        .body;

    let structure = parse_signed_request(&captured_body);
    assert_eq!(
        structure.security_headers, 1,
        "the signed request must carry exactly one wsse:Security header, got {:?}\nbody:\n{}",
        structure, captured_body
    );
    assert_eq!(
        structure.signatures, 1,
        "the signed request must carry exactly one ds:Signature, got {:?}\nbody:\n{}",
        structure, captured_body
    );
    assert!(
        !structure.body_ids.is_empty(),
        "the SOAP Body must carry an Id the signature can reference, got {:?}\nbody:\n{}",
        structure,
        captured_body
    );
    let covers_body = structure.reference_uris.iter().any(|uri| {
        uri.strip_prefix('#')
            .is_some_and(|id| structure.body_ids.iter().any(|body_id| body_id == id))
    });
    assert!(
        covers_body,
        "a ds:Reference URI must point at the SOAP Body Id (URI=\"#...\"), got references \
         {:?} and body ids {:?}\nbody:\n{}",
        structure.reference_uris, structure.body_ids, captured_body
    );

    h_producer.stop().await;

    // --- LEG B: inbound verification exports the verified signer ---
    // allow-free-port: external JVM consumer, see reserve_local_port (bd rc-d70up)
    let port = reserve_local_port();
    let h_consumer = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .with_component(wss_single_profile_component(
            "wss_e2e_consumer",
            None,
            port,
            &wsdl_path,
        ))
        .build()
        .await;

    let consumer_yaml = format!(
        "routes:\n  - id: wss-e2e-consumer\n    from: \"cxf://http://127.0.0.1:{port}/cxf/wss_e2e_consumer?wsdl={wsdl_path}&service={{http://example.com/hello}}HelloService&port={{http://example.com/hello}}HelloPort&profile=wss_e2e_consumer\"\n    steps:\n      - set_body:\n          value:\n            constant: \"<hel:sayHelloResponse xmlns:hel='http://example.com/hello'><return>ok</return></hel:sayHelloResponse>\"\n      - to: \"mock:consumed\"\n"
    );
    for route in parse_yaml(&consumer_yaml).unwrap() {
        h_consumer.add_route(route).await.unwrap();
    }
    h_consumer.start().await;

    let base = format!("http://127.0.0.1:{port}/cxf/wss_e2e_consumer");
    wait_until(
        "wss_e2e_consumer endpoint ready",
        Duration::from_secs(15),
        Duration::from_millis(200),
        || {
            let url = base.clone();
            async move {
                match http_client().get(&url).send().await {
                    Ok(_) => Ok(true),
                    Err(_) => Ok(false),
                }
            }
        },
    )
    .await
    .unwrap();

    // Closed loop: verify the EXACT bytes the native bridge just signed (LEG A
    // captured request), not a static fixture. The consumer's WSS verification
    // must accept the native-produced signature and export the alice signer
    // descriptor — this is what distinguishes "signature-shaped XML" from "a
    // cryptographically valid signature". The signature carries no wsu:Timestamp,
    // so it never expires during the test.
    let res = post_soap(&base, "text/xml", "sayHello", captured_body.clone()).await;
    assert_eq!(
        res.status(),
        StatusCode::OK,
        "the native-signed request must pass WS-Security verification and reach the route, \
         body:\n{captured_body}"
    );

    let consumed = h_consumer.mock().get_endpoint("consumed").unwrap();
    wait_until(
        "wss_e2e_consumer received the signed request",
        Duration::from_secs(10),
        Duration::from_millis(200),
        || {
            let consumed = consumed.clone();
            async move { Ok(!consumed.get_received_exchanges().await.is_empty()) }
        },
    )
    .await
    .unwrap();

    let hits = consumed.get_received_exchanges().await;
    assert_alice_signer_properties(&hits[0]);

    h_consumer.stop().await;
}

/// One WSS consumer profile in its own pool/bridge, with caller-supplied
/// security fields. Mirrors `wss_single_profile_component` (which fixes the
/// security shape to `wss_security`), so LEG C can opt one profile into the
/// legacy algorithm allow-lists.
fn wss_consumer_component(
    name: &str,
    security: CxfSecurityFields,
    port: u16,
    wsdl_path: &str,
) -> CxfComponent {
    let pool = Arc::new(
        CxfBridgePool::from_config(CxfPoolConfig {
            profiles: vec![CxfProfileConfig {
                name: name.to_string(),
                address: None,
                wsdl_path: wsdl_path.to_string(),
                service_name: "{http://example.com/hello}HelloService".to_string(),
                port_name: "{http://example.com/hello}HelloPort".to_string(),
                security,
            }],
            max_bridges: 4,
            bridge_start_timeout_ms: 30_000,
            health_check_interval_ms: 5_000,
            bridge_cache_dir: None,
            version: camel_component_cxf::BRIDGE_VERSION.to_string(),
            bind_address: Some(format!("http://127.0.0.1:{port}/cxf")),
            listener_tls: None,
            reconnect: Default::default(),
        })
        .unwrap(),
    );
    CxfComponent::new(pool)
}

/// Starts a WSS consumer with `security` and waits until the listener accepts
/// HTTP. Returns the context and the profile's base URL.
async fn start_wss_consumer(
    profile: &str,
    security: CxfSecurityFields,
    wsdl_path: &str,
) -> (CamelTestContext, String) {
    // allow-free-port: external JVM consumer, see reserve_local_port (bd rc-d70up)
    let port = reserve_local_port();
    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .with_component(wss_consumer_component(profile, security, port, wsdl_path))
        .build()
        .await;

    let consumer_yaml = format!(
        "routes:\n  - id: {profile}\n    from: \"cxf://http://127.0.0.1:{port}/cxf/{profile}?wsdl={wsdl_path}&service={{http://example.com/hello}}HelloService&port={{http://example.com/hello}}HelloPort&profile={profile}\"\n    steps:\n      - set_body:\n          value:\n            constant: \"<hel:sayHelloResponse xmlns:hel='http://example.com/hello'><return>ok</return></hel:sayHelloResponse>\"\n      - to: \"mock:consumed\"\n"
    );
    for route in parse_yaml(&consumer_yaml).unwrap() {
        h.add_route(route).await.unwrap();
    }
    h.start().await;

    let base = format!("http://127.0.0.1:{port}/cxf/{profile}");
    wait_until(
        "wss consumer endpoint ready",
        Duration::from_secs(15),
        Duration::from_millis(200),
        || {
            let url = base.clone();
            async move {
                match http_client().get(&url).send().await {
                    Ok(_) => Ok(true),
                    Err(_) => Ok(false),
                }
            }
        },
    )
    .await
    .unwrap();

    (h, base)
}

/// Runs the no-knobs producer once against the capturing service and returns
/// the EXACT bytes the native bridge signed. Asserts the signature really
/// carries the legacy `rsa-sha1` + `sha1` URIs, so a change to WSS4J's
/// outbound default cannot silently turn LEG C into a SHA-2 round trip.
async fn capture_legacy_signed_request(wsdl_path: &str) -> String {
    let (addr, captured) = start_capturing_mock_soap_service(SoapVersion::Soap11).await;
    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .with_component(legacy_signing_producer_component(addr).await)
        .build()
        .await;

    let producer_yaml = format!(
        "routes:\n  - id: cxf-wss-legacy-signed-call\n    from: direct:start\n    steps:\n      - to: \"cxf://http://{addr}/service?wsdl={wsdl_path}&service={{http://example.com/hello}}HelloService&port={{http://example.com/hello}}HelloPort&operation=sayHello&profile=wss_sign\"\n"
    );
    for route in parse_yaml(&producer_yaml).unwrap() {
        h.add_route(route).await.unwrap();
    }
    h.start().await;

    let exchange = Exchange::new(Message::new(Body::Text(SAY_HELLO_PAYLOAD.to_string())));
    let response = send_to_direct(&h, "direct:start", exchange)
        .await
        .expect("legacy outbound WS-Security signing must not fail before the request is sent");
    let response_body = response.body_as::<String>().unwrap_or_default();
    assert!(
        response_body.contains("Hello World"),
        "the legacy-signed call must complete against the capturing service, got: {response_body}"
    );

    let captured_body = captured
        .lock()
        .expect("captured request lock")
        .first()
        .cloned()
        .expect("capturing service should have recorded one legacy-signed request")
        .body;
    h.stop().await;

    let structure = parse_signed_request(&captured_body);
    assert!(
        structure
            .signature_algorithms
            .iter()
            .any(|algorithm| algorithm == LEGACY_SIGNATURE_ALGORITHM),
        "the no-knobs producer must sign with WSS4J's legacy rsa-sha1 default, got {:?}\nbody:\n{}",
        structure.signature_algorithms,
        captured_body
    );
    assert!(
        structure
            .digest_algorithms
            .iter()
            .any(|algorithm| algorithm == LEGACY_DIGEST_ALGORITHM),
        "the no-knobs producer must digest with WSS4J's legacy sha1 default, got {:?}\nbody:\n{}",
        structure.digest_algorithms,
        captured_body
    );

    captured_body
}

/// LEG C, opted-in half: the exact legacy-signed bytes are accepted when the
/// consumer profile adds the SHA-1 URIs to its inbound allow-lists. HTTP 200
/// and a recorded route hit prove the request passed WS-Security verification.
///
/// Locally the native bridge must be available through
/// `CAMEL_CXF_BRIDGE_BINARY_PATH` (or its `CXF_BRIDGE_PATH` alias); without it
/// `require_cxf_bridge_binary` fails exactly like the existing legs. CI builds
/// the bridge natively.
#[tokio::test(flavor = "multi_thread")]
async fn wss_e2e_leg_c_opted_in_profile_accepts_legacy_sha1() {
    init_tracing();
    install_crypto_provider();
    let _binary = require_cxf_bridge_binary();
    let wsdl_path = cxf_wsdl_path();

    let captured_body = capture_legacy_signed_request(&wsdl_path).await;

    let mut security = wss_security(None);
    security.allowed_signature_algorithms = Some(LEGACY_SIGNATURE_ALGORITHM.to_string());
    security.allowed_digest_algorithms = Some(LEGACY_DIGEST_ALGORITHM.to_string());
    let (h_consumer, base) =
        start_wss_consumer("wss_e2e_legacy_opt_in", security, &wsdl_path).await;

    let res = post_soap(&base, "text/xml", "sayHello", captured_body.clone()).await;
    assert_eq!(
        res.status(),
        StatusCode::OK,
        "an opted-in consumer profile must accept the legacy-signed request, body:\n{captured_body}"
    );

    let consumed = h_consumer.mock().get_endpoint("consumed").unwrap();
    wait_until(
        "opted-in consumer received the legacy-signed request",
        Duration::from_secs(10),
        Duration::from_millis(200),
        || {
            let consumed = consumed.clone();
            async move { Ok(!consumed.get_received_exchanges().await.is_empty()) }
        },
    )
    .await
    .unwrap();

    let hits = consumed.get_received_exchanges().await;
    assert_alice_signer_properties(&hits[0]);

    h_consumer.stop().await;
}

/// LEG C, default half: the same exact legacy-signed bytes are rejected by a
/// consumer profile that leaves the inbound allow-lists unset (secure default:
/// SHA-2 only). The response is a SOAP 1.1 WSS fault (500, the version's default
/// status) and the route never runs.
#[tokio::test(flavor = "multi_thread")]
async fn wss_e2e_leg_c_default_profile_rejects_legacy_sha1() {
    init_tracing();
    install_crypto_provider();
    let _binary = require_cxf_bridge_binary();
    let wsdl_path = cxf_wsdl_path();

    let captured_body = capture_legacy_signed_request(&wsdl_path).await;
    let (h_consumer, base) =
        start_wss_consumer("wss_e2e_legacy_default", wss_security(None), &wsdl_path).await;

    let res = post_soap(&base, "text/xml", "sayHello", captured_body.clone()).await;
    assert_eq!(
        res.status(),
        StatusCode::INTERNAL_SERVER_ERROR,
        "the default consumer profile must reject the legacy-signed request with the SOAP 1.1 \
         WSS fault status, body:\n{captured_body}"
    );
    let fault_body = res.text().await.unwrap();
    assert!(
        fault_body.contains("Fault"),
        "expected a SOAP fault body for the rejected legacy signature, got: {fault_body}"
    );

    let consumed = h_consumer.mock().get_endpoint("consumed").unwrap();
    assert!(
        consumed.get_received_exchanges().await.is_empty(),
        "the rejected legacy-signed request must never reach the route"
    );

    h_consumer.stop().await;
}

/// Allow-listed application header (T9): a signed SOAP 1.2 request that also
/// carries an application header is accepted, and the header is exported as
/// exchange data only. The profile allow-lists
/// `{http://tempuri.org/commonSchema}requestHeaderType`, so the bridge emits
/// `CxfSoapHeader.requestHeaderType` with the header XML and
/// `CxfSoapHeaderSigned.requestHeaderType = "false"` (the signature covers the
/// Body only). Verified Body-signer metadata remains independent of the unsigned
/// application header, which creates no authenticated principal:
/// `principal_from_exchange` stays `None`.
#[tokio::test(flavor = "multi_thread")]
async fn cxf_native_wss_exports_allow_listed_soap_header() {
    use camel_api::security_policy::principal_from_exchange;

    init_tracing();
    install_crypto_provider();
    let _binary = require_cxf_bridge_binary();
    let wsdl_path = cxf_wsdl_path();

    let fixture_xml = std::fs::read_to_string(cxf_wss_signed_request_with_header_fixture())
        .expect("checked-in signed request with application header must exist");

    let security = CxfSecurityFields {
        exposed_soap_headers: vec![
            "{http://tempuri.org/commonSchema}requestHeaderType".to_string(),
        ],
        ..wss_security(None)
    };
    let (h_consumer, base) =
        start_wss_consumer("wss_e2e_exposed_header", security, &wsdl_path).await;

    let res = post_soap(&base, "text/xml", "sayHello", fixture_xml.clone()).await;
    assert_eq!(
        res.status(),
        StatusCode::OK,
        "the signed request carrying an application header must pass WS-Security verification, \
         body:\n{fixture_xml}"
    );

    let consumed = h_consumer.mock().get_endpoint("consumed").unwrap();
    wait_until(
        "exposed-header consumer received the signed request",
        Duration::from_secs(10),
        Duration::from_millis(200),
        || {
            let consumed = consumed.clone();
            async move { Ok(!consumed.get_received_exchanges().await.is_empty()) }
        },
    )
    .await
    .unwrap();

    let hits = consumed.get_received_exchanges().await;
    let exchange = &hits[0];
    let header_xml = exchange
        .input
        .header("CxfSoapHeader.requestHeaderType")
        .and_then(|value| value.as_str())
        .expect("the allow-listed application header must be exported as exchange data");
    assert!(
        header_xml.contains("organisation"),
        "the exported header must carry its organisation element, got: {header_xml}"
    );
    assert!(
        header_xml.contains("28071"),
        "the exported header must carry its organisation value, got: {header_xml}"
    );
    assert_eq!(
        exchange
            .input
            .header("CxfSoapHeaderSigned.requestHeaderType")
            .and_then(|value| value.as_str()),
        Some("false"),
        "the Body-only signature must not mark the application header as signed"
    );
    assert!(
        principal_from_exchange(exchange).is_none(),
        "an application header is data, not authentication"
    );

    h_consumer.stop().await;
}
