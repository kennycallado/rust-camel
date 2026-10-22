//! Native-image regression tests for Xerces schema diagnostics.
//!
//! The Xerces regex engine resolves its diagnostics from the resource bundle
//! `org/apache/xerces/impl/xpath/regex/message*.properties`. A native image
//! that does not package this bundle degrades every `xs:pattern` diagnostic to
//! `Installation Problem??? Couldn't load messages ...`. A JVM test cannot see
//! this (the bundle is on the classpath), so these cases run against the
//! worktree-native xml-bridge binary only.
//!
//! **Requires `integration-tests` feature and the native binary:**
//!   `cargo xtask build-xml-bridge`
//!   `CAMEL_XML_BRIDGE_BINARY_PATH=bridges/xml/build/native/xml-bridge \
//!    cargo test -p camel-test --features integration-tests \
//!    --test xml_bridge_xerces_diagnostics_test`

#![cfg(feature = "integration-tests")]

mod support;

use std::collections::BTreeMap;
use std::time::Duration;

use camel_component_validator::error::ValidatorError;
use camel_component_validator::schema_set::SchemaSet;
use camel_component_validator::xsd_bridge::{XsdBridge, XsdBridgeBackend};
use support::install_crypto_provider;
use support::xml_bridge::require_worktree_native_bridge;

/// Bounded timeout for a register call, which may start the native bridge.
const REGISTER_TIMEOUT: Duration = Duration::from_secs(120);
const VALIDATE_TIMEOUT: Duration = Duration::from_secs(60);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(30);

const PLACEHOLDER: &str = "Installation Problem";

/// A single-file schema whose element `v` is a string restricted by `pattern`.
fn pattern_schema(pattern: &str) -> SchemaSet {
    let xsd = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema">
  <xs:element name="v">
    <xs:simpleType>
      <xs:restriction base="xs:string">
        <xs:pattern value="{pattern}"/>
      </xs:restriction>
    </xs:simpleType>
  </xs:element>
</xs:schema>"#
    );
    SchemaSet {
        root_system_id: "root.xsd".to_string(),
        resources: BTreeMap::from([("root.xsd".to_string(), xsd.into_bytes())]),
        aliases: BTreeMap::new(),
    }
}

/// Registers `pattern`; returns the bridge's compile error text.
async fn register_error(backend: &XsdBridgeBackend, pattern: &str) -> String {
    let set = pattern_schema(pattern);
    let res = tokio::time::timeout(REGISTER_TIMEOUT, backend.register(&set.cache_key(), &set))
        .await
        .expect("register timed out");
    match res.expect_err("an invalid xs:pattern must fail registration") {
        ValidatorError::CompilationFailed { message, .. } => message,
        other => panic!("expected CompilationFailed, got: {other}"),
    }
}

async fn shutdown(backend: &XsdBridgeBackend) {
    tokio::time::timeout(SHUTDOWN_TIMEOUT, backend.shutdown())
        .await
        .expect("shutdown timed out");
}

/// T1: an invalid pattern surfaces the real Xerces message and the pattern.
#[tokio::test(flavor = "multi_thread")]
async fn invalid_pattern_surfaces_real_message() {
    let _binary = require_worktree_native_bridge();
    install_crypto_provider();
    let backend = XsdBridgeBackend::new();
    let msg = register_error(&backend, "[unclosed").await;
    shutdown(&backend).await;

    assert!(
        !msg.contains(PLACEHOLDER),
        "regex bundle missing from native image: {msg}"
    );
    assert!(msg.contains("[unclosed"), "offending pattern lost: {msg}");
    assert!(
        msg.contains("not a valid regular expression"),
        "Xerces schema message lost: {msg}"
    );
    assert!(
        msg.contains("Unexpected end of the pattern in a character class"),
        "Xerces regex parse message lost: {msg}"
    );
}

/// T2: the partner's pattern family compiles and validates.
#[tokio::test(flavor = "multi_thread")]
async fn valid_patterns_compile() {
    let _binary = require_worktree_native_bridge();
    install_crypto_provider();
    let backend = XsdBridgeBackend::new();
    let cases: [(&str, &str, &str); 2] = [
        (
            r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(\.\d+)?(Z|[+-]\d{2}:\d{2})?",
            "2026-10-21T10:15:30Z",
            "2026-10-21",
        ),
        (
            "([0-9])+[.][0-9][0-9] [A-Z][A-Z][A-Z]",
            "12.50 EUR",
            "12.5 EUR",
        ),
    ];
    for (pattern, good, bad) in cases {
        let set = pattern_schema(pattern);
        let id = tokio::time::timeout(REGISTER_TIMEOUT, backend.register(&set.cache_key(), &set))
            .await
            .expect("register timed out")
            .unwrap_or_else(|e| panic!("pattern {pattern:?} must compile: {e}"));
        let doc = |v: &str| format!("<v>{v}</v>").into_bytes();
        tokio::time::timeout(VALIDATE_TIMEOUT, backend.validate(&id, doc(good)))
            .await
            .expect("validate timed out")
            .unwrap_or_else(|e| panic!("{good:?} must match {pattern:?}: {e}"));
        tokio::time::timeout(VALIDATE_TIMEOUT, backend.validate(&id, doc(bad)))
            .await
            .expect("validate timed out")
            .expect_err("a non-matching value must fail validation");
    }
    shutdown(&backend).await;
}

/// T3: a regex-bundle diagnostic is rendered in English, not the placeholder.
#[tokio::test(flavor = "multi_thread")]
async fn bundle_present() {
    let _binary = require_worktree_native_bridge();
    install_crypto_provider();
    let backend = XsdBridgeBackend::new();
    let msg = register_error(&backend, "a{2,1}").await;
    shutdown(&backend).await;

    assert!(
        msg.contains("A min quantity must be <= a max quantity"),
        "regex bundle text not rendered in English: {msg}"
    );
    assert!(!msg.contains(PLACEHOLDER), "placeholder leaked: {msg}");
    assert!(!msg.contains("???"), "unresolved message marker: {msg}");
}
