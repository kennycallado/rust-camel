//! Template materializer — resolves parameters, substitutes placeholders in JSON,
//! and compiles the resulting declarative routes.
//!
//! This module bridges the template system (Phase 1) with the DSL compiler,
//! enabling runtime instantiation of parameterized route templates.

use std::collections::BTreeMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use camel_api::CamelError;
use camel_api::template::{
    RouteTemplateSpec, TemplateError, TemplateParamType, TemplateParameterSpec, TemplatedRouteSpec,
};
use camel_core::route::RouteDefinition;

use crate::compile::compile_declarative_route_with_stream_cache_threshold;
use crate::json::parse_json_to_declarative;
use crate::model::{DeclarativeRoute, SecurityCompileContext};
use crate::template::placeholder::{
    param_type_name, parse_typed_bool, parse_typed_number, substitute_placeholders,
    whole_node_placeholder,
};

/// Result of compiling a materialized template instance.
pub struct CompiledMaterializationResult {
    /// The compiled route definition.
    pub route_def: RouteDefinition,
    /// Optional instance-sensitive source hash (template body + resolved
    /// params + effective route id) for hot-reload detection.
    pub source_hash: Option<u64>,
}

/// Resolve template parameters by merging declared defaults with provided values.
///
/// - Parameters supplied in `provided` override defaults.
/// - Parameters not supplied but with a `default_value` use the default.
/// - Parameters not supplied and without a default produce [`TemplateError::MissingParameter`].
/// - Parameters supplied but not declared produce [`TemplateError::UnknownParameter`].
/// - Values for `number`/`boolean` parameters must be coercible; violations
///   produce [`TemplateError::InvalidParameter`] (resolution-time coercion).
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

    // Resolution-time coercion validation: typed parameter values must be
    // coercible to their declared type before substitution runs.
    for param in &template.parameters {
        let value = &resolved[&param.name];
        let coercible = match param.parameter_type {
            TemplateParamType::Number => parse_typed_number(&param.name, value).is_ok(),
            TemplateParamType::Boolean => parse_typed_bool(&param.name, value).is_ok(),
            _ => true,
        };
        if !coercible {
            return Err(TemplateError::InvalidParameter(
                param.name.clone(),
                param_type_name(param.parameter_type).to_string(),
                value.clone(),
            ));
        }
    }

    Ok(resolved)
}

/// Recursively walk a [`serde_json::Value`] and substitute `{{name}}` placeholders
/// in every string using the resolved parameter values.
///
/// When a string node is EXACTLY one placeholder (`"{{p}}"`, whole node) and
/// `p` is declared `number`/`boolean`, the node is emitted as a JSON
/// number/bool instead of a string. Embedded occurrences (`"x{{p}}"`) and
/// all-`string` parameters keep the textual behavior.
pub fn substitute_strings_in_json(
    value: serde_json::Value,
    resolved: &BTreeMap<String, String>,
    specs: &[TemplateParameterSpec],
) -> Result<serde_json::Value, TemplateError> {
    let declared_names: Vec<String> = specs.iter().map(|s| s.name.clone()).collect();
    substitute_json_value(value, resolved, specs, &declared_names)
}

fn substitute_json_value(
    value: serde_json::Value,
    resolved: &BTreeMap<String, String>,
    specs: &[TemplateParameterSpec],
    declared_names: &[String],
) -> Result<serde_json::Value, TemplateError> {
    match value {
        serde_json::Value::String(s) => substitute_string_node(s, resolved, specs, declared_names),
        serde_json::Value::Object(map) => {
            let new_map: serde_json::Map<String, serde_json::Value> = map
                .into_iter()
                .map(|(k, v)| {
                    substitute_json_value(v, resolved, specs, declared_names)
                        .map(|new_v| (k, new_v))
                })
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .collect();
            Ok(serde_json::Value::Object(new_map))
        }
        serde_json::Value::Array(arr) => {
            let new_arr: Vec<serde_json::Value> = arr
                .into_iter()
                .map(|v| substitute_json_value(v, resolved, specs, declared_names))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(serde_json::Value::Array(new_arr))
        }
        // Numbers, booleans, null — no substitution needed.
        other => Ok(other),
    }
}

/// Substitute one string node. Whole-node placeholders for typed
/// (`number`/`boolean`) parameters become JSON numbers/bools; everything
/// else falls through to textual substitution (which keeps unknown- and
/// missing-parameter detection).
fn substitute_string_node(
    s: String,
    resolved: &BTreeMap<String, String>,
    specs: &[TemplateParameterSpec],
    declared_names: &[String],
) -> Result<serde_json::Value, TemplateError> {
    if let Some(name) = whole_node_placeholder(&s) {
        let typed = specs
            .iter()
            .find(|spec| spec.name == name)
            .and_then(|spec| {
                let value = resolved.get(name)?;
                match spec.parameter_type {
                    TemplateParamType::Number => {
                        Some(parse_typed_number(name, value).map(serde_json::Value::Number))
                    }
                    TemplateParamType::Boolean => {
                        Some(parse_typed_bool(name, value).map(serde_json::Value::Bool))
                    }
                    _ => None,
                }
            });
        if let Some(result) = typed {
            return result;
        }
    }
    let substituted = substitute_placeholders(&s, resolved, declared_names)?;
    Ok(serde_json::Value::String(substituted))
}

/// Compute an instance-sensitive source hash covering the raw template body,
/// the resolved parameter map, and the effective route id.
///
/// Two instances of the same template that differ only in `route_id` override
/// or parameter values produce different hashes, so hot-reload detects
/// per-instance changes instead of skipping them.
pub fn compute_instance_source_hash(
    template_routes: &[serde_json::Value],
    resolved_params: &BTreeMap<String, String>,
    effective_route_id: &str,
) -> u64 {
    let mut hasher = DefaultHasher::new();
    let json_str = serde_json::to_string(template_routes).unwrap_or_default();
    json_str.hash(&mut hasher);
    for (key, value) in resolved_params {
        key.hash(&mut hasher);
        value.hash(&mut hasher);
    }
    effective_route_id.hash(&mut hasher);
    hasher.finish()
}

/// Main entry point: instantiate a template with concrete parameters and
/// return the declarative routes plus the resolved parameter map.
///
/// Steps:
/// 1. Resolve parameters (defaults + provided values).
/// 2. Substitute placeholders in the template's JSON body.
/// 3. Wrap the substituted body in `{ "routes": [substituted] }`.
/// 4. Parse via [`parse_json_to_declarative`].
/// 5. Apply the explicit `route_id` override (single-route templates only).
///
/// Returns the resulting [`Vec<DeclarativeRoute>`] and the resolved
/// parameters, so callers can build instance-sensitive source hashes.
pub fn materialize_template(
    template: &RouteTemplateSpec,
    templated: &TemplatedRouteSpec,
) -> Result<(Vec<DeclarativeRoute>, BTreeMap<String, String>), CamelError> {
    if template.routes.is_empty() {
        return Err(CamelError::Config(
            TemplateError::InvalidBody("template has empty routes array".to_string()).to_string(),
        ));
    }

    if templated.route_id.is_some() && template.routes.len() > 1 {
        return Err(CamelError::Config(
            TemplateError::InvalidBody(
                "route_id override is only valid for single-route templates; set per-route ids inside the template body".to_string(),
            )
            .to_string(),
        ));
    }

    // Step 1: resolve parameters.
    let resolved = resolve_params(template, &templated.parameters)
        .map_err(|e| CamelError::Config(e.to_string()))?;

    // Step 2: substitute placeholders in each route body (whole-node typed
    // substitution for number/boolean parameters per the parameter specs).
    let substituted_routes: Vec<serde_json::Value> = template
        .routes
        .iter()
        .map(|r| {
            substitute_strings_in_json(r.clone(), &resolved, &template.parameters)
                .map_err(|e| CamelError::Config(e.to_string()))
        })
        .collect::<Result<Vec<_>, _>>()?;

    // Step 3: wrap in `{ "routes": [substituted...] }`.
    let wrapped = serde_json::json!({
        "routes": substituted_routes
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

    Ok((routes, resolved))
}

/// Helper used by discovery: materialize a template and compile each resulting
/// declarative route into a [`RouteDefinition`].
///
/// Returns a vector of [`CompiledMaterializationResult`], one per route produced
/// by the template, each carrying the compiled definition and the
/// instance-sensitive source hash (template body + resolved params + the
/// route's effective id, post-override).
pub fn materialize_and_compile(
    template: &RouteTemplateSpec,
    templated: &TemplatedRouteSpec,
    stream_cache_threshold: usize,
    security_ctx: SecurityCompileContext,
) -> Result<Vec<CompiledMaterializationResult>, CamelError> {
    let (declarative_routes, resolved) = materialize_template(template, templated)?;

    declarative_routes
        .into_iter()
        .map(|route| {
            let source_hash =
                compute_instance_source_hash(&template.routes, &resolved, &route.route_id);
            let route_def = compile_declarative_route_with_stream_cache_threshold(
                route,
                stream_cache_threshold,
                security_ctx.clone(),
            )?;
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
    use camel_api::template::{TemplateParamType, TemplateParameterSpec};

    // --- resolve_params tests ---

    fn make_template(
        id: &str,
        params: Vec<TemplateParameterSpec>,
        routes: Vec<serde_json::Value>,
    ) -> RouteTemplateSpec {
        RouteTemplateSpec {
            id: id.into(),
            parameters: params,
            routes,
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
                    parameter_type: TemplateParamType::String,
                },
                TemplateParameterSpec {
                    name: "port".into(),
                    default_value: None,
                    description: None,
                    parameter_type: TemplateParamType::String,
                },
            ],
            vec![serde_json::json!({})],
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
                    parameter_type: TemplateParamType::String,
                },
                TemplateParameterSpec {
                    name: "port".into(),
                    default_value: Some("8080".into()),
                    description: None,
                    parameter_type: TemplateParamType::String,
                },
            ],
            vec![serde_json::json!({})],
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
                parameter_type: TemplateParamType::String,
            }],
            vec![serde_json::json!({})],
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
                parameter_type: TemplateParamType::String,
            }],
            vec![serde_json::json!({})],
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
                parameter_type: TemplateParamType::String,
            }],
            vec![serde_json::json!({})],
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
                    parameter_type: TemplateParamType::String,
                },
                TemplateParameterSpec {
                    name: "port".into(),
                    default_value: None,
                    description: None,
                    parameter_type: TemplateParamType::String,
                },
                TemplateParameterSpec {
                    name: "protocol".into(),
                    default_value: Some("http".into()),
                    description: None,
                    parameter_type: TemplateParamType::String,
                },
            ],
            vec![serde_json::json!({})],
        );
        let provided = [("port".into(), "9090".into())].into_iter().collect();
        let resolved = resolve_params(&template, &provided).unwrap();
        assert_eq!(resolved["host"], "localhost");
        assert_eq!(resolved["port"], "9090");
        assert_eq!(resolved["protocol"], "http");
    }

    // --- substitute_strings_in_json tests ---

    fn specs(pairs: &[(&str, TemplateParamType)]) -> Vec<TemplateParameterSpec> {
        pairs
            .iter()
            .map(|(name, ty)| TemplateParameterSpec {
                name: (*name).into(),
                default_value: None,
                description: None,
                parameter_type: *ty,
            })
            .collect()
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
            &specs(&[
                ("host", TemplateParamType::String),
                ("port", TemplateParamType::String),
            ]),
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
            &specs(&[
                ("period", TemplateParamType::String),
                ("level", TemplateParamType::String),
            ]),
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
            &specs(&[("host", TemplateParamType::String)]),
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
        let result = substitute_strings_in_json(value, &BTreeMap::new(), &specs(&[])).unwrap();
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
            &specs(&[("endpoint", TemplateParamType::String)]),
        )
        .unwrap();
        assert_eq!(result["outer"]["inner"]["uri"], "direct:target");
    }

    // --- whole-node typed substitution tests ---

    #[test]
    fn whole_node_number_param_becomes_json_number() {
        let value = serde_json::Value::String("{{delay}}".into());
        let result = substitute_strings_in_json(
            value,
            &resolved(&[("delay", "5000")]),
            &specs(&[("delay", TemplateParamType::Number)]),
        )
        .unwrap();
        assert_eq!(result, serde_json::json!(5000));
        assert!(result.is_number());
    }

    #[test]
    fn whole_node_negative_and_float_number_params() {
        let result = substitute_strings_in_json(
            serde_json::Value::String("{{n}}".into()),
            &resolved(&[("n", "-3")]),
            &specs(&[("n", TemplateParamType::Number)]),
        )
        .unwrap();
        assert_eq!(result, serde_json::json!(-3));
        let result = substitute_strings_in_json(
            serde_json::Value::String("{{n}}".into()),
            &resolved(&[("n", "2.5")]),
            &specs(&[("n", TemplateParamType::Number)]),
        )
        .unwrap();
        assert_eq!(result, serde_json::json!(2.5));
    }

    #[test]
    fn whole_node_boolean_param_becomes_json_bool() {
        let result = substitute_strings_in_json(
            serde_json::Value::String("{{flag}}".into()),
            &resolved(&[("flag", "true")]),
            &specs(&[("flag", TemplateParamType::Boolean)]),
        )
        .unwrap();
        assert_eq!(result, serde_json::json!(true));
        let result = substitute_strings_in_json(
            serde_json::Value::String("{{flag}}".into()),
            &resolved(&[("flag", "false")]),
            &specs(&[("flag", TemplateParamType::Boolean)]),
        )
        .unwrap();
        assert_eq!(result, serde_json::json!(false));
    }

    #[test]
    fn embedded_typed_occurrence_stays_textual() {
        let value = serde_json::json!({"id": "x{{p}}"});
        let result = substitute_strings_in_json(
            value,
            &resolved(&[("p", "7")]),
            &specs(&[("p", TemplateParamType::Number)]),
        )
        .unwrap();
        assert_eq!(result["id"], serde_json::json!("x7"));
    }

    #[test]
    fn string_param_whole_node_stays_string() {
        let value = serde_json::Value::String("{{p}}".into());
        let result = substitute_strings_in_json(
            value,
            &resolved(&[("p", "7")]),
            &specs(&[("p", TemplateParamType::String)]),
        )
        .unwrap();
        assert_eq!(result, serde_json::Value::String("7".into()));
    }

    #[test]
    fn walker_rejects_non_coercible_whole_node_number() {
        let value = serde_json::Value::String("{{p}}".into());
        let err = substitute_strings_in_json(
            value,
            &resolved(&[("p", "abc")]),
            &specs(&[("p", TemplateParamType::Number)]),
        )
        .unwrap_err();
        assert!(
            matches!(err, TemplateError::InvalidParameter(ref n, ref ty, ref v) if n == "p" && ty == "number" && v == "abc")
        );
    }

    #[test]
    fn walker_keeps_unknown_placeholder_detection() {
        let value = serde_json::Value::String("{{typo}}".into());
        let err = substitute_strings_in_json(
            value,
            &resolved(&[("p", "1")]),
            &specs(&[("p", TemplateParamType::Number)]),
        )
        .unwrap_err();
        assert!(matches!(err, TemplateError::UnknownParameter(ref n) if n == "typo"));
    }

    // --- resolve_params typed coercion tests ---

    fn typed_param_template(
        name: &str,
        ty: TemplateParamType,
        value: &str,
    ) -> (RouteTemplateSpec, TemplatedRouteSpec) {
        let template = make_template(
            "typed",
            vec![TemplateParameterSpec {
                name: name.into(),
                default_value: None,
                description: None,
                parameter_type: ty,
            }],
            vec![serde_json::json!({})],
        );
        let templated = make_templated(
            "typed",
            [(name.to_string(), value.to_string())]
                .into_iter()
                .collect(),
        );
        (template, templated)
    }

    #[test]
    fn resolve_params_rejects_non_coercible_number() {
        let (template, templated) = typed_param_template("delay", TemplateParamType::Number, "abc");
        let err = resolve_params(&template, &templated.parameters).unwrap_err();
        assert!(
            matches!(err, TemplateError::InvalidParameter(ref n, ref ty, ref v) if n == "delay" && ty == "number" && v == "abc")
        );
    }

    #[test]
    fn resolve_params_accepts_coercible_number() {
        let (template, templated) =
            typed_param_template("delay", TemplateParamType::Number, "5000.5");
        let resolved = resolve_params(&template, &templated.parameters).unwrap();
        assert_eq!(resolved["delay"], "5000.5");
    }

    #[test]
    fn resolve_params_rejects_non_boolean_value() {
        let (template, templated) = typed_param_template("flag", TemplateParamType::Boolean, "yes");
        let err = resolve_params(&template, &templated.parameters).unwrap_err();
        assert!(
            matches!(err, TemplateError::InvalidParameter(ref n, ref ty, ref v) if n == "flag" && ty == "boolean" && v == "yes")
        );
    }

    #[test]
    fn resolve_params_coerces_default_number_value() {
        let template = make_template(
            "typed",
            vec![TemplateParameterSpec {
                name: "delay".into(),
                default_value: Some("5000".into()),
                description: None,
                parameter_type: TemplateParamType::Number,
            }],
            vec![serde_json::json!({})],
        );
        let templated = make_templated("typed", BTreeMap::new());
        let resolved = resolve_params(&template, &templated.parameters).unwrap();
        assert_eq!(resolved["delay"], "5000");
    }

    #[test]
    fn resolve_params_rejects_non_coercible_default_number() {
        let template = make_template(
            "typed",
            vec![TemplateParameterSpec {
                name: "delay".into(),
                default_value: Some("abc".into()),
                description: None,
                parameter_type: TemplateParamType::Number,
            }],
            vec![serde_json::json!({})],
        );
        let templated = make_templated("typed", BTreeMap::new());
        let err = resolve_params(&template, &templated.parameters).unwrap_err();
        assert!(
            matches!(err, TemplateError::InvalidParameter(ref n, ref ty, ref v) if n == "delay" && ty == "number" && v == "abc")
        );
    }

    #[test]
    fn materialize_non_coercible_number_param_reports_type() {
        let (template, templated) = typed_param_template("delay", TemplateParamType::Number, "abc");
        let err = materialize_template(&template, &templated).unwrap_err();
        assert!(
            err.to_string()
                .contains("parameter 'delay' declared type number"),
            "unexpected error: {err}"
        );
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
                parameter_type: TemplateParamType::String,
            }],
            vec![serde_json::json!({
                "id": "my-http-route",
                "from": "rest:{{path}}",
                "steps": [{"to": "log:info"}]
            })],
        );
        let templated = make_templated(
            "http-route",
            [("path".into(), "/api/users".into())].into_iter().collect(),
        );
        let (routes, _) = materialize_template(&template, &templated).unwrap();
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
                parameter_type: TemplateParamType::String,
            }],
            vec![serde_json::json!({
                "id": "timer-route",
                "from": "timer:tick?period={{period}}",
                "steps": []
            })],
        );
        let templated = make_templated("timer-route", BTreeMap::new());
        let (routes, _) = materialize_template(&template, &templated).unwrap();
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
                parameter_type: TemplateParamType::String,
            }],
            vec![serde_json::json!({})],
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
                parameter_type: TemplateParamType::String,
            }],
            vec![serde_json::json!({})],
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
                parameter_type: TemplateParamType::String,
            }],
            vec![serde_json::json!({
                "id": "compiled-route",
                "from": "{{uri}}",
                "steps": [{"to": "log:info"}]
            })],
        );
        let templated = make_templated(
            "compile-test",
            [("uri".into(), "timer:tick?period=500".into())]
                .into_iter()
                .collect(),
        );
        let results = materialize_and_compile(
            &template,
            &templated,
            camel_api::stream_cache::DEFAULT_STREAM_CACHE_THRESHOLD,
            SecurityCompileContext::default(),
        )
        .unwrap();
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
            vec![serde_json::json!({
                "id": "hash-route",
                "from": "timer:tick",
                "steps": []
            })],
        );
        let templated = make_templated("hash-test", BTreeMap::new());
        let results1 = materialize_and_compile(
            &template,
            &templated,
            camel_api::stream_cache::DEFAULT_STREAM_CACHE_THRESHOLD,
            SecurityCompileContext::default(),
        )
        .unwrap();
        let results2 = materialize_and_compile(
            &template,
            &templated,
            camel_api::stream_cache::DEFAULT_STREAM_CACHE_THRESHOLD,
            SecurityCompileContext::default(),
        )
        .unwrap();
        assert_eq!(results1[0].source_hash, results2[0].source_hash);
    }

    #[test]
    fn source_hash_differs_for_different_templates() {
        let template_a = make_template(
            "a",
            vec![],
            vec![serde_json::json!({
                "id": "route-a",
                "from": "timer:tick",
                "steps": []
            })],
        );
        let template_b = make_template(
            "b",
            vec![],
            vec![serde_json::json!({
                "id": "route-b",
                "from": "timer:tock",
                "steps": []
            })],
        );
        let templated = make_templated("a", BTreeMap::new());
        let results_a = materialize_and_compile(
            &template_a,
            &templated,
            camel_api::stream_cache::DEFAULT_STREAM_CACHE_THRESHOLD,
            SecurityCompileContext::default(),
        )
        .unwrap();
        let results_b = materialize_and_compile(
            &template_b,
            &templated,
            camel_api::stream_cache::DEFAULT_STREAM_CACHE_THRESHOLD,
            SecurityCompileContext::default(),
        )
        .unwrap();
        assert_ne!(results_a[0].source_hash, results_b[0].source_hash);
    }

    #[test]
    fn materialize_multi_route_template() {
        let template = make_template(
            "chain",
            vec![TemplateParameterSpec {
                name: "PROV".into(),
                default_value: None,
                description: None,
                parameter_type: TemplateParamType::String,
            }],
            vec![
                serde_json::json!({
                    "id": "step1-{{PROV}}",
                    "from": "direct:start",
                    "steps": [{"to": "controlbus:route?routeId=step2-{{PROV}}&action=start"}]
                }),
                serde_json::json!({
                    "id": "step2-{{PROV}}",
                    "from": "direct:step2",
                    "steps": [{"to": "log:done"}]
                }),
            ],
        );
        let templated = make_templated(
            "chain",
            [("PROV".into(), "granada".into())].into_iter().collect(),
        );
        let (routes, _) = materialize_template(&template, &templated).unwrap();
        assert_eq!(routes.len(), 2);
        assert_eq!(routes[0].route_id, "step1-granada");
        assert_eq!(routes[1].route_id, "step2-granada");
    }

    #[test]
    fn materialize_multi_route_rejects_route_id_override() {
        let template = make_template(
            "chain",
            vec![],
            vec![
                serde_json::json!({"id": "r1", "from": "direct:a", "steps": []}),
                serde_json::json!({"id": "r2", "from": "direct:b", "steps": []}),
            ],
        );
        let templated = TemplatedRouteSpec {
            route_template_ref: "chain".into(),
            route_id: Some("override-id".into()),
            parameters: BTreeMap::new(),
        };
        let err = materialize_template(&template, &templated).unwrap_err();
        assert!(
            err.to_string()
                .contains("route_id override is only valid for single-route templates")
        );
    }

    #[test]
    fn materialize_empty_routes_returns_error() {
        let template = make_template("empty", vec![], vec![]);
        let templated = make_templated("empty", BTreeMap::new());
        let err = materialize_template(&template, &templated).unwrap_err();
        assert!(err.to_string().contains("empty routes array"));
    }

    #[test]
    fn templated_route_receives_configured_threshold() {
        // Template whose route contains a `stream_cache` step (no threshold
        // declared in the step). The threshold flows into step compilation via
        // `stream_cache_config`, which is not observable on `RouteDefinition`;
        // parity is asserted at the only observable depth: both the materialized
        // and the equivalent direct route compile Ok with the same structure.
        let template = make_template(
            "cache-tpl",
            vec![],
            vec![serde_json::json!({
                "id": "cache-route",
                "from": "timer:tick",
                "steps": [
                    {"stream_cache": true},
                    {"to": "log:info"}
                ]
            })],
        );
        let templated = make_templated("cache-tpl", BTreeMap::new());

        let results =
            materialize_and_compile(&template, &templated, 7, SecurityCompileContext::default())
                .unwrap();
        assert_eq!(results.len(), 1);
        let materialized = &results[0].route_def;
        assert_eq!(materialized.route_id(), "cache-route");
        assert_eq!(materialized.steps().len(), 2);

        // Equivalent direct route parsed from the same route JSON, threshold 7.
        let direct_json = serde_json::json!({
            "routes": [{
                "id": "cache-route",
                "from": "timer:tick",
                "steps": [
                    {"stream_cache": true},
                    {"to": "log:info"}
                ]
            }]
        });
        let direct_routes =
            crate::json::parse_json_to_declarative(&direct_json.to_string()).unwrap();
        assert_eq!(direct_routes.len(), 1);
        let direct = crate::compile::compile_declarative_route_with_stream_cache_threshold(
            direct_routes.into_iter().next().unwrap(),
            7,
            SecurityCompileContext::default(),
        )
        .unwrap();
        assert_eq!(direct.route_id(), "cache-route");
        assert_eq!(direct.steps().len(), 2);
    }

    #[test]
    fn override_only_instances_hash_distinctly() {
        let body = vec![serde_json::json!({
            "id": "tpl-route",
            "from": "rest:{{host}}",
            "steps": [{"to": "log:info"}]
        })];
        let resolved = resolved(&[("host", "h")]);
        let hash_a = compute_instance_source_hash(&body, &resolved, "a");
        let hash_b = compute_instance_source_hash(&body, &resolved, "b");
        let hash_c = compute_instance_source_hash(&body, &resolved, "c");
        assert_ne!(hash_a, hash_b);
        assert_ne!(hash_a, hash_c);
        assert_ne!(hash_b, hash_c);
    }

    #[test]
    fn param_value_changes_hash() {
        let body = vec![serde_json::json!({
            "id": "r",
            "from": "timer:tick?period={{delay}}",
            "steps": []
        })];
        let hash_one = compute_instance_source_hash(&body, &resolved(&[("delay", "1")]), "r");
        let hash_two = compute_instance_source_hash(&body, &resolved(&[("delay", "2")]), "r");
        assert_ne!(hash_one, hash_two);
    }

    #[test]
    fn source_hash_covers_multi_route_array() {
        let template = make_template(
            "multi-hash",
            vec![],
            vec![
                serde_json::json!({"id": "r1", "from": "direct:a", "steps": []}),
                serde_json::json!({"id": "r2", "from": "direct:b", "steps": []}),
            ],
        );
        let templated = make_templated("multi-hash", BTreeMap::new());
        let results = materialize_and_compile(
            &template,
            &templated,
            camel_api::stream_cache::DEFAULT_STREAM_CACHE_THRESHOLD,
            SecurityCompileContext::default(),
        )
        .unwrap();
        assert_eq!(results.len(), 2);
        let resolved = BTreeMap::new();
        let hash_r1 = compute_instance_source_hash(&template.routes, &resolved, "r1");
        let hash_r2 = compute_instance_source_hash(&template.routes, &resolved, "r2");
        assert_eq!(results[0].source_hash, Some(hash_r1));
        assert_eq!(results[1].source_hash, Some(hash_r2));
        assert_ne!(hash_r1, hash_r2);
    }
}
