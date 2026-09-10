//! YAML template parser — extracts `RouteTemplateSpec` and `TemplatedRouteSpec`
//! from YAML documents that contain `templates` and `templated_routes` sections.

use camel_api::template::{
    RouteTemplateSpec, TemplateError, TemplateParameterSpec, TemplatedRouteSpec,
};
use serde::Deserialize;

use crate::route_ast::{RouteDslTemplate, RouteDslTemplateParameter, RouteDslTemplatedRoute};

// serde_yml migrated to noyalib (compat-serde-yaml shim) — closes RUSTSEC-2025-0068.
// Module alias preserves call-site paths byte-for-byte.
use noyalib::compat::serde_yaml as serde_yml;

/// Template-section view of a route document: `templates` and
/// `templated_routes` only, WITHOUT re-validating the `routes`/`rest`/`mcp`
/// sections. Discovery's route arm gates route validity first, and the
/// typed-probe path (env-int-placeholder-typing) coerces integer
/// placeholder leaves only for the ROUTE parse — the interpolated text
/// handed to template extraction can still carry a string at an
/// integer-typed route position. Mapping-only enforcement mirrors
/// `RouteDslRoutes` (rc-m5ah: positional sequences are rejected); the
/// manual `Deserialize` impl below carries it.
struct TemplateSections {
    templates: Vec<RouteDslTemplate>,
    templated_routes: Vec<RouteDslTemplatedRoute>,
}

/// Field-for-field mirror of [`TemplateSections`] carrying the serde field
/// attributes; the mapping-only enforcement lives in the manual
/// `Deserialize` impl below (same shape as `RouteDslRoutes`).
#[derive(Deserialize)]
struct TemplateSectionsMapping {
    #[serde(default)]
    templates: Vec<RouteDslTemplate>,
    #[serde(default)]
    templated_routes: Vec<RouteDslTemplatedRoute>,
}

// Deliberate pattern-mirror of RouteDslRoutes' mapping-only Deserialize (rc-m5ah) — not ceremony.
impl<'de> Deserialize<'de> for TemplateSections {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        /// Accepts mappings only. Sequences, scalars, and `null` fail with
        /// the visitor's `expecting` message — the same construction
        /// `RouteDslRoutes` uses so both serde front-ends reject the same
        /// document shapes.
        struct TemplateSectionsVisitor;

        impl<'de> serde::de::Visitor<'de> for TemplateSectionsVisitor {
            type Value = TemplateSections;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a route document mapping (YAML mapping)")
            }

            fn visit_map<A>(self, map: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                TemplateSectionsMapping::deserialize(serde::de::value::MapAccessDeserializer::new(
                    map,
                ))
                .map(|m| TemplateSections {
                    templates: m.templates,
                    templated_routes: m.templated_routes,
                })
            }
        }

        deserializer.deserialize_map(TemplateSectionsVisitor)
    }
}

/// Parse the `templates` section of a YAML document into [`RouteTemplateSpec`]s.
///
/// Converts the raw `serde_yml::Value` route body into `serde_json::Value`
/// so it can be processed by the materializer.
///
/// This is the `TemplateSections` view: only the `templates` section is
/// deserialized, so unknown fields in sibling sections (e.g. a typo like
/// `templaes:`) and invalid `routes`/`rest` content are NOT rejected here —
/// discovery's route arm performs the full-document validation (bd rc-28b90,
/// archive review pending).
pub fn parse_yaml_templates(yaml_str: &str) -> Result<Vec<RouteTemplateSpec>, TemplateError> {
    let sections: TemplateSections =
        serde_yml::from_str(yaml_str).map_err(|e| TemplateError::InvalidBody(e.to_string()))?;

    sections
        .templates
        .into_iter()
        .map(yaml_template_to_spec)
        .collect()
}

/// Parse the `templated_routes` section of a YAML document into [`TemplatedRouteSpec`]s.
///
/// This is the `TemplateSections` view: only the `templated_routes` section
/// is deserialized, so unknown fields in sibling sections (e.g. a typo like
/// `templaes:`) and invalid `routes`/`rest` content are NOT rejected here —
/// discovery's route arm performs the full-document validation (bd rc-28b90,
/// archive review pending).
pub fn parse_yaml_templated_routes(
    yaml_str: &str,
) -> Result<Vec<TemplatedRouteSpec>, TemplateError> {
    let sections: TemplateSections =
        serde_yml::from_str(yaml_str).map_err(|e| TemplateError::InvalidBody(e.to_string()))?;

    Ok(sections
        .templated_routes
        .into_iter()
        .map(|yt| TemplatedRouteSpec {
            route_template_ref: yt.route_template_ref,
            route_id: yt.route_id,
            parameters: yt.parameters,
        })
        .collect())
}

fn yaml_template_to_spec(yt: RouteDslTemplate) -> Result<RouteTemplateSpec, TemplateError> {
    if yt.routes.is_empty() {
        return Err(TemplateError::InvalidBody(format!(
            "template '{}': routes array is empty",
            yt.id
        )));
    }

    let parameters: Vec<TemplateParameterSpec> =
        yt.parameters.into_iter().map(yaml_param_to_spec).collect();

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

fn yaml_param_to_spec(yp: RouteDslTemplateParameter) -> TemplateParameterSpec {
    TemplateParameterSpec {
        name: yp.name,
        default_value: yp.default_value,
        description: yp.description,
        parameter_type: yp.parameter_type,
    }
}

use super::conversion::yaml_value_to_json_value;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_yaml_templates_basic() {
        let yaml = r#"
routes: []
templates:
  - id: http-route
    parameters:
      - name: path
        default_value: /api
        description: The REST path
    routes:
      - id: "my-route"
        from: "rest:{{path}}"
        steps:
          - to: "log:info"
"#;
        let specs = parse_yaml_templates(yaml).unwrap();
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
    fn parse_yaml_templates_multiple() {
        let yaml = r#"
routes: []
templates:
  - id: tpl-a
    routes:
      - id: route-a
        from: timer:tick
  - id: tpl-b
    parameters:
      - name: uri
    routes:
      - id: route-b
        from: "{{uri}}"
"#;
        let specs = parse_yaml_templates(yaml).unwrap();
        assert_eq!(specs.len(), 2);
        assert_eq!(specs[0].id, "tpl-a");
        assert_eq!(specs[1].id, "tpl-b");
        assert_eq!(specs[1].parameters.len(), 1);
    }

    #[test]
    fn parse_yaml_templated_routes_basic() {
        let yaml = r#"
routes: []
templated_routes:
  - route_template_ref: http-route
    route_id: my-http-route
    parameters:
      path: /users
  - route_template_ref: timer-route
    parameters:
      period: "5000"
"#;
        let specs = parse_yaml_templated_routes(yaml).unwrap();
        assert_eq!(specs.len(), 2);
        assert_eq!(specs[0].route_template_ref, "http-route");
        assert_eq!(specs[0].route_id.as_deref(), Some("my-http-route"));
        assert_eq!(specs[0].parameters["path"], "/users");
        assert_eq!(specs[1].route_template_ref, "timer-route");
        assert!(specs[1].route_id.is_none());
        assert_eq!(specs[1].parameters["period"], "5000");
    }

    #[test]
    fn parse_yaml_backward_compat_no_templates() {
        let yaml = r#"
routes:
  - id: r1
    from: direct:start
"#;
        let templates = parse_yaml_templates(yaml).unwrap();
        assert!(templates.is_empty());

        let templated = parse_yaml_templated_routes(yaml).unwrap();
        assert!(templated.is_empty());
    }

    #[test]
    fn yaml_value_to_json_conversion() {
        let yaml_val: serde_yml::Value = serde_yml::from_str(
            r#"
id: test-route
from: timer:tick
steps:
  - to: log:info
count: 42
enabled: true
nothing: null
"#,
        )
        .unwrap();

        let json_val = yaml_value_to_json_value(yaml_val).unwrap();
        assert_eq!(json_val["id"], "test-route");
        assert_eq!(json_val["from"], "timer:tick");
        assert_eq!(json_val["steps"][0]["to"], "log:info");
        assert_eq!(json_val["count"], 42);
        assert_eq!(json_val["enabled"], true);
        assert_eq!(json_val["nothing"], serde_json::Value::Null);
    }

    #[test]
    fn parse_yaml_template_with_nested_route_body() {
        let yaml = r#"
routes: []
templates:
  - id: complex-tpl
    parameters:
      - name: host
      - name: port
    routes:
      - id: complex-route
        from: "http:{{host}}:{{port}}"
        steps:
          - choice:
              when:
                - simple: "${header.type} == 'A'"
                  steps:
                    - to: "mock:a"
              otherwise:
                - to: "mock:other"
"#;
        let specs = parse_yaml_templates(yaml).unwrap();
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].id, "complex-tpl");
        let choice = &specs[0].routes[0]["steps"][0]["choice"];
        assert!(choice["when"].is_array());
        assert!(choice["otherwise"].is_array());
    }

    #[test]
    fn parse_yaml_template_with_empty_routes_returns_error() {
        let yaml = r#"
routes: []
templates:
  - id: empty-tpl
    routes: []
"#;
        let err = parse_yaml_templates(yaml).unwrap_err();
        assert!(err.to_string().contains("routes array is empty"));
    }

    #[test]
    fn template_param_type_parses_from_yaml() {
        let yaml = r#"
routes: []
templates:
  - id: delay-tpl
    parameters:
      - name: delay
        type: number
    routes:
      - id: delay-route
        from: timer:tick
"#;
        let specs = parse_yaml_templates(yaml).unwrap();
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].parameters.len(), 1);
        assert_eq!(specs[0].parameters[0].name, "delay");
        assert_eq!(
            specs[0].parameters[0].parameter_type,
            camel_api::template::TemplateParamType::Number
        );
    }

    #[test]
    fn template_param_type_defaults_to_string() {
        let yaml = r#"
routes: []
templates:
  - id: delay-tpl
    parameters:
      - name: delay
    routes:
      - id: delay-route
        from: timer:tick
"#;
        let specs = parse_yaml_templates(yaml).unwrap();
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].parameters.len(), 1);
        assert_eq!(
            specs[0].parameters[0].parameter_type,
            camel_api::template::TemplateParamType::String
        );
    }

    #[test]
    fn template_param_type_unknown_value_fails_closed() {
        // `int` is not a valid TemplateParamType variant — serde rejects
        // the whole document at template parse time (fail-closed).
        let yaml = r#"
routes: []
templates:
  - id: bad-tpl
    parameters:
      - name: delay
        type: int
    routes: []
"#;
        let err = parse_yaml_templates(yaml).unwrap_err();
        assert!(
            err.to_string().to_lowercase().contains("int"),
            "error should name the unknown variant: {err}"
        );
    }
}
