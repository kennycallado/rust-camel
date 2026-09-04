//! Layered environment source tests (ADR-0069 §4).
//!
//! Unit-test module of the lib target, declared in `src/lib.rs` under
//! `#[cfg(test)]`. Every test is hermetic: the ambient lookup is an
//! injected map closure, so no test reads (or writes) the real process
//! environment.

use crate::env_layers::{AmbientLookup, LayeredEnv};
use std::collections::BTreeMap;
use std::sync::Arc;

/// Builds an ambient lookup closure over a fixed (key, value) map.
fn ambient_from(map: &[(&str, &str)]) -> AmbientLookup {
    let map: BTreeMap<String, String> = map
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    Arc::new(move |key| map.get(key).cloned())
}

#[test]
fn env_document_value_wins_over_unlisted_ambient() {
    let doc = BTreeMap::from([("HTTP_PORT".to_string(), "18080".to_string())]);
    let env = LayeredEnv::new(
        doc,
        BTreeMap::new(),
        Vec::new(),
        ambient_from(&[("HTTP_PORT", "8080")]),
    );
    assert_eq!(env.lookup("HTTP_PORT").as_deref(), Some("18080"));
}

#[test]
fn harness_provisioned_wins_over_doc() {
    let env = LayeredEnv::new(
        BTreeMap::from([("KEY".to_string(), "doc-value".to_string())]),
        BTreeMap::from([("KEY".to_string(), "harness-value".to_string())]),
        Vec::new(),
        ambient_from(&[("KEY", "ambient-value")]),
    );
    assert_eq!(env.lookup("KEY").as_deref(), Some("harness-value"));
}

#[test]
fn env_unlisted_ambient_is_invisible() {
    let env = LayeredEnv::new(
        BTreeMap::new(),
        BTreeMap::new(),
        Vec::new(),
        ambient_from(&[("NOPE", "leaked")]),
    );
    assert_eq!(env.lookup("NOPE"), None);

    // The same layered lookup fails closed through the config placeholder
    // resolver: `${env:NOPE}` errors naming the variable.
    let lookup = |key: &str| env.lookup(key);
    let mut tree = toml::Value::Table(toml::toml! { port = "${env:NOPE}" });
    let err = camel_config::config::resolve_tree_with(&mut tree, &lookup)
        .expect_err("unlisted ambient must not resolve");
    assert!(
        err.to_string().contains("NOPE"),
        "error must name the variable: {err}"
    );
}

#[test]
fn env_allowlisted_passthrough_visible() {
    let env = LayeredEnv::new(
        BTreeMap::new(),
        BTreeMap::new(),
        vec!["API_KEY".to_string()],
        ambient_from(&[("API_KEY", "secret-value")]),
    );
    assert_eq!(env.lookup("API_KEY").as_deref(), Some("secret-value"));
}
