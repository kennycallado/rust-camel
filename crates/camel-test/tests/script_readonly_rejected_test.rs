//! Task 2.2 (`364-simpleset`): a `script` step requires a language that
//! implements `MutatingExpression`.
//!
//! YAML and JSON route-add entry points must reject a read-only language
//! (`simple`, `jsonpath`) at compile time with the shared capability message,
//! and the rejection must leave no registration state behind. A mutating
//! language (`rhai`) must still add successfully.

use std::time::Duration;

use camel_api::CamelError;
use camel_core::{LanguageRegistryError, RouteDefinition};
use camel_dsl::{parse_json, parse_yaml};
use camel_language_jsonpath::JsonPathLanguage;
use camel_language_rhai::RhaiLanguage;
use camel_test::CamelTestContext;

/// Register `jsonpath` and `rhai` before any `add_route`.
///
/// The harness registry comes from `languages_from_config`, which may already
/// pre-register these languages when the workspace unifies the language
/// features. `LanguageRegistryError` has the single `AlreadyRegistered`
/// variant, so this tolerant match is exhaustive and cannot swallow another
/// error. `simple` is registered by the default registry and is not touched.
async fn register_languages(h: &CamelTestContext) {
    let mut guard = tokio::time::timeout(Duration::from_secs(10), h.ctx().lock())
        .await
        .expect("language registry lock timed out");
    match guard.register_language("jsonpath", Box::new(JsonPathLanguage::new())) {
        Ok(()) | Err(LanguageRegistryError::AlreadyRegistered { .. }) => {}
    }
    match guard.register_language("rhai", Box::new(RhaiLanguage::new())) {
        Ok(()) | Err(LanguageRegistryError::AlreadyRegistered { .. }) => {}
    }
    drop(guard);
}

/// The exact shared capability message for a read-only `{language}`.
fn capability_message(language: &str) -> String {
    format!(
        "script: language '{language}' does not support mutating expressions; a `script` step requires a language that implements MutatingExpression. To replace the body with a read-only expression, use a `set_body`/`transform` step with this language instead"
    )
}

/// Assert the read-only `script` route is rejected with the shared capability
/// message, then prove no registration state was left: a valid route with the
/// SAME id must add cleanly.
async fn assert_rejected_without_state(
    h: &CamelTestContext,
    mut routes: Vec<RouteDefinition>,
    language: &str,
    route_id: &str,
) {
    assert_eq!(routes.len(), 1, "fixture must contain exactly one route");

    let err = h
        .add_route(routes.remove(0))
        .await
        .expect_err("a read-only script language must be rejected at route add");

    match &err {
        CamelError::RouteError(_) => {}
        other => panic!("expected CamelError::RouteError, got: {other:?}"),
    }

    let text = err.to_string();
    let expected = capability_message(language);
    assert!(
        text.contains(&expected),
        "error must contain the exact shared capability message\nexpected: {expected}\nactual:   {text}"
    );

    // If the rejected add had left registration state, this duplicate-id add
    // would fail. `direct:start` is reused on purpose.
    let valid = format!(
        "routes:\n  - id: {route_id}\n    from: \"direct:start\"\n    steps:\n      - to: \"mock:result\"\n"
    );
    let mut valid_routes = parse_yaml(&valid).expect("valid route must parse");
    assert_eq!(
        valid_routes.len(),
        1,
        "valid fixture must contain one route"
    );
    let result = h.add_route(valid_routes.remove(0)).await;
    assert!(
        result.is_ok(),
        "rejected route must leave no registration state; re-adding id {route_id:?} failed: {result:?}"
    );
}

const YAML_SIMPLE: &str = r#"routes:
  - id: script-simple
    from: "direct:start"
    steps:
      - script:
          language: simple
          source: "${body}"
      - to: "mock:result"
"#;

const YAML_JSONPATH: &str = r#"routes:
  - id: script-jsonpath
    from: "direct:start"
    steps:
      - script:
          language: jsonpath
          source: "$.a"
      - to: "mock:result"
"#;

const JSON_SIMPLE: &str = r#"{
  "routes": [
    {
      "id": "script-simple",
      "from": "direct:start",
      "steps": [
        { "script": { "language": "simple", "source": "${body}" } },
        { "to": "mock:result" }
      ]
    }
  ]
}"#;

const JSON_JSONPATH: &str = r#"{
  "routes": [
    {
      "id": "script-jsonpath",
      "from": "direct:start",
      "steps": [
        { "script": { "language": "jsonpath", "source": "$.a" } },
        { "to": "mock:result" }
      ]
    }
  ]
}"#;

const YAML_RHAI: &str = r#"routes:
  - id: script-rhai
    from: "direct:start"
    steps:
      - script:
          language: rhai
          source: 'body = "ok";'
      - to: "mock:result"
"#;

#[tokio::test(flavor = "multi_thread")]
async fn yaml_script_simple_rejected_at_route_add() {
    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .build()
        .await;
    register_languages(&h).await;

    let routes = parse_yaml(YAML_SIMPLE).expect("YAML simple fixture must parse");
    assert_rejected_without_state(&h, routes, "simple", "script-simple").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn yaml_script_jsonpath_rejected_at_route_add() {
    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .build()
        .await;
    register_languages(&h).await;

    let routes = parse_yaml(YAML_JSONPATH).expect("YAML jsonpath fixture must parse");
    assert_rejected_without_state(&h, routes, "jsonpath", "script-jsonpath").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn json_script_simple_rejected_at_route_add() {
    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .build()
        .await;
    register_languages(&h).await;

    let routes = parse_json(JSON_SIMPLE).expect("JSON simple fixture must parse");
    assert_rejected_without_state(&h, routes, "simple", "script-simple").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn json_script_jsonpath_rejected_at_route_add() {
    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .build()
        .await;
    register_languages(&h).await;

    let routes = parse_json(JSON_JSONPATH).expect("JSON jsonpath fixture must parse");
    assert_rejected_without_state(&h, routes, "jsonpath", "script-jsonpath").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn rhai_script_route_add_still_succeeds() {
    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .build()
        .await;
    register_languages(&h).await;

    let mut routes = parse_yaml(YAML_RHAI).expect("YAML rhai fixture must parse");
    assert_eq!(routes.len(), 1, "fixture must contain exactly one route");
    let result = h.add_route(routes.remove(0)).await;
    assert!(
        result.is_ok(),
        "rhai implements MutatingExpression, its script route must add: {result:?}"
    );
}
