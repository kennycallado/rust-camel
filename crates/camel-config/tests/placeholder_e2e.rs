//! End-to-end integration tests for the rewired load path: the merged TOML
//! tree (main file + includes + `CAMEL_*` env overrides) is materialized,
//! walked by `resolve_tree_placeholders`, then strictly deserialized.
//!
//! These tests exercise the REAL production entry (`CamelConfig::from_file`),
//! not the walk in isolation.

use camel_config::CamelConfig;

mod common;

/// Sets a uniquely-named env var and removes it on drop (panic-safe restore).
struct EnvCleanup(&'static str);

impl EnvCleanup {
    fn set(name: &'static str, value: &str) -> Self {
        unsafe { std::env::set_var(name, value) };
        EnvCleanup(name)
    }
}

impl Drop for EnvCleanup {
    fn drop(&mut self) {
        unsafe { std::env::remove_var(self.0) };
    }
}

/// Ensures a uniquely-named env var is ABSENT for the test body and stays
/// absent afterwards.
fn ensure_unset(name: &'static str) {
    unsafe { std::env::remove_var(name) };
}

/// Writes `content` as `Camel.toml` (plus optional sibling files) into a new
/// temp dir and returns the main file's path.
fn write_main(content: &str, siblings: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(dir.path().join("Camel.toml"), content).expect("write Camel.toml");
    for (name, content) in siblings {
        std::fs::write(dir.path().join(name), content)
            .unwrap_or_else(|e| panic!("write {name}: {e}"));
    }
    dir
}

fn load(dir: &tempfile::TempDir) -> Result<CamelConfig, config::ConfigError> {
    let path = dir
        .path()
        .join("Camel.toml")
        .to_str()
        .expect("utf-8 temp path")
        .to_string();
    CamelConfig::from_file(&path)
}

/// An unknown top-level section must resolve through the walk and land in the
/// `_extra` capture with ZERO resolver code changes (anti-regression for the
/// retired hand-enumerated allowlist class).
#[test]
fn future_section_resolves_without_code_change() {
    let _guard = common::env_lock();
    let _env = EnvCleanup::set("RUST_CAMEL_TEST_FUT_A", "fut-val");
    let dir = write_main(
        r#"[future_section]
value = "${env:RUST_CAMEL_TEST_FUT_A}"
"#,
        &[],
    );

    let cfg = load(&dir).expect("future section config must load");
    let future = cfg
        ._extra
        .get("future_section")
        .unwrap_or_else(|| panic!("future_section must land in _extra: {:?}", cfg._extra));
    assert_eq!(
        future.get("value").and_then(|v| v.as_str()),
        Some("fut-val"),
        "walked-tree leaf must be resolved inside _extra"
    );
}

/// A placeholder arriving via an INCLUDE file must resolve — proves the walk
/// runs on the POST-merge builder output, not the pre-builder main-file value.
#[test]
fn placeholder_in_include_file_resolves() {
    let _guard = common::env_lock();
    ensure_unset("RUST_CAMEL_TEST_INC_A");
    let dir = write_main(
        r#"include = ["inc.toml"]"#,
        &[(
            "inc.toml",
            r#"[observability.otel]
endpoint = "${env:RUST_CAMEL_TEST_INC_A:-http://localhost:4317}"
"#,
        )],
    );

    let cfg = load(&dir).expect("include-file placeholder must resolve");
    let otel = cfg
        .observability
        .otel
        .as_ref()
        .expect("otel section from include file");
    assert_eq!(otel.endpoint, "http://localhost:4317");
}

/// rc-xb19 contract under the new syntax: an unset env var behind a strict
/// credential leaf fails closed, never installs the literal.
#[test]
fn security_bearer_token_e2e_never_literal() {
    let _guard = common::env_lock();
    ensure_unset("RUST_CAMEL_TEST_E2E_A");
    let dir = write_main(
        r#"[security.native]
subject = "svc"
bearer_token = "${env:RUST_CAMEL_TEST_E2E_A}"
"#,
        &[],
    );

    let err = load(&dir).expect_err("unset credential var must fail closed");
    let msg = err.to_string();
    assert!(
        msg.contains("security.native.bearer_token"),
        "error must name the field: {msg}"
    );
    assert!(
        msg.contains("RUST_CAMEL_TEST_E2E_A"),
        "error must name the env var: {msg}"
    );
}

/// `${env:NAME:-default}` on a plain leaf honors the default when unset.
#[test]
fn otel_endpoint_default_honored_e2e() {
    let _guard = common::env_lock();
    ensure_unset("RUST_CAMEL_TEST_E2E_B");
    let dir = write_main(
        r#"[observability.otel]
endpoint = "${env:RUST_CAMEL_TEST_E2E_B:-http://localhost:4317}"
"#,
        &[],
    );

    let cfg = load(&dir).expect("otel default must resolve");
    let otel = cfg.observability.otel.as_ref().expect("otel section");
    assert_eq!(otel.endpoint, "http://localhost:4317");
}

/// Array-of-tables leaves are walked with index-aware paths; the unset entry
/// fails closed naming `security.native.credentials[1].secret`.
#[test]
fn nested_array_leaves_walked() {
    let _guard = common::env_lock();
    let _env = EnvCleanup::set("RUST_CAMEL_TEST_CRED0", "cred-0-secret");
    ensure_unset("RUST_CAMEL_TEST_CRED1");
    let dir = write_main(
        r#"[security.native]
subject = "svc"

[[security.native.credentials]]
subject = "cred-0"
secret = "${env:RUST_CAMEL_TEST_CRED0}"

[[security.native.credentials]]
subject = "cred-1"
secret = "${env:RUST_CAMEL_TEST_CRED1}"
"#,
        &[],
    );

    let err = load(&dir).expect_err("unset array-entry secret must fail closed");
    let msg = err.to_string();
    assert!(
        msg.contains("security.native.credentials[1].secret"),
        "error must name the array-indexed field: {msg}"
    );
}

/// Pinned semantics swap: the materialized tree deserializes STRICTLY, so a
/// quoted numeric (previously coerced by the config crate) is now rejected.
#[test]
fn quoted_numeric_root_field_is_rejected_after_materialization() {
    let _guard = common::env_lock();
    let dir = write_main(
        r#"timeout_ms = "1000"
"#,
        &[],
    );

    let err = load(&dir).expect_err("quoted numeric must fail strict from_value");
    let msg = err.to_string();
    assert!(
        msg.contains("timeout_ms") || msg.contains("u64"),
        "error must point at the quoted numeric field: {msg}"
    );
}

/// The retired warn-and-keep passthrough: any legacy `{{...}}` marker on a
/// raw leaf is a hard load error pointing at the `${env:}` replacement forms
/// (successor of the old `test_from_file_unresolved_placeholder_keeps_original_string`).
#[test]
fn legacy_braces_rejected_on_load_path() {
    let _guard = common::env_lock();
    let dir = write_main(
        r#"[components.redis]
url = "redis://{{MISSING_PLACEHOLDER}}"
"#,
        &[],
    );

    let err = load(&dir).expect_err("legacy braces must be rejected on load");
    let msg = err.to_string();
    assert!(
        msg.contains("components.redis.url"),
        "error must name the field: {msg}"
    );
    assert!(
        msg.contains("${env:"),
        "error must point at the replacement syntax: {msg}"
    );
}
