//! Byte-level charset probes for the xml-bridge native image.
//!
//! These cases pin how the bridge honours the encoding declaration of the
//! input document and the `xsl:output @encoding` of the stylesheet. They only
//! run against the worktree-native binary, so a JVM/classpath test cannot
//! mask a charset table that the native image failed to include.
//!
//! **Requires `integration-tests` feature and the native binary:**
//!   `cargo xtask build-xml-bridge`
//!   `CAMEL_XML_BRIDGE_BINARY_PATH=bridges/xml/build/native/xml-bridge \
//!    cargo test -p camel-test --features integration-tests \
//!    --test xml_bridge_charset_probe_test`

#![cfg(feature = "integration-tests")]

mod support;

use std::path::PathBuf;

use bytes::Bytes;
use camel_api::{CamelError, Exchange, Message, body::Body};
use camel_dsl::parse_yaml;
use camel_test::CamelTestContext;
use camel_xslt::{XsltComponent, XsltComponentConfig};
use support::xml_bridge::require_worktree_native_bridge;
use support::{init_tracing, install_crypto_provider, send_to_direct};

// ── Canonical fixtures ──────────────────────────────────────────────────────

const DOC_8859_15_DECL: &[u8] = b"<?xml version=\"1.0\" encoding=\"ISO-8859-15\"?>";
const DOC_LATIN9_DECL: &[u8] = b"<?xml version=\"1.0\" encoding=\"LATIN-9\"?>";
const DOC_8859_1_DECL: &[u8] = b"<?xml version=\"1.0\" encoding=\"ISO-8859-1\"?>";
const DOC_ISO_8859_15_UNDERSCORE_DECL: &[u8] = b"<?xml version=\"1.0\" encoding=\"ISO_8859-15\"?>";

const DOC_BODY_ASCII: &[u8] = b"<doc><sym>AB-42</sym></doc>";
const DOC_BODY_EURO_8859_15: &[u8] = b"<doc><sym>\xA4</sym></doc>";
const DOC_BODY_CURRENCY_8859_1: &[u8] = b"<doc><sym>\xA4</sym></doc>";

const DOC_UTF8_EURO: &[u8] =
    b"<?xml version=\"1.0\" encoding=\"UTF-8\"?><doc><sym>\xE2\x82\xAC</sym></doc>";
const DOC_UNKNOWN_INPUT: &[u8] =
    b"<?xml version=\"1.0\" encoding=\"X-BOGUS-CHARSET\"?><doc><sym>x</sym></doc>";

const OUT_EURO_UTF8: &[u8] = b"<out>\xE2\x82\xAC</out>";
const OUT_CURRENCY_UTF8: &[u8] = b"<out>\xC2\xA4</out>";
const OUT_8859_15_EURO: &[u8] = b"<out>\xA4</out>";

const DECL_OUT_8859_15: &[u8] = b"<?xml version=\"1.0\" encoding=\"ISO-8859-15\"?>";
const DECL_OUT_8859_1: &[u8] = b"<?xml version=\"1.0\" encoding=\"ISO-8859-1\"?>";
const DECL_OUT_UTF8: &[u8] = b"<?xml version=\"1.0\" encoding=\"UTF-8\"?>";
const OUT_8859_1_CURRENCY: &[u8] = b"<out>\xA4</out>";

// ── Helpers ─────────────────────────────────────────────────────────────────

fn concat_document(declaration: &[u8], body: &[u8]) -> Vec<u8> {
    [declaration, body].concat()
}

/// Writes a stylesheet transforming `/doc` to `<out><xsl:value-of select="sym"/></out>`.
///
/// `None` emits the input-side form (`omit-xml-declaration="yes"`, UTF-8 out);
/// `Some(encoding)` emits the output-side form with the requested encoding.
fn write_stylesheet(output_encoding: Option<&str>) -> tempfile::NamedTempFile {
    let stylesheet = match output_encoding {
        None => concat!(
            r#"<?xml version="1.0" encoding="UTF-8"?>"#,
            r#"<xsl:stylesheet version="3.0" xmlns:xsl="http://www.w3.org/1999/XSL/Transform">"#,
            r#"<xsl:output method="xml" omit-xml-declaration="yes"/>"#,
            r#"<xsl:template match="/doc"><out><xsl:value-of select="sym"/></out></xsl:template>"#,
            r#"</xsl:stylesheet>"#,
        )
        .to_string(),
        Some(encoding) => concat!(
            r#"<?xml version="1.0" encoding="UTF-8"?>"#,
            r#"<xsl:stylesheet version="3.0" xmlns:xsl="http://www.w3.org/1999/XSL/Transform">"#,
            r#"<xsl:output method="xml" encoding="__ENC__"/>"#,
            r#"<xsl:template match="/doc"><out><xsl:value-of select="sym"/></out></xsl:template>"#,
            r#"</xsl:stylesheet>"#,
        )
        .replace("__ENC__", encoding),
    };

    let mut file = tempfile::Builder::new()
        .suffix(".xslt")
        .tempfile()
        .expect("stylesheet tempfile");
    use std::io::Write;
    file.write_all(stylesheet.as_bytes())
        .expect("write stylesheet");
    file
}

/// Runs one transform through a `direct:start` route and returns the raw result bytes.
async fn transform_document(
    binary_path: PathBuf,
    stylesheet: &tempfile::NamedTempFile,
    document: Vec<u8>,
) -> Result<Vec<u8>, CamelError> {
    let xslt_uri = format!("xslt:{}?output=xml", stylesheet.path().display());

    let component = XsltComponent::new(XsltComponentConfig {
        bridge_binary_path: Some(binary_path),
        ..XsltComponentConfig::default()
    });

    let h = CamelTestContext::builder()
        .with_direct()
        .with_component(component)
        .build()
        .await;

    let yaml = format!(
        r#"
routes:
  - id: "xml-bridge-charset-probe"
    from: "direct:start"
    steps:
      - to: "{xslt_uri}"
"#
    );

    for route in parse_yaml(&yaml).expect("YAML parse failed") {
        h.add_route(route).await.expect("add_route failed");
    }

    h.start().await;

    let exchange = Exchange::new(Message::new(Body::Bytes(Bytes::from(document))));
    let outcome = send_to_direct(&h, "direct:start", exchange).await;

    h.stop().await;

    let exchange = outcome?;
    let bytes = exchange
        .input
        .body
        .materialize()
        .await
        .expect("result body must materialize");
    Ok(bytes.to_vec())
}

/// Asserts output starts with `declaration`, is followed by exactly
/// `expected_body`, and contains none of the `forbidden` byte sequences.
fn assert_output_side(
    output: &[u8],
    declaration: &[u8],
    expected_body: &[u8],
    forbidden: &[&[u8]],
) {
    assert!(
        output.starts_with(declaration),
        "output must start with {declaration:?}, got {:?}",
        String::from_utf8_lossy(output)
    );
    assert_eq!(
        &output[declaration.len()..],
        expected_body,
        "body after declaration must be {expected_body:?}, got {:?}",
        String::from_utf8_lossy(output)
    );
    for needle in forbidden {
        assert!(
            !output.windows(needle.len()).any(|window| window == *needle),
            "output must not contain {needle:?}, got {:?}",
            String::from_utf8_lossy(output)
        );
    }
}

fn assert_transform_error(err: &CamelError, case: &str) {
    match err {
        CamelError::ProcessorError(message) => assert!(
            message.contains("TRANSFORM_ERROR:"),
            "{case}: error message must contain TRANSFORM_ERROR:, got: {message}"
        ),
        other => panic!("{case}: expected CamelError::ProcessorError, got: {other}"),
    }
}

// ── Input-side charset probes ───────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn charset_8859_15_ascii_declaration_parses() {
    init_tracing();
    install_crypto_provider();
    let binary_path = require_worktree_native_bridge();

    let document = concat_document(DOC_8859_15_DECL, DOC_BODY_ASCII);
    let stylesheet = write_stylesheet(None);
    let output = transform_document(binary_path, &stylesheet, document)
        .await
        .expect("ISO-8859-15 declared ASCII document must transform");

    assert_eq!(output.as_slice(), b"<out>AB-42</out>");
}

#[tokio::test(flavor = "multi_thread")]
async fn charset_8859_15_euro_input_decodes_as_latin9() {
    init_tracing();
    install_crypto_provider();
    let binary_path = require_worktree_native_bridge();

    let document = concat_document(DOC_8859_15_DECL, DOC_BODY_EURO_8859_15);
    let stylesheet = write_stylesheet(None);
    let output = transform_document(binary_path, &stylesheet, document)
        .await
        .expect("ISO-8859-15 euro byte must decode as U+20AC");

    // 0xA4 in ISO-8859-15 is U+20AC; a Latin-1 misdecode yields C2 A4 instead.
    assert_eq!(output.as_slice(), b"<out>\xE2\x82\xAC</out>");
}

// ── Output-side charset probes ──────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn charset_8859_15_output_side_serialized_byte_exact() {
    init_tracing();
    install_crypto_provider();
    let binary_path = require_worktree_native_bridge();

    let stylesheet = write_stylesheet(Some("ISO-8859-15"));
    let output = transform_document(binary_path, &stylesheet, DOC_UTF8_EURO.to_vec())
        .await
        .expect("ISO-8859-15 output encoding must transform UTF-8 euro input");

    assert_output_side(
        &output,
        DECL_OUT_8859_15,
        OUT_8859_15_EURO,
        &[b"\xE2\x82\xAC", b"&#8364;"],
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn charset_8859_15_round_trip_in_and_out() {
    init_tracing();
    install_crypto_provider();
    let binary_path = require_worktree_native_bridge();

    let document = concat_document(DOC_8859_15_DECL, DOC_BODY_EURO_8859_15);
    let stylesheet = write_stylesheet(Some("ISO-8859-15"));
    let output = transform_document(binary_path, &stylesheet, document)
        .await
        .expect("ISO-8859-15 euro round trip must transform");

    assert_output_side(
        &output,
        DECL_OUT_8859_15,
        OUT_8859_15_EURO,
        &[b"\xE2\x82\xAC", b"&#8364;"],
    );
}

// ── Control cases ───────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn charset_controls_iso8859_1_and_utf8() {
    init_tracing();
    install_crypto_provider();
    let binary_path = require_worktree_native_bridge();

    // (a) ISO-8859-1 input: 0xA4 is the currency sign U+00A4.
    let latin1_document = concat_document(DOC_8859_1_DECL, DOC_BODY_CURRENCY_8859_1);
    let input_stylesheet = write_stylesheet(None);
    let output = transform_document(binary_path.clone(), &input_stylesheet, latin1_document)
        .await
        .expect("ISO-8859-1 currency sign must decode as U+00A4");
    assert_eq!(output.as_slice(), OUT_CURRENCY_UTF8);

    // (b) UTF-8 input: E2 82 AC is the euro sign U+20AC.
    let output = transform_document(
        binary_path.clone(),
        &input_stylesheet,
        DOC_UTF8_EURO.to_vec(),
    )
    .await
    .expect("UTF-8 euro document must transform");
    assert_eq!(output.as_slice(), OUT_EURO_UTF8);

    // (c) ISO-8859-1 output: the U+00A4 currency sign serializes as the single byte 0xA4.
    // The input must stay inside Latin-1 — a U+20AC euro cannot be encoded as ISO-8859-1
    // and would be written as a character reference instead.
    let output_stylesheet = write_stylesheet(Some("ISO-8859-1"));
    let output = transform_document(
        binary_path.clone(),
        &output_stylesheet,
        concat_document(DOC_8859_1_DECL, DOC_BODY_CURRENCY_8859_1),
    )
    .await
    .expect("ISO-8859-1 output encoding must transform ISO-8859-1 input");
    assert_output_side(&output, DECL_OUT_8859_1, OUT_8859_1_CURRENCY, &[]);

    // (d) UTF-8 output: the U+20AC euro sign serializes as E2 82 AC and the
    // declaration names UTF-8.
    let utf8_output_stylesheet = write_stylesheet(Some("UTF-8"));
    let output = transform_document(binary_path, &utf8_output_stylesheet, DOC_UTF8_EURO.to_vec())
        .await
        .expect("UTF-8 output encoding must transform UTF-8 input");
    assert_output_side(&output, DECL_OUT_UTF8, OUT_EURO_UTF8, &[]);
}

#[tokio::test(flavor = "multi_thread")]
async fn charset_alias_latin9_decl_resolves() {
    init_tracing();
    install_crypto_provider();
    let binary_path = require_worktree_native_bridge();

    let stylesheet = write_stylesheet(None);

    // (a) Xerces alias LATIN-9.
    let latin9_document = concat_document(DOC_LATIN9_DECL, DOC_BODY_EURO_8859_15);
    let output = transform_document(binary_path.clone(), &stylesheet, latin9_document)
        .await
        .expect("LATIN-9 alias must resolve to ISO-8859-15");
    assert_eq!(output.as_slice(), OUT_EURO_UTF8);

    // (b) Xerces alias ISO_8859-15 (underscore form).
    let underscore_document =
        concat_document(DOC_ISO_8859_15_UNDERSCORE_DECL, DOC_BODY_EURO_8859_15);
    let output = transform_document(binary_path, &stylesheet, underscore_document)
        .await
        .expect("ISO_8859-15 alias must resolve to ISO-8859-15");
    assert_eq!(output.as_slice(), OUT_EURO_UTF8);
}

// ── Error cases ─────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn charset_unknown_input_name_transform_error() {
    init_tracing();
    install_crypto_provider();
    let binary_path = require_worktree_native_bridge();

    let stylesheet = write_stylesheet(None);
    let err = transform_document(binary_path, &stylesheet, DOC_UNKNOWN_INPUT.to_vec())
        .await
        .expect_err("unknown input charset must fail the transform");

    assert_transform_error(&err, "unknown input charset");
}

#[tokio::test(flavor = "multi_thread")]
async fn charset_unknown_output_name_transform_error() {
    init_tracing();
    install_crypto_provider();
    let binary_path = require_worktree_native_bridge();

    let stylesheet = write_stylesheet(Some("X-BOGUS-CHARSET"));
    let err = transform_document(binary_path, &stylesheet, DOC_UTF8_EURO.to_vec())
        .await
        .expect_err("unknown output charset must fail the transform");

    assert_transform_error(&err, "unknown output charset");
}
