//! Integration tests for the xj XML→JSON conversion using the xml-bridge binary.
//!
//! **Requires `integration-tests` feature:** `cargo test -p camel-test --features integration-tests`
//! **Requires the xml-bridge binary.** Build it with:
//!   `cd bridges/xml && ./build-native.sh`
//! Or set `CAMEL_XML_BRIDGE_BINARY_PATH` / `XML_BRIDGE_PATH` to the binary path.

#![cfg(feature = "integration-tests")]

mod support;

use camel_api::{Exchange, Message};
use camel_component_api::{Component, NoOpComponentContext, ProducerContext};
use camel_xj::{XjComponent, XjComponentConfig};
use serde_json::Value;
use support::xml_bridge::require_xml_bridge_binary;
use tower::ServiceExt;

fn build_xj_component() -> XjComponent {
    let binary_path = require_xml_bridge_binary();
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
        .create_producer(&producer_ctx)
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
