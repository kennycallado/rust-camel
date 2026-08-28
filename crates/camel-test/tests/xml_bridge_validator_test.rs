//! Integration tests for validator (XSD mode) using the xml-bridge binary.
//!
//! **Requires `integration-tests` feature:** `cargo test -p camel-test --features integration-tests`
//! **Requires the xml-bridge binary.** Build it with:
//!   `cd bridges/xml && ./build-native.sh`
//! Or set `CAMEL_XML_BRIDGE_BINARY_PATH` / `XML_BRIDGE_PATH` to the binary path.

#![cfg(feature = "integration-tests")]

mod support;

use std::time::Duration;

use camel_api::{Exchange, Message, body::Body};
use camel_component_validator::ValidatorComponent;
use camel_dsl::parse_yaml;
use camel_test::CamelTestContext;
use support::wait::wait_until;
use support::xml_bridge::require_xml_bridge_binary;
use support::{init_tracing, install_crypto_provider, send_to_direct};

fn write_xsd() -> tempfile::NamedTempFile {
    let mut xsd = tempfile::Builder::new().suffix(".xsd").tempfile().unwrap();
    use std::io::Write;
    xsd.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema">
  <xs:element name="order">
    <xs:complexType>
      <xs:sequence>
        <xs:element name="id" type="xs:string"/>
      </xs:sequence>
    </xs:complexType>
  </xs:element>
</xs:schema>"#,
    )
    .unwrap();
    xsd
}

#[tokio::test(flavor = "multi_thread")]
async fn xsd_validation_valid_document_passes() {
    init_tracing();
    install_crypto_provider();
    // Ensures CAMEL_XML_BRIDGE_BINARY_PATH is set so ValidatorComponent finds
    // the binary without downloading from GitHub.
    let _binary = require_xml_bridge_binary();

    let xsd = write_xsd();
    let validator_uri = format!("validator:{}?type=xml", xsd.path().display());

    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .with_component(ValidatorComponent::new())
        .build()
        .await;

    let yaml = format!(
        r#"
routes:
  - id: "xml-bridge-validator-valid"
    from: "direct:start"
    steps:
      - to: "{validator_uri}"
      - to: "mock:validated"
"#
    );

    for route in parse_yaml(&yaml).expect("YAML parse failed") {
        h.add_route(route).await.unwrap();
    }

    h.start().await;

    let exchange = Exchange::new(Message::new(Body::Xml(
        "<order><id>123</id></order>".to_string(),
    )));
    send_to_direct(&h, "direct:start", exchange)
        .await
        .expect("valid XML should pass XSD validation");

    let endpoint = h.mock().get_endpoint("validated").unwrap();
    wait_until(
        "validator valid message reaches mock",
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
    endpoint.assert_exchange_count(1).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn xsd_validation_invalid_document_returns_error() {
    init_tracing();
    install_crypto_provider();
    let _binary = require_xml_bridge_binary();

    let xsd = write_xsd();
    let validator_uri = format!("validator:{}?type=xml", xsd.path().display());

    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .with_component(ValidatorComponent::new())
        .build()
        .await;

    let yaml = format!(
        r#"
routes:
  - id: "xml-bridge-validator-invalid"
    from: "direct:start"
    steps:
      - to: "{validator_uri}"
      - to: "mock:validated"
"#
    );

    for route in parse_yaml(&yaml).expect("YAML parse failed") {
        h.add_route(route).await.unwrap();
    }

    h.start().await;

    let exchange = Exchange::new(Message::new(Body::Xml("<order></order>".to_string())));
    let err = send_to_direct(&h, "direct:start", exchange)
        .await
        .expect_err("invalid XML should fail XSD validation");

    h.stop().await;

    let msg = err.to_string();
    assert!(
        msg.contains("validation failed")
            || msg.contains("XSD validation failed")
            || msg.contains("VALIDATION_ERROR"),
        "unexpected error message: {msg}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn xsd_validation_xml11_invalid_document_diagnosed() {
    init_tracing();
    install_crypto_provider();
    let _binary = require_xml_bridge_binary();

    let xsd = write_xsd();
    let validator_uri = format!("validator:{}?type=xml", xsd.path().display());

    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .with_component(ValidatorComponent::new())
        .build()
        .await;

    let yaml = format!(
        r#"
routes:
  - id: "xml-bridge-validator-xml11"
    from: "direct:start"
    steps:
      - to: "{validator_uri}"
      - to: "mock:validated"
"#
    );

    for route in parse_yaml(&yaml).expect("YAML parse failed") {
        h.add_route(route).await.unwrap();
    }

    h.start().await;

    // XML 1.1 declaration: XIncludeAwareParserConfiguration eagerly loads the
    // XML 1.1 datatype validator factory (XML11DTDDVFactoryImpl) on version
    // detection. Without native registration that load fails with
    // "Provider ... cannot be found" and the error text would leak the
    // regression signature.
    let exchange = Exchange::new(Message::new(Body::Xml(
        "<?xml version=\"1.1\"?><order></order>".to_string(),
    )));
    let err = send_to_direct(&h, "direct:start", exchange)
        .await
        .expect_err("invalid XML 1.1 document should fail XSD validation");

    h.stop().await;

    let msg = err.to_string();
    assert!(
        !msg.contains("resource bundle"),
        "error message must not contain the missing-resource-bundle regression signature: {msg}"
    );
    assert!(
        !msg.contains("Provider"),
        "error message must not contain the missing-native-factory regression signature: {msg}"
    );
    assert!(
        msg.contains("validation failed")
            || msg.contains("XSD validation failed")
            || msg.contains("VALIDATION_ERROR"),
        "unexpected error message: {msg}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn xsd_validation_doctype_rejected() {
    init_tracing();
    install_crypto_provider();
    let _binary = require_xml_bridge_binary();

    let xsd = write_xsd();
    let validator_uri = format!("validator:{}?type=xml", xsd.path().display());

    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .with_component(ValidatorComponent::new())
        .build()
        .await;

    let yaml = format!(
        r#"
routes:
  - id: "xml-bridge-validator-doctype"
    from: "direct:start"
    steps:
      - to: "{validator_uri}"
      - to: "mock:validated"
"#
    );

    for route in parse_yaml(&yaml).expect("YAML parse failed") {
        h.add_route(route).await.unwrap();
    }

    h.start().await;

    let exchange = Exchange::new(Message::new(Body::Xml(
        r#"<!DOCTYPE order [<!ENTITY xxe SYSTEM "file:///etc/passwd">]>
<order><id>&xxe;</id></order>"#
            .to_string(),
    )));
    let err = send_to_direct(&h, "direct:start", exchange)
        .await
        .expect_err("DOCTYPE with external entity should be rejected");

    h.stop().await;

    let msg = err.to_string();
    assert!(
        !msg.contains("resource bundle"),
        "error message must not contain the missing-resource-bundle regression signature: {msg}"
    );
    assert!(
        msg.contains("DOCTYPE") || msg.contains("doctype") || msg.contains("disallow"),
        "unexpected error message: {msg}"
    );
}
