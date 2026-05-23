//! End-to-end transform tests (XJ-005).
//!
//! These tests use a mock XsltTransformBackend to exercise the full path from
//! producer `call()` through the bridge client to verify:
//! - The correct stylesheet is selected based on direction
//! - XML input is passed through unchanged for xml2json direction
//! - JSON input is set as the `jsonInput` parameter for json2xml direction

mod support;

use camel_api::{Exchange, Message};
use camel_component_api::{Body, Component, NoOpComponentContext, ProducerContext};
use camel_xj::{JSON_TO_XML_XSLT, XML_TO_JSON_XSLT};
use support::transform_recorder::XjTestHarness;
use tower::Service;
use tower::ServiceExt;

/// Verify that an xml2json transform passes the XML document to the bridge
/// as the document payload (no jsonInput parameter).
#[tokio::test(flavor = "multi_thread")]
async fn xml2json_e2e_passes_xml_as_document() {
    let harness = XjTestHarness::new();
    let endpoint = harness
        .component
        .create_endpoint(
            "xj:classpath:identity?direction=xml2json",
            &NoOpComponentContext,
        )
        .expect("endpoint creation");

    let ctx = ProducerContext::new();
    let mut producer = endpoint.create_producer(&ctx).expect("producer creation");

    let xml = b"<root><child>hello</child></root>".to_vec();
    let exchange = Exchange::new(Message::new(Body::from(xml.clone())));
    let result = producer
        .ready()
        .await
        .expect("poll_ready")
        .call(exchange)
        .await;
    assert!(
        result.is_ok(),
        "transform should succeed: {:?}",
        result.err()
    );

    let transforms_guard = harness.backend.recorded_transforms();
    let transforms = transforms_guard.lock().unwrap();
    assert_eq!(
        transforms.len(),
        1,
        "should have recorded exactly one transform"
    );
    let recorded = &transforms[0];
    assert_eq!(
        recorded.document, xml,
        "xml2json should pass XML as document payload"
    );
    assert!(
        !recorded.parameters.contains_key("jsonInput"),
        "xml2json should NOT set jsonInput parameter"
    );

    // Verify the correct identity stylesheet was compiled
    let stylesheets = harness.backend.compiled_stylesheets();
    let compiled = stylesheets.lock().unwrap();
    assert_eq!(compiled.len(), 1);
    let stylesheet = compiled.values().next().unwrap();
    assert_eq!(stylesheet.as_slice(), XML_TO_JSON_XSLT.as_bytes());
}

/// Verify that a json2xml transform sets the JSON string as the `jsonInput`
/// parameter and sends a minimal `<root/>` XML document as the document payload.
#[tokio::test(flavor = "multi_thread")]
async fn json2xml_e2e_passes_json_as_parameter() {
    let harness = XjTestHarness::new();
    let endpoint = harness
        .component
        .create_endpoint(
            "xj:classpath:identity?direction=json2xml",
            &NoOpComponentContext,
        )
        .expect("endpoint creation");

    let ctx = ProducerContext::new();
    let mut producer = endpoint.create_producer(&ctx).expect("producer creation");

    let json = r#"{"name":"test","value":42}"#.as_bytes().to_vec();
    let exchange = Exchange::new(Message::new(Body::from(json.clone())));
    let result = producer
        .ready()
        .await
        .expect("poll_ready")
        .call(exchange)
        .await;
    assert!(
        result.is_ok(),
        "transform should succeed: {:?}",
        result.err()
    );

    let transforms_guard = harness.backend.recorded_transforms();
    let transforms = transforms_guard.lock().unwrap();
    assert_eq!(
        transforms.len(),
        1,
        "should have recorded exactly one transform"
    );
    let recorded = &transforms[0];
    // json2xml sends `<root/>` as the document and the JSON string as a parameter
    assert_eq!(
        recorded.document,
        b"<root/>".to_vec(),
        "json2xml should send <root/> as document"
    );
    let json_input = recorded
        .parameters
        .get("jsonInput")
        .expect("jsonInput parameter must be set");
    assert_eq!(
        json_input.as_bytes().to_vec(),
        json,
        "jsonInput should contain the original JSON string"
    );

    // Verify the correct identity stylesheet was compiled
    let stylesheets = harness.backend.compiled_stylesheets();
    let compiled = stylesheets.lock().unwrap();
    assert_eq!(compiled.len(), 1);
    let stylesheet = compiled.values().next().unwrap();
    assert_eq!(stylesheet.as_slice(), JSON_TO_XML_XSLT.as_bytes());
}
