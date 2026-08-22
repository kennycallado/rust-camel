//! Phase-1 parity regression tests: security context and stream-cache threshold
//! threading through template materialization (bd rc-f2kp, Task 1.3).
//!
//! These tests pin ALREADY-IMPLEMENTED behavior — the engine threads
//! `SecurityCompileContext` and `stream_cache_threshold` through template
//! materialization. They must pass; if one fails it is a defect, not a test
//! to weaken.
//!
//! Phase-2 aggregation tests (bd rc-f2kp, Task 2.2): Pass 2 collects every
//! materialization failure instead of aborting on the first one.
//! Task 2.3 snapshot tests: sibling diagnostics are never discarded and
//! missing template refs are collected as NotFound failures.
//!
//! Phase-3 typed substitution tests (bd rc-f2kp, Task 3.2): whole-node
//! `{{param}}` placeholders for `type: number`/`type: boolean` parameters
//! materialize as JSON numbers/booleans, and non-coercible values are
//! rejected at resolution time.

use async_trait::async_trait;
use camel_api::CamelError;
use camel_api::security_policy::Principal;
use camel_api::template::TemplateError;
use camel_dsl::{
    DiscoveryError, SecurityCompileContext, discover_routes_with_threshold_and_security,
};
use std::path::Path;
use std::sync::Arc;

struct TestAuthenticator;

#[async_trait]
impl camel_auth::TokenAuthenticator for TestAuthenticator {
    async fn authenticate_bearer(&self, _token: &str) -> Result<Principal, CamelError> {
        Ok(Principal {
            subject: "test-user".into(),
            issuer: "test-issuer".into(),
            audience: vec![],
            scopes: vec!["read:api".into()],
            roles: vec!["admin".into()],
            claims: serde_json::Value::Null,
        })
    }
}

/// Write a YAML file into `dir` and return a glob pattern matching it.
fn write_temp_yaml(dir: &Path, name: &str, body: &str) -> String {
    std::fs::write(dir.join(name), body).unwrap();
    dir.join(name).to_string_lossy().to_string()
}

fn real_ctx() -> SecurityCompileContext {
    let auth = Arc::new(TestAuthenticator) as Arc<dyn camel_auth::TokenAuthenticator>;
    SecurityCompileContext::new(Some(auth), None)
}

#[test]
fn secured_templated_route_compiles_with_real_context() {
    let dir = tempfile::tempdir().unwrap();
    let pattern = write_temp_yaml(
        dir.path(),
        "secured.yaml",
        r#"
routes: []
templates:
  - id: secured-tpl
    parameters: []
    routes:
      - id: "secured-route"
        from: "direct:start"
        security_policy:
          roles: ["admin"]
        steps:
          - to: "log:info"
templated_routes:
  - route_template_ref: secured-tpl
    parameters: {}
"#,
    );

    let routes = discover_routes_with_threshold_and_security(&[pattern], 1024, real_ctx()).unwrap();
    assert_eq!(routes.len(), 1);
    assert!(routes[0].security_authenticator().is_some());
}

#[test]
fn secured_templated_route_fails_closed_with_default_context() {
    let dir = tempfile::tempdir().unwrap();
    let pattern = write_temp_yaml(
        dir.path(),
        "secured.yaml",
        r#"
routes: []
templates:
  - id: secured-tpl
    parameters: []
    routes:
      - id: "secured-route"
        from: "direct:start"
        security_policy:
          roles: ["admin"]
        steps:
          - to: "log:info"
templated_routes:
  - route_template_ref: secured-tpl
    parameters: {}
"#,
    );

    let err = match discover_routes_with_threshold_and_security(
        &[pattern],
        1024,
        SecurityCompileContext::default(),
    ) {
        Ok(_) => panic!("expected secured templated route to fail closed"),
        Err(e) => e,
    };
    assert!(
        err.to_string().contains("route requires an authenticator"),
        "unexpected error: {err}"
    );
}

#[test]
fn mixed_direct_and_templated_secured_routes_both_materialize() {
    let dir = tempfile::tempdir().unwrap();
    let pattern = write_temp_yaml(
        dir.path(),
        "mixed.yaml",
        r#"
routes:
  - id: direct-secured
    from: direct:start
    security_policy:
      roles: ["admin"]
    steps:
      - to: log:info
templates:
  - id: secured-tpl
    parameters: []
    routes:
      - id: "templated-secured"
        from: "direct:start"
        security_policy:
          roles: ["admin"]
        steps:
          - to: "log:info"
templated_routes:
  - route_template_ref: secured-tpl
    parameters: {}
"#,
    );

    let routes = discover_routes_with_threshold_and_security(&[pattern], 1024, real_ctx()).unwrap();
    assert_eq!(routes.len(), 2);
    let ids: Vec<&str> = routes.iter().map(|r| r.route_id()).collect();
    assert!(ids.contains(&"direct-secured"));
    assert!(ids.contains(&"templated-secured"));
    assert_ne!(ids[0], ids[1]);
    for route in &routes {
        assert!(route.security_authenticator().is_some());
    }
}

#[test]
fn cross_file_secured_template_compiles() {
    let dir = tempfile::tempdir().unwrap();
    // A.yaml defines the secured template.
    write_temp_yaml(
        dir.path(),
        "A.yaml",
        r#"
routes: []
templates:
  - id: shared-secured
    parameters: []
    routes:
      - id: "shared-secured-route"
        from: "direct:start"
        security_policy:
          roles: ["admin"]
        steps:
          - to: "log:info"
"#,
    );
    // B.yaml instantiates the template.
    write_temp_yaml(
        dir.path(),
        "B.yaml",
        r#"
routes: []
templated_routes:
  - route_template_ref: shared-secured
    parameters: {}
"#,
    );

    let pattern = dir.path().join("*.yaml").to_string_lossy().to_string();
    let routes = discover_routes_with_threshold_and_security(&[pattern], 1024, real_ctx()).unwrap();
    assert_eq!(routes.len(), 1);
    assert_eq!(routes[0].route_id(), "shared-secured-route");
    assert!(routes[0].security_authenticator().is_some());
}

/// A template whose body contains an unknown step kind — materialization
/// fails in Pass 2 with `TemplateError::InvalidBody`.
fn invalid_body_template(id: &str, route_id: &str, bogus_key: &str) -> String {
    format!(
        r#"
routes: []
templates:
  - id: {id}
    parameters: []
    routes:
      - id: "{route_id}"
        from: "direct:start"
        steps:
          - {bogus_key}: "boom"
templated_routes:
  - route_template_ref: {id}
    parameters: {{}}
"#
    )
}

#[test]
fn two_failing_specs_both_reported() {
    let dir = tempfile::tempdir().unwrap();
    // ONE file with two templated specs referencing two different templates,
    // each failing materialization for a different reason (distinct unknown
    // step kinds) — the literal shape of the blessed scenario.
    write_temp_yaml(
        dir.path(),
        "two-failures.yaml",
        r#"
routes: []
templates:
  - id: tpl-alpha
    parameters: []
    routes:
      - id: "alpha-route"
        from: "direct:start"
        steps:
          - bogus_step_a: "boom"
  - id: tpl-beta
    parameters: []
    routes:
      - id: "beta-route"
        from: "direct:start"
        steps:
          - bogus_step_b: "boom"
templated_routes:
  - route_template_ref: tpl-alpha
    parameters: {}
  - route_template_ref: tpl-beta
    parameters: {}
"#,
    );
    let pattern = dir.path().join("*.yaml").to_string_lossy().to_string();

    let err = match discover_routes_with_threshold_and_security(
        &[pattern],
        camel_api::stream_cache::DEFAULT_STREAM_CACHE_THRESHOLD,
        SecurityCompileContext::default(),
    ) {
        Ok(_) => panic!("expected both templated specs to fail materialization"),
        Err(e) => e,
    };
    let DiscoveryError::MaterializationFailures { failures } = &err else {
        panic!("expected MaterializationFailures, got: {err:?}");
    };
    assert_eq!(failures.len(), 2, "both failures must be reported: {err:?}");
    let refs: Vec<&str> = failures.iter().map(|f| f.template_ref.as_str()).collect();
    assert!(refs.contains(&"tpl-alpha"), "missing tpl-alpha: {refs:?}");
    assert!(refs.contains(&"tpl-beta"), "missing tpl-beta: {refs:?}");
    // Each failure carries its own cause: both are InvalidBody, but the
    // messages must differ (distinct unknown step kinds fail at distinct
    // positions in the materialized body).
    let mut messages: Vec<&str> = Vec::new();
    for failure in failures {
        let TemplateError::InvalidBody(msg) = &failure.error else {
            panic!("expected InvalidBody, got: {:?}", failure.error);
        };
        messages.push(msg);
    }
    assert_ne!(messages[0], messages[1], "causes must differ: {messages:?}");
}

#[test]
fn distinct_error_classes_for_security_and_body() {
    let dir = tempfile::tempdir().unwrap();
    // One secured template (roles, no authenticator configured) and one with
    // a malformed body — the two failures must keep distinct error classes.
    write_temp_yaml(
        dir.path(),
        "secured.yaml",
        r#"
routes: []
templates:
  - id: secured-tpl
    parameters: []
    routes:
      - id: "secured-route"
        from: "direct:start"
        security_policy:
          roles: ["admin"]
        steps:
          - to: "log:info"
templated_routes:
  - route_template_ref: secured-tpl
    parameters: {}
"#,
    );
    write_temp_yaml(
        dir.path(),
        "malformed.yaml",
        &invalid_body_template("malformed-tpl", "malformed-route", "bogus_step"),
    );
    let pattern = dir.path().join("*.yaml").to_string_lossy().to_string();

    let err = match discover_routes_with_threshold_and_security(
        &[pattern],
        camel_api::stream_cache::DEFAULT_STREAM_CACHE_THRESHOLD,
        SecurityCompileContext::default(),
    ) {
        Ok(_) => panic!("expected both templated specs to fail materialization"),
        Err(e) => e,
    };
    let DiscoveryError::MaterializationFailures { failures } = &err else {
        panic!("expected MaterializationFailures, got: {err:?}");
    };
    assert_eq!(failures.len(), 2, "both failures must be reported: {err:?}");
    let mut saw_security = false;
    let mut saw_invalid_body = false;
    for failure in failures {
        match &failure.error {
            TemplateError::SecurityRequired { template_id, .. } => {
                assert_eq!(template_id, "secured-tpl");
                saw_security = true;
            }
            TemplateError::InvalidBody(_) => {
                assert_eq!(failure.template_ref, "malformed-tpl");
                saw_invalid_body = true;
            }
            other => panic!("unexpected error class: {other:?}"),
        }
    }
    assert!(saw_security, "missing SecurityRequired failure: {err:?}");
    assert!(saw_invalid_body, "missing InvalidBody failure: {err:?}");
}

#[test]
fn good_sibling_routes_materialize_alongside_reported_failures() {
    let dir = tempfile::tempdir().unwrap();
    write_temp_yaml(
        dir.path(),
        "sibling.yaml",
        r#"
routes: []
templates:
  - id: good-tpl
    parameters: []
    routes:
      - id: "good-route"
        from: "direct:start"
        steps:
          - to: "log:info"
  - id: bad-tpl
    parameters: []
    routes:
      - id: "bad-route"
        from: "direct:start"
        steps:
          - bogus_step: "boom"
templated_routes:
  - route_template_ref: good-tpl
    parameters: {}
  - route_template_ref: bad-tpl
    parameters: {}
"#,
    );
    let pattern = dir.path().join("*.yaml").to_string_lossy().to_string();

    let err = match discover_routes_with_threshold_and_security(
        &[pattern],
        camel_api::stream_cache::DEFAULT_STREAM_CACHE_THRESHOLD,
        SecurityCompileContext::default(),
    ) {
        Ok(_) => panic!("expected bad templated spec to fail materialization"),
        Err(e) => e,
    };
    let DiscoveryError::MaterializationFailures { failures } = &err else {
        panic!("expected MaterializationFailures, got: {err:?}");
    };
    assert_eq!(
        failures.len(),
        1,
        "only the bad spec must produce a failure: {err:?}"
    );
    assert_eq!(failures[0].template_ref, "bad-tpl");
}

#[test]
fn template_not_found_is_a_collected_failure() {
    let dir = tempfile::tempdir().unwrap();
    write_temp_yaml(
        dir.path(),
        "not-found.yaml",
        r#"
routes: []
templates:
  - id: valid-tpl
    parameters: []
    routes:
      - id: "valid-route"
        from: "direct:start"
        steps:
          - to: "log:info"
templated_routes:
  - route_template_ref: valid-tpl
    parameters: {}
  - route_template_ref: missing-tpl
    parameters: {}
"#,
    );
    let pattern = dir.path().join("*.yaml").to_string_lossy().to_string();

    let err = match discover_routes_with_threshold_and_security(
        &[pattern],
        camel_api::stream_cache::DEFAULT_STREAM_CACHE_THRESHOLD,
        SecurityCompileContext::default(),
    ) {
        Ok(_) => panic!("expected missing template ref to fail"),
        Err(e) => e,
    };
    let DiscoveryError::MaterializationFailures { failures } = &err else {
        panic!("expected MaterializationFailures, got: {err:?}");
    };
    assert_eq!(
        failures.len(),
        1,
        "exactly one failure for the missing ref: {err:?}"
    );
    let failure = &failures[0];
    assert_eq!(failure.template_ref, "missing-tpl");
    let TemplateError::NotFound(missing) = &failure.error else {
        panic!("expected NotFound, got: {:?}", failure.error);
    };
    assert_eq!(missing, "missing-tpl");
}

#[test]
fn number_param_whole_node_populates_numeric_field() {
    let dir = tempfile::tempdir().unwrap();
    let pattern = write_temp_yaml(
        dir.path(),
        "delay.yaml",
        r#"
routes: []
templates:
  - id: delay-tpl
    parameters:
      - name: delay
        type: number
    routes:
      - id: "delay-route"
        from: "direct:start"
        steps:
          - delay: "{{delay}}"
templated_routes:
  - route_template_ref: delay-tpl
    parameters:
      delay: "5000"
"#,
    );

    let routes = discover_routes_with_threshold_and_security(
        &[pattern],
        camel_api::stream_cache::DEFAULT_STREAM_CACHE_THRESHOLD,
        SecurityCompileContext::default(),
    )
    .unwrap();
    assert_eq!(routes.len(), 1);
    assert_eq!(routes[0].route_id(), "delay-route");
}

#[test]
fn non_coercible_number_param_rejected_loudly() {
    let dir = tempfile::tempdir().unwrap();
    let pattern = write_temp_yaml(
        dir.path(),
        "delay.yaml",
        r#"
routes: []
templates:
  - id: delay-tpl
    parameters:
      - name: delay
        type: number
    routes:
      - id: "delay-route"
        from: "direct:start"
        steps:
          - delay: "{{delay}}"
templated_routes:
  - route_template_ref: delay-tpl
    parameters:
      delay: "abc"
"#,
    );

    let err = match discover_routes_with_threshold_and_security(
        &[pattern],
        camel_api::stream_cache::DEFAULT_STREAM_CACHE_THRESHOLD,
        SecurityCompileContext::default(),
    ) {
        Ok(_) => panic!("expected non-coercible number parameter to fail materialization"),
        Err(e) => e,
    };
    assert!(
        err.to_string()
            .contains("parameter 'delay' declared type number"),
        "unexpected error: {err}"
    );
}

#[test]
fn typed_param_whole_node_and_embedded_in_same_template() {
    let dir = tempfile::tempdir().unwrap();
    let pattern = write_temp_yaml(
        dir.path(),
        "mixed.yaml",
        r#"
routes: []
templates:
  - id: mixed-tpl
    parameters:
      - name: p
        type: number
    routes:
      - id: "x{{p}}"
        from: "direct:start"
        steps:
          - delay: "{{p}}"
templated_routes:
  - route_template_ref: mixed-tpl
    parameters:
      p: "7"
"#,
    );

    let routes = discover_routes_with_threshold_and_security(
        &[pattern],
        camel_api::stream_cache::DEFAULT_STREAM_CACHE_THRESHOLD,
        SecurityCompileContext::default(),
    )
    .unwrap();
    assert_eq!(routes.len(), 1);
    assert_eq!(routes[0].route_id(), "x7");
}

#[test]
fn boolean_param_whole_node_substitutes() {
    let dir = tempfile::tempdir().unwrap();
    let pattern = write_temp_yaml(
        dir.path(),
        "bool.yaml",
        r#"
routes: []
templates:
  - id: bool-tpl
    parameters:
      - name: flag
        type: boolean
    routes:
      - id: "bool-route"
        from: "direct:start"
        auto_startup: "{{flag}}"
        steps:
          - to: "log:info"
templated_routes:
  - route_template_ref: bool-tpl
    parameters:
      flag: "false"
"#,
    );

    let routes = discover_routes_with_threshold_and_security(
        &[pattern],
        camel_api::stream_cache::DEFAULT_STREAM_CACHE_THRESHOLD,
        SecurityCompileContext::default(),
    )
    .unwrap();
    assert_eq!(routes.len(), 1);
    assert_eq!(routes[0].route_id(), "bool-route");
    // "false" (not "true") distinguishes a genuinely substituted boolean
    // from the unwrap_or(true) default that fires when the key is dropped.
    assert!(!routes[0].auto_startup());
}

#[test]
fn non_coercible_boolean_param_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let pattern = write_temp_yaml(
        dir.path(),
        "bool.yaml",
        r#"
routes: []
templates:
  - id: bool-tpl
    parameters:
      - name: flag
        type: boolean
    routes:
      - id: "bool-route"
        from: "direct:start"
        auto_startup: "{{flag}}"
        steps:
          - to: "log:info"
templated_routes:
  - route_template_ref: bool-tpl
    parameters:
      flag: "yes"
"#,
    );

    let err = match discover_routes_with_threshold_and_security(
        &[pattern],
        camel_api::stream_cache::DEFAULT_STREAM_CACHE_THRESHOLD,
        SecurityCompileContext::default(),
    ) {
        Ok(_) => panic!("expected non-coercible boolean parameter to fail materialization"),
        Err(e) => e,
    };
    assert!(
        err.to_string()
            .contains("parameter 'flag' declared type boolean"),
        "unexpected error: {err}"
    );
}

#[test]
fn multi_route_template_with_override_fails() {
    let dir = tempfile::tempdir().unwrap();
    write_temp_yaml(
        dir.path(),
        "multi-route-override.yaml",
        r#"
routes: []
templates:
  - id: multi-tpl
    parameters: []
    routes:
      - id: "first-route"
        from: "direct:start"
        steps:
          - to: "log:info"
      - id: "second-route"
        from: "direct:other"
        steps:
          - to: "log:info"
templated_routes:
  - route_template_ref: multi-tpl
    route_id: override-x
    parameters: {}
"#,
    );
    let pattern = dir.path().join("*.yaml").to_string_lossy().to_string();

    let err = match discover_routes_with_threshold_and_security(
        &[pattern],
        camel_api::stream_cache::DEFAULT_STREAM_CACHE_THRESHOLD,
        SecurityCompileContext::default(),
    ) {
        Ok(_) => panic!("expected multi-route override to fail"),
        Err(e) => e,
    };
    assert!(
        err.to_string()
            .contains("route_id override is only valid for single-route templates"),
        "unexpected error: {err}"
    );
}

#[test]
fn single_route_template_three_instances_distinct_ids() {
    let dir = tempfile::tempdir().unwrap();
    write_temp_yaml(
        dir.path(),
        "three-instances.yaml",
        r#"
routes: []
templates:
  - id: solo-tpl
    parameters:
      - name: target
        default_value: "log:info"
        description: none
    routes:
      - id: "base-route"
        from: "direct:start"
        steps:
          - to: "{{target}}"
templated_routes:
  - route_template_ref: solo-tpl
    route_id: a
    parameters: {}
  - route_template_ref: solo-tpl
    route_id: b
    parameters: {}
  - route_template_ref: solo-tpl
    route_id: c
    parameters: {}
"#,
    );
    let pattern = dir.path().join("*.yaml").to_string_lossy().to_string();

    let routes = discover_routes_with_threshold_and_security(
        &[pattern],
        camel_api::stream_cache::DEFAULT_STREAM_CACHE_THRESHOLD,
        SecurityCompileContext::default(),
    )
    .unwrap();
    let mut ids: Vec<&str> = routes.iter().map(|r| r.route_id()).collect();
    ids.sort_unstable();
    assert_eq!(ids, vec!["a", "b", "c"], "three distinct ids expected");
}

#[test]
fn invalid_parameter_class_survives_aggregation() {
    let dir = tempfile::tempdir().unwrap();
    write_temp_yaml(
        dir.path(),
        "bad-coercion.yaml",
        r#"
routes: []
templates:
  - id: num-tpl
    parameters:
      - name: delay
        type: number
    routes:
      - id: num-route
        from: "direct:start"
        steps:
          - delay: "{{delay}}"
templated_routes:
  - route_template_ref: num-tpl
    parameters:
      delay: "abc"
"#,
    );
    let pattern = dir.path().join("*.yaml").to_string_lossy().to_string();

    let err = match discover_routes_with_threshold_and_security(
        &[pattern],
        camel_api::stream_cache::DEFAULT_STREAM_CACHE_THRESHOLD,
        SecurityCompileContext::default(),
    ) {
        Ok(_) => panic!("expected non-coercible number parameter to fail"),
        Err(e) => e,
    };
    let camel_dsl::DiscoveryError::MaterializationFailures { failures } = &err else {
        panic!("expected MaterializationFailures, got: {err}")
    };
    assert_eq!(failures.len(), 1, "one failure expected: {err}");
    assert!(
        matches!(
            &failures[0].error,
            TemplateError::InvalidParameter(name, ty, value)
                if name == "delay" && ty == "number" && value == "abc"
        ),
        "expected InvalidParameter class at aggregated surface, got: {:?}",
        failures[0].error
    );
}
