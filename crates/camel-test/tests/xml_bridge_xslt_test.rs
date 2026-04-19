//! Integration tests for the xslt component using the xml-bridge binary.
//!
//! **Requires `integration-tests` feature:** `cargo test -p camel-test --features integration-tests`
//! **Requires the xml-bridge binary.** Build it with:
//!   `cd bridges/xml && ./build-native.sh`
//! Or set `CAMEL_XML_BRIDGE_BINARY_PATH` / `XML_BRIDGE_PATH` to the binary path.

#![cfg(feature = "integration-tests")]

mod support;

use std::time::Duration;

use camel_api::{Exchange, Message, body::Body};
use camel_dsl::parse_yaml;
use camel_test::CamelTestContext;
use camel_xslt::{XsltComponent, XsltComponentConfig};
use support::wait::wait_until;
use support::xml_bridge::require_xml_bridge_binary;
use support::{init_tracing, send_to_direct};

fn write_xslt() -> tempfile::NamedTempFile {
    let mut xslt = tempfile::Builder::new().suffix(".xslt").tempfile().unwrap();
    use std::io::Write;
    xslt.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<xsl:stylesheet version="3.0" xmlns:xsl="http://www.w3.org/1999/XSL/Transform">
  <xsl:output method="xml" omit-xml-declaration="yes"/>
  <xsl:template match="/order">
    <result>
      <orderId><xsl:value-of select="id"/></orderId>
    </result>
  </xsl:template>
</xsl:stylesheet>"#,
    )
    .unwrap();
    xslt
}

fn write_invalid_xslt() -> tempfile::NamedTempFile {
    let mut xslt = tempfile::Builder::new().suffix(".xslt").tempfile().unwrap();
    use std::io::Write;
    xslt.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<xsl:stylesheet version="3.0" xmlns:xsl="http://www.w3.org/1999/XSL/Transform">
  <xsl:template match="/">
    <broken>
  </xsl:template>
</xsl:stylesheet>"#,
    )
    .unwrap();
    xslt
}

#[tokio::test]
async fn xslt_transform_produces_expected_output() {
    init_tracing();
    let binary_path = require_xml_bridge_binary();

    let xslt = write_xslt();
    let xslt_uri = format!("xslt:{}?output=xml", xslt.path().display());

    let component = XsltComponent::new(XsltComponentConfig {
        bridge_binary_path: Some(binary_path),
        ..XsltComponentConfig::default()
    });

    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .with_component(component)
        .build()
        .await;

    let yaml = format!(
        r#"
routes:
  - id: "xml-bridge-xslt-transform"
    from: "direct:start"
    steps:
      - to: "{xslt_uri}"
      - to: "mock:transformed"
"#
    );

    for route in parse_yaml(&yaml).expect("YAML parse failed") {
        h.add_route(route).await.unwrap();
    }

    h.start().await;

    let exchange = Exchange::new(Message::new(Body::Xml(
        "<order><id>ABC-42</id></order>".to_string(),
    )));
    send_to_direct(&h, "direct:start", exchange)
        .await
        .expect("XSLT transform should succeed");

    let endpoint = h.mock().get_endpoint("transformed").unwrap();
    wait_until(
        "xslt transformed message reaches mock",
        Duration::from_secs(20),
        Duration::from_millis(200),
        || {
            let endpoint = endpoint.clone();
            async move { Ok(!endpoint.get_received_exchanges().await.is_empty()) }
        },
    )
    .await
    .unwrap();

    h.stop().await;

    let exchanges = endpoint.get_received_exchanges().await;
    assert_eq!(exchanges.len(), 1);

    let body_bytes = exchanges[0].input.body.clone().materialize().await.unwrap();
    let body_text = String::from_utf8(body_bytes.to_vec()).unwrap();
    assert!(
        body_text.contains("<result>") && body_text.contains("<orderId>ABC-42</orderId>"),
        "unexpected XSLT output: {body_text}"
    );
}

#[tokio::test]
async fn xslt_compile_error_surfaces_on_transform() {
    init_tracing();
    let binary_path = require_xml_bridge_binary();

    let xslt = write_invalid_xslt();
    let xslt_uri = format!("xslt:{}", xslt.path().display());

    let component = XsltComponent::new(XsltComponentConfig {
        bridge_binary_path: Some(binary_path),
        ..XsltComponentConfig::default()
    });

    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .with_component(component)
        .build()
        .await;

    let yaml = format!(
        r#"
routes:
  - id: "xml-bridge-xslt-compile-error"
    from: "direct:start"
    steps:
      - to: "{xslt_uri}"
      - to: "mock:transformed"
"#
    );

    for route in parse_yaml(&yaml).expect("YAML parse failed") {
        h.add_route(route).await.unwrap();
    }

    h.start().await;

    let exchange = Exchange::new(Message::new(Body::Xml("<order><id>1</id></order>".to_string())));
    let err = send_to_direct(&h, "direct:start", exchange)
        .await
        .expect_err("invalid XSLT should fail on transform");

    h.stop().await;

    let msg = err.to_string();
    assert!(
        msg.contains("compile") || msg.contains("COMPILATION_FAILED") || msg.contains("xslt"),
        "unexpected error message: {msg}"
    );
}
