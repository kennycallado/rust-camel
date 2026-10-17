//! Producer SOAP wire-fidelity checks: a namespace-aware structural model over
//! a captured request plus the single-envelope assertion.
//!
//! This is a nested module of the `cxf_test` integration target (not a
//! separate `tests/*.rs` target), so the `--test cxf_test` filter keeps
//! working.

use camel_api::{Body, Exchange, Message};
use camel_dsl::parse_yaml;
use camel_test::CamelTestContext;
use quick_xml::NsReader;
use quick_xml::events::{BytesStart, Event};
use quick_xml::name::ResolveResult;

use crate::support::cxf::{
    HELLO_NS, SOAP11_ENVELOPE_NS, SOAP12_ENVELOPE_NS, SoapVersion, require_cxf_bridge_binary,
    start_capturing_mock_soap_service,
};
use crate::support::install_crypto_provider;
use crate::support::send_to_direct;
use crate::{
    cxf_wsdl_path, cxf_wsdl_path_soap12, init_tracing, shared_cxf_component,
    shared_cxf_soap12_component,
};

/// Namespace-resolved structure of a captured SOAP request body.
#[derive(Debug, Default)]
struct CapturedSoapStructure {
    /// Resolved `{namespace}local` names of every `Envelope` element.
    envelopes: Vec<String>,
    /// Resolved `{namespace}local` names of every `Body` element.
    bodies: Vec<String>,
    /// Resolved `{namespace}local` names of every element whose parent is a
    /// SOAP `Body`.
    body_children: Vec<String>,
}

/// Parses `xml` with a namespace-aware reader and returns the resolved SOAP
/// structure. Prefixes are never counted; only resolved `{namespace}local`
/// names are recorded, so a `soapenv:`/`soap:` prefix difference is invisible.
fn parse_soap_structure(xml: &str) -> CapturedSoapStructure {
    let mut reader = NsReader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut stack: Vec<(String, String)> = Vec::new();
    let mut structure = CapturedSoapStructure::default();

    loop {
        match reader.read_resolved_event_into(&mut buf) {
            Ok((resolve, Event::Start(start))) => {
                let (ns, local) = resolved_name(&resolve, &start);
                record_element(&mut structure, &stack, &ns, &local);
                stack.push((ns, local));
            }
            Ok((resolve, Event::Empty(start))) => {
                let (ns, local) = resolved_name(&resolve, &start);
                record_element(&mut structure, &stack, &ns, &local);
            }
            Ok((_, Event::End(_))) => {
                stack.pop();
            }
            Ok((_, Event::Eof)) => break,
            Ok(_) => {}
            Err(err) => panic!("captured request body is not well-formed XML: {err}"),
        }
    }
    structure
}

/// Resolves an element to its `(namespace, local name)` pair.
pub(crate) fn resolved_name(
    resolve: &ResolveResult<'_>,
    start: &BytesStart<'_>,
) -> (String, String) {
    let local = start.local_name().as_ref().to_string();
    let ns = match resolve {
        ResolveResult::Bound(ns) => ns.0.to_string(),
        ResolveResult::Unbound => String::new(),
        ResolveResult::Unknown(prefix) => format!("unknown-prefix:{prefix}"),
    };
    (ns, local)
}

/// Records `(ns, local)` into `structure`, using `stack.last()` as the parent.
fn record_element(
    structure: &mut CapturedSoapStructure,
    stack: &[(String, String)],
    ns: &str,
    local: &str,
) {
    if is_soap_envelope_ns(ns) {
        if local == "Envelope" {
            structure.envelopes.push(format!("{{{ns}}}{local}"));
        } else if local == "Body" {
            structure.bodies.push(format!("{{{ns}}}{local}"));
        }
    }

    if let Some((parent_ns, parent_local)) = stack.last()
        && is_soap_envelope_ns(parent_ns)
        && parent_local == "Body"
    {
        structure.body_children.push(format!("{{{ns}}}{local}"));
    }
}

/// True when `ns` is a SOAP 1.1 or SOAP 1.2 envelope namespace.
pub(crate) fn is_soap_envelope_ns(ns: &str) -> bool {
    ns == SOAP11_ENVELOPE_NS || ns == SOAP12_ENVELOPE_NS
}

/// Captures the real producer SOAP wire and asserts the namespace-resolved DOM
/// contains exactly one `Envelope` and one `Body`, with the bare operation
/// element as the only direct `Body` child.
#[tokio::test(flavor = "multi_thread")]
async fn cxf_producer_wire_single_envelope_direct_operation_child() {
    init_tracing();
    install_crypto_provider();
    let _binary = require_cxf_bridge_binary();
    let (addr, captured) = start_capturing_mock_soap_service(SoapVersion::Soap11).await;
    let wsdl_path = cxf_wsdl_path();

    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .with_component(shared_cxf_component().await)
        .build()
        .await;

    let yaml = format!(
        "routes:\n  - id: cxf-wire-envelope-test\n    from: direct:start\n    steps:\n      - to: \"cxf://http://{addr}/service?wsdl={wsdl_path}&service={{http://example.com/hello}}HelloService&port={{http://example.com/hello}}HelloPort&operation=sayHello&profile=test_profile\"\n      - to: \"mock:done\"\n"
    );
    for route in parse_yaml(&yaml).unwrap() {
        h.add_route(route).await.unwrap();
    }

    h.start().await;

    let payload = format!(
        r#"<hel:sayHelloRequest xmlns:hel="{HELLO_NS}"><hel:name>World</hel:name></hel:sayHelloRequest>"#
    );
    let exchange = Exchange::new(Message::new(Body::Text(payload)));
    let response = send_to_direct(&h, "direct:start", exchange)
        .await
        .expect("producer invocation against the capturing SOAP service should succeed");
    let response_body = response.body_as::<String>().unwrap_or_default();
    assert!(
        response_body.contains("Hello World"),
        "expected the fixture response 'Hello World', got: {response_body}"
    );

    // The capturing service records the request before replying, so the capture
    // is available once the roundtrip above completes.
    let captured_request = captured
        .lock()
        .expect("captured request lock")
        .first()
        .cloned()
        .expect("capturing service should have recorded one request");

    let structure = parse_soap_structure(&captured_request.body);

    assert_eq!(
        structure.envelopes,
        vec![format!("{{{SOAP11_ENVELOPE_NS}}}Envelope")],
        "captured body must contain exactly one SOAP Envelope, got {:?}\nbody:\n{}",
        structure.envelopes,
        captured_request.body
    );
    assert_eq!(
        structure.bodies,
        vec![format!("{{{SOAP11_ENVELOPE_NS}}}Body")],
        "captured body must contain exactly one SOAP Body, got {:?}\nbody:\n{}",
        structure.bodies,
        captured_request.body
    );
    assert_eq!(
        structure.body_children,
        vec![format!("{{{HELLO_NS}}}sayHelloRequest")],
        "the operation element must be the only direct Body child (no duplicate \
         Envelope), got {:?}\nbody:\n{}",
        structure.body_children,
        captured_request.body
    );

    let content_type = captured_request
        .headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("content-type"))
        .map(|(_, value)| value.as_str())
        .expect("captured request must carry a content-type header");
    assert!(
        content_type.contains("text/xml"),
        "SOAP 1.1 request content-type must contain text/xml, got: {content_type}"
    );

    let soap_action = captured_request
        .headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("soapaction"))
        .map(|(_, value)| value.as_str())
        .expect("captured request must carry a SOAPAction header");
    assert_eq!(
        soap_action
            .strip_prefix('"')
            .and_then(|action| action.strip_suffix('"'))
            .unwrap_or(soap_action),
        "sayHello",
        "SOAPAction header must match hello.wsdl, got: {soap_action:?}"
    );

    h.stop().await;
}

/// Case-insensitive lookup of a captured request header.
fn captured_header<'a>(
    request: &'a crate::support::cxf::CapturedRequest,
    name: &str,
) -> Option<&'a str> {
    request
        .headers
        .iter()
        .find(|(header, _)| header.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

/// The SOAP 1.2 WSDL port binding makes CXF emit a SOAP 1.2 request: the SOAP
/// 1.2 envelope namespace and an `application/soap+xml` content type carrying
/// the operation action. The capturing service replies version-matched, so a
/// SOAP 1.2 request must roundtrip successfully.
#[tokio::test(flavor = "multi_thread")]
async fn cxf_producer_soap12_binding_emits_soap12() {
    init_tracing();
    install_crypto_provider();
    let _binary = require_cxf_bridge_binary();
    let (addr, captured) = start_capturing_mock_soap_service(SoapVersion::Soap12).await;
    let wsdl_path = cxf_wsdl_path_soap12();

    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .with_component(shared_cxf_soap12_component().await)
        .build()
        .await;

    let yaml = format!(
        "routes:\n  - id: cxf-soap12-binding-test\n    from: direct:start\n    steps:\n      - to: \"cxf://http://{addr}/service?wsdl={wsdl_path}&service={{http://example.com/hello}}HelloService&port={{http://example.com/hello}}HelloPort&operation=sayHello&profile=soap12_profile\"\n      - to: \"mock:done\"\n"
    );
    for route in parse_yaml(&yaml).unwrap() {
        h.add_route(route).await.unwrap();
    }

    h.start().await;

    let payload = format!(
        r#"<hel:sayHelloRequest xmlns:hel="{HELLO_NS}"><hel:name>World</hel:name></hel:sayHelloRequest>"#
    );
    let exchange = Exchange::new(Message::new(Body::Text(payload)));
    let response = send_to_direct(&h, "direct:start", exchange)
        .await
        .expect("SOAP 1.2 producer invocation against the capturing service should succeed");
    let response_body = response.body_as::<String>().unwrap_or_default();
    assert!(
        response_body.contains("Hello World"),
        "expected the SOAP 1.2 fixture response 'Hello World', got: {response_body}"
    );

    let captured_request = captured
        .lock()
        .expect("captured request lock")
        .first()
        .cloned()
        .expect("capturing service should have recorded one request");

    let structure = parse_soap_structure(&captured_request.body);
    assert_eq!(
        structure.envelopes,
        vec![format!("{{{SOAP12_ENVELOPE_NS}}}Envelope")],
        "captured body must use the SOAP 1.2 Envelope namespace, got {:?}\nbody:\n{}",
        structure.envelopes,
        captured_request.body
    );
    assert_eq!(
        structure.bodies,
        vec![format!("{{{SOAP12_ENVELOPE_NS}}}Body")],
        "captured body must use the SOAP 1.2 Body namespace, got {:?}\nbody:\n{}",
        structure.bodies,
        captured_request.body
    );
    assert_eq!(
        structure.body_children,
        vec![format!("{{{HELLO_NS}}}sayHelloRequest")],
        "the operation element must be the only direct Body child, got {:?}\nbody:\n{}",
        structure.body_children,
        captured_request.body
    );

    let content_type = captured_header(&captured_request, "content-type")
        .expect("captured request must carry a content-type header");
    assert!(
        content_type.contains("application/soap+xml"),
        "SOAP 1.2 request content-type must contain application/soap+xml, got: {content_type}"
    );
    assert!(
        content_type.contains("action=") && content_type.contains("sayHello"),
        "SOAP 1.2 content-type must carry the action, got: {content_type}"
    );

    h.stop().await;
}

/// The SOAP 1.1 WSDL port binding makes CXF emit a SOAP 1.1 request: the SOAP
/// 1.1 envelope namespace, a `text/xml` content type, and a `SOAPAction`
/// header.
#[tokio::test(flavor = "multi_thread")]
async fn cxf_producer_soap11_binding_sets_text_xml() {
    init_tracing();
    install_crypto_provider();
    let _binary = require_cxf_bridge_binary();
    let (addr, captured) = start_capturing_mock_soap_service(SoapVersion::Soap11).await;
    let wsdl_path = cxf_wsdl_path();

    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .with_component(shared_cxf_component().await)
        .build()
        .await;

    let yaml = format!(
        "routes:\n  - id: cxf-soap11-binding-test\n    from: direct:start\n    steps:\n      - to: \"cxf://http://{addr}/service?wsdl={wsdl_path}&service={{http://example.com/hello}}HelloService&port={{http://example.com/hello}}HelloPort&operation=sayHello&profile=test_profile\"\n      - to: \"mock:done\"\n"
    );
    for route in parse_yaml(&yaml).unwrap() {
        h.add_route(route).await.unwrap();
    }

    h.start().await;

    let payload = format!(
        r#"<hel:sayHelloRequest xmlns:hel="{HELLO_NS}"><hel:name>World</hel:name></hel:sayHelloRequest>"#
    );
    let exchange = Exchange::new(Message::new(Body::Text(payload)));
    let response = send_to_direct(&h, "direct:start", exchange)
        .await
        .expect("SOAP 1.1 producer invocation against the capturing service should succeed");
    let response_body = response.body_as::<String>().unwrap_or_default();
    assert!(
        response_body.contains("Hello World"),
        "expected the SOAP 1.1 fixture response 'Hello World', got: {response_body}"
    );

    let captured_request = captured
        .lock()
        .expect("captured request lock")
        .first()
        .cloned()
        .expect("capturing service should have recorded one request");

    let structure = parse_soap_structure(&captured_request.body);
    assert_eq!(
        structure.envelopes,
        vec![format!("{{{SOAP11_ENVELOPE_NS}}}Envelope")],
        "captured body must use the SOAP 1.1 Envelope namespace, got {:?}\nbody:\n{}",
        structure.envelopes,
        captured_request.body
    );

    let content_type = captured_header(&captured_request, "content-type")
        .expect("captured request must carry a content-type header");
    assert!(
        content_type.contains("text/xml"),
        "SOAP 1.1 request content-type must contain text/xml, got: {content_type}"
    );

    let soap_action = captured_header(&captured_request, "soapaction")
        .expect("captured request must carry a SOAPAction header");
    assert_eq!(
        soap_action
            .strip_prefix('"')
            .and_then(|action| action.strip_suffix('"'))
            .unwrap_or(soap_action),
        "sayHello",
        "SOAPAction header must match hello.wsdl, got: {soap_action:?}"
    );

    h.stop().await;
}

/// The SOAP version is a property of the selected WSDL port binding, not of a
/// request header. A `soap-version: 1.2` header on a SOAP 1.1 binding must not
/// change the emitted wire: the request stays SOAP 1.1 and completes a
/// roundtrip against the version-matched capturing service.
#[tokio::test(flavor = "multi_thread")]
async fn cxf_producer_soap_version_header_ignored_on_wire() {
    init_tracing();
    install_crypto_provider();
    let _binary = require_cxf_bridge_binary();
    let (addr, captured) = start_capturing_mock_soap_service(SoapVersion::Soap11).await;
    let wsdl_path = cxf_wsdl_path();

    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .with_component(shared_cxf_component().await)
        .build()
        .await;

    let yaml = format!(
        "routes:\n  - id: cxf-soap-version-header-test\n    from: direct:start\n    steps:\n      - to: \"cxf://http://{addr}/service?wsdl={wsdl_path}&service={{http://example.com/hello}}HelloService&port={{http://example.com/hello}}HelloPort&operation=sayHello&profile=test_profile\"\n      - to: \"mock:done\"\n"
    );
    for route in parse_yaml(&yaml).unwrap() {
        h.add_route(route).await.unwrap();
    }

    h.start().await;

    let payload = format!(
        r#"<hel:sayHelloRequest xmlns:hel="{HELLO_NS}"><hel:name>World</hel:name></hel:sayHelloRequest>"#
    );
    let mut message = Message::new(Body::Text(payload));
    message.set_header("soap-version", "1.2");
    let exchange = Exchange::new(message);
    let response = send_to_direct(&h, "direct:start", exchange)
        .await
        .expect("producer invocation with a soap-version header should still succeed");
    let response_body = response.body_as::<String>().unwrap_or_default();
    assert!(
        response_body.contains("Hello World"),
        "expected the fixture response 'Hello World', got: {response_body}"
    );

    let captured_request = captured
        .lock()
        .expect("captured request lock")
        .first()
        .cloned()
        .expect("capturing service should have recorded one request");

    let structure = parse_soap_structure(&captured_request.body);
    assert_eq!(
        structure.envelopes,
        vec![format!("{{{SOAP11_ENVELOPE_NS}}}Envelope")],
        "the soap-version header must be ignored: a SOAP 1.1 binding must emit a \
         SOAP 1.1 Envelope, got {:?}\nbody:\n{}",
        structure.envelopes,
        captured_request.body
    );

    h.stop().await;
}

/// A payload that is already a SOAP Envelope is rejected before any network
/// call. The invocation fails, and the capturing service records nothing. The
/// route uses the known-working SOAP 1.1 profile shared with the success tests,
/// so the failure cannot be a WSDL/port or connectivity error.
#[tokio::test(flavor = "multi_thread")]
async fn cxf_producer_prewrapped_envelope_sends_no_request() {
    init_tracing();
    install_crypto_provider();
    let _binary = require_cxf_bridge_binary();
    let (addr, captured) = start_capturing_mock_soap_service(SoapVersion::Soap11).await;
    let wsdl_path = cxf_wsdl_path();

    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .with_component(shared_cxf_component().await)
        .build()
        .await;

    let yaml = format!(
        "routes:\n  - id: cxf-prewrapped-envelope-test\n    from: direct:start\n    steps:\n      - to: \"cxf://http://{addr}/service?wsdl={wsdl_path}&service={{http://example.com/hello}}HelloService&port={{http://example.com/hello}}HelloPort&operation=sayHello&profile=test_profile\"\n      - to: \"mock:done\"\n"
    );
    for route in parse_yaml(&yaml).unwrap() {
        h.add_route(route).await.unwrap();
    }

    h.start().await;

    let payload = format!(
        r#"<soapenv:Envelope xmlns:soapenv="{SOAP11_ENVELOPE_NS}" xmlns:hel="{HELLO_NS}"><soapenv:Body><hel:sayHelloRequest><hel:name>World</hel:name></hel:sayHelloRequest></soapenv:Body></soapenv:Envelope>"#
    );
    let exchange = Exchange::new(Message::new(Body::Text(payload)));
    let err = send_to_direct(&h, "direct:start", exchange)
        .await
        .expect_err("a pre-wrapped SOAP Envelope payload must be rejected");
    let err_msg = err.to_string();
    assert!(
        err_msg.contains("SOAP Envelope"),
        "the pre-wrapped rejection must be surfaced, not a WSDL/connection error; got: {err_msg}"
    );

    let captured_count = captured.lock().expect("captured request lock").len();
    assert_eq!(
        captured_count, 0,
        "a rejected pre-wrapped Envelope must not reach the network; the capturing \
         service recorded {captured_count} request(s)"
    );

    h.stop().await;
}
