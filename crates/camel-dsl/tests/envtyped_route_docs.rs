//! envtyped-routes Task 1.8 — loader-path integration fixtures.
//!
//! Pins the typed-leaf env-interpolation contract through the DISCOVERY
//! loader (`discover_routes_with_threshold_security_and_env`), not the bare
//! parse seams:
//!
//! - YAML/JSON route documents with `${env:}` at typed leaves outside
//!   `routes:` (`rest:` ports) coerce on the repair pass, and a
//!   non-clean default keeps the first-pass typed error.
//! - A `type: number` template parameter fed by `${env:}` materializes as a
//!   typed numeric value.
//! - A DIRECT `${env:}` token inside a template BODY stays out of the probe
//!   (stay-out): the document loads (the widened probe repairs `rest:`), and
//!   materialization still fails with today's typed error.
//!
//! Every lookup is injected through the hermetic discovery entry — the
//! process environment is never consulted (`RC8_*` names are unique to this
//! file).

use camel_api::stream_cache::DEFAULT_STREAM_CACHE_THRESHOLD;
use camel_component_api::test_support::EnvGuard;
use camel_core::route::{BuilderStep, RouteDefinition};
use camel_dsl::SecurityCompileContext;
use camel_dsl::discovery::{DiscoveryError, discover_routes_with_threshold_security_and_env};
use std::path::Path;
use tempfile::tempdir;

/// Write an inline fixture document into `dir` and return a literal glob
/// pattern naming it.
fn write_doc(dir: &Path, name: &str, body: &str) -> String {
    let path = dir.join(name);
    std::fs::write(&path, body).expect("fixture document must be writable");
    path.to_string_lossy().to_string()
}

/// Discover one fixture through the env-lookup-injected loader entry.
fn discover(
    pattern: &str,
    lookup: &dyn Fn(&str) -> Option<String>,
) -> Result<Vec<RouteDefinition>, DiscoveryError> {
    discover_routes_with_threshold_security_and_env(
        &[pattern.to_string()],
        DEFAULT_STREAM_CACHE_THRESHOLD,
        SecurityCompileContext::default(),
        lookup,
    )
}

/// The first step's `delay_ms` (the typed-parameter acceptance shape).
fn delay_ms_of(route: &RouteDefinition) -> u64 {
    match &route.steps()[0] {
        BuilderStep::Delay { config } => config.delay_ms,
        other => panic!("expected a delay step, got: {other:?}"),
    }
}

/// `max_requests` of the route's first throttle step, if it has one.
fn throttle_max_requests(route: &RouteDefinition) -> Option<usize> {
    route.steps().iter().find_map(|step| match step {
        BuilderStep::Throttle { config, .. } => Some(config.max_requests),
        _ => None,
    })
}

// ── Fixtures ────────────────────────────────────────────────────────────

/// One REST block, one GET operation; the port default is a clean integer.
const YAML_REST_DOC: &str = r#"
rest:
  - host: 127.0.0.1
    port: ${env:RC8_YAML_REST_PORT:-8184}
    path: /api
    operations:
      - method: GET
        to: direct:list
"#;

/// Same shape, non-clean default: no probe candidate, first-pass error kept.
const YAML_REST_BAD_DOC: &str = r#"
rest:
  - host: 127.0.0.1
    port: ${env:RC8_YAML_BAD_PORT:-notanumber}
    path: /api
    operations:
      - method: GET
        to: direct:list
"#;

/// `type: number` parameter used as a whole-node placeholder at a numeric
/// body position; the instantiation supplies an `${env:}` value.
const YAML_TYPED_TEMPLATE_DOC: &str = r#"
routes: []
templates:
  - id: rc8-delay-tpl
    parameters:
      - name: delay
        type: number
    routes:
      - id: "rc8-delay-route"
        from: "direct:start"
        steps:
          - delay: "{{delay}}"
templated_routes:
  - route_template_ref: rc8-delay-tpl
    parameters:
      delay: "${env:RC8_TPL_DELAY:-500}"
"#;

/// Template body carrying a DIRECT `${env:}` token at a numeric position
/// (no typed parameter) plus a `rest:` port the widened probe repairs.
const YAML_BODY_TOKEN_CORE: &str = r#"
routes: []
rest:
  - host: 127.0.0.1
    port: ${env:RC8_BODY_REST_PORT:-8185}
    path: /api
    operations:
      - method: GET
        to: direct:list
templates:
  - id: rc8-body-token-tpl
    parameters: []
    routes:
      - id: "rc8-body-token-route"
        from: "direct:start"
        steps:
          - delay: ${env:RC8_BODY_DELAY:-500}
"#;

/// Instantiation appended to [`YAML_BODY_TOKEN_CORE`] to reach
/// materialization (the full failing document).
const YAML_BODY_TOKEN_INSTANCE: &str = r#"
templated_routes:
  - route_template_ref: rc8-body-token-tpl
    parameters: {}
"#;

/// JSON route document: `rest:` port placeholder plus a `routes:` throttle
/// numeric placeholder (two probe candidates, both must coerce).
const JSON_ROUTE_DOC: &str = r#"{
  "rest": [
    {
      "host": "127.0.0.1",
      "port": "${env:RC8_JSON_REST_PORT:-8186}",
      "path": "/api",
      "operations": [
        { "method": "GET", "to": "direct:list" }
      ]
    }
  ],
  "routes": [
    {
      "id": "rc8-json-throttle",
      "from": "direct:start",
      "steps": [
        {
          "throttle": {
            "max_requests": "${env:RC8_JSON_THROTTLE:-3}",
            "period_secs": 1
          }
        }
      ]
    }
  ]
}
"#;

/// Same JSON shape; the port default is not a clean integer and keeps the
/// first-pass typed error.
const JSON_ROUTE_BAD_DOC: &str = r#"{
  "rest": [
    {
      "host": "127.0.0.1",
      "port": "${env:RC8_JSON_BAD_PORT:-notanumber}",
      "path": "/api",
      "operations": [
        { "method": "GET", "to": "direct:list" }
      ]
    }
  ],
  "routes": [
    {
      "id": "rc8-json-throttle",
      "from": "direct:start",
      "steps": [
        {
          "throttle": {
            "max_requests": "${env:RC8_JSON_THROTTLE:-3}",
            "period_secs": 1
          }
        }
      ]
    }
  ]
}
"#;

// ── YAML route document: rest port through the loader ───────────────────

#[test]
fn yaml_rest_doc_loader_path_coerces() {
    // The process environment is never consulted: even with the variable set
    // process-wide, the injected lookup (returning `None`) yields the default.
    let _guard = EnvGuard::set("RC8_YAML_REST_PORT", "9999");

    let dir = tempdir().expect("tempdir");
    let pattern = write_doc(dir.path(), "rest.yaml", YAML_REST_DOC);

    let routes = discover(&pattern, &|_| None).expect("rest port default must coerce");
    assert_eq!(routes.len(), 1, "one REST block lowers to one route");
    let uri = routes[0].from_uri();
    assert!(
        uri.contains(":8184/"),
        "expected the lowered REST URI to carry the injected default port, got: {uri}"
    );
    assert!(
        !uri.contains("9999"),
        "the process environment must not leak into discovery: {uri}"
    );
}

#[test]
fn yaml_rest_doc_loader_path_negative_keeps_error() {
    let dir = tempdir().expect("tempdir");
    let pattern = write_doc(dir.path(), "rest-bad.yaml", YAML_REST_BAD_DOC);

    let err = match discover(&pattern, &|_| None) {
        Ok(routes) => panic!(
            "a non-clean port default must keep the first-pass typed error, got {} routes",
            routes.len()
        ),
        Err(err) => err,
    };
    let DiscoveryError::Yaml { error, .. } = &err else {
        panic!("expected the first-pass YAML parse error, got: {err:?}");
    };
    assert!(
        error.contains("type mismatch") && error.contains("expected unsigned integer"),
        "expected the typed error class (string at an integer position), got: {error}"
    );
    assert!(
        error.contains("rest[0].port"),
        "expected the error to name the offending position, got: {error}"
    );
}

// ── YAML templates: typed parameter vs. body direct token ───────────────

#[test]
fn yaml_template_numeric_via_typed_parameter_materializes() {
    let dir = tempdir().expect("tempdir");
    let pattern = write_doc(dir.path(), "typed-delay.yaml", YAML_TYPED_TEMPLATE_DOC);

    // Injected lookup: unset, so the `${env:...:-500}` default is the value.
    let routes = discover(&pattern, &|_| None)
        .expect("a typed number parameter must materialize from the env default");
    assert_eq!(routes.len(), 1);
    assert_eq!(routes[0].route_id(), "rc8-delay-route");
    assert_eq!(
        delay_ms_of(&routes[0]),
        500,
        "the typed number parameter must land as a numeric delay, not a string"
    );
}

#[test]
fn yaml_template_body_direct_token_stays_limited() {
    let dir = tempdir().expect("tempdir");

    // (1) The document itself loads: without an instantiation the widened
    // probe still repairs the `rest:` port, so discovery returns the lowered
    // REST route carrying the coerced default.
    let load_only = write_doc(dir.path(), "body-token-core.yaml", YAML_BODY_TOKEN_CORE);
    let routes = discover(&load_only, &|_| None)
        .expect("the document must load: the widened probe repairs the rest port");
    assert_eq!(routes.len(), 1, "one REST block lowers to one route");
    let uri = routes[0].from_uri();
    assert!(
        uri.contains(":8185/"),
        "expected the repaired REST port in the lowered URI, got: {uri}"
    );

    // (2) Materializing the templated route still fails: the template BODY is
    // a stay-out subtree, so its direct token stays the string "500" and the
    // typed compile rejects it. Reaching materialization at all proves the
    // document parsed (a still-string rest port would have failed earlier
    // with `DiscoveryError::Yaml`).
    let full_doc = format!("{YAML_BODY_TOKEN_CORE}{YAML_BODY_TOKEN_INSTANCE}");
    let full = write_doc(dir.path(), "body-token-full.yaml", &full_doc);
    let err = match discover(&full, &|_| None) {
        Ok(routes) => panic!(
            "a direct template-body token must fail materialization, got {} routes",
            routes.len()
        ),
        Err(err) => err,
    };
    let DiscoveryError::MaterializationFailures { failures } = &err else {
        panic!(
            "the document must LOAD (rest port repaired) and fail only at \
             materialization, got: {err:?}"
        );
    };
    assert_eq!(failures.len(), 1, "exactly one failing instance: {err:?}");
    assert_eq!(failures[0].template_ref, "rc8-body-token-tpl");
    let message = failures[0].error.to_string();
    assert!(
        message.contains("data did not match any variant of untagged enum RouteDslStep")
            || message.contains("invalid type: string"),
        "expected today's typed materialization error, got: {message}"
    );
}

// ── JSON route document: rest port + throttle through the loader ────────

#[test]
fn json_route_doc_loader_path_coerces() {
    let dir = tempdir().expect("tempdir");
    let pattern = write_doc(dir.path(), "routes.json", JSON_ROUTE_DOC);

    let routes = discover(&pattern, &|_| None)
        .expect("both JSON numeric placeholders must coerce through discovery");
    assert_eq!(routes.len(), 2, "rest route + regular route");

    let rest_uri = routes
        .iter()
        .map(RouteDefinition::from_uri)
        .find(|uri| uri.starts_with("http://"))
        .unwrap_or_else(|| {
            panic!(
                "expected a lowered REST route, got from_uri values: {:?}",
                routes
                    .iter()
                    .map(RouteDefinition::from_uri)
                    .collect::<Vec<_>>()
            )
        });
    assert!(
        rest_uri.contains(":8186/"),
        "expected the coerced REST port in the lowered URI, got: {rest_uri}"
    );

    let throttle = routes
        .iter()
        .find_map(throttle_max_requests)
        .expect("expected the regular route's throttle step");
    assert_eq!(
        throttle, 3,
        "the throttle placeholder must coerce to the numeric default"
    );
}

#[test]
fn json_route_doc_loader_path_negative_keeps_error() {
    let dir = tempdir().expect("tempdir");
    let pattern = write_doc(dir.path(), "routes-bad.json", JSON_ROUTE_BAD_DOC);

    let err = match discover(&pattern, &|_| None) {
        Ok(routes) => panic!(
            "a non-clean port default must keep the first-pass error, got {} routes",
            routes.len()
        ),
        Err(err) => err,
    };
    let DiscoveryError::Json { error, .. } = &err else {
        panic!("expected the first-pass JSON parse error, got: {err:?}");
    };
    assert!(
        error.contains("invalid type: string"),
        "expected the typed error class (string at the u16 port), got: {error}"
    );
    assert!(
        error.contains("notanumber"),
        "expected the error to name the offending value, got: {error}"
    );
}
