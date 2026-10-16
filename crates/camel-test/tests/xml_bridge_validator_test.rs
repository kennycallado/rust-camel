//! Integration tests for validator (XSD mode) using the xml-bridge binary.
//!
//! **Requires `integration-tests` feature:** `cargo test -p camel-test --features integration-tests`
//! **Requires the xml-bridge binary.** Build it with:
//!   `cd bridges/xml && ./build-native.sh`
//! Or set `CAMEL_XML_BRIDGE_BINARY_PATH` / `XML_BRIDGE_PATH` to the binary path.
//!
//! The schema-set cases below exercise `discover_schema_set` plus the real
//! `XsdBridgeBackend` (native xml-bridge), not a mock: discovery reads the
//! fixtures from disk once, the transmitted set is registered verbatim, and
//! native compilation resolves imports/includes from the supplied keys only.

#![cfg(feature = "integration-tests")]

mod support;

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use camel_api::{Exchange, Message, body::Body};
use camel_component_api::{Component, NoOpComponentContext, ProducerContext};
use camel_component_validator::ValidatorComponent;
use camel_component_validator::config::ValidatorConfig;
use camel_component_validator::error::ValidatorError;
use camel_component_validator::schema_set::{SchemaSet, discover_schema_set};
use camel_component_validator::xsd_bridge::{XsdBridge, XsdBridgeBackend};
use camel_dsl::parse_yaml;
use camel_test::CamelTestContext;
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use support::wait::wait_until;
use support::xml_bridge::require_worktree_native_bridge;
use support::{init_tracing, install_crypto_provider, send_to_direct, test_rt};
use tower::ServiceExt;

// ── Baseline schema (single file, no references) ────────────────────────────

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

// ── Schema-set fixtures ─────────────────────────────────────────────────────

const XS: &str = "http://www.w3.org/2001/XMLSchema";

/// Bounded timeout for a register call, which may start the native bridge.
const REGISTER_TIMEOUT: Duration = Duration::from_secs(120);
/// Bounded timeout for a single native validate RPC. Generous because a
/// validate after an owned child stop also restarts the bridge internally.
const VALIDATE_TIMEOUT: Duration = Duration::from_secs(60);
/// Bounded timeout for stopping an owned child bridge process.
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(30);
/// Overall bound for a reconnect/replay retry predicate.
const REPLAY_DEADLINE: Duration = Duration::from_secs(90);

fn write_file(dir: &Path, rel: &str, contents: &str) {
    let path = dir.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&path, contents).unwrap();
}

fn symlink(target: &Path, link: &Path) {
    std::os::unix::fs::symlink(target, link).unwrap();
}

/// A root schema that imports `type_name` from `location` in `child_ns`.
fn root_importing(child_ns: &str, location: &str, type_name: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<xs:schema xmlns:xs="{XS}" xmlns:c="{child_ns}" targetNamespace="urn:root" elementFormDefault="qualified">
  <xs:import namespace="{child_ns}" schemaLocation="{location}"/>
  <xs:element name="note" type="c:{type_name}"/>
</xs:schema>"#
    )
}

/// A schema that includes `location` in its own namespace.
fn include_schema(ns: &str, location: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<xs:schema xmlns:xs="{XS}" targetNamespace="{ns}" elementFormDefault="qualified">
  <xs:include schemaLocation="{location}"/>
</xs:schema>"#
    )
}

/// A schema defining complex type `type_name` requiring one `required` child.
fn type_schema(ns: &str, type_name: &str, required: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<xs:schema xmlns:xs="{XS}" targetNamespace="{ns}" elementFormDefault="qualified">
  <xs:complexType name="{type_name}">
    <xs:sequence>
      <xs:element name="{required}" type="xs:string"/>
    </xs:sequence>
  </xs:complexType>
</xs:schema>"#
    )
}

/// An empty schema in `ns`, optionally including one location.
fn empty_schema(ns: &str, include: Option<&str>) -> String {
    let include = include
        .map(|loc| format!("\n  <xs:include schemaLocation=\"{loc}\"/>"))
        .unwrap_or_default();
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<xs:schema xmlns:xs="{XS}" targetNamespace="{ns}" elementFormDefault="qualified">{include}
</xs:schema>"#
    )
}

fn note_doc(required: &str) -> String {
    format!(
        r#"<note xmlns="urn:root" xmlns:c="urn:child"><c:{required}>Alice</c:{required}></note>"#
    )
}

fn discover(root: &Path, aliases: &BTreeMap<String, String>) -> SchemaSet {
    discover_schema_set(root, aliases).expect("schema discovery must succeed")
}

/// Builds a `validator:<root>?type=xml&schemaAliases=<pct-json>` operator URI.
fn operator_uri(root: &Path, aliases: &BTreeMap<String, String>) -> String {
    let json = serde_json::to_string(aliases).unwrap();
    let encoded = utf8_percent_encode(&json, NON_ALPHANUMERIC).to_string();
    format!(
        "validator:{}?type=xml&schemaAliases={encoded}",
        root.display()
    )
}

/// Extracts the alias map through the real operator URI parser (never a config
/// literal), proving the supplied aliases arrive via `schemaAliases`.
fn aliases_from_uri(uri: &str) -> BTreeMap<String, String> {
    ValidatorConfig::from_uri(uri)
        .expect("operator URI must parse")
        .schema_aliases
}

/// Constructs a wired-up set directly, bypassing discovery, so a native guard
/// can be exercised without the Rust discovery path rejecting it first.
fn manual_set(
    root_system_id: &str,
    resources: &[(&str, &str)],
    aliases: &[(&str, &str)],
) -> SchemaSet {
    SchemaSet {
        root_system_id: root_system_id.to_string(),
        resources: resources
            .iter()
            .map(|(k, v)| (k.to_string(), v.as_bytes().to_vec()))
            .collect(),
        aliases: aliases
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
    }
}

async fn register_set(backend: &XsdBridgeBackend, set: &SchemaSet) -> String {
    tokio::time::timeout(REGISTER_TIMEOUT, backend.register(&set.cache_key(), set))
        .await
        .expect("register timed out")
        .expect("register must succeed")
}

async fn validate(backend: &XsdBridgeBackend, id: &str, doc: &str) -> Result<(), String> {
    validate_raw(backend, id, doc)
        .await
        .expect("validate timed out")
        .map_err(|e| e.to_string())
}

/// Raw `validate` with a bounded per-RPC timeout, preserving the
/// [`ValidatorError`] variant so a caller can distinguish a transient transport
/// recovery from a domain failure.
async fn validate_raw(
    backend: &XsdBridgeBackend,
    id: &str,
    doc: &str,
) -> Result<Result<(), ValidatorError>, tokio::time::error::Elapsed> {
    tokio::time::timeout(
        VALIDATE_TIMEOUT,
        backend.validate(id, doc.as_bytes().to_vec()),
    )
    .await
}

async fn shutdown(backend: &XsdBridgeBackend) {
    tokio::time::timeout(SHUTDOWN_TIMEOUT, backend.shutdown())
        .await
        .expect("shutdown timed out");
}

/// Asserts a native registration was refused for the expected bounded reason.
///
/// A bridge domain refusal surfaces either as `CompilationFailed` (resolver and
/// compile refusals carry the bounded `SCHEMA_PARSE_ERROR` category) or as a
/// source-less `Transport` for a bounded `SCHEMA_SET_*` category. A real
/// transport failure carries the tonic `Status` in `source`, so an attempted
/// fetch that fails to connect can never satisfy this helper.
fn assert_register_refused(res: Result<String, ValidatorError>, category: &str) {
    let err = res.expect_err("native registration must be refused");
    let (message, source_is_none) = match &err {
        ValidatorError::CompilationFailed { message, source } => (message, source.is_none()),
        ValidatorError::Transport { message, source } => (message, source.is_none()),
        other => panic!("expected a bounded bridge refusal, got: {other}"),
    };
    assert!(
        source_is_none,
        "refusal must be a bridge domain error, not a transport failure carrying a source: {err}"
    );
    assert!(
        message.contains(category),
        "expected refusal category {category:?}, got: {message}"
    );
}

/// Registers a raw (discovery-bypassed) two-entry set and asserts the native
/// bridge refused it with `category`, then stops the owned child.
///
/// This is the shared raw-native refusal probe: it proves the JVM guard is
/// independent of the Rust discovery path and that the refused reference is
/// never fetched (the fixtures point at tripwire URLs / escaping paths).
async fn assert_native_refusal(root: &str, child: &str, category: &str) {
    let set = manual_set("root.xsd", &[("root.xsd", root), ("child.xsd", child)], &[]);
    let backend = XsdBridgeBackend::new();
    let res = tokio::time::timeout(REGISTER_TIMEOUT, backend.register(&set.cache_key(), &set))
        .await
        .expect("register timed out");
    assert_register_refused(res, category);
    shutdown(&backend).await;
}

/// A schema resource carrying a DOCTYPE whose external system id is a tripwire.
fn dtd_resource() -> String {
    format!(
        "<!DOCTYPE xs:schema SYSTEM \"http://127.0.0.1:9/evil.dtd\">\n{}",
        type_schema("urn:child", "noteType", "to")
    )
}

/// True only for a transient bridge-recovery state worth retrying: a transport
/// RPC failure that carries the underlying tonic status in `source` (the bridge
/// is restarting/reseeding). Domain failures — validation constraints, register
/// rejections, malformed sets — never carry that source and must surface
/// immediately.
fn is_transient_recovery(err: &ValidatorError) -> bool {
    matches!(
        err,
        ValidatorError::Transport {
            source: Some(_),
            ..
        }
    )
}

/// Bounded causal predicate: retries `validate` only while it fails with a
/// transient transport-recovery error, until it succeeds or ONE overall
/// deadline passes. A domain failure returns immediately, so a broken reconnect
/// replay is not masked, and a regression cannot be retried away. It never
/// re-registers.
async fn validate_until_replayed(
    backend: &XsdBridgeBackend,
    id: &str,
    doc: &str,
) -> Result<(), String> {
    let mut last = String::new();
    let outcome = tokio::time::timeout(REPLAY_DEADLINE, async {
        loop {
            match backend.validate(id, doc.as_bytes().to_vec()).await {
                Ok(()) => return Ok(()),
                Err(err) if is_transient_recovery(&err) => last = err.to_string(),
                Err(err) => return Err(err.to_string()),
            }
        }
    })
    .await;
    match outcome {
        Ok(Ok(())) => Ok(()),
        Ok(Err(err)) => Err(err),
        Err(_) => Err(format!(
            "replay did not recover before {REPLAY_DEADLINE:?}: {last}"
        )),
    }
}

// ── Baseline XSD validation cases ───────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn xsd_validation_valid_document_passes() {
    init_tracing();
    install_crypto_provider();
    // Requires CAMEL_XML_BRIDGE_BINARY_PATH (set by the test runner) so
    // ValidatorComponent finds the binary without downloading from GitHub.
    let _binary = require_worktree_native_bridge();

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
    let _binary = require_worktree_native_bridge();

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
    let _binary = require_worktree_native_bridge();

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
    let _binary = require_worktree_native_bridge();

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

// ── Schema-set integration cases ────────────────────────────────────────────

/// Root imports a different-TNS child that includes a same-TNS grandchild.
#[tokio::test(flavor = "multi_thread")]
async fn xml_bridge_validator_imports_include_chain_resolves() {
    let _binary = require_worktree_native_bridge();
    install_crypto_provider();
    let dir = tempfile::tempdir().unwrap();
    write_file(
        dir.path(),
        "root.xsd",
        &root_importing("urn:child", "child.xsd", "noteType"),
    );
    write_file(
        dir.path(),
        "child.xsd",
        &include_schema("urn:child", "grandchild.xsd"),
    );
    write_file(
        dir.path(),
        "grandchild.xsd",
        &type_schema("urn:child", "noteType", "to"),
    );

    let set = discover(&dir.path().join("root.xsd"), &BTreeMap::new());
    assert!(
        set.resources.contains_key("child.xsd"),
        "imported child must be transmitted: {:?}",
        set.resources.keys().collect::<Vec<_>>()
    );
    assert!(
        set.resources.contains_key("grandchild.xsd"),
        "included grandchild must be transmitted: {:?}",
        set.resources.keys().collect::<Vec<_>>()
    );

    let backend = XsdBridgeBackend::new();
    let id = register_set(&backend, &set).await;
    // The transitive include chain must resolve natively, not a blank schema:
    // a document satisfying the grandchild type passes and a document
    // violating its constraint fails.
    validate(&backend, &id, &note_doc("to"))
        .await
        .expect("valid chain document must pass");
    let err = validate(&backend, &id, &note_doc("wrong"))
        .await
        .expect_err("constraint violation must fail");
    assert!(
        err.contains("VALIDATION_ERROR") || err.contains("cvc-"),
        "expected a constraint-refusal error, got: {err}"
    );
    shutdown(&backend).await;
}

/// Diamond and cycle terminate; depth cap and unreachable-file exclusion hold.
#[tokio::test(flavor = "multi_thread")]
async fn xml_bridge_validator_diamond_and_cycle_terminate() {
    let _binary = require_worktree_native_bridge();
    install_crypto_provider();
    let dir = tempfile::tempdir().unwrap();
    // Diamond: root includes b and c, both include the empty d (once).
    write_file(
        dir.path(),
        "root.xsd",
        r#"<?xml version="1.0" encoding="UTF-8"?>
<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" targetNamespace="urn:root" elementFormDefault="qualified">
  <xs:include schemaLocation="b.xsd"/>
  <xs:include schemaLocation="c.xsd"/>
  <xs:include schemaLocation="cycle_a.xsd"/>
  <xs:element name="note" type="xs:string"/>
</xs:schema>"#,
    );
    write_file(dir.path(), "b.xsd", &include_schema("urn:root", "d.xsd"));
    write_file(dir.path(), "c.xsd", &include_schema("urn:root", "d.xsd"));
    write_file(dir.path(), "d.xsd", &empty_schema("urn:root", None));
    // Cycle: cycle_a includes cycle_b, which includes cycle_a (same namespace).
    write_file(
        dir.path(),
        "cycle_a.xsd",
        &include_schema("urn:root", "cycle_b.xsd"),
    );
    write_file(
        dir.path(),
        "cycle_b.xsd",
        &include_schema("urn:root", "cycle_a.xsd"),
    );
    // Unrelated sibling: never referenced, so discovery must exclude it.
    write_file(dir.path(), "unrelated.xsd", &empty_schema("urn:root", None));

    let root = dir.path().join("root.xsd");
    let set = discover(&root, &BTreeMap::new());
    for key in ["b.xsd", "c.xsd", "d.xsd", "cycle_a.xsd", "cycle_b.xsd"] {
        assert!(
            set.resources.contains_key(key),
            "reachable '{key}' must be transmitted"
        );
    }
    assert!(
        !set.resources.contains_key("unrelated.xsd"),
        "unreachable file must be excluded"
    );
    let expected_d = empty_schema("urn:root", None);
    assert_eq!(
        set.resources.get("d.xsd").map(Vec::as_slice),
        Some(expected_d.as_bytes()),
        "diamond target d.xsd must be present exactly once with its bytes"
    );

    let backend = XsdBridgeBackend::new();
    let id = register_set(&backend, &set).await;
    validate(&backend, &id, r#"<note xmlns="urn:root">hi</note>"#)
        .await
        .expect("diamond+cycle set must compile and validate");
    shutdown(&backend).await;

    // Depth cap: a 34-deep include chain exceeds MAX_IMPORT_DEPTH (32).
    let deep = tempfile::tempdir().unwrap();
    write_file(
        deep.path(),
        "root.xsd",
        &include_schema("urn:root", "c1.xsd"),
    );
    for i in 1..=34 {
        let next = if i < 34 {
            format!("c{}.xsd", i + 1)
        } else {
            "leaf.xsd".to_string()
        };
        write_file(
            deep.path(),
            &format!("c{i}.xsd"),
            &include_schema("urn:root", &next),
        );
    }
    write_file(deep.path(), "leaf.xsd", &empty_schema("urn:root", None));
    let depth_err = discover_schema_set(&deep.path().join("root.xsd"), &BTreeMap::new())
        .expect_err("depth beyond the cap must be refused");
    assert!(
        depth_err.to_string().contains("depth"),
        "expected a depth-cap error, got: {depth_err}"
    );
}

/// In-root symlink is accepted; an escaping symlink and raw traversal/DTD are refused.
#[tokio::test(flavor = "multi_thread")]
async fn xml_bridge_validator_symlink_and_traversal_rejected() {
    let _binary = require_worktree_native_bridge();
    install_crypto_provider();
    let dir = tempfile::tempdir().unwrap();
    write_file(
        dir.path(),
        "real.xsd",
        &type_schema("urn:child", "noteType", "to"),
    );
    symlink(Path::new("real.xsd"), &dir.path().join("link.xsd"));
    write_file(
        dir.path(),
        "root.xsd",
        &root_importing("urn:child", "link.xsd", "noteType"),
    );

    let root = dir.path().join("root.xsd");
    let set = discover(&root, &BTreeMap::new());
    assert!(
        set.resources.contains_key("link.xsd"),
        "in-root symlink key must be transmitted"
    );
    assert!(
        !set.resources.contains_key("real.xsd"),
        "symlink target must not add an unreferenced key"
    );

    let backend = XsdBridgeBackend::new();
    let id = register_set(&backend, &set).await;
    validate(&backend, &id, &note_doc("to"))
        .await
        .expect("in-root symlink must resolve natively");
    shutdown(&backend).await;

    // Escaping symlink: target lives outside the schema root.
    let outside = tempfile::tempdir().unwrap();
    write_file(
        outside.path(),
        "outside.xsd",
        &type_schema("urn:child", "noteType", "to"),
    );
    let esc_dir = tempfile::tempdir().unwrap();
    symlink(
        &outside.path().join("outside.xsd"),
        &esc_dir.path().join("esc.xsd"),
    );
    write_file(
        esc_dir.path(),
        "root.xsd",
        &root_importing("urn:child", "esc.xsd", "noteType"),
    );
    let esc_err = discover_schema_set(&esc_dir.path().join("root.xsd"), &BTreeMap::new())
        .expect_err("escaping symlink must be refused by discovery");
    assert!(
        esc_err.to_string().contains("outside the schema root"),
        "expected an escape refusal, got: {esc_err}"
    );

    // Raw native negatives bypass Rust discovery, proving the JVM guard is
    // independent: an escaping relative path and a DTD-bearing resource are
    // both refused by the bridge (no fetch).
    let raw_root = root_importing("urn:child", "../escape.xsd", "noteType");
    let raw_child = type_schema("urn:child", "noteType", "to");
    assert_native_refusal(&raw_root, &raw_child, "SCHEMA_PARSE_ERROR").await;

    let ok_root = root_importing("urn:child", "child.xsd", "noteType");
    let dtd_child = dtd_resource();
    assert_native_refusal(&ok_root, &dtd_child, "SCHEMA_SET_RESOURCE_INVALID").await;
}

/// A supplied absolute-URI alias resolves with no fetch; an unsupplied URL is refused.
#[tokio::test(flavor = "multi_thread")]
async fn xml_bridge_validator_supplied_alias_resolves_and_unsupplied_url_refused() {
    let _binary = require_worktree_native_bridge();
    install_crypto_provider();
    let dir = tempfile::tempdir().unwrap();
    write_file(
        dir.path(),
        "root.xsd",
        &root_importing("urn:child", "http://127.0.0.1:9/parent.xsd", "noteType"),
    );
    write_file(
        dir.path(),
        "a/parent.xsd",
        &include_schema("urn:child", "child.xsd"),
    );
    write_file(
        dir.path(),
        "a/child.xsd",
        &type_schema("urn:child", "noteType", "to"),
    );

    let mut aliases = BTreeMap::new();
    aliases.insert(
        "http://127.0.0.1:9/parent.xsd".to_string(),
        "a/parent.xsd".to_string(),
    );
    let uri = operator_uri(&dir.path().join("root.xsd"), &aliases);
    let parsed = aliases_from_uri(&uri);
    assert_eq!(
        parsed
            .get("http://127.0.0.1:9/parent.xsd")
            .map(String::as_str),
        Some("a/parent.xsd"),
        "alias must be parsed from the operator URI"
    );

    let root = dir.path().join("root.xsd");
    let set = discover(&root, &parsed);
    assert!(
        set.resources.contains_key("a/child.xsd"),
        "aliased base must expand its relative child: {:?}",
        set.resources.keys().collect::<Vec<_>>()
    );

    let backend = XsdBridgeBackend::new();
    let id = register_set(&backend, &set).await;
    // Success proves the alias was used, not fetched: the URI is an unreachable
    // tripwire, so an actual fetch would fail the compile.
    validate(&backend, &id, &note_doc("to"))
        .await
        .expect("supplied alias must resolve without a fetch");
    validate(&backend, &id, &note_doc("wrong"))
        .await
        .expect_err("constraint violation must fail");
    shutdown(&backend).await;

    // Unsupplied URL: a raw native registration (discovery bypassed) must be
    // refused by the bridge itself, not only by Rust discovery.
    let unsupplied_root =
        root_importing("urn:child", "http://127.0.0.1:9/external.xsd", "noteType");
    let child = type_schema("urn:child", "noteType", "to");
    let unsupplied = manual_set(
        "root.xsd",
        &[("root.xsd", &unsupplied_root), ("child.xsd", &child)],
        &[],
    );
    let unsupplied_backend = XsdBridgeBackend::new();
    let res = tokio::time::timeout(
        REGISTER_TIMEOUT,
        unsupplied_backend.register(&unsupplied.cache_key(), &unsupplied),
    )
    .await
    .expect("register timed out");
    assert_register_refused(res, "SCHEMA_PARSE_ERROR");
    shutdown(&unsupplied_backend).await;
}

/// A changed child rotates the key; a child stop replays the full set from memory.
///
/// The restart's full-set reseed is awaited before the fresh channel is handed
/// back, and the fixture directory is deleted before the replayed validate, so
/// recovery is proven to come from the retained cache and not a rediscovery.
#[tokio::test(flavor = "multi_thread")]
async fn xml_bridge_validator_changed_child_rotates_and_replays() {
    let _binary = require_worktree_native_bridge();
    install_crypto_provider();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("root.xsd");
    // root imports the child AND an aliased extra schema, so a replay that
    // dropped either children or aliases would fail the native compile.
    write_file(
        dir.path(),
        "root.xsd",
        r#"<?xml version="1.0" encoding="UTF-8"?>
<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" xmlns:c="urn:child"
           targetNamespace="urn:root" elementFormDefault="qualified">
  <xs:import namespace="urn:child" schemaLocation="child.xsd"/>
  <xs:import namespace="urn:extra" schemaLocation="http://127.0.0.1:9/extra.xsd"/>
  <xs:element name="note" type="c:noteType"/>
</xs:schema>"#,
    );
    write_file(
        dir.path(),
        "child.xsd",
        &type_schema("urn:child", "noteType", "to"),
    );
    write_file(
        dir.path(),
        "extra.xsd",
        &type_schema("urn:extra", "extraType", "x"),
    );

    let mut aliases = BTreeMap::new();
    aliases.insert(
        "http://127.0.0.1:9/extra.xsd".to_string(),
        "extra.xsd".to_string(),
    );
    // The alias arrives through the operator URI, never a config literal.
    let uri = operator_uri(&root, &aliases);
    let parsed = aliases_from_uri(&uri);

    let set_old = discover(&root, &parsed);
    assert!(!set_old.aliases.is_empty(), "alias map must be transmitted");
    let old_root_bytes = set_old.root_bytes().to_vec();
    let old_id = set_old.cache_key();

    let backend = XsdBridgeBackend::new();
    register_set(&backend, &set_old).await;
    validate(&backend, &old_id, &note_doc("to"))
        .await
        .expect("old child accepts c:to");

    // Change ONLY the child; the root bytes and alias map are unchanged.
    write_file(
        dir.path(),
        "child.xsd",
        &type_schema("urn:child", "noteType", "from"),
    );
    let set_new = discover(&root, &parsed);
    let new_id = set_new.cache_key();
    assert_ne!(old_id, new_id, "a changed child must rotate the cache key");
    assert_eq!(
        old_root_bytes.as_slice(),
        set_new.root_bytes(),
        "only the child changed; root bytes stay identical"
    );
    register_set(&backend, &set_new).await;
    validate(&backend, &new_id, &note_doc("from"))
        .await
        .expect("new child accepts c:from");
    validate(&backend, &new_id, &note_doc("to"))
        .await
        .expect_err("new child rejects c:to");
    validate(&backend, &old_id, &note_doc("to"))
        .await
        .expect("old child still accepts c:to");

    // Delete every fixture BEFORE the child is stopped, so a replay that touched
    // the filesystem would fail instead of silently re-reading it.
    drop(dir);

    // Stop the owned child; the retained channel then fails transport and the
    // backend restarts + replays the cached sets. We never re-register.
    shutdown(&backend).await;
    validate_until_replayed(&backend, &old_id, &note_doc("to"))
        .await
        .expect("old set must be replayed from the retained snapshot");
    validate_until_replayed(&backend, &new_id, &note_doc("from"))
        .await
        .expect("new set must be replayed from the retained snapshot");
    validate(&backend, &new_id, &note_doc("to"))
        .await
        .expect_err("replayed new set must keep rejecting c:to");
    shutdown(&backend).await;
}

/// A symlinked include cycle compiles; every virtual loop key is transmitted.
#[tokio::test(flavor = "multi_thread")]
async fn xml_bridge_validator_symlink_cycle_compiles_all_keys() {
    let _binary = require_worktree_native_bridge();
    install_crypto_provider();
    let dir = tempfile::tempdir().unwrap();
    write_file(
        dir.path(),
        "root.xsd",
        r#"<?xml version="1.0" encoding="UTF-8"?>
<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" targetNamespace="urn:root" elementFormDefault="qualified">
  <xs:include schemaLocation="loop.xsd"/>
  <xs:element name="note" type="xs:string"/>
</xs:schema>"#,
    );
    write_file(
        dir.path(),
        "loop.xsd",
        &include_schema("urn:root", "loop-link.xsd"),
    );
    // Symlink back to the empty loop.xsd (never the root), so the root's single
    // global element is not duplicated.
    symlink(Path::new("loop.xsd"), &dir.path().join("loop-link.xsd"));

    let root = dir.path().join("root.xsd");
    let set = discover(&root, &BTreeMap::new());
    assert!(
        set.resources.contains_key("loop.xsd") && set.resources.contains_key("loop-link.xsd"),
        "both loop virtual keys must be transmitted: {:?}",
        set.resources.keys().collect::<Vec<_>>()
    );
    assert_eq!(
        set.resources.len(),
        3,
        "root + two loop virtual keys (canonical dedup must not drop a key): {:?}",
        set.resources.keys().collect::<Vec<_>>()
    );

    let backend = XsdBridgeBackend::new();
    let id = register_set(&backend, &set).await;
    validate(&backend, &id, r#"<note xmlns="urn:root">hi</note>"#)
        .await
        .expect("symlink cycle must compile without duplicate-global error");
    shutdown(&backend).await;
}

/// Two virtual bases over one physical shared target expand their own children.
#[tokio::test(flavor = "multi_thread")]
async fn xml_bridge_validator_two_bases_expand_distinct_children() {
    let _binary = require_worktree_native_bridge();
    install_crypto_provider();
    let dir = tempfile::tempdir().unwrap();
    // One physical shared.xsd with no global declarations, and an empty child.
    write_file(
        dir.path(),
        "shared.xsd",
        &empty_schema("urn:root", Some("child.xsd")),
    );
    write_file(dir.path(), "child.xsd", &empty_schema("urn:root", None));
    std::fs::create_dir_all(dir.path().join("a")).unwrap();
    std::fs::create_dir_all(dir.path().join("b")).unwrap();
    symlink(Path::new("../shared.xsd"), &dir.path().join("a/shared.xsd"));
    symlink(Path::new("../shared.xsd"), &dir.path().join("b/shared.xsd"));
    symlink(Path::new("../child.xsd"), &dir.path().join("a/child.xsd"));
    symlink(Path::new("../child.xsd"), &dir.path().join("b/child.xsd"));
    write_file(
        dir.path(),
        "root.xsd",
        r#"<?xml version="1.0" encoding="UTF-8"?>
<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" targetNamespace="urn:root" elementFormDefault="qualified">
  <xs:include schemaLocation="a/shared.xsd"/>
  <xs:include schemaLocation="b/shared.xsd"/>
  <xs:element name="note" type="xs:string"/>
</xs:schema>"#,
    );

    let root = dir.path().join("root.xsd");
    let set = discover(&root, &BTreeMap::new());
    assert!(
        set.resources.contains_key("a/child.xsd") && set.resources.contains_key("b/child.xsd"),
        "both bases must expand their own child key: {:?}",
        set.resources.keys().collect::<Vec<_>>()
    );

    let backend = XsdBridgeBackend::new();
    let id = register_set(&backend, &set).await;
    validate(&backend, &id, r#"<note xmlns="urn:root">hi</note>"#)
        .await
        .expect("two-base shared target must compile natively");
    shutdown(&backend).await;
}

/// A nested importer's base-relative child beats an exact root raw key.
#[tokio::test(flavor = "multi_thread")]
async fn xml_bridge_validator_base_relative_beats_exact_root_key() {
    let _binary = require_worktree_native_bridge();
    install_crypto_provider();
    let dir = tempfile::tempdir().unwrap();
    write_file(
        dir.path(),
        "root.xsd",
        &root_importing("urn:child", "a/parent.xsd", "noteType"),
    );
    write_file(
        dir.path(),
        "a/parent.xsd",
        &include_schema("urn:child", "child.xsd"),
    );
    write_file(
        dir.path(),
        "a/child.xsd",
        &type_schema("urn:child", "noteType", "to"),
    );
    // Root-level decoy with the SAME type name but a different constraint. It is
    // registered (forced into the set by an unused alias seed) but never
    // compiled, because a/parent's relative include resolves to a/child.xsd.
    write_file(
        dir.path(),
        "child.xsd",
        &type_schema("urn:child", "noteType", "from"),
    );

    let mut aliases = BTreeMap::new();
    aliases.insert(
        "http://127.0.0.1:9/decoy.xsd".to_string(),
        "child.xsd".to_string(),
    );
    let uri = operator_uri(&dir.path().join("root.xsd"), &aliases);
    let parsed = aliases_from_uri(&uri);

    let root = dir.path().join("root.xsd");
    let set = discover(&root, &parsed);
    assert!(
        set.resources.contains_key("child.xsd") && set.resources.contains_key("a/child.xsd"),
        "decoy and nested child must both be transmitted: {:?}",
        set.resources.keys().collect::<Vec<_>>()
    );

    let backend = XsdBridgeBackend::new();
    let id = register_set(&backend, &set).await;
    validate(&backend, &id, &note_doc("to"))
        .await
        .expect("base-relative a/child.xsd must be the compiled child");
    validate(&backend, &id, &note_doc("from"))
        .await
        .expect_err("root decoy must not be the compiled child");
    shutdown(&backend).await;
}

/// End to end through the real operator endpoint: alias parsed from the URI.
#[tokio::test(flavor = "multi_thread")]
async fn xml_bridge_validator_alias_uri_end_to_end() {
    let _binary = require_worktree_native_bridge();
    install_crypto_provider();
    let dir = tempfile::tempdir().unwrap();
    write_file(
        dir.path(),
        "root.xsd",
        &root_importing("urn:child", "http://127.0.0.1:9/parent.xsd", "noteType"),
    );
    write_file(
        dir.path(),
        "a/parent.xsd",
        &include_schema("urn:child", "child.xsd"),
    );
    write_file(
        dir.path(),
        "a/child.xsd",
        &type_schema("urn:child", "noteType", "to"),
    );
    // Conflicting root-level decoy, registered via a second alias seed.
    write_file(
        dir.path(),
        "child.xsd",
        &type_schema("urn:child", "noteType", "from"),
    );

    let mut aliases = BTreeMap::new();
    aliases.insert(
        "http://127.0.0.1:9/parent.xsd".to_string(),
        "a/parent.xsd".to_string(),
    );
    aliases.insert(
        "http://127.0.0.1:9/decoy.xsd".to_string(),
        "child.xsd".to_string(),
    );
    let root = dir.path().join("root.xsd");
    let uri = operator_uri(&root, &aliases);
    let parsed = aliases_from_uri(&uri);

    // Discovery assertions mirror the operator path.
    let set = discover(&root, &parsed);
    assert!(
        set.resources.contains_key("a/child.xsd") && set.resources.contains_key("child.xsd"),
        "aliased base and decoy must both be present: {:?}",
        set.resources.keys().collect::<Vec<_>>()
    );

    // The real operator surface: create_endpoint parses schemaAliases from the
    // URI and discovers the graph; registration/validation run through it.
    let component = ValidatorComponent::new();
    let endpoint = component
        .create_endpoint(&uri, &NoOpComponentContext)
        .expect("operator URI must parse and discover the aliased graph");
    drop(dir); // no filesystem access may be needed at native compile time

    let producer = endpoint
        .create_producer(test_rt(), &ProducerContext::new())
        .expect("producer creation succeeds");
    producer
        .oneshot(Exchange::new(Message::new(Body::Xml(note_doc("to")))))
        .await
        .expect("alias must resolve a/child.xsd from the mapped virtual base");
    let producer = endpoint
        .create_producer(test_rt(), &ProducerContext::new())
        .expect("producer creation succeeds");
    producer
        .oneshot(Exchange::new(Message::new(Body::Xml(note_doc("from")))))
        .await
        .expect_err("root-level decoy must not be compiled for the alias");
    component
        .xsd_bridge_backend()
        .expect("xsd backend")
        .shutdown()
        .await;
}

/// Alias resolution uses the mapped virtual base, not the absolute URI base.
#[tokio::test(flavor = "multi_thread")]
async fn xml_bridge_validator_alias_resolves_virtual_base() {
    let _binary = require_worktree_native_bridge();
    install_crypto_provider();
    let dir = tempfile::tempdir().unwrap();
    write_file(
        dir.path(),
        "root.xsd",
        &root_importing("urn:child", "http://127.0.0.1:9/parent.xsd", "noteType"),
    );
    write_file(
        dir.path(),
        "a/parent.xsd",
        &include_schema("urn:child", "child.xsd"),
    );
    write_file(
        dir.path(),
        "a/child.xsd",
        &type_schema("urn:child", "noteType", "to"),
    );

    let mut aliases = BTreeMap::new();
    aliases.insert(
        "http://127.0.0.1:9/parent.xsd".to_string(),
        "a/parent.xsd".to_string(),
    );
    let root = dir.path().join("root.xsd");
    let uri = operator_uri(&root, &aliases);
    let parsed = aliases_from_uri(&uri);
    assert_eq!(
        parsed
            .get("http://127.0.0.1:9/parent.xsd")
            .map(String::as_str),
        Some("a/parent.xsd"),
    );

    let set = discover(&root, &parsed);
    assert!(
        set.resources.contains_key("a/child.xsd"),
        "mapped base must expand to a/child.xsd: {:?}",
        set.resources.keys().collect::<Vec<_>>()
    );
    assert!(
        !set.resources.contains_key("child.xsd"),
        "no root-level child should be involved"
    );

    let backend = XsdBridgeBackend::new();
    let id = register_set(&backend, &set).await;
    validate(&backend, &id, &note_doc("to"))
        .await
        .expect("mapped virtual base must resolve a/child.xsd without a fetch");
    validate(&backend, &id, &note_doc("wrong"))
        .await
        .expect_err("constraint violation must fail");
    shutdown(&backend).await;
}

/// Nested importer plus alias must not double the mapped base.
#[tokio::test(flavor = "multi_thread")]
async fn xml_bridge_validator_nested_importer_alias_single_base() {
    let _binary = require_worktree_native_bridge();
    install_crypto_provider();
    let dir = tempfile::tempdir().unwrap();
    write_file(
        dir.path(),
        "root.xsd",
        r#"<?xml version="1.0" encoding="UTF-8"?>
<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" xmlns:c="urn:child" xmlns:n="urn:nested"
           targetNamespace="urn:root" elementFormDefault="qualified">
  <xs:import namespace="urn:nested" schemaLocation="nested/child.xsd"/>
  <xs:import namespace="urn:child" schemaLocation="http://127.0.0.1:9/parent.xsd"/>
  <xs:element name="note" type="c:childType"/>
</xs:schema>"#,
    );
    write_file(
        dir.path(),
        "nested/child.xsd",
        &type_schema("urn:nested", "nestedType", "to"),
    );
    write_file(
        dir.path(),
        "a/parent.xsd",
        &include_schema("urn:child", "child.xsd"),
    );
    write_file(
        dir.path(),
        "a/child.xsd",
        &type_schema("urn:child", "childType", "to"),
    );
    // Root-level decoy with the same type name; forced in by an alias seed.
    write_file(
        dir.path(),
        "child.xsd",
        &type_schema("urn:child", "childType", "from"),
    );

    let mut aliases = BTreeMap::new();
    aliases.insert(
        "http://127.0.0.1:9/parent.xsd".to_string(),
        "a/parent.xsd".to_string(),
    );
    aliases.insert(
        "http://127.0.0.1:9/decoy.xsd".to_string(),
        "child.xsd".to_string(),
    );
    let root = dir.path().join("root.xsd");
    let uri = operator_uri(&root, &aliases);
    let parsed = aliases_from_uri(&uri);

    let set = discover(&root, &parsed);
    for absent in ["a/a/child.xsd", "nested/a/child.xsd"] {
        assert!(
            !set.resources.contains_key(absent),
            "mapped base must not double: {absent} must be absent"
        );
    }
    assert!(
        set.resources.contains_key("a/child.xsd"),
        "alias must resolve a/child.xsd from the single mapped base: {:?}",
        set.resources.keys().collect::<Vec<_>>()
    );

    let backend = XsdBridgeBackend::new();
    let id = register_set(&backend, &set).await;
    let valid = r#"<note xmlns="urn:root" xmlns:c="urn:child"><c:to>Alice</c:to></note>"#;
    let invalid = r#"<note xmlns="urn:root" xmlns:c="urn:child"><c:from>Alice</c:from></note>"#;
    validate(&backend, &id, valid)
        .await
        .expect("single mapped base must serve a/child.xsd");
    validate(&backend, &id, invalid)
        .await
        .expect_err("decoy constraint must not be the compiled one");
    shutdown(&backend).await;
}

/// In-root `..` normalizes and resolves; escaping `..` and DTD are refused natively.
#[tokio::test(flavor = "multi_thread")]
async fn xml_bridge_validator_in_root_traversal_normalizes() {
    let _binary = require_worktree_native_bridge();
    install_crypto_provider();
    let dir = tempfile::tempdir().unwrap();
    write_file(
        dir.path(),
        "root.xsd",
        &root_importing("urn:child", "a/../child.xsd", "noteType"),
    );
    write_file(
        dir.path(),
        "child.xsd",
        &type_schema("urn:child", "noteType", "to"),
    );

    let root = dir.path().join("root.xsd");
    let set = discover(&root, &BTreeMap::new());
    assert!(
        set.resources.contains_key("child.xsd"),
        "in-root traversal must normalize to child.xsd: {:?}",
        set.resources.keys().collect::<Vec<_>>()
    );
    assert!(
        !set.resources.contains_key("a/../child.xsd"),
        "unnormalized key must not be transmitted"
    );

    let backend = XsdBridgeBackend::new();
    let id = register_set(&backend, &set).await;
    validate(&backend, &id, &note_doc("to"))
        .await
        .expect("normalized in-root traversal must resolve natively");
    shutdown(&backend).await;

    // Raw native negatives (discovery bypassed): escaping `..` and a
    // DTD-bearing resource are both refused by the bridge itself (no fetch).
    let escape_root = root_importing("urn:child", "../escape.xsd", "noteType");
    let child = type_schema("urn:child", "noteType", "to");
    assert_native_refusal(&escape_root, &child, "SCHEMA_PARSE_ERROR").await;

    let ok_root = root_importing("urn:child", "child.xsd", "noteType");
    let dtd_child = dtd_resource();
    assert_native_refusal(&ok_root, &dtd_child, "SCHEMA_SET_RESOURCE_INVALID").await;
}
