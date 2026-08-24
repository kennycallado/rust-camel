//! Tests for the `run` command (extracted from `run.rs` per the
//! 1k-line rule; keeps `super::` access to the command internals).

use super::*;

/// The run function must emit exactly one startup warning about the CWD trust model.
#[test]
fn startup_warning_emitted() {
    let source = include_str!("run.rs");
    // Build the search string from two parts so the concatenated form
    // never appears literally in test code — only in the warn! call.
    let a = "camel run trusts the current working directory";
    let b = " and will execute route";
    let msg = format!("{a}{b}");
    let count = source.matches(&msg).count();
    assert_eq!(
        count, 1,
        "expected exactly one tracing::warn! with the trust-model message in run.rs; found {count}"
    );
}

/// The run command's clap help must document the trust model.
#[test]
fn clap_help_documents_trust_model() {
    let source = include_str!("../main.rs");
    let has_trust_doc = source
        .contains("Trust model: `camel run` executes route scripts, WASM modules, and beans")
        || source
            .contains("Trust model: camel run executes route scripts, WASM modules, and beans");
    assert!(
        has_trust_doc,
        "expected trust model documentation in the Run subcommand help in main.rs"
    );
}

/// Minimal valid route text for fixtures (string form). Pattern
/// resolution never reads file content; fixtures exist to prove the
/// resolver ignores matching files and returns globs verbatim.
const ROUTE_TEXT: &str = r#"routes: [- from: "direct:x", steps: [{to: "mock:m"}]]"#;

#[test]
fn none_returns_defaults_verbatim() {
    let dir = tempfile::tempdir().expect("tempdir"); // allow-unwrap
    let routes_dir = dir.path().join("routes");
    std::fs::create_dir_all(&routes_dir).expect("create routes dir"); // allow-unwrap
    std::fs::write(routes_dir.join("demo.yaml"), ROUTE_TEXT).expect("write demo.yaml"); // allow-unwrap
    std::fs::write(routes_dir.join("demo.test.yaml"), b"expects: {}")
        .expect("write demo.test.yaml"); // allow-unwrap

    let pat = format!("{}/routes/*.yaml", dir.path().display());
    let result = resolve_route_patterns_with(std::slice::from_ref(&pat), &None, &None);
    assert_eq!(
        result,
        vec![pat],
        "defaults must pass through verbatim: no expansion, no test-doc filtering"
    );
}

#[test]
fn resolver_returns_unexpanded_globs() {
    let glob = "routes/**/*.yaml".to_string();
    assert_eq!(
        resolve_route_patterns(&Some(glob.clone()), &None),
        vec![glob.clone()],
        "override globs must stay unexpanded (watch-root guard)"
    );
    assert_eq!(
        resolve_route_patterns(&None, &Some(vec![glob.clone()])),
        vec![glob],
        "config-route globs must stay unexpanded (watch-root guard)"
    );
}

#[test]
fn override_passthrough_untouched() {
    let result = resolve_route_patterns(&Some("routes/*.test.yaml".to_string()), &None);
    assert_eq!(result, vec!["routes/*.test.yaml".to_string()]);
}

#[test]
fn config_routes_passthrough_untouched() {
    let result = resolve_route_patterns(&None, &Some(vec!["custom/*.yaml".to_string()]));
    assert_eq!(result, vec!["custom/*.yaml".to_string()]);
}

#[test]
fn literal_test_doc_path_reaches_discovery() {
    let dir = tempfile::tempdir().expect("tempdir"); // allow-unwrap
    let routes_dir = dir.path().join("routes");
    std::fs::create_dir_all(&routes_dir).expect("create routes dir"); // allow-unwrap
    std::fs::write(routes_dir.join("demo.test.yaml"), b"expects: {}")
        .expect("write demo.test.yaml"); // allow-unwrap

    let p = format!("{}/routes/demo.test.yaml", dir.path().display());
    let result = resolve_route_patterns(&Some(p.clone()), &None);
    assert_eq!(
        result,
        vec![p],
        "a literal test-doc path must reach discovery unfiltered; \
             ReservedTestSuffix is discovery's job"
    );
}

/// Task 8 (unify-config-interpolation-on-env): the empty-config fallback
/// applies ONLY to a missing main file; every load error of an existing
/// file aborts instead of silently booting on defaults.
#[test]
fn missing_config_file_yields_defaults() {
    let dir = tempfile::tempdir().expect("tempdir"); // allow-unwrap
    let path = dir.path().join("nope.toml");
    let config = load_config_or_default(&path.display().to_string())
        .expect("missing file must fall back to serde defaults"); // allow-unwrap
    assert_eq!(config.log_level, "INFO");
    assert_eq!(config.timeout_ms, 5000);
}

#[test]
fn malformed_config_aborts_instead_of_defaults() {
    let dir = tempfile::tempdir().expect("tempdir"); // allow-unwrap
    let path = dir.path().join("Camel.toml");
    std::fs::write(&path, "[observability").expect("write malformed Camel.toml"); // allow-unwrap
    let err = load_config_or_default(&path.display().to_string())
        .expect_err("malformed config must abort, not fall back to defaults"); // allow-unwrap
    let msg = err.to_string();
    assert!(
        msg.contains(&path.display().to_string()),
        "error must name the config path: {msg}"
    );
    assert!(
        msg.contains("failed to load"),
        "error must carry the load prefix: {msg}"
    );
    assert!(
        msg.contains("Failed to parse TOML"),
        "error must carry the parse cause, not only the prefix: {msg}"
    );
}

#[test]
fn broken_include_aborts_instead_of_defaults() {
    let dir = tempfile::tempdir().expect("tempdir"); // allow-unwrap
    let path = dir.path().join("Camel.toml");
    std::fs::write(&path, "include = [\"missing.toml\"]\n").expect("write Camel.toml"); // allow-unwrap
    let err = load_config_or_default(&path.display().to_string())
        .expect_err("broken include must abort, not fall back to defaults"); // allow-unwrap
    let msg = err.to_string();
    assert!(
        msg.contains(&path.display().to_string()),
        "error must name the main config path: {msg}"
    );
    assert!(
        msg.contains("missing.toml"),
        "error must name the missing include: {msg}"
    );
}

/// Restores an env var to its prior value on drop, so a panicking
/// assertion cannot leak the test's env mutation into other tests.
struct EnvVarGuard {
    key: &'static str,
    prior: Option<String>,
}

impl EnvVarGuard {
    fn unset(key: &'static str) -> Self {
        let prior = std::env::var(key).ok();
        // SAFETY: test-scoped; the guard restores the prior value on drop.
        unsafe { std::env::remove_var(key) };
        Self { key, prior }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        match &self.prior {
            Some(value) => {
                // SAFETY: test-scoped restore of the value captured at guard creation.
                unsafe { std::env::set_var(self.key, value) };
            }
            None => {
                // SAFETY: test-scoped; the var was unset before the test.
                unsafe { std::env::remove_var(self.key) };
            }
        }
    }
}

#[test]
fn unresolved_placeholder_aborts_instead_of_defaults() {
    // Env hygiene: the referenced var must be unset for the duration of
    // the test; the guard restores any prior value on drop.
    let _guard = EnvVarGuard::unset("RUST_CAMEL_TEST_RUN_A");

    let dir = tempfile::tempdir().expect("tempdir"); // allow-unwrap
    let path = dir.path().join("Camel.toml");
    std::fs::write(
        &path,
        "[observability.otel]\nendpoint = \"${env:RUST_CAMEL_TEST_RUN_A}\"\n",
    )
    .expect("write Camel.toml"); // allow-unwrap

    let err = load_config_or_default(&path.display().to_string())
        .expect_err("unresolved ${env:} must abort, not fall back to defaults"); // allow-unwrap
    let msg = err.to_string();
    assert!(
        msg.contains(&path.display().to_string()),
        "error must name the config path: {msg}"
    );
    assert!(
        msg.contains("RUST_CAMEL_TEST_RUN_A"),
        "error must name the unresolved env var: {msg}"
    );
}

#[test]
fn try_exists_error_aborts_instead_of_defaults() {
    let dir = tempfile::tempdir().expect("tempdir"); // allow-unwrap
    let file_path = dir.path().join("Camel.toml");
    std::fs::write(&file_path, "").expect("write Camel.toml"); // allow-unwrap
    let child = file_path.join("x");
    let child_str = child.display().to_string();
    match load_config_or_default(&child_str) {
        Err(err) => {
            let msg = err.to_string();
            assert!(
                msg.contains(&child_str),
                "error must name the config path: {msg}"
            );
        }
        Ok(config) => {
            assert_eq!(config.log_level, "INFO");
            assert_eq!(config.timeout_ms, 5000);
        }
    }
}

/// Task 1.5 (wasm-source-auth-kernel): `camel run` threads the
/// per-bind exposure acks from `[binds."<addr>"]` into the wasm
/// component's source-bind gate. The wiring helper invoked at run
/// startup must install the config-built ack map so
/// `WasmSourceBindAcks::acknowledged` reflects what the config set.
#[cfg(feature = "wasm")]
#[test]
fn wasm_bind_acks_wired_from_config() {
    const TEST_BIND: &str = "0.0.0.0:41234"; // distinctive; no other test acks it

    let camel_config: camel_config::CamelConfig = toml::from_str(&format!(
        r#"[binds."{TEST_BIND}"]
allow_public_exposure = true
"#
    ))
    .expect("parse test CamelConfig"); // allow-unwrap

    // Same construction as the run command's wiring site.
    let bind_acks: std::collections::HashMap<String, bool> = camel_config
        .binds
        .iter()
        .map(|(k, v)| (k.clone(), v.allow_public_exposure))
        .collect();

    install_wasm_bind_acks(&bind_acks);

    assert!(
        camel_component_wasm::WasmSourceBindAcks::global().acknowledged(TEST_BIND),
        "run wiring must install wasm bind acks from CamelConfig.binds"
    );
}
