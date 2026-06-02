//! JSON template parser — extracts `RouteTemplateSpec` and `TemplatedRouteSpec`
//! from JSON documents that contain `templates` and `templated_routes` sections.

use camel_api::template::{
    RouteTemplateSpec, TemplateError, TemplateParameterSpec, TemplatedRouteSpec,
};

use crate::yaml_ast::{YamlRoutes, YamlTemplateParameter};

/// Parse the `templates` section of a JSON document into [`RouteTemplateSpec`]s.
///
/// Since JSON is a subset of YAML's data model, we reuse the same AST types
/// but deserialize with `serde_json`.
pub fn parse_json_templates(json_str: &str) -> Result<Vec<RouteTemplateSpec>, TemplateError> {
    let routes: YamlRoutes =
        serde_json::from_str(json_str).map_err(|e| TemplateError::InvalidBody(e.to_string()))?;

    routes
        .templates
        .into_iter()
        .map(json_template_to_spec)
        .collect()
}

/// Parse the `templated_routes` section of a JSON document into [`TemplatedRouteSpec`]s.
pub fn parse_json_templated_routes(
    json_str: &str,
) -> Result<Vec<TemplatedRouteSpec>, TemplateError> {
    let routes: YamlRoutes =
        serde_json::from_str(json_str).map_err(|e| TemplateError::InvalidBody(e.to_string()))?;

    Ok(routes
        .templated_routes
        .into_iter()
        .map(|yt| TemplatedRouteSpec {
            route_template_ref: yt.route_template_ref,
            route_id: yt.route_id,
            parameters: yt.parameters,
        })
        .collect())
}

fn json_template_to_spec(
    yt: crate::yaml_ast::YamlTemplate,
) -> Result<RouteTemplateSpec, TemplateError> {
    if yt.routes.is_empty() {
        return Err(TemplateError::InvalidBody(format!(
            "template '{}': routes array is empty",
            yt.id
        )));
    }

    let parameters: Vec<TemplateParameterSpec> =
        yt.parameters.into_iter().map(json_param_to_spec).collect();

    let routes: Vec<serde_json::Value> = yt
        .routes
        .into_iter()
        .enumerate()
        .map(|(i, r)| {
            yaml_value_to_json_value(r).map_err(|e| {
                TemplateError::InvalidBody(format!("template '{}': route[{i}]: {e}", yt.id))
            })
        })
        .collect::<Result<Vec<_>, _>>()?;

    Ok(RouteTemplateSpec {
        id: yt.id,
        parameters,
        routes,
    })
}

fn json_param_to_spec(yp: YamlTemplateParameter) -> TemplateParameterSpec {
    TemplateParameterSpec {
        name: yp.name,
        default_value: yp.default_value,
        description: yp.description,
    }
}

use super::conversion::yaml_value_to_json_value;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_json_templates_basic() {
        let json = r#"
{
    "routes": [],
    "templates": [
        {
            "id": "http-route",
            "parameters": [
                {
                    "name": "path",
                    "default_value": "/api",
                    "description": "The REST path"
                }
            ],
            "routes": [
                {
                    "id": "my-route",
                    "from": "rest:{{path}}",
                    "steps": [
                        {"to": "log:info"}
                    ]
                }
            ]
        }
    ]
}"#;
        let specs = parse_json_templates(json).unwrap();
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].id, "http-route");
        assert_eq!(specs[0].parameters.len(), 1);
        assert_eq!(specs[0].parameters[0].name, "path");
        assert_eq!(
            specs[0].parameters[0].default_value.as_deref(),
            Some("/api")
        );
        assert_eq!(specs[0].routes[0]["id"], "my-route");
        assert_eq!(specs[0].routes[0]["from"], "rest:{{path}}");
    }

    #[test]
    fn parse_json_templates_multiple() {
        let json = r#"
{
    "routes": [],
    "templates": [
        {
            "id": "tpl-a",
            "routes": [
                {
                    "id": "route-a",
                    "from": "timer:tick"
                }
            ]
        },
        {
            "id": "tpl-b",
            "parameters": [
                {"name": "uri"}
            ],
            "routes": [
                {
                    "id": "route-b",
                    "from": "{{uri}}"
                }
            ]
        }
    ]
}"#;
        let specs = parse_json_templates(json).unwrap();
        assert_eq!(specs.len(), 2);
        assert_eq!(specs[0].id, "tpl-a");
        assert_eq!(specs[1].id, "tpl-b");
        assert_eq!(specs[1].parameters.len(), 1);
    }

    #[test]
    fn parse_json_templated_routes_basic() {
        let json = r#"
{
    "routes": [],
    "templated_routes": [
        {
            "route_template_ref": "http-route",
            "route_id": "my-http-route",
            "parameters": {
                "path": "/users"
            }
        },
        {
            "route_template_ref": "timer-route",
            "parameters": {
                "period": "5000"
            }
        }
    ]
}"#;
        let specs = parse_json_templated_routes(json).unwrap();
        assert_eq!(specs.len(), 2);
        assert_eq!(specs[0].route_template_ref, "http-route");
        assert_eq!(specs[0].route_id.as_deref(), Some("my-http-route"));
        assert_eq!(specs[0].parameters["path"], "/users");
        assert_eq!(specs[1].route_template_ref, "timer-route");
        assert!(specs[1].route_id.is_none());
        assert_eq!(specs[1].parameters["period"], "5000");
    }

    #[test]
    fn parse_json_backward_compat_no_templates() {
        let json = r#"
{
    "routes": [
        {
            "id": "r1",
            "from": "direct:start"
        }
    ]
}"#;
        let templates = parse_json_templates(json).unwrap();
        assert!(templates.is_empty());

        let templated = parse_json_templated_routes(json).unwrap();
        assert!(templated.is_empty());
    }

    #[test]
    fn parse_json_template_with_nested_route_body() {
        let json = r#"
{
    "routes": [],
    "templates": [
        {
            "id": "complex-tpl",
            "parameters": [
                {"name": "host"},
                {"name": "port"}
            ],
            "routes": [
                {
                    "id": "complex-route",
                    "from": "http:{{host}}:{{port}}",
                    "steps": [
                        {
                            "choice": {
                                "when": [
                                    {
                                        "simple": "${header.type} == 'A'",
                                        "steps": [{"to": "mock:a"}]
                                    }
                                ],
                                "otherwise": [{"to": "mock:other"}]
                            }
                        }
                    ]
                }
            ]
        }
    ]
}"#;
        let specs = parse_json_templates(json).unwrap();
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].id, "complex-tpl");
        let choice = &specs[0].routes[0]["steps"][0]["choice"];
        assert!(choice["when"].is_array());
        assert!(choice["otherwise"].is_array());
    }

    #[test]
    fn parse_json_template_preserves_number_types() {
        let json = r#"
{
    "routes": [],
    "templates": [
        {
            "id": "num-tpl",
            "routes": [
                {
                    "id": "num-route",
                    "from": "timer:tick",
                    "count": 42,
                    "ratio": 3.14,
                    "enabled": true,
                    "nothing": null
                }
            ]
        }
    ]
}"#;
        let specs = parse_json_templates(json).unwrap();
        assert_eq!(specs[0].routes[0]["count"], 42);
        assert_eq!(specs[0].routes[0]["ratio"], 3.14);
        assert_eq!(specs[0].routes[0]["enabled"], true);
        assert_eq!(specs[0].routes[0]["nothing"], serde_json::Value::Null);
    }

    #[test]
    fn parse_json_template_with_empty_routes_returns_error() {
        let json = r#"
{
    "routes": [],
    "templates": [
        {
            "id": "empty-tpl",
            "routes": []
        }
    ]
}"#;
        let err = parse_json_templates(json).unwrap_err();
        assert!(err.to_string().contains("routes array is empty"));
    }
}
