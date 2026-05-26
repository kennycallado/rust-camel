use std::collections::BTreeMap;
use thiserror::Error;
use uuid::Uuid;

/// A single parameter that a route template accepts.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TemplateParameterSpec {
    /// The parameter name (used inside `{{name}}` placeholders).
    pub name: String,
    /// Optional default value used when the caller does not supply one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_value: Option<String>,
    /// Optional human-readable description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// A reusable route template with declared parameters and a route body.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RouteTemplateSpec {
    /// Unique identifier for this template.
    pub id: String,
    /// Parameters that callers must (or may) supply.
    pub parameters: Vec<TemplateParameterSpec>,
    /// The route definition body (YAML/JSON fragment as a generic value).
    pub route: serde_json::Value,
}

/// A request to instantiate a template with concrete parameter values.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TemplatedRouteSpec {
    /// Reference to the template to instantiate.
    pub route_template_ref: String,
    /// Optional explicit route id for the resulting instance.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub route_id: Option<String>,
    /// Concrete parameter values keyed by parameter name.
    pub parameters: BTreeMap<String, String>,
}

/// A successfully instantiated template record.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TemplateInstanceRecord {
    /// The template that was instantiated.
    pub template_id: String,
    /// Unique identifier for this instance.
    pub instance_id: Uuid,
    /// The route id assigned to the resulting route.
    pub route_id: String,
    /// The parameter values that were applied.
    pub parameters: BTreeMap<String, String>,
}

/// Errors that can occur during template processing.
#[derive(Debug, Clone, Error)]
#[non_exhaustive]
pub enum TemplateError {
    #[error("missing required parameter: {0}")]
    MissingParameter(String),

    #[error("unknown parameter: {0}")]
    UnknownParameter(String),

    #[error("template already registered: {0}")]
    AlreadyRegistered(String),

    #[error("invalid template body: {0}")]
    InvalidBody(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_error_converts_to_camel_error() {
        use crate::error::CamelError;
        let err = TemplateError::MissingParameter("host".into());
        let camel: CamelError = err.into();
        assert!(matches!(camel, CamelError::Config(_)));
    }

    #[test]
    fn template_parameter_spec_serializes() {
        let spec = TemplateParameterSpec {
            name: "host".into(),
            default_value: Some("localhost".into()),
            description: Some("The target host".into()),
        };
        let json = serde_json::to_string(&spec).unwrap();
        assert!(json.contains("host"));
        assert!(json.contains("localhost"));
    }

    #[test]
    fn template_parameter_spec_roundtrip() {
        let spec = TemplateParameterSpec {
            name: "port".into(),
            default_value: None,
            description: None,
        };
        let json = serde_json::to_string(&spec).unwrap();
        let round: TemplateParameterSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(round.name, "port");
        assert!(round.default_value.is_none());
        assert!(round.description.is_none());
    }

    #[test]
    fn route_template_spec_serializes() {
        let tpl = RouteTemplateSpec {
            id: "http-route".into(),
            parameters: vec![TemplateParameterSpec {
                name: "path".into(),
                default_value: None,
                description: None,
            }],
            route: serde_json::json!({"from": {"uri": "rest:{{path}}"}}),
        };
        let json = serde_json::to_string(&tpl).unwrap();
        assert!(json.contains("http-route"));
        assert!(json.contains("path"));
    }

    #[test]
    fn templated_route_spec_serializes() {
        let spec = TemplatedRouteSpec {
            route_template_ref: "http-route".into(),
            route_id: Some("my-route".into()),
            parameters: [("path".into(), "/api".into())].into_iter().collect(),
        };
        let json = serde_json::to_string(&spec).unwrap();
        assert!(json.contains("http-route"));
        assert!(json.contains("/api"));
    }

    #[test]
    fn template_instance_record_serializes() {
        let instance = TemplateInstanceRecord {
            template_id: "http-route".into(),
            instance_id: Uuid::nil(),
            route_id: "my-route".into(),
            parameters: [("path".into(), "/api".into())].into_iter().collect(),
        };
        let json = serde_json::to_string(&instance).unwrap();
        assert!(json.contains("http-route"));
        assert!(json.contains("my-route"));
    }

    #[test]
    fn template_error_display_messages() {
        let cases: Vec<(TemplateError, &str)> = vec![
            (
                TemplateError::MissingParameter("x".into()),
                "missing required parameter: x",
            ),
            (
                TemplateError::UnknownParameter("y".into()),
                "unknown parameter: y",
            ),
            (
                TemplateError::AlreadyRegistered("z".into()),
                "template already registered: z",
            ),
            (
                TemplateError::InvalidBody("d".into()),
                "invalid template body: d",
            ),
        ];
        for (err, expected) in cases {
            assert_eq!(format!("{err}"), expected);
        }
    }

    #[test]
    fn template_error_is_clone() {
        let err = TemplateError::MissingParameter("host".into());
        let cloned = err.clone();
        assert!(matches!(cloned, TemplateError::MissingParameter(_)));
    }
}
