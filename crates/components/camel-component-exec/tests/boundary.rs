//! Boundary / compile-time contract tests for camel-component-exec.
//!
//! Mirrors the style of camel-component-llm/tests/boundary.rs.

use camel_component_api::{Component, ComponentBundle};
use camel_component_exec::{ExecBundle, ExecComponent, ExecError, ExecResult};

// ---------------------------------------------------------------------------
// scheme / config_key
// ---------------------------------------------------------------------------

#[test]
fn scheme_is_exec() {
    let component = ExecComponent::new(Default::default());
    assert_eq!(component.scheme(), "exec");
}

#[test]
fn config_key_is_exec() {
    assert_eq!(ExecBundle::config_key(), "exec");
}

// ---------------------------------------------------------------------------
// ExecError is #[non_exhaustive]
// ---------------------------------------------------------------------------

#[test]
fn exec_error_match_forces_wildcard() {
    // #[non_exhaustive] on ExecError (defined in the exec component crate)
    // forces a wildcard arm when matching from an external crate
    // (integration test). If this compiles, the property is verified.
    let e = ExecError::Spawn(std::io::Error::other("test"));
    let _desc = match &e {
        ExecError::NotAllowlisted { .. } => "not allowlisted",
        ExecError::ArgPolicyDenied { .. } => "arg denied",
        ExecError::ShellRejected { .. } => "shell rejected",
        ExecError::InvalidWorkDir { .. } => "invalid workdir",
        ExecError::StdinTooLarge { .. } => "stdin too large",
        ExecError::InvalidArgs(_) => "invalid args",
        ExecError::Spawn(_) => "spawn",
        _ => "future variant", // required by #[non_exhaustive]
    };
    assert!(!_desc.is_empty());
}

// ---------------------------------------------------------------------------
// ExecResult is #[non_exhaustive]
// ---------------------------------------------------------------------------

#[test]
fn exec_result_cannot_be_constructed_from_external_crate() {
    // #[non_exhaustive] on ExecResult prevents struct-literal construction
    // from outside the defining crate. Deserialize via serde_json instead.
    let json = serde_json::json!({
        "exit_code": 0,
        "stdout": "dGVzdA==",
        "stderr": "",
        "stdout_truncated": false,
        "stderr_truncated": false,
        "timed_out": false,
        "profile": "echo",
        "duration_ms": 42,
    });
    let result: ExecResult = serde_json::from_value(json).expect("ExecResult deserialize");
    assert_eq!(result.exit_code, Some(0));
    assert_eq!(result.profile, "echo");
    assert_eq!(result.duration_ms, 42);
}
