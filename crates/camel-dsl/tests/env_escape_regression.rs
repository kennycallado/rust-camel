//! Route-surface regression tests for `${env:}` interpolation and the `$$`
//! escape (Task 1 of unify-config-interpolation-on-env).
//!
//! Discovery interpolates `${env:VAR}` placeholders in the raw route file
//! content before YAML parsing (see `discovery.rs`). These tests pin the
//! route-surface semantics: set-variable interpolation, fail-closed missing
//! variables, and the `$$` escape forms introduced by Task 1.
//!
//! Body surface: the `set_body` step (`BuilderStep::DeclarativeSetBody`) is the
//! closest real "step body" carrier in the route model — the same field type
//! the existing env unit tests exercise through `interpolate_env` directly. A
//! surviving `${...}` slot in the substituted text is gated to a Simple
//! `ValueSourceDef::Expression`; plain substituted text stays
//! `ValueSourceDef::Literal`.
//!
//! The cases that observe process-environment mutation run in a dedicated
//! child process: the parent re-executes this test binary with an exact test
//! filter and installs the case environment on the child, then asserts the
//! child succeeded. The child branch sets nothing itself — its environment was
//! fixed at spawn — and runs the original assertions verbatim.

use camel_api::ValueSourceDef;
use camel_core::BuilderStep;
use camel_dsl::discover_routes;
use camel_dsl::discovery::DiscoveryError;
use std::fs;
use std::process::Command;
use tempfile::tempdir;

const SUBPROCESS_ENV: &str = "CAMEL_TEST_SUBPROCESS";

/// Writes `yaml` into a temp `routes/` dir and discovers it through the
/// normal route-loading path (glob pattern + `discover_routes`).
fn discover(yaml: &str) -> Result<Vec<camel_core::RouteDefinition>, DiscoveryError> {
    let dir = tempdir().unwrap();
    let routes_dir = dir.path().join("routes");
    fs::create_dir(&routes_dir).unwrap();
    fs::write(routes_dir.join("route.yaml"), yaml).unwrap();
    let pattern = dir
        .path()
        .join("routes/*.yaml")
        .to_str()
        .unwrap()
        .to_string();
    discover_routes(&[pattern])
}

fn route_yaml_with_to(uri: &str) -> String {
    format!(
        "routes:\n  - id: \"env-escape-route\"\n    from: \"direct:start\"\n    steps:\n      - to: \"{uri}\"\n"
    )
}

fn route_yaml_with_body(body: &str) -> String {
    format!(
        "routes:\n  - id: \"env-escape-route\"\n    from: \"direct:start\"\n    steps:\n      - set_body: \"{body}\"\n"
    )
}

/// Re-run this test binary with an exact filter for `case`, installing the
/// case environment on the child. The child branch runs the original
/// assertions and the parent asserts it passed.
fn run_case_in_subprocess(case: &str, env: &[(&str, Option<&str>)]) {
    let exe = std::env::current_exe().expect("current test executable");
    let mut command = Command::new(exe);
    command
        .arg("--exact")
        .arg(case)
        .arg("--nocapture")
        .env(SUBPROCESS_ENV, case);
    for (name, value) in env {
        match value {
            Some(value) => {
                command.env(name, value);
            }
            None => {
                command.env_remove(name);
            }
        }
    }
    let output = command.output().expect("spawn subprocess");
    assert!(
        output.status.success(),
        "subprocess case {case} failed: {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("1 passed"),
        "subprocess case {case} did not run exactly one test\nstdout:\n{}",
        String::from_utf8_lossy(&output.stdout),
    );
}

fn in_subprocess(case: &str) -> bool {
    std::env::var(SUBPROCESS_ENV).as_deref() == Ok(case)
}

#[test]
fn route_env_set_interpolation_unchanged() {
    let case = "route_env_set_interpolation_unchanged";
    if in_subprocess(case) {
        let routes = discover(&route_yaml_with_to("log://${env:RUST_CAMEL_TEST_ROUTE_A}"))
            .expect("route with set env var should discover");
        assert_eq!(routes.len(), 1);
        let steps = routes[0].steps();
        assert_eq!(steps.len(), 1);
        match &steps[0] {
            BuilderStep::To(uri) => assert_eq!(uri, "log://route-val"),
            other => panic!("expected BuilderStep::To, got {other:?}"),
        }
        return;
    }
    run_case_in_subprocess(case, &[("RUST_CAMEL_TEST_ROUTE_A", Some("route-val"))]);
}

#[test]
fn route_env_missing_fails_discovery() {
    let case = "route_env_missing_fails_discovery";
    if in_subprocess(case) {
        let err = match discover(&route_yaml_with_to(
            "log://${env:RUST_CAMEL_TEST_ROUTE_MISSING}",
        )) {
            Ok(routes) => panic!(
                "route with unset env var must fail discovery, got {} route(s)",
                routes.len()
            ),
            Err(err) => err,
        };
        match err {
            DiscoveryError::Env { var_name, .. } => {
                assert_eq!(var_name, "RUST_CAMEL_TEST_ROUTE_MISSING");
            }
            other => panic!("expected DiscoveryError::Env, got {other:?}"),
        }
        return;
    }
    run_case_in_subprocess(case, &[("RUST_CAMEL_TEST_ROUTE_MISSING", None)]);
}

#[test]
fn route_escape_yields_simple_slot_in_body() {
    let case = "route_escape_yields_simple_slot_in_body";
    if in_subprocess(case) {
        let routes = discover(&route_yaml_with_body("$${env:RUST_CAMEL_TEST_ROUTE_B}"))
            .expect("escaped placeholder should not be resolved");
        assert_eq!(routes.len(), 1);
        let steps = routes[0].steps();
        assert_eq!(steps.len(), 1);
        match &steps[0] {
            BuilderStep::DeclarativeSetBody { value } => match value {
                ValueSourceDef::Expression(expr) => {
                    assert_eq!(expr.language, "simple");
                    assert_eq!(expr.source, "${env:RUST_CAMEL_TEST_ROUTE_B}");
                }
                other => panic!("expected ValueSourceDef::Expression, got {other:?}"),
            },
            other => panic!("expected BuilderStep::DeclarativeSetBody, got {other:?}"),
        }
        return;
    }
    run_case_in_subprocess(case, &[("RUST_CAMEL_TEST_ROUTE_B", None)]);
}

#[test]
fn route_standalone_dollar_converts() {
    let routes = discover(&route_yaml_with_body("a$$b"))
        .expect("standalone $$ escape should convert to a single $");
    assert_eq!(routes.len(), 1);
    let steps = routes[0].steps();
    assert_eq!(steps.len(), 1);
    match &steps[0] {
        BuilderStep::DeclarativeSetBody { value } => match value {
            ValueSourceDef::Literal(v) => {
                assert_eq!(v.as_str().unwrap(), "a$b");
            }
            other => panic!("expected ValueSourceDef::Literal, got {other:?}"),
        },
        other => panic!("expected BuilderStep::DeclarativeSetBody, got {other:?}"),
    }
}
