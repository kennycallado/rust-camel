//! JSON route definition parser.
//!
//! Reuses the same AST types as the YAML parser ([`crate::yaml::RouteDslRoutes`],
//! [`crate::yaml::RouteDslRoute`], [`crate::yaml::RouteDslStep`])
//! since JSON is a subset of YAML's data model. The only difference is the deserializer.
//!
//! ## Stability note
//!
//! The parser uses [`RouteDslRoutes`] directly (no separate JSON-specific AST types).
//! The `RouteDsl*` AST types are **not a stable SDK contract**. SDKs and external
//! consumers should target [`CanonicalRouteSpec`] for forward compatibility.

use std::path::Path;

use camel_api::{CamelError, CanonicalRouteSpec};
use camel_core::route::RouteDefinition;

use crate::compile::{
    compile_declarative_route, compile_declarative_route_to_canonical,
    compile_declarative_route_with_stream_cache_threshold,
};
use crate::input_format::{InputFormat, annotate_format};
use crate::model::SecurityCompileContext;
use crate::yaml::{RouteDslRoutes, route_dsl_to_declarative_route};

/// Parse a JSON string into declarative route models.
pub fn parse_json_to_declarative(
    json: &str,
) -> Result<Vec<crate::model::DeclarativeRoute>, CamelError> {
    annotate_format(InputFormat::Json, parse_json_to_declarative_inner(json))
}

fn parse_json_to_declarative_inner(
    json: &str,
) -> Result<Vec<crate::model::DeclarativeRoute>, CamelError> {
    let mut dsl: RouteDslRoutes = serde_json::from_str(json)
        .map_err(|e| CamelError::RouteError(format!("JSON parse error: {e}")))?;

    // Expand REST blocks (review I2): the JSON authoring path previously
    // dropped `rest:` silently. JSON is a full-DSL authoring format
    // (ADR-0026), so it lowers `rest:` exactly like the YAML path — via the
    // shared helper that also runs cross-block validation.
    crate::rest::expand_rest_into(&mut dsl.routes, &dsl.rest)?;
    crate::rest::check_duplicate_route_ids(&dsl.routes)?;

    dsl.routes
        .into_iter()
        .map(route_dsl_to_declarative_route)
        .collect()
}

/// Parse a JSON string into compiled [`RouteDefinition`]s.
pub fn parse_json(json: &str) -> Result<Vec<RouteDefinition>, CamelError> {
    annotate_format(InputFormat::Json, parse_json_inner(json))
}

fn parse_json_inner(json: &str) -> Result<Vec<RouteDefinition>, CamelError> {
    parse_json_to_declarative_inner(json)?
        .into_iter()
        .map(compile_declarative_route)
        .collect()
}

/// Parse a JSON string with a custom stream-cache threshold.
pub fn parse_json_with_threshold(
    json: &str,
    stream_cache_threshold: usize,
) -> Result<Vec<RouteDefinition>, CamelError> {
    annotate_format(
        InputFormat::Json,
        parse_json_with_threshold_inner(json, stream_cache_threshold),
    )
}

fn parse_json_with_threshold_inner(
    json: &str,
    stream_cache_threshold: usize,
) -> Result<Vec<RouteDefinition>, CamelError> {
    parse_json_with_threshold_and_security_inner(
        json,
        stream_cache_threshold,
        SecurityCompileContext::default(),
    )
}

pub fn parse_json_with_threshold_and_security(
    json: &str,
    stream_cache_threshold: usize,
    security_ctx: SecurityCompileContext,
) -> Result<Vec<RouteDefinition>, CamelError> {
    annotate_format(
        InputFormat::Json,
        parse_json_with_threshold_and_security_inner(json, stream_cache_threshold, security_ctx),
    )
}

fn parse_json_with_threshold_and_security_inner(
    json: &str,
    stream_cache_threshold: usize,
    security_ctx: SecurityCompileContext,
) -> Result<Vec<RouteDefinition>, CamelError> {
    parse_json_to_declarative_inner(json)?
        .into_iter()
        .map(|route| {
            compile_declarative_route_with_stream_cache_threshold(
                route,
                stream_cache_threshold,
                security_ctx.clone(),
            )
        })
        .collect()
}

/// Parse a JSON string into canonical route specs.
pub fn parse_json_to_canonical(json: &str) -> Result<Vec<CanonicalRouteSpec>, CamelError> {
    annotate_format(InputFormat::Json, parse_json_to_canonical_inner(json))
}

fn parse_json_to_canonical_inner(json: &str) -> Result<Vec<CanonicalRouteSpec>, CamelError> {
    let routes = parse_json_to_declarative_inner(json)?;
    for route in &routes {
        if route.security_policy.is_some() {
            return Err(CamelError::RouteError(
                "routes with security_policy cannot use the canonical/hot-reload path (not yet supported)".into(),
            ));
        }
    }
    routes
        .into_iter()
        .map(|r| compile_declarative_route_to_canonical(r, false).map(|(spec, _)| spec))
        .collect()
}

/// Load a JSON route definition from a file.
///
/// Reads the file contents and parses them as JSON. No environment variable interpolation is performed.
pub fn load_json_from_file(path: &Path) -> Result<Vec<RouteDefinition>, CamelError> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| CamelError::Io(format!("Failed to read {}: {e}", path.display())))?;
    let annotated = annotate_format(InputFormat::Json, parse_json_inner(&content));
    annotated.map_err(|e| match e {
        CamelError::RouteError(msg) => {
            CamelError::RouteError(format!("{msg} (in {})", path.display()))
        }
        other => other,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::DeclarativeStep;

    /// Basic route: parse JSON to declarative model.
    #[test]
    fn test_basic_route_to_declarative() {
        let json = r#"
        {
            "routes": [
                {
                    "id": "test-route",
                    "from": "timer:tick?period=1000",
                    "steps": [
                        {"set_header": {"key": "source", "value": "timer"}},
                        {"to": "log:info"}
                    ]
                }
            ]
        }"#;
        let routes = parse_json_to_declarative(json).unwrap();
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].route_id, "test-route");
        assert_eq!(routes[0].from, "timer:tick?period=1000");
        assert_eq!(routes[0].steps.len(), 2);
    }

    /// Route metadata: auto_startup and startup_order round-trip correctly.
    #[test]
    fn test_route_metadata_auto_startup_and_startup_order() {
        let json = r#"
        {
            "routes": [
                {
                    "id": "meta-route",
                    "from": "direct:start",
                    "auto_startup": false,
                    "startup_order": 42
                }
            ]
        }"#;
        let routes = parse_json_to_declarative(json).unwrap();
        assert_eq!(routes.len(), 1);
        assert!(!routes[0].auto_startup);
        assert_eq!(routes[0].startup_order, 42);
    }

    /// Error handler with on_exceptions including handled_by.
    #[test]
    fn test_error_handler_with_handled_by() {
        let json = r#"
        {
            "routes": [
                {
                    "id": "eh-route",
                    "from": "direct:start",
                    "error_handler": {
                        "dead_letter_channel": "log:dlc",
                        "on_exceptions": [
                            {
                                "kind": "Io",
                                "retry": {
                                    "max_attempts": 3,
                                    "handled_by": "log:io"
                                }
                            }
                        ]
                    }
                }
            ]
        }"#;
        let routes = parse_json_to_declarative(json).unwrap();
        let eh = routes[0]
            .error_handler
            .as_ref()
            .expect("error handler should be present");
        let clauses = eh
            .on_exceptions
            .as_ref()
            .expect("on_exceptions should be present");
        assert_eq!(clauses.len(), 1);
        assert_eq!(clauses[0].kind.as_deref(), Some("Io"));
        let retry = clauses[0].retry.as_ref().expect("retry should be present");
        assert_eq!(retry.max_attempts, 3);
        assert_eq!(retry.handled_by.as_deref(), Some("log:io"));
    }

    /// Empty route id must be rejected.
    #[test]
    fn test_empty_id_fails() {
        let json = r#"
        {
            "routes": [
                {
                    "id": "",
                    "from": "timer:tick"
                }
            ]
        }"#;
        let result = parse_json_to_declarative(json);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("route 'id' must not be empty"),
            "unexpected error: {err}"
        );
    }

    /// Invalid JSON must produce an error containing "JSON".
    #[test]
    fn test_invalid_json_error_says_json() {
        let json = "{ not valid json }}}";
        let result = parse_json_to_declarative(json);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("JSON DSL error: JSON parse error:"),
            "expected 'JSON DSL error: JSON parse error:' in error, got: {err}"
        );
    }

    /// Compiled route via parse_json produces a RouteDefinition.
    #[test]
    fn test_compiled_route_via_parse_json() {
        let json = r#"
        {
            "routes": [
                {
                    "id": "compiled-route",
                    "from": "timer:tick",
                    "steps": [
                        {"to": "log:info"}
                    ]
                }
            ]
        }"#;
        let defs = parse_json(json).unwrap();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].route_id(), "compiled-route");
        assert_eq!(defs[0].from_uri(), "timer:tick");
    }

    /// parse_json_with_threshold compiles routes with custom stream-cache threshold.
    #[test]
    fn test_threshold_parse() {
        let json = r#"
        {
            "routes": [
                {
                    "id": "threshold-route",
                    "from": "timer:tick",
                    "steps": [
                        {"stream_cache": true},
                        {"to": "log:info"}
                    ]
                }
            ]
        }"#;
        let defs = parse_json_with_threshold(json, 8192).unwrap();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].route_id(), "threshold-route");
    }

    /// Canonical conversion via parse_json_to_canonical.
    #[test]
    fn test_canonical_conversion() {
        let json = r#"
        {
            "routes": [
                {
                    "id": "canonical-v1",
                    "from": "direct:start",
                    "steps": [
                        {"to": "mock:out"},
                        {"log": {"message": "hello"}},
                        {"stop": true}
                    ]
                }
            ]
        }"#;
        let routes = parse_json_to_canonical(json).unwrap();
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].route_id, "canonical-v1");
        assert_eq!(routes[0].from, "direct:start");
        assert_eq!(routes[0].version, 2);
        assert_eq!(routes[0].steps.len(), 3);
    }

    /// The "loop" step JSON key works.
    #[test]
    fn test_loop_step_json_key() {
        let json = r#"
        {
            "routes": [
                {
                    "id": "loop-route",
                    "from": "direct:start",
                    "steps": [
                        {"loop": 3}
                    ]
                }
            ]
        }"#;
        let routes = parse_json_to_declarative(json).unwrap();
        assert_eq!(routes.len(), 1);
        match &routes[0].steps[0] {
            DeclarativeStep::Loop(def) => {
                assert_eq!(def.count, Some(3));
            }
            other => panic!("expected Loop step, got {:?}", other),
        }
    }

    /// Multiple routes in a single JSON document.
    #[test]
    fn test_multiple_routes() {
        let json = r#"
        {
            "routes": [
                {
                    "id": "route-a",
                    "from": "timer:tick",
                    "steps": [{"to": "log:info"}]
                },
                {
                    "id": "route-b",
                    "from": "timer:tock",
                    "auto_startup": false,
                    "startup_order": 10
                }
            ]
        }"#;
        let defs = parse_json(json).unwrap();
        assert_eq!(defs.len(), 2);
        assert_eq!(defs[0].route_id(), "route-a");
        assert_eq!(defs[1].route_id(), "route-b");
    }

    /// Defaults: auto_startup=true, startup_order=1000 when omitted.
    #[test]
    fn test_defaults() {
        let json = r#"
        {
            "routes": [
                {
                    "id": "default-route",
                    "from": "timer:tick"
                }
            ]
        }"#;
        let defs = parse_json(json).unwrap();
        assert!(defs[0].auto_startup());
        assert_eq!(defs[0].startup_order(), 1000);
    }

    /// File loading via load_json_from_file.
    #[test]
    fn test_file_loading() {
        use std::io::Write;
        let mut file = tempfile::NamedTempFile::new().unwrap();

        let json_content = r#"
        {
            "routes": [
                {
                    "id": "file-route",
                    "from": "timer:tick",
                    "steps": [{"to": "log:info"}]
                }
            ]
        }"#;

        file.write_all(json_content.as_bytes()).unwrap();

        let defs = load_json_from_file(file.path()).unwrap();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].route_id(), "file-route");
    }

    /// Missing file must return an error.
    #[test]
    fn test_missing_file_error() {
        let result = load_json_from_file(Path::new("/nonexistent/path/routes.json"));
        assert!(result.is_err());
        let err = result.err().unwrap().to_string();
        assert!(
            err.contains("Failed to read"),
            "expected file read error, got: {err}"
        );
    }

    #[test]
    fn test_function_step_json_parses_and_compiles() {
        let json = r#"
        {
            "routes": [
                {
                    "id": "fn-json-route",
                    "from": "direct:start",
                    "steps": [
                        {
                            "function": {
                                "runtime": "deno",
                                "source": "return { body: 'ok' };",
                                "timeout_ms": 3000
                            }
                        }
                    ]
                }
            ]
        }"#;
        let defs = parse_json(json).unwrap();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].route_id(), "fn-json-route");
    }

    #[test]
    fn test_function_step_json_compiles_to_declarative_function() {
        let json = r#"
        {
            "routes": [
                {
                    "id": "fn-decl",
                    "from": "direct:start",
                    "steps": [
                        {
                            "function": {
                                "runtime": "deno",
                                "source": "return { body: 1 };"
                            }
                        }
                    ]
                }
            ]
        }"#;
        let routes = parse_json_to_declarative(json).unwrap();
        match &routes[0].steps[0] {
            DeclarativeStep::Function(def) => {
                assert_eq!(def.runtime, "deno");
                assert_eq!(def.source, "return { body: 1 };");
                assert_eq!(def.timeout_ms, None);
            }
            other => panic!("expected Function, got {other:?}"),
        }
    }

    #[test]
    fn test_function_step_rejected_by_canonical_json() {
        let json = r#"
        {
            "routes": [
                {
                    "id": "fn-canonical-reject",
                    "from": "direct:start",
                    "steps": [
                        {
                            "function": {
                                "runtime": "deno",
                                "source": "return {};",
                                "timeout_ms": 1000
                            }
                        }
                    ]
                }
            ]
        }"#;
        let err = parse_json_to_canonical(json).unwrap_err().to_string();
        assert!(
            err.contains("canonical v1 does not support step `function`"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn test_set_property_step_json_parses_to_declarative() {
        let json = r#"
        {
            "routes": [
                {
                    "id": "property-json-route",
                    "from": "direct:start",
                    "steps": [
                        {"set_property": {"name": "traceId", "value": "abc-123"}}
                    ]
                }
            ]
        }"#;
        let routes = parse_json_to_declarative(json).unwrap();
        match &routes[0].steps[0] {
            DeclarativeStep::SetProperty(def) => {
                assert_eq!(def.key, "traceId");
                assert_eq!(
                    def.value,
                    crate::model::ValueSourceDef::Literal(serde_json::Value::String(
                        "abc-123".into()
                    ))
                );
            }
            other => panic!("expected SetProperty, got {other:?}"),
        }
    }

    #[test]
    fn dollar_schema_key_is_accepted() {
        let json = r#"{
            "$schema": "https://example.com/route-schema.json",
            "routes": [{"id": "r1", "from": "direct:start", "steps": [{"to": "direct:end"}]}]
        }"#;
        let parsed = parse_json_to_declarative(json).expect("$schema key must be accepted");
        assert_eq!(parsed.len(), 1);
    }

    #[test]
    fn parse_json_to_canonical_rejects_security_policy() {
        let json = r#"{
            "routes": [{
                "id": "r-sec",
                "from": "direct:start",
                "security_policy": { "roles": ["admin"] },
                "steps": [{ "to": "log:info" }]
            }]
        }"#;
        let result = parse_json_to_canonical(json);
        assert!(result.is_err());
    }

    #[test]
    fn semantic_error_carries_json_format_prefix() {
        let json = r#"{"routes": [{"id": "", "from": "timer:tick"}]}"#;
        let result = parse_json_to_declarative(json);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("JSON DSL error:"),
            "expected 'JSON DSL error:' prefix, got: {err}"
        );
        assert!(
            err.contains("route 'id' must not be empty"),
            "expected underlying semantic message, got: {err}"
        );
    }

    #[test]
    fn json_parse_error_carries_format_prefix() {
        let json = "{ not valid json }}}";
        let result = parse_json_to_declarative(json);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("JSON DSL error: JSON parse error:"),
            "expected double prefix for parse errors, got: {err}"
        );
    }

    /// Review I2: the JSON authoring path must lower `rest:` blocks, not
    /// silently drop them. JSON is a full-DSL authoring format (ADR-0026),
    /// so a `rest:` block expands exactly like the YAML path.
    #[test]
    fn json_rest_block_expands_into_routes() {
        let json = r#"{
            "rest": [
                {
                    "host": "0.0.0.0",
                    "port": 8080,
                    "path": "/users",
                    "operations": [
                        { "method": "GET", "path": "/{id}", "operation_id": "getUser", "to": "bean:svc" },
                        { "method": "POST", "path": "/", "operation_id": "createUser", "to": "bean:create" }
                    ]
                }
            ]
        }"#;
        let routes = parse_json_to_declarative(json).unwrap();
        // Two operations → two lowered routes.
        assert_eq!(routes.len(), 2);
        let ids: Vec<&str> = routes.iter().map(|r| r.route_id.as_str()).collect();
        assert!(ids.contains(&"getUser"));
        assert!(ids.contains(&"createUser"));
        // Each route's from-URI carries the templated path + httpMethod.
        let get_route = routes.iter().find(|r| r.route_id == "getUser").unwrap();
        assert!(get_route.from.contains("/users/{id}"));
        assert!(get_route.from.contains("httpMethod=GET"));
    }

    /// Review I2 + C3: the JSON path runs the same cross-block validation
    /// as YAML — duplicate (method, path) tuples must be rejected.
    #[test]
    fn json_rest_rejects_duplicate_method_path() {
        let json = r#"{
            "rest": [
                {
                    "path": "/users",
                    "operations": [
                        { "method": "GET", "path": "/{id}", "operation_id": "a", "to": "bean:x" }
                    ]
                },
                {
                    "path": "/users",
                    "operations": [
                        { "method": "GET", "path": "/{id}", "operation_id": "b", "to": "bean:y" }
                    ]
                }
            ]
        }"#;
        let result = parse_json_to_declarative(json);
        assert!(
            result.is_err(),
            "JSON rest must reject duplicate (method, path)"
        );
    }
}
