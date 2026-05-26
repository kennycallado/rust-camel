//! Template materializer — resolves parameters, substitutes placeholders in JSON,
//! and compiles the resulting declarative routes.
//!
//! This module bridges the template system (Phase 1) with the DSL compiler,
//! enabling runtime instantiation of parameterized route templates.

use std::collections::BTreeMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use camel_api::CamelError;
use camel_api::template::{RouteTemplateSpec, TemplateError, TemplatedRouteSpec};
use camel_core::route::RouteDefinition;

use crate::compile::compile_declarative_route;
use crate::json::parse_json_to_declarative;
use crate::model::DeclarativeRoute;
use crate::template::placeholder::substitute_placeholders;

/// Result of compiling a materialized template instance.
pub struct CompiledMaterializationResult {
    /// The compiled route definition.
    pub route_def: RouteDefinition,
    /// Optional hash of the source template body for hot-reload detection.
    pub source_hash: Option<u64>,
}

/// Resolve template parameters by merging declared defaults with provided values.
///
/// - Parameters supplied in `provided` override defaults.
/// - Parameters not supplied but with a `default_value` use the default.
/// - Parameters not supplied and without a default produce [`TemplateError::MissingParameter`].
/// - Parameters supplied but not declared produce [`TemplateError::UnknownParameter`].
pub fn resolve_params(
    template: &RouteTemplateSpec,
    provided: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>, TemplateError> {
    let declared_names: Vec<String> = template.parameters.iter().map(|p| p.name.clone()).collect();
    let declared_set: std::collections::HashSet<&str> =
        declared_names.iter().map(|s| s.as_str()).collect();

    // Validate that all supplied values correspond to declared parameters.
    for key in provided.keys() {
        if !declared_set.contains(key.as_str()) {
            return Err(TemplateError::UnknownParameter(key.clone()));
        }
    }

    // Build resolved map: provided value > default value > error if required and missing.
    let mut resolved = BTreeMap::new();
    for param in &template.parameters {
        if let Some(value) = provided.get(&param.name) {
            resolved.insert(param.name.clone(), value.clone());
        } else if let Some(ref default) = param.default_value {
            resolved.insert(param.name.clone(), default.clone());
        } else {
            return Err(TemplateError::MissingParameter(param.name.clone()));
        }
    }

    Ok(resolved)
}

/// Recursively walk a [`serde_json::Value`] and substitute `{{name}}` placeholders
/// in every string using the resolved parameter values.
pub fn substitute_strings_in_json(
    value: serde_json::Value,
    resolved: &BTreeMap<String, String>,
    declared_params: &[String],
) -> Result<serde_json::Value, TemplateError> {
    substitute_json_value(value, resolved, declared_params)
}

fn substitute_json_value(
    value: serde_json::Value,
    resolved: &BTreeMap<String, String>,
    declared_params: &[String],
) -> Result<serde_json::Value, TemplateError> {
    match value {
        serde_json::Value::String(s) => {
            let substituted = substitute_placeholders(&s, resolved, declared_params)?;
            Ok(serde_json::Value::String(substituted))
        }
        serde_json::Value::Object(map) => {
            let new_map: serde_json::Map<String, serde_json::Value> = map
                .into_iter()
                .map(|(k, v)| {
                    substitute_json_value(v, resolved, declared_params).map(|new_v| (k, new_v))
                })
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .collect();
            Ok(serde_json::Value::Object(new_map))
        }
        serde_json::Value::Array(arr) => {
            let new_arr: Vec<serde_json::Value> = arr
                .into_iter()
                .map(|v| substitute_json_value(v, resolved, declared_params))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(serde_json::Value::Array(new_arr))
        }
        // Numbers, booleans, null — no substitution needed.
        other => Ok(other),
    }
}

/// Compute a hash of the template's route body for source-change detection.
fn compute_source_hash(route: &serde_json::Value) -> u64 {
    let json_str = serde_json::to_string(route).unwrap_or_default();
    let mut hasher = DefaultHasher::new();
    json_str.hash(&mut hasher);
    hasher.finish()
}

/// Main entry point: instantiate a template with concrete parameters and
/// return compiled declarative routes.
///
/// Steps:
/// 1. Resolve parameters (defaults + provided values).
/// 2. Substitute placeholders in the template's JSON body.
/// 3. Wrap the substituted body in `{ "routes": [substituted] }`.
/// 4. Parse via [`parse_json_to_declarative`].
/// 5. Return the resulting [`Vec<DeclarativeRoute>`].
pub fn materialize_template(
    template: &RouteTemplateSpec,
    templated: &TemplatedRouteSpec,
) -> Result<Vec<DeclarativeRoute>, CamelError> {
    // Step 1: resolve parameters.
    let resolved = resolve_params(template, &templated.parameters)
        .map_err(|e| CamelError::Config(e.to_string()))?;

    let declared_names: Vec<String> = template.parameters.iter().map(|p| p.name.clone()).collect();

    // Step 2: substitute placeholders in the template body.
    let substituted =
        substitute_strings_in_json(template.route.clone(), &resolved, &declared_names)
            .map_err(|e| CamelError::Config(e.to_string()))?;

    // Step 3: wrap in `{ "routes": [substituted] }`.
    let wrapped = serde_json::json!({
        "routes": [substituted]
    });

    // Step 4: serialize to string.
    let json_str = serde_json::to_string(&wrapped).map_err(|e| {
        CamelError::RouteError(format!("failed to serialize materialized template: {e}"))
    })?;

    // Step 5: parse to declarative routes.
    let mut routes = parse_json_to_declarative(&json_str)?;

    // Step 6: apply explicit route_id override from the instantiation.
    if let Some(override_id) = &templated.route_id {
        for route in &mut routes {
            route.route_id = override_id.clone();
        }
    }

    Ok(routes)
}

/// Helper used by discovery: materialize a template and compile each resulting
/// declarative route into a [`RouteDefinition`].
///
/// Returns a vector of [`CompiledMaterializationResult`], one per route produced
/// by the template, each carrying the compiled definition and the source hash.
pub fn materialize_and_compile(
    template: &RouteTemplateSpec,
    templated: &TemplatedRouteSpec,
) -> Result<Vec<CompiledMaterializationResult>, CamelError> {
    let source_hash = compute_source_hash(&template.route);
    let declarative_routes = materialize_template(template, templated)?;

    declarative_routes
        .into_iter()
        .map(|route| {
            let route_def = compile_declarative_route(route)?;
            Ok(CompiledMaterializationResult {
                route_def,
                source_hash: Some(source_hash),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_api::template::TemplateParameterSpec;

    // --- resolve_params tests ---

    fn make_template(
        id: &str,
        params: Vec<TemplateParameterSpec>,
        route: serde_json::Value,
    ) -> RouteTemplateSpec {
        RouteTemplateSpec {
            id: id.into(),
            parameters: params,
            route,
        }
    }

    fn make_templated(template_ref: &str, params: BTreeMap<String, String>) -> TemplatedRouteSpec {
        TemplatedRouteSpec {
            route_template_ref: template_ref.into(),
            route_id: None,
            parameters: params,
        }
    }

    #[test]
    fn resolve_params_with_all_provided() {
        let template = make_template(
            "test",
            vec![
                TemplateParameterSpec {
                    name: "host".into(),
                    default_value: None,
                    description: None,
                },
                TemplateParameterSpec {
                    name: "port".into(),
                    default_value: None,
                    description: None,
                },
            ],
            serde_json::json!({}),
        );
        let provided = [
            ("host".into(), "localhost".into()),
            ("port".into(), "8080".into()),
        ]
        .into_iter()
        .collect();
        let resolved = resolve_params(&template, &provided).unwrap();
        assert_eq!(resolved["host"], "localhost");
        assert_eq!(resolved["port"], "8080");
    }

    #[test]
    fn resolve_params_uses_defaults() {
        let template = make_template(
            "test",
            vec![
                TemplateParameterSpec {
                    name: "host".into(),
                    default_value: Some("localhost".into()),
                    description: None,
                },
                TemplateParameterSpec {
                    name: "port".into(),
                    default_value: Some("8080".into()),
                    description: None,
                },
            ],
            serde_json::json!({}),
        );
        let provided = BTreeMap::new();
        let resolved = resolve_params(&template, &provided).unwrap();
        assert_eq!(resolved["host"], "localhost");
        assert_eq!(resolved["port"], "8080");
    }

    #[test]
    fn resolve_params_provided_overrides_default() {
        let template = make_template(
            "test",
            vec![TemplateParameterSpec {
                name: "host".into(),
                default_value: Some("localhost".into()),
                description: None,
            }],
            serde_json::json!({}),
        );
        let provided = [("host".into(), "example.com".into())]
            .into_iter()
            .collect();
        let resolved = resolve_params(&template, &provided).unwrap();
        assert_eq!(resolved["host"], "example.com");
    }

    #[test]
    fn resolve_params_missing_required() {
        let template = make_template(
            "test",
            vec![TemplateParameterSpec {
                name: "host".into(),
                default_value: None,
                description: None,
            }],
            serde_json::json!({}),
        );
        let provided = BTreeMap::new();
        let err = resolve_params(&template, &provided).unwrap_err();
        assert!(matches!(err, TemplateError::MissingParameter(ref n) if n == "host"));
    }

    #[test]
    fn resolve_params_unknown_parameter() {
        let template = make_template(
            "test",
            vec![TemplateParameterSpec {
                name: "host".into(),
                default_value: None,
                description: None,
            }],
            serde_json::json!({}),
        );
        let mut provided = BTreeMap::new();
        provided.insert("host".into(), "localhost".into());
        provided.insert("unknown".into(), "val".into());
        let err = resolve_params(&template, &provided).unwrap_err();
        assert!(matches!(err, TemplateError::UnknownParameter(ref n) if n == "unknown"));
    }

    #[test]
    fn resolve_params_mixed_provided_and_default() {
        let template = make_template(
            "test",
            vec![
                TemplateParameterSpec {
                    name: "host".into(),
                    default_value: Some("localhost".into()),
                    description: None,
                },
                TemplateParameterSpec {
                    name: "port".into(),
                    default_value: None,
                    description: None,
                },
                TemplateParameterSpec {
                    name: "protocol".into(),
                    default_value: Some("http".into()),
                    description: None,
                },
            ],
            serde_json::json!({}),
        );
        let provided = [("port".into(), "9090".into())].into_iter().collect();
        let resolved = resolve_params(&template, &provided).unwrap();
        assert_eq!(resolved["host"], "localhost");
        assert_eq!(resolved["port"], "9090");
        assert_eq!(resolved["protocol"], "http");
    }

    // --- substitute_strings_in_json tests ---

    fn declared(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    fn resolved(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn substitute_in_string_value() {
        let value = serde_json::Value::String("http://{{host}}:{{port}}".into());
        let result = substitute_strings_in_json(
            value,
            &resolved(&[("host", "localhost"), ("port", "8080")]),
            &declared(&["host", "port"]),
        )
        .unwrap();
        assert_eq!(
            result,
            serde_json::Value::String("http://localhost:8080".into())
        );
    }

    #[test]
    fn substitute_in_object_values() {
        let value = serde_json::json!({
            "from": "timer:{{period}}",
            "steps": [{"to": "log:{{level}}"}]
        });
        let result = substitute_strings_in_json(
            value,
            &resolved(&[("period", "5s"), ("level", "info")]),
            &declared(&["period", "level"]),
        )
        .unwrap();
        assert_eq!(result["from"], "timer:5s");
        assert_eq!(result["steps"][0]["to"], "log:info");
    }

    #[test]
    fn substitute_in_array_elements() {
        let value = serde_json::json!(["http://{{host}}/a", "http://{{host}}/b"]);
        let result = substitute_strings_in_json(
            value,
            &resolved(&[("host", "example.com")]),
            &declared(&["host"]),
        )
        .unwrap();
        assert_eq!(result[0], "http://example.com/a");
        assert_eq!(result[1], "http://example.com/b");
    }

    #[test]
    fn non_string_values_unchanged() {
        let value = serde_json::json!({
            "count": 42,
            "enabled": true,
            "nothing": null
        });
        let result = substitute_strings_in_json(value, &BTreeMap::new(), &declared(&[])).unwrap();
        assert_eq!(result["count"], 42);
        assert_eq!(result["enabled"], true);
        assert_eq!(result["nothing"], serde_json::Value::Null);
    }

    #[test]
    fn deeply_nested_substitution() {
        let value = serde_json::json!({
            "outer": {
                "inner": {
                    "uri": "{{endpoint}}"
                }
            }
        });
        let result = substitute_strings_in_json(
            value,
            &resolved(&[("endpoint", "direct:target")]),
            &declared(&["endpoint"]),
        )
        .unwrap();
        assert_eq!(result["outer"]["inner"]["uri"], "direct:target");
    }

    // --- materialize_template tests ---

    #[test]
    fn materialize_simple_template() {
        let template = make_template(
            "http-route",
            vec![TemplateParameterSpec {
                name: "path".into(),
                default_value: None,
                description: None,
            }],
            serde_json::json!({
                "id": "my-http-route",
                "from": "rest:{{path}}",
                "steps": [{"to": "log:info"}]
            }),
        );
        let templated = make_templated(
            "http-route",
            [("path".into(), "/api/users".into())].into_iter().collect(),
        );
        let routes = materialize_template(&template, &templated).unwrap();
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].route_id, "my-http-route");
        assert_eq!(routes[0].from, "rest:/api/users");
    }

    #[test]
    fn materialize_with_default_params() {
        let template = make_template(
            "timer-route",
            vec![TemplateParameterSpec {
                name: "period".into(),
                default_value: Some("1000".into()),
                description: None,
            }],
            serde_json::json!({
                "id": "timer-route",
                "from": "timer:tick?period={{period}}",
                "steps": []
            }),
        );
        let templated = make_templated("timer-route", BTreeMap::new());
        let routes = materialize_template(&template, &templated).unwrap();
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].from, "timer:tick?period=1000");
    }

    #[test]
    fn materialize_missing_required_param() {
        let template = make_template(
            "test",
            vec![TemplateParameterSpec {
                name: "host".into(),
                default_value: None,
                description: None,
            }],
            serde_json::json!({}),
        );
        let templated = make_templated("test", BTreeMap::new());
        let err = materialize_template(&template, &templated).unwrap_err();
        assert!(err.to_string().contains("missing required parameter"));
    }

    #[test]
    fn materialize_unknown_param() {
        let template = make_template(
            "test",
            vec![TemplateParameterSpec {
                name: "host".into(),
                default_value: None,
                description: None,
            }],
            serde_json::json!({}),
        );
        let mut params = BTreeMap::new();
        params.insert("bogus".into(), "val".into());
        let templated = make_templated("test", params);
        let err = materialize_template(&template, &templated).unwrap_err();
        assert!(err.to_string().contains("unknown parameter"));
    }

    // --- materialize_and_compile tests ---

    #[test]
    fn compile_materialized_route() {
        let template = make_template(
            "compile-test",
            vec![TemplateParameterSpec {
                name: "uri".into(),
                default_value: None,
                description: None,
            }],
            serde_json::json!({
                "id": "compiled-route",
                "from": "{{uri}}",
                "steps": [{"to": "log:info"}]
            }),
        );
        let templated = make_templated(
            "compile-test",
            [("uri".into(), "timer:tick?period=500".into())]
                .into_iter()
                .collect(),
        );
        let results = materialize_and_compile(&template, &templated).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].route_def.route_id(), "compiled-route");
        assert_eq!(results[0].route_def.from_uri(), "timer:tick?period=500");
        assert!(results[0].source_hash.is_some());
    }

    #[test]
    fn source_hash_is_deterministic() {
        let template = make_template(
            "hash-test",
            vec![],
            serde_json::json!({
                "id": "hash-route",
                "from": "timer:tick",
                "steps": []
            }),
        );
        let templated = make_templated("hash-test", BTreeMap::new());
        let results1 = materialize_and_compile(&template, &templated).unwrap();
        let results2 = materialize_and_compile(&template, &templated).unwrap();
        assert_eq!(results1[0].source_hash, results2[0].source_hash);
    }

    #[test]
    fn source_hash_differs_for_different_templates() {
        let template_a = make_template(
            "a",
            vec![],
            serde_json::json!({
                "id": "route-a",
                "from": "timer:tick",
                "steps": []
            }),
        );
        let template_b = make_template(
            "b",
            vec![],
            serde_json::json!({
                "id": "route-b",
                "from": "timer:tock",
                "steps": []
            }),
        );
        let templated = make_templated("a", BTreeMap::new());
        let results_a = materialize_and_compile(&template_a, &templated).unwrap();
        let results_b = materialize_and_compile(&template_b, &templated).unwrap();
        assert_ne!(results_a[0].source_hash, results_b[0].source_hash);
    }
}
