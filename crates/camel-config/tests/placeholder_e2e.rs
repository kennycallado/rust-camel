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

/// `CAMEL_*` override-merging loader: plain `from_file` keeps the seal
/// (`merge_env = false`); only the `_with_env` family reads the allowlisted
/// `CAMEL_*` overrides.
fn load_with_env(dir: &tempfile::TempDir) -> Result<CamelConfig, config::ConfigError> {
    let path = dir
        .path()
        .join("Camel.toml")
        .to_str()
        .expect("utf-8 temp path")
        .to_string();
    CamelConfig::from_file_with_env(&path)
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

// --- Typed probe at the deserialize boundary (env-int-placeholder-typing) ---

/// `timeout_ms = "${env:CFG_TIMEOUT_MS:-8000}"` with the var unset coerces
/// to the integer default at the deserialize boundary.
#[test]
fn placeholder_int_root_field_coerces_default() {
    let _guard = common::env_lock();
    ensure_unset("CFG_TIMEOUT_MS");
    let dir = write_main(
        r#"timeout_ms = "${env:CFG_TIMEOUT_MS:-8000}"
"#,
        &[],
    );

    let cfg = load(&dir).expect("integer default must coerce at the boundary");
    assert_eq!(cfg.timeout_ms, 8000);
}

/// Same shape with the var SET: the substituted value coerces, not the
/// default.
#[test]
fn placeholder_int_root_field_coerces_env_value() {
    let _guard = common::env_lock();
    let _env = EnvCleanup::set("CFG_TIMEOUT_MS", "9000");
    let dir = write_main(
        r#"timeout_ms = "${env:CFG_TIMEOUT_MS:-8000}"
"#,
        &[],
    );

    let cfg = load(&dir).expect("substituted env value must coerce");
    assert_eq!(cfg.timeout_ms, 9000);
}

/// The clean-integer rule rejects leading zeros: `007` is not a candidate,
/// so the first-pass strict error stands.
#[test]
fn placeholder_int_field_leading_zero_rejected() {
    let _guard = common::env_lock();
    ensure_unset("CFG_T");
    let dir = write_main(
        r#"timeout_ms = "${env:CFG_T:-007}"
"#,
        &[],
    );

    let err = load(&dir).expect_err("leading-zero default must stay rejected");
    let msg = err.to_string();
    assert!(
        msg.contains("timeout_ms") || msg.contains("u64"),
        "error must point at the field/type: {msg}"
    );
}

/// i64 overflow is lexically clean but not clean_i64: no candidate, first
/// error stands.
#[test]
fn placeholder_int_field_overflow_rejected() {
    let _guard = common::env_lock();
    ensure_unset("CFG_T");
    let dir = write_main(
        r#"timeout_ms = "${env:CFG_T:-9223372036854775808}"
"#,
        &[],
    );

    let err = load(&dir).expect_err("i64-overflow default must stay rejected");
    let msg = err.to_string();
    assert!(
        msg.contains("timeout_ms") || msg.contains("u64"),
        "error must point at the field/type: {msg}"
    );
}

/// A non-numeric default is not a candidate; the error names the field.
#[test]
fn placeholder_int_field_notanumber_rejected() {
    let _guard = common::env_lock();
    ensure_unset("CFG_T");
    let dir = write_main(
        r#"timeout_ms = "${env:CFG_T:-notanumber}"
"#,
        &[],
    );

    let err = load(&dir).expect_err("non-numeric default must stay rejected");
    let msg = err.to_string();
    assert!(
        msg.contains("timeout_ms"),
        "error must name the field: {msg}"
    );
}

/// String-typed positions are never probed: pass-1 succeeds, the numeric
/// default stays a string.
#[test]
fn string_field_numeric_default_stays_string() {
    let _guard = common::env_lock();
    ensure_unset("CFG_LL");
    let dir = write_main(
        r#"log_level = "${env:CFG_LL:-8080}"
"#,
        &[],
    );

    let cfg = load(&dir).expect("string field with numeric default must load");
    assert_eq!(cfg.log_level, "8080");
}

/// Literal quoted numerics carry no token, never enter the provenance set,
/// and stay rejected — pinned companion to the coercion tests above.
#[test]
fn literal_quoted_numeric_still_rejected_alongside_probe() {
    let _guard = common::env_lock();
    let dir = write_main(
        r#"log_level = "${env:CFG_LL_OK:-info}"
timeout_ms = "1000"
"#,
        &[],
    );

    let err = load(&dir).expect_err("literal quoted numeric must stay rejected");
    let msg = err.to_string();
    assert!(
        msg.contains("timeout_ms") || msg.contains("u64"),
        "error must point at the field/type: {msg}"
    );
}

/// Unresolved no-default placeholder keeps today's fail-closed surface,
/// naming the variable.
#[test]
fn unresolved_no_default_placeholder_names_variable() {
    let _guard = common::env_lock();
    ensure_unset("CFG_MISSING");
    let dir = write_main(
        r#"timeout_ms = "${env:CFG_MISSING}"
"#,
        &[],
    );

    let err = load(&dir).expect_err("unset no-default var must fail closed");
    let msg = err.to_string();
    assert!(
        msg.contains("CFG_MISSING"),
        "error must name the env var: {msg}"
    );
}

/// Probe cap: nine clean-integer candidates exceed the cap, so the
/// first-pass error stands. The eight-field companion loads, proving the
/// cap (not candidate count alone) declines the nine-field document.
#[test]
fn probe_cap_nine_config_candidates_fails() {
    let _guard = common::env_lock();
    ensure_unset("CFG_CAP_1");
    ensure_unset("CFG_CAP_2");
    ensure_unset("CFG_CAP_3");
    ensure_unset("CFG_CAP_4");
    ensure_unset("CFG_CAP_5");
    ensure_unset("CFG_CAP_6");
    ensure_unset("CFG_CAP_7");
    ensure_unset("CFG_CAP_8");
    ensure_unset("CFG_CAP_9");
    let nine = concat!(
        "timeout_ms = \"${env:CFG_CAP_1:-5000}\"\n",
        "drain_timeout_ms = \"${env:CFG_CAP_2:-10000}\"\n",
        "watch_debounce_ms = \"${env:CFG_CAP_3:-300}\"\n",
        "\n",
        "[supervision]\n",
        "initial_delay_ms = \"${env:CFG_CAP_4:-1000}\"\n",
        "max_delay_ms = \"${env:CFG_CAP_5:-60000}\"\n",
        "\n",
        "[observability.otel]\n",
        "metrics_interval_ms = \"${env:CFG_CAP_6:-60000}\"\n",
        "\n",
        "[observability.health]\n",
        "handler_timeout_ms = \"${env:CFG_CAP_7:-6000}\"\n",
        "\n",
        "[runtime_journal]\n",
        "path = \"cap-test-journal.db\"\n",
        "compaction_threshold_events = \"${env:CFG_CAP_8:-10000}\"\n",
        "\n",
        "[platform]\n",
        "type = \"kubernetes\"\n",
        "lease_duration_secs = \"${env:CFG_CAP_9:-15}\"\n",
    );
    let dir = write_main(nine, &[]);
    let err = load(&dir).expect_err("nine candidates must exceed the probe cap");
    let msg = err.to_string();
    assert!(
        msg.contains("Failed to deserialize merged config"),
        "cap decline must surface the FIRST-PASS deserialize error: {msg}"
    );

    // Companion: drop the journal field → eight candidates → loads.
    let eight = concat!(
        "timeout_ms = \"${env:CFG_CAP_1:-5000}\"\n",
        "drain_timeout_ms = \"${env:CFG_CAP_2:-10000}\"\n",
        "watch_debounce_ms = \"${env:CFG_CAP_3:-300}\"\n",
        "\n",
        "[supervision]\n",
        "initial_delay_ms = \"${env:CFG_CAP_4:-1000}\"\n",
        "max_delay_ms = \"${env:CFG_CAP_5:-60000}\"\n",
        "\n",
        "[observability.otel]\n",
        "metrics_interval_ms = \"${env:CFG_CAP_6:-60000}\"\n",
        "\n",
        "[observability.health]\n",
        "handler_timeout_ms = \"${env:CFG_CAP_7:-6000}\"\n",
        "\n",
        "[platform]\n",
        "type = \"kubernetes\"\n",
        "lease_duration_secs = \"${env:CFG_CAP_9:-15}\"\n",
    );
    let dir = write_main(eight, &[]);
    let cfg = load(&dir).expect("eight candidates must load through the probe");
    assert_eq!(cfg.timeout_ms, 5000);
    assert_eq!(
        cfg.supervision.as_ref().expect("supervision").max_delay_ms,
        60000
    );
}

/// Overrides merge BEFORE resolution (via the `from_file_with_env`
/// loader): a token-bearing `CAMEL_*` override value enters the provenance
/// set and probes exactly like a file-authored leaf.
#[test]
fn token_bearing_override_coerces() {
    let _guard = common::env_lock();
    ensure_unset("CFG_N");
    let _override_var = EnvCleanup::set("CAMEL_TIMEOUT_MS", "${env:CFG_N:-8}");
    let dir = write_main("", &[]);

    let cfg = load_with_env(&dir).expect("token-bearing override must coerce");
    assert_eq!(cfg.timeout_ms, 8);
}

/// Token-free override keeps today's typed contract: `notanumber` on a u64
/// field is a deserialization error (no token → no candidate → first-pass
/// error).
#[test]
fn token_free_override_notanumber_rejected() {
    let _guard = common::env_lock();
    let _override_var = EnvCleanup::set("CAMEL_TIMEOUT_MS", "notanumber");
    let dir = write_main("", &[]);

    let err = load_with_env(&dir).expect_err("token-free non-numeric override must stay rejected");
    let msg = err.to_string();
    assert!(
        msg.contains("timeout_ms") || msg.contains("u64"),
        "typed error must point at the field/type: {msg}"
    );
}
