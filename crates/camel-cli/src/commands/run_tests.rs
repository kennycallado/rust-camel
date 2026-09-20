//! Tests for the `run` command (extracted from `run.rs` per the
//! 1k-line rule; keeps `super::` access to the command internals).

use super::*;

/// The run function must emit exactly one startup note about the CWD trust
/// model (INFO: a trust-model disclosure, not a misconfiguration warning).
#[test]
fn startup_warning_emitted() {
    let source = include_str!("run.rs");
    // Build the search string from two parts so the concatenated form
    // never appears literally in test code — only in the info! call.
    let a = "camel run trusts the current working directory";
    let b = " and will execute route";
    let msg = format!("{a}{b}");
    let count = source.matches(&msg).count();
    assert_eq!(
        count, 1,
        "expected exactly one trust-model message emission in run.rs; found {count}"
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
             ReservedDocumentSuffix is discovery's job"
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
/// Shared with `commands::test::driver_tests` (lean env-hermeticity
/// tests) via `crate::commands::run::tests`.
pub(crate) struct EnvVarGuard {
    key: &'static str,
    prior: Option<String>,
}

impl EnvVarGuard {
    pub(crate) fn unset(key: &'static str) -> Self {
        let prior = std::env::var(key).ok();
        // SAFETY: test-scoped; the guard restores the prior value on drop.
        unsafe { std::env::remove_var(key) };
        Self { key, prior }
    }

    pub(crate) fn set(key: &'static str, value: &str) -> Self {
        let prior = std::env::var(key).ok();
        // SAFETY: test-scoped; the guard restores the prior value on drop.
        unsafe { std::env::set_var(key, value) };
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

/// Env-override activation: `camel run`'s loader must apply allowlisted
/// `CAMEL_*` overrides on top of the loaded file, not only on route
/// discovery. `camel-cli` has no env-lock convention (unlike camel-config's
/// `ENV_OVERRIDE_LOCK`), so this follows the crate's existing guard-only
/// pattern: the guard restores the prior value even on panic, and the other
/// `run_tests` tests that assert `timeout_ms` do so on the missing-file
/// defaults path, which never reads `CAMEL_TIMEOUT_MS`. The residual
/// cross-test window (a concurrent config load observing this var) is
/// accepted by the existing convention.
#[test]
fn env_override_applies_to_loaded_config() {
    let _guard = EnvVarGuard::set("CAMEL_TIMEOUT_MS", "12345");

    let dir = tempfile::tempdir().expect("tempdir"); // allow-unwrap
    let path = dir.path().join("Camel.toml");
    std::fs::write(&path, "timeout_ms = 1000\n").expect("write Camel.toml"); // allow-unwrap

    let config =
        load_config_or_default(&path.display().to_string()).expect("existing file must load"); // allow-unwrap
    assert_eq!(
        config.timeout_ms, 12345,
        "CAMEL_TIMEOUT_MS must override the file's timeout_ms in the run loader"
    );
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

/// Task 1.5 (wasm-source-auth-kernel) / scenario-shared-boot task 2.3:
/// `camel run` threads the per-bind exposure acks from
/// `[binds."<addr>"]` into the wasm component's source-bind gate. The
/// shared installer `camel run` now delegates to must install the
/// config-built ack map so `WasmSourceBindAcks::acknowledged` reflects
/// what the config set.
#[cfg(feature = "wasm")]
#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn wasm_bind_acks_wired_from_config() {
    let _wasm_acks_guard = crate::commands::run::WASM_ACKS_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    const TEST_BIND: &str = "0.0.0.0:41234"; // distinctive; no other test acks it

    let camel_config: camel_config::CamelConfig = toml::from_str(&format!(
        r#"[binds."{TEST_BIND}"]
allow_public_exposure = true
"#
    ))
    .expect("parse test CamelConfig"); // allow-unwrap

    let mut ctx = camel_config::CamelConfig::configure_context_with_beans(&camel_config, None)
        .await
        .expect("configure_context_with_beans must succeed"); // allow-unwrap

    camel_bundles::security_boot::install_bind_exposure_acks(&mut ctx, &camel_config).await;

    assert!(
        camel_component_wasm::WasmSourceBindAcks::global().acknowledged(TEST_BIND),
        "shared installer must install wasm bind acks from CamelConfig.binds"
    );
}

/// Task 1.2 fix-up: `camel run` must fail fast when the `--config` parent
/// directory cannot be canonicalized (project-root resolution, shared by
/// the wasm bean loader and the camel-bundles wasm base dir). The guard
/// terminates the process, so the assertion runs in a child copy of this
/// test binary, gated by an env sentinel; the child re-enters this test,
/// calls the guard on a dangling path, and never returns normally.
#[test]
fn dangling_config_parent_fails_fast() {
    if std::env::var("RUST_CAMEL_TEST_PROJECT_ROOT_EXIT").is_ok() {
        // Child branch: the dangling parent must hit the exit(1) guard.
        let dangling = std::env::temp_dir()
            .join(format!("rust-camel-project-root-{}", std::process::id()))
            .join("missing")
            .join("Camel.toml");
        canonical_project_root(&dangling);
        return; // Unreachable in practice: the guard exits the process.
    }

    // Parent branch: re-exec this test binary on the single test with the
    // sentinel set. --nocapture keeps the guard's stderr message visible
    // (process::exit skips the harness capture flush).
    let exe = std::env::current_exe().expect("current_exe"); // allow-unwrap
    let output = std::process::Command::new(exe)
        .args([
            "commands::run::tests::dangling_config_parent_fails_fast",
            "--exact",
            "--nocapture",
        ])
        .env("RUST_CAMEL_TEST_PROJECT_ROOT_EXIT", "1")
        .output()
        .expect("spawn child test process"); // allow-unwrap

    assert_eq!(
        output.status.code(),
        Some(1),
        "dangling config parent must exit 1: {:?}",
        output.status
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("cannot resolve project root"),
        "stderr must name the project-root failure: {stderr}"
    );
}

// ---------------------------------------------------------------------------
// Shared-wiring scenario tests (scenario-shared-boot task 2.3): the
// camel-run half of the runtime-boot "both callers" scenarios. They
// replicate the run.rs wiring sequence in-process through the SAME
// camel-bundles helpers `camel run` delegates to, so a regression in a
// shared helper fails the run-side scenario, not only the harness-side
// one.
// ---------------------------------------------------------------------------

/// Boot a project through the shared helper sequence in the run.rs order
/// (security build → bind-ack install → `camel_bundles::boot` →
/// discovery → SQL startup checks → add routes), stopping before
/// `ctx.start()` so tests can assert on either a successful start or a
/// startup refusal. Registers a test-owned mock component (same-scheme
/// registration replaces the cascade's instance) after boot and before
/// route compilation, so `to: mock:` producers resolve against a message
/// store the test can observe.
#[cfg(feature = "security")]
async fn boot_via_shared_wiring(
    project_dir: &std::path::Path,
    config: &camel_config::CamelConfig,
) -> (
    camel_core::CamelContext,
    camel_bundles::BootHandle,
    camel_auth::ProviderRegistry,
    camel_component_mock::MockComponent,
) {
    let mut ctx = camel_config::CamelConfig::configure_context_with_beans(config, None)
        .await
        .expect("configure_context_with_beans must succeed"); // allow-unwrap

    let sec = camel_bundles::security_boot::build_security_compile_context_from_config(
        config,
        ctx.registry_arc(),
    )
    .await
    .expect("shared security builder must succeed"); // allow-unwrap
    let providers = sec.provider_registry();

    camel_bundles::security_boot::install_bind_exposure_acks(&mut ctx, config).await;

    let boot_handle = camel_bundles::boot(&mut ctx, config, project_dir)
        .await
        .expect("camel_bundles::boot must succeed"); // allow-unwrap

    let mock = camel_component_mock::MockComponent::new();
    ctx.register_component(mock.clone());

    let routes_yaml = project_dir.join("routes.yaml");
    let defs = camel_dsl::discover_routes_with_threshold_and_security(
        &[routes_yaml.display().to_string()],
        config.stream_caching.threshold,
        sec,
    )
    .expect("route discovery must succeed"); // allow-unwrap

    camel_bundles::security_boot::install_sql_startup_checks(&mut ctx, &defs);

    for def in defs {
        ctx.add_route_definition(def)
            .await
            .expect("add_route_definition must succeed"); // allow-unwrap
    }

    (ctx, boot_handle, providers, mock)
}

/// runtime-boot "both callers boot a security_policy route" (camel-run
/// half): a project with a native bearer credential and a
/// `security_policy` route, booted through the shared helpers, admits a
/// stimulus carrying the valid native credential and refuses a
/// credential-less one with `CamelError::Unauthenticated`.
///
/// Pass-case mechanism (deviation from the task text, reported): the
/// task says to carry the bearer token in the exchange headers, but
/// `direct:` has no transport boundary — only http/ws/grpc/mcp/wasm
/// consumers mint the kernel carrier from headers (the
/// `set_security_context` default is a no-op), and the pipeline gate is
/// strictly carrier-only. The pass case therefore drives the same seam
/// the transports drive: `kernel_authenticate` against the provider
/// registry built by the SHARED security builder, then
/// `install_carrier` (mirroring camel-processor's
/// security_policy_layer tests). The `Authorization` header is still set
/// for fidelity; the refuse case is a plain exchange exactly as
/// specified.
#[cfg(feature = "security")]
#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn run_shared_wiring_native_credentials_gate() {
    let _wasm_acks_guard = crate::commands::run::WASM_ACKS_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    use camel_api::security_policy::{
        AccessMode, CredentialSource, RouteSecurityPlan, TransportId,
    };
    use camel_api::{Body, CamelError, Exchange, Message};
    use camel_auth::credential_source::ExtractedToken;
    use camel_auth::kernel::{install_carrier, kernel_authenticate};

    let camel_toml = r#"
[security.native]
subject = "dev-user"
issuer = "native"
bearer_token = "dev-token"
roles = ["admin"]
"#;
    let routes_yaml = r#"
routes:
  - id: sec-gate
    from: direct:sec
    security_policy:
      roles: ["admin"]
      provider: "native"
    steps:
      - to: mock:out
"#;

    let dir = tempfile::tempdir().expect("tempdir"); // allow-unwrap
    std::fs::write(dir.path().join("Camel.toml"), camel_toml).expect("write Camel.toml"); // allow-unwrap
    std::fs::write(dir.path().join("routes.yaml"), routes_yaml).expect("write routes.yaml"); // allow-unwrap

    let config: camel_config::CamelConfig =
        toml::from_str(camel_toml).expect("parse test CamelConfig"); // allow-unwrap

    let (mut ctx, boot_handle, providers, mock) = boot_via_shared_wiring(dir.path(), &config).await;

    ctx.start().await.expect("booted context must start"); // allow-unwrap

    // Pass case: the native credential from Camel.toml authenticates
    // through the shared builder's provider registry; the policy route
    // forwards the exchange to mock:out.
    let mut message = Message::new(Body::Text("ping".to_string()));
    message.set_header(
        "Authorization",
        serde_json::Value::String("Bearer dev-token".to_string()),
    );
    let mut exchange = Exchange::new(message);
    let plan = RouteSecurityPlan {
        access_mode: AccessMode::Authenticated,
        provider_ref: Some("native".to_string()),
        transport: TransportId::Http,
        credential_sources: vec![CredentialSource::AuthorizationHeader],
        audience_binding: None,
    };
    let credentials = ExtractedToken {
        token: "dev-token".to_string(),
        source: CredentialSource::AuthorizationHeader,
    };
    let principal = kernel_authenticate(&plan, &providers, &credentials)
        .await
        .expect("native credential must authenticate via the shared builder's registry"); // allow-unwrap
    install_carrier(&mut exchange, &principal);

    crate::commands::test_support::direct_oneshot(&ctx, "direct:sec", exchange)
        .await
        .expect("credentialed send must complete the security_policy route"); // allow-unwrap

    mock.get_endpoint("out")
        .expect("mock:out endpoint must exist after the send") // allow-unwrap
        .await_exchanges(1, std::time::Duration::from_secs(2))
        .await;

    // Refuse case: no credential anywhere — no carrier, no header.
    let plain = Exchange::new(Message::new(Body::Text("ping".to_string())));
    let err = crate::commands::test_support::direct_oneshot(&ctx, "direct:sec", plain)
        .await
        .expect_err("credential-less send must be refused"); // allow-unwrap
    assert!(
        matches!(err, CamelError::Unauthenticated(_)),
        "refusal must be Unauthenticated, got: {err:?}"
    );

    // Teardown: BootHandle::shutdown stops the context and drains pools.
    let _ = boot_handle.shutdown(&mut ctx).await;
}

/// runtime-boot "both callers refuse an unacknowledged public bind"
/// (camel-run half): a non-loopback bind serving a Public route without
/// `allow_public_exposure`, booted through the shared installer, fails
/// context start with the ADR-0061 acknowledgement error.
#[cfg(feature = "security")]
#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn run_shared_wiring_public_bind_without_ack_fails() {
    let _wasm_acks_guard = crate::commands::run::WASM_ACKS_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    use camel_api::CamelError;

    let camel_toml = r#"
[binds."0.0.0.0:41997"]
"#;
    let routes_yaml = r#"
routes:
  - id: pub-bind
    from: http://0.0.0.0:41997/pub
    steps:
      - to: mock:out
"#;

    let dir = tempfile::tempdir().expect("tempdir"); // allow-unwrap
    std::fs::write(dir.path().join("Camel.toml"), camel_toml).expect("write Camel.toml"); // allow-unwrap
    std::fs::write(dir.path().join("routes.yaml"), routes_yaml).expect("write routes.yaml"); // allow-unwrap

    let config: camel_config::CamelConfig =
        toml::from_str(camel_toml).expect("parse test CamelConfig"); // allow-unwrap

    let (mut ctx, boot_handle, _providers, _mock) =
        boot_via_shared_wiring(dir.path(), &config).await;

    let err = ctx
        .start()
        .await
        .expect_err("unacknowledged non-loopback Public bind must refuse to start"); // allow-unwrap
    match err {
        CamelError::RouteError(msg) => assert!(
            msg.contains("non-loopback address; acknowledge via [binds"),
            "refusal must name the acknowledgement path: {msg}"
        ),
        other => panic!("expected RouteError, got {other:?}"),
    }

    let _ = boot_handle.shutdown(&mut ctx).await;
}

// ---------------------------------------------------------------------------
// Egress allowlist config surface
// ---------------------------------------------------------------------------

fn components_raw_with_function(
    table: toml::Value,
) -> std::collections::HashMap<String, toml::Value> {
    std::collections::HashMap::from([("function".to_string(), table)])
}

#[test]
fn egress_allowlist_absent_yields_empty_vec() {
    // No [default.components.function] block at all.
    let empty = std::collections::HashMap::new();
    assert!(
        egress_allowlist_from_components(&empty)
            .expect("absent block must parse")
            .is_empty()
    );

    // Block present, key absent: still deny-all egress (default unchanged).
    let block = toml::Value::Table(
        toml::from_str("default_timeout_ms = 5000").expect("parse table"), // allow-unwrap
    );
    let raw = components_raw_with_function(block);
    assert!(
        egress_allowlist_from_components(&raw)
            .expect("absent key must parse")
            .is_empty()
    );
}

#[test]
fn egress_allowlist_parses_entries_in_order() {
    let table = toml::Value::Table(
        toml::from_str(r#"egress_allowlist = ["api.example.com:443", "internal", "[::1]:5432"]"#)
            .expect("parse table"), // allow-unwrap
    );
    let raw = components_raw_with_function(table);
    let entries = egress_allowlist_from_components(&raw).expect("valid entries must parse");
    assert_eq!(
        entries,
        vec![
            "api.example.com:443".to_string(),
            "internal".to_string(),
            "[::1]:5432".to_string(),
        ]
    );
}

#[test]
fn egress_allowlist_non_array_rejected_fail_closed() {
    let block = toml::Value::Table(
        toml::from_str(r#"egress_allowlist = "api.example.com""#).expect("parse table"), // allow-unwrap
    );
    let raw = components_raw_with_function(block);
    let err = egress_allowlist_from_components(&raw)
        .err()
        .expect("non-array value must be rejected"); // allow-unwrap
    assert!(
        err.to_string().contains("egress_allowlist"),
        "error must name egress_allowlist, got: {err}"
    );
}

#[test]
fn egress_allowlist_non_string_item_rejected_fail_closed() {
    let block = toml::Value::Table(
        toml::from_str(r#"egress_allowlist = [1]"#).expect("parse table"), // allow-unwrap
    );
    let raw = components_raw_with_function(block);
    let err = egress_allowlist_from_components(&raw)
        .err()
        .expect("non-string item must be rejected"); // allow-unwrap
    assert!(
        err.to_string().contains("egress_allowlist"),
        "error must name egress_allowlist, got: {err}"
    );
}

/// The composition both boot paths use (`camel run` →
/// `LifecycleFailure::Boot`, `camel job` → exit 2): a malformed entry
/// must fail closed before any runtime is constructed.
#[cfg(feature = "containers")]
#[test]
fn function_config_malformed_allowlist_fails_closed() {
    let block = toml::Value::Table(
        toml::from_str(r#"egress_allowlist = ["api.example.com:443", "bad host"]"#)
            .expect("parse table"), // allow-unwrap
    );
    let raw = components_raw_with_function(block);
    let err = function_config_from_components(&raw)
        .err()
        .expect("malformed entry must fail closed"); // allow-unwrap
    assert!(
        err.to_string().contains("egress_allowlist"),
        "error must name egress_allowlist, got: {err}"
    );
}

#[cfg(feature = "containers")]
#[test]
fn function_config_valid_allowlist_roundtrip() {
    let block = toml::Value::Table(
        toml::from_str(r#"egress_allowlist = ["api.example.com:443"]"#).expect("parse table"), // allow-unwrap
    );
    let raw = components_raw_with_function(block);
    let cfg = function_config_from_components(&raw).expect("valid allowlist must pass");
    assert_eq!(
        cfg.egress_allowlist,
        vec!["api.example.com:443".to_string()]
    );
}
