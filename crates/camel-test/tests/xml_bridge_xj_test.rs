//! Integration tests for the xj XML→JSON conversion using the xml-bridge binary.
//!
//! **Requires `integration-tests` feature:** `cargo test -p camel-test --features integration-tests`
//! **Requires the xml-bridge binary.** Build it with:
//!   `cd bridges/xml && ./build-native.sh`
//! Or set `CAMEL_XML_BRIDGE_BINARY_PATH` / `XML_BRIDGE_PATH` to the binary path.

#![cfg(feature = "integration-tests")]

mod support;

/// Suite-level skip gate: before libtest discovers any case, skip the
/// whole binary loudly when the xml-bridge is unprovisioned (selector
/// unset and worktree-native build absent). A set selector — even
/// invalid — always reaches the real suite.
#[ctor::ctor(unsafe)]
fn suite_skip_gate() {
    if !support::bridge_skip::xml_bridge_suite_may_run("xml_bridge_xj_test") {
        std::process::exit(0);
    }
}

use camel_api::{Exchange, Message};
use camel_component_api::{CamelError, Component, NoOpComponentContext, ProducerContext};
use camel_xj::{XjComponent, XjComponentConfig};
use serde_json::Value;
use support::xml_bridge::require_xml_bridge_binary;
use support::{install_crypto_provider, test_rt};
use tower::ServiceExt;

fn build_xj_component() -> XjComponent {
    let binary_path = require_xml_bridge_binary();
    install_crypto_provider();
    XjComponent::new(XjComponentConfig {
        bridge_binary_path: Some(binary_path),
        ..XjComponentConfig::default()
    })
}

async fn transform_xml_to_json(xml: &str) -> Value {
    let component = build_xj_component();
    let endpoint = component
        .create_endpoint(
            "xj:classpath:identity?direction=xml2json",
            &NoOpComponentContext,
        )
        .expect("endpoint creation should succeed");

    let producer_ctx = ProducerContext::new();
    let producer = endpoint
        .create_producer(test_rt(), &producer_ctx)
        .expect("producer creation succeeds");

    let exchange = Exchange::new(Message::new(xml.as_bytes().to_vec()));
    let result = producer
        .oneshot(exchange)
        .await
        .expect("transform should succeed");
    let output = result
        .input
        .body
        .materialize()
        .await
        .expect("body materializes");
    let json_str = String::from_utf8(output.to_vec()).expect("output is valid UTF-8");
    serde_json::from_str(&json_str).expect("output is valid JSON")
}

fn write_custom_order_xslt() -> tempfile::NamedTempFile {
    let mut xslt = tempfile::Builder::new().suffix(".xslt").tempfile().unwrap();
    use std::io::Write;
    xslt.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<xsl:stylesheet version="3.0" xmlns:xsl="http://www.w3.org/1999/XSL/Transform">
  <xsl:output method="xml" omit-xml-declaration="yes"/>
  <xsl:template match="/order">
    <data id="{@id}">
      <name><xsl:value-of select="item"/></name>
      <count><xsl:value-of select="qty"/></count>
    </data>
  </xsl:template>
</xsl:stylesheet>"#,
    )
    .unwrap();
    xslt
}

async fn transform_with_uri(uri: &str, body: &str) -> Result<Exchange, CamelError> {
    let component = build_xj_component();
    let endpoint = component
        .create_endpoint(uri, &NoOpComponentContext)
        .expect("endpoint creation should succeed");
    let producer_ctx = ProducerContext::new();
    let producer = endpoint
        .create_producer(test_rt(), &producer_ctx)
        .expect("producer creation succeeds");
    let exchange = Exchange::new(Message::new(body.as_bytes().to_vec()));
    producer.oneshot(exchange).await
}

#[tokio::test(flavor = "multi_thread")]
async fn xml2json_preserves_attributes() {
    let out = transform_xml_to_json("<user id=\"42\" role=\"admin\">Ken</user>").await;
    let user = &out["user"];
    assert_eq!(user["@id"], "42");
    assert_eq!(user["@role"], "admin");
    assert_eq!(user["#text"], "Ken");
}

#[tokio::test(flavor = "multi_thread")]
async fn xml2json_repeated_siblings_become_array() {
    let out = transform_xml_to_json("<items><item>a</item><item>b</item></items>").await;
    let items = &out["items"]["item"];
    assert!(items.is_array());
    assert_eq!(items.as_array().unwrap().len(), 2);
    assert_eq!(items[0], "a");
    assert_eq!(items[1], "b");
}

#[tokio::test(flavor = "multi_thread")]
async fn xml2json_single_sibling_is_scalar() {
    let out = transform_xml_to_json("<items><item>a</item></items>").await;
    assert_eq!(out["items"]["item"], "a");
}

#[tokio::test(flavor = "multi_thread")]
async fn xml2json_self_closing_and_empty_tag_are_null() {
    let out1 = transform_xml_to_json("<root><e/></root>").await;
    assert_eq!(out1["root"]["e"], Value::Null);
    let out2 = transform_xml_to_json("<root><e></e></root>").await;
    assert_eq!(out2["root"]["e"], Value::Null);
}

#[tokio::test(flavor = "multi_thread")]
async fn xml2json_escapes_special_chars() {
    let out = transform_xml_to_json("<msg>He said \"hello\"</msg>").await;
    let msg = out["msg"].as_str().expect("msg should be string");
    assert!(msg.contains("hello"));
}

#[tokio::test(flavor = "multi_thread")]
async fn xml2json_nested_with_attributes_and_children() {
    let out = transform_xml_to_json("<root x=\"1\"><child y=\"2\">text</child></root>").await;
    let root = &out["root"];
    let child = &root["child"];
    assert_eq!(root["@x"], "1");
    assert_eq!(child["@y"], "2");
    assert_eq!(child["#text"], "text");
}

#[tokio::test(flavor = "multi_thread")]
async fn xml2json_mixed_groups_dont_interfere() {
    let out = transform_xml_to_json("<root><a>1</a><b>2</b><a>3</a></root>").await;
    let root = &out["root"];
    assert!(root["a"].is_array());
    assert_eq!(root["a"][0], "1");
    assert_eq!(root["a"][1], "3");
    assert_eq!(root["b"], "2");
}

#[tokio::test(flavor = "multi_thread")]
async fn xml2json_self_closing_with_attrs_only() {
    let out = transform_xml_to_json("<root><e x=\"1\"/></root>").await;
    let e = &out["root"]["e"];
    assert_eq!(e["@x"], "1");
    assert!(e.get("#text").is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn xml2json_complex_nested_arrays() {
    let out = transform_xml_to_json(
        "<root><items><item>a</item><item>b</item></items><items><item>c</item></items></root>",
    )
    .await;
    let items = &out["root"]["items"];
    assert!(items.is_array());
    assert_eq!(items[0]["item"][0], "a");
    assert_eq!(items[0]["item"][1], "b");
    assert_eq!(items[1]["item"], "c");
}

/// Custom xml2json stylesheet, verbatim from Apache Camel upstream
/// (`components/camel-xj/src/test/resources/helloxml2json.xsl`, Apache-2.0):
/// it wraps the input in an `<object>` element — the JSON-XML intermediate
/// shape Java's X2J processor expects.
fn write_upstream_hello_xml2json_xslt() -> tempfile::NamedTempFile {
    let mut xslt = tempfile::Builder::new().suffix(".xslt").tempfile().unwrap();
    use std::io::Write;
    xslt.write_all(
        br##"<?xml version="1.0" encoding="UTF-8" ?>
<xsl:stylesheet version="1.0"
                xmlns:xsl="http://www.w3.org/1999/XSL/Transform"
                exclude-result-prefixes="#all">

    <xsl:template match="/">
        <object>
            <xsl:copy-of select="*"/>
        </object>
    </xsl:template>
</xsl:stylesheet>"##,
    )
    .unwrap();
    xslt
}

/// Golden chain test with an upstream-traceable custom stylesheet: Apache
/// Camel's `helloxml2json.xsl` with the `X2JOutputStringTest` input
/// `<hello>world!</hello>`. Structural parity with Java X2J: the custom XSL
/// output is converted to JSON, so the body is JSON. Value-mapping
/// divergence (documented in the crate README and CONTEXT.md): Java's
/// processor consumes the `object` wrapper as a JSON-XML container and
/// yields `{"hello":"world!"}` (upstream `X2JOutputStringTest`), while
/// rust-camel's generic XML->JSON conversion keeps element names as literal
/// keys and yields `{"object":{"hello":"world!"}}`. Java's identity path
/// collapses a simple root to `"#text"` (upstream `X2JOutputIdentityTest`).
#[tokio::test(flavor = "multi_thread")]
async fn custom_xml2json_chain_produces_json_upstream_xsl() {
    let xslt = write_upstream_hello_xml2json_xslt();
    let uri = format!("xj:{}?direction=xml2json", xslt.path().display());
    let result = transform_with_uri(&uri, "<hello>world!</hello>").await;
    let exchange = result.expect("chained custom xml2json should succeed");
    let output = exchange
        .input
        .body
        .materialize()
        .await
        .expect("body materializes");
    let json_str = String::from_utf8(output.to_vec()).expect("output is valid UTF-8");
    let out: Value = serde_json::from_str(&json_str).expect("final body is valid JSON");
    assert_eq!(
        out,
        serde_json::json!({"object": {"hello": "world!"}}),
        "upstream custom XSL through the rust-camel conversion step"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn custom_xml2json_malformed_input_is_typed_error() {
    let xslt = write_custom_order_xslt();
    let uri = format!("xj:{}?direction=xml2json", xslt.path().display());
    let result = transform_with_uri(&uri, "<order id=\"42\"><item>Widget</item>").await;
    let err = result.expect_err("malformed XML must fail the exchange");
    let CamelError::ProcessorError(payload) = err else {
        panic!("expected ProcessorError, got a different CamelError variant");
    };
    assert!(
        payload.contains("transform failed")
            || payload.contains("parse")
            || payload.contains("Parser")
            || payload.contains("Premature end of file"),
        "typed bridge error must surface for malformed input, got: {payload}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn custom_xml2json_json_emitting_stylesheet_fails_conversion() {
    // Contract (crate README): the chain is unconditional — a custom xml2json
    // stylesheet must emit XML. A stylesheet that emits JSON text directly is
    // fed to the identity XML->JSON conversion and fails there, same as
    // Apache Camel Java. No output sniffing.
    let mut xslt = tempfile::Builder::new().suffix(".xslt").tempfile().unwrap();
    use std::io::Write;
    xslt.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<xsl:stylesheet version="3.0" xmlns:xsl="http://www.w3.org/1999/XSL/Transform">
  <xsl:output method="text"/>
  <xsl:template match="/">{"already":"json"}</xsl:template>
</xsl:stylesheet>"#,
    )
    .unwrap();
    let uri = format!("xj:{}?direction=xml2json", xslt.path().display());
    let result = transform_with_uri(&uri, "<order/>").await;
    let err = result.expect_err("JSON-emitting custom stylesheet must fail the conversion step");
    let CamelError::ProcessorError(payload) = err else {
        panic!("expected ProcessorError, got a different CamelError variant");
    };
    assert!(
        payload.contains("transform failed")
            || payload.contains("parse")
            || payload.contains("Parser")
            || payload.contains("Premature end of file"),
        "typed conversion failure must surface, got: {payload}"
    );
}
