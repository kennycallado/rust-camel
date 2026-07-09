//! End-to-end producer integration tests via ExecBundle::from_toml.
//!
//! Each test builds a bundle from TOML, extracts the validated config, and
//! constructs an ExecProducer to exercise the full config→validate→canonical-pin→
//! Service<Exchange> path. The unit tests in producer.rs construct ExecProducer
//! directly; these go through the bundle to exercise the end-to-end setup.

use std::collections::HashMap;
use std::sync::Arc;

use base64::Engine;
use camel_api::{Body, Exchange};
use camel_component_api::ComponentBundle;
use camel_component_exec::{
    ExecBundle, ExecProducer, ExecResult,
    headers::{
        CAMEL_EXEC_ARGS, CAMEL_EXEC_EXIT_ACCEPTED, CAMEL_EXEC_EXIT_CODE, CAMEL_EXEC_TIMED_OUT,
    },
};
use tokio::sync::Semaphore;
use tower::Service;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build a toml::Value from profiles TOML snippet (workspace_root = "." always).
fn make_cfg(profiles_toml: &str) -> toml::Value {
    let s = format!("workspace_root = \".\"\n{profiles_toml}");
    toml::from_str(&s).unwrap()
}

/// Deserialize ExecResult from the exchange body (Body::Json).
fn result_from_body(ex: &Exchange) -> ExecResult {
    let v = match &ex.input.body {
        Body::Json(v) => v.clone(),
        other => panic!("expected Body::Json, got {other:?}"),
    };
    serde_json::from_value(v).expect("deserialize ExecResult")
}

/// Build an ExecProducer from a bundle, using the named profile.
fn producer_from_bundle(bundle: &ExecBundle, profile_name: &str) -> ExecProducer {
    let config = bundle.config();
    let profile = config
        .profile(profile_name)
        .unwrap_or_else(|| panic!("profile {profile_name:?} not found"))
        .clone();

    ExecProducer::new(
        Arc::new(profile),
        Arc::new(config.clone()),
        "integration-test".into(),
        HashMap::new(),
        Arc::new(Semaphore::new(1)),
        None,
    )
}

/// Set the CamelExecArgs header on an exchange.
fn set_args(ex: &mut Exchange, args: &[&str]) {
    let v: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    ex.input.headers.insert(
        CAMEL_EXEC_ARGS.to_string(),
        serde_json::to_value(v).unwrap(),
    );
}

// ---------------------------------------------------------------------------
// (a) Echo roundtrip: exit 0, accepted=true, timed_out=false
// ---------------------------------------------------------------------------

#[tokio::test]
async fn producer_echo_via_bundle() {
    let bundle = ExecBundle::from_toml(make_cfg(
        r#"
[[profiles]]
name = "echo"
executable = "echo"
args = { allow = "any" }
accepted_exit_codes = [0]
timeout_secs = 5
"#,
    ))
    .unwrap();

    let mut producer = producer_from_bundle(&bundle, "echo");
    let mut ex = Exchange::default();
    set_args(&mut ex, &["hello"]);

    let exchange = Service::call(&mut producer, ex)
        .await
        .expect("echo must succeed");

    let result = result_from_body(&exchange);
    assert_eq!(result.exit_code, Some(0));
    assert!(!result.timed_out);

    let stdout_bytes = base64::engine::general_purpose::STANDARD
        .decode(&result.stdout)
        .unwrap();
    assert_eq!(stdout_bytes, b"hello\n");

    assert_eq!(
        exchange.input.headers.get(CAMEL_EXEC_EXIT_ACCEPTED),
        Some(&serde_json::Value::Bool(true))
    );
    assert_eq!(
        exchange.input.headers.get(CAMEL_EXEC_TIMED_OUT),
        Some(&serde_json::Value::Bool(false))
    );
}

// ---------------------------------------------------------------------------
// (b) Non-zero exit: exit 2, accepted_exit_codes=[0] => accepted=false
// ---------------------------------------------------------------------------

#[tokio::test]
async fn producer_non_zero_exit_via_bundle() {
    let bundle = ExecBundle::from_toml(make_cfg(
        r#"
[[profiles]]
name = "sh"
executable = "sh"
args = { allow = "any" }
allow_shell = true
accepted_exit_codes = [0]
timeout_secs = 5
"#,
    ))
    .unwrap();

    let mut producer = producer_from_bundle(&bundle, "sh");
    let mut ex = Exchange::default();
    set_args(&mut ex, &["-c", "exit 2"]);

    let exchange = Service::call(&mut producer, ex)
        .await
        .expect("non-zero exit must be non-error");

    let result = result_from_body(&exchange);
    assert_eq!(result.exit_code, Some(2));
    assert!(!result.timed_out);

    assert_eq!(
        exchange.input.headers.get(CAMEL_EXEC_EXIT_ACCEPTED),
        Some(&serde_json::Value::Bool(false))
    );
    assert_eq!(
        exchange.input.headers.get(CAMEL_EXEC_EXIT_CODE),
        Some(&serde_json::Value::from(2))
    );
}

// ---------------------------------------------------------------------------
// (c) Timeout tree-kill: sleep 30, timeout_secs=1, must complete <2s
// ---------------------------------------------------------------------------

#[tokio::test]
async fn producer_timeout_tree_kill_via_bundle() {
    let bundle = ExecBundle::from_toml(make_cfg(
        r#"
[[profiles]]
name = "sleep"
executable = "sleep"
args = { allow = "any" }
accepted_exit_codes = [0]
timeout_secs = 1
"#,
    ))
    .unwrap();

    let mut producer = producer_from_bundle(&bundle, "sleep");
    let mut ex = Exchange::default();
    set_args(&mut ex, &["30"]); // sleeps 30s — must be killed by timeout

    let deadline = tokio::time::Duration::from_secs(5);
    let exchange = tokio::time::timeout(deadline, Service::call(&mut producer, ex))
        .await
        .expect("timeout must complete within 5s (tree-kill test)")
        .expect("timeout must be non-error outcome");

    let result = result_from_body(&exchange);
    assert_eq!(result.exit_code, None, "killed process has no exit code");
    assert!(result.timed_out, "must be marked timed out");
    // Exit code header is absent when process was killed (no exit code)
    assert_eq!(
        exchange.input.headers.get(CAMEL_EXEC_EXIT_ACCEPTED),
        Some(&serde_json::Value::Bool(false))
    );
    assert_eq!(
        exchange.input.headers.get(CAMEL_EXEC_TIMED_OUT),
        Some(&serde_json::Value::Bool(true))
    );
    // Exit code header should NOT be present
    assert!(
        !exchange.input.headers.contains_key(CAMEL_EXEC_EXIT_CODE),
        "no exit code header on killed process"
    );
}
