//! Route-surface regression tests for `${env:}` interpolation and the `$$`
//! escape (Task 1 of unify-config-interpolation-on-env).
//!
//! Discovery interpolates `${env:VAR}` placeholders in the raw route file
//! content before YAML parsing (see `discovery.rs`). These tests pin the
//! route-surface semantics: set-variable interpolation, fail-closed missing
//! variables, and the `$$` escape forms introduced by Task 1.
//!
//! Body surface: the `set_body` step (`BuilderStep::DeclarativeSetBody` with a
//! `ValueSourceDef::Literal`) is the closest real "step body" carrier in the
//! route model — the same field type the existing env unit tests exercise
//! through `interpolate_env` directly.

use camel_api::ValueSourceDef;
use camel_core::BuilderStep;
use camel_dsl::discover_routes;
use camel_dsl::discovery::DiscoveryError;
use std::fs;
use tempfile::tempdir;

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

/// Sets or removes an env var for the duration of the test, restoring the
/// previous value (or absence) on drop.
struct EnvGuard {
    name: &'static str,
    previous: Option<String>,
}

impl EnvGuard {
    fn set(name: &'static str, value: &str) -> Self {
        let previous = std::env::var(name).ok();
        unsafe { std::env::set_var(name, value) };
        Self { name, previous }
    }

    fn unset(name: &'static str) -> Self {
        let previous = std::env::var(name).ok();
        unsafe { std::env::remove_var(name) };
        Self { name, previous }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        match &self.previous {
            Some(value) => unsafe { std::env::set_var(self.name, value) },
            None => unsafe { std::env::remove_var(self.name) },
        }
    }
}

#[test]
fn route_env_set_interpolation_unchanged() {
    let _guard = EnvGuard::set("RUST_CAMEL_TEST_ROUTE_A", "route-val");
    let routes = discover(&route_yaml_with_to("log://${env:RUST_CAMEL_TEST_ROUTE_A}"))
        .expect("route with set env var should discover");
    assert_eq!(routes.len(), 1);
    let steps = routes[0].steps();
    assert_eq!(steps.len(), 1);
    match &steps[0] {
        BuilderStep::To(uri) => assert_eq!(uri, "log://route-val"),
        other => panic!("expected BuilderStep::To, got {other:?}"),
    }
}

#[test]
fn route_env_missing_fails_discovery() {
    let _guard = EnvGuard::unset("RUST_CAMEL_TEST_ROUTE_MISSING");
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
}

#[test]
fn route_escape_yields_literal_in_body() {
    let _guard = EnvGuard::unset("RUST_CAMEL_TEST_ROUTE_B");
    let routes = discover(&route_yaml_with_body("$${env:RUST_CAMEL_TEST_ROUTE_B}"))
        .expect("escaped placeholder should not be resolved");
    assert_eq!(routes.len(), 1);
    let steps = routes[0].steps();
    assert_eq!(steps.len(), 1);
    match &steps[0] {
        BuilderStep::DeclarativeSetBody { value } => match value {
            ValueSourceDef::Literal(v) => {
                assert_eq!(v.as_str().unwrap(), "${env:RUST_CAMEL_TEST_ROUTE_B}");
            }
            other => panic!("expected ValueSourceDef::Literal, got {other:?}"),
        },
        other => panic!("expected BuilderStep::DeclarativeSetBody, got {other:?}"),
    }
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
