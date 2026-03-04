//! YAML route definition parser.

use camel_api::CamelError;
use camel_core::route::{BuilderStep, RouteDefinition};
use serde::Deserialize;

/// Top-level YAML structure.
#[derive(Deserialize)]
pub struct YamlRoutes {
    pub routes: Vec<YamlRoute>,
}

/// A single route in YAML.
#[derive(Deserialize)]
pub struct YamlRoute {
    /// Route ID (mandatory).
    pub id: String,
    /// Source URI.
    pub from: String,
    /// Processing steps.
    #[serde(default)]
    pub steps: Vec<YamlStep>,
    /// Auto-startup (default: true).
    #[serde(default = "default_true")]
    pub auto_startup: bool,
    /// Startup order (default: 1000).
    #[serde(default = "default_startup_order")]
    pub startup_order: i32,
}

fn default_true() -> bool {
    true
}
fn default_startup_order() -> i32 {
    1000
}

/// A step in the YAML pipeline.
///
/// Uses untagged deserialization to support the "one-key map" YAML format:
/// ```yaml
/// - to: "log:info"
/// - set_header:
///     key: "source"
///     value: "timer"
/// - log: "message"
/// ```
#[derive(Deserialize, Debug)]
#[serde(untagged)]
pub enum YamlStep {
    To(ToStep),
    SetHeader(SetHeaderStep),
    Log(LogStep),
}

#[derive(Deserialize, Debug)]
pub struct ToStep {
    pub to: String,
}

#[derive(Deserialize, Debug)]
pub struct SetHeaderStep {
    pub set_header: SetHeaderData,
}

#[derive(Deserialize, Debug)]
pub struct SetHeaderData {
    pub key: String,
    pub value: String,
}

#[derive(Deserialize, Debug)]
pub struct LogStep {
    pub log: String,
}

/// Parse YAML text into route definitions.
pub fn parse_yaml(yaml: &str) -> Result<Vec<RouteDefinition>, CamelError> {
    let routes: YamlRoutes = serde_yaml::from_str(yaml)
        .map_err(|e| CamelError::RouteError(format!("YAML parse error: {e}")))?;

    let mut definitions = Vec::new();
    for route in routes.routes {
        if route.id.is_empty() {
            return Err(CamelError::RouteError(
                "route 'id' must not be empty".into(),
            ));
        }
        let steps = route
            .steps
            .into_iter()
            .map(yaml_step_to_builder_step)
            .collect();
        let def = RouteDefinition::new(&route.from, steps)
            .with_route_id(&route.id)
            .with_auto_startup(route.auto_startup)
            .with_startup_order(route.startup_order);
        definitions.push(def);
    }
    Ok(definitions)
}

fn yaml_step_to_builder_step(step: YamlStep) -> BuilderStep {
    match step {
        YamlStep::To(ToStep { to: uri }) => BuilderStep::To(uri),
        YamlStep::SetHeader(SetHeaderStep {
            set_header: SetHeaderData { key, value },
        }) => {
            use camel_api::{IdentityProcessor, Value};
            use camel_processor::SetHeader;
            BuilderStep::Processor(camel_api::BoxProcessor::new(SetHeader::new(
                IdentityProcessor,
                key,
                Value::String(value),
            )))
        }
        YamlStep::Log(LogStep { log: message }) => {
            use camel_processor::LogLevel;
            BuilderStep::Processor(camel_api::BoxProcessor::new(
                camel_processor::LogProcessor::new(LogLevel::Info, message),
            ))
        }
    }
}

/// Load routes from a YAML file.
pub fn load_from_file(path: &std::path::Path) -> Result<Vec<RouteDefinition>, CamelError> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| CamelError::Io(format!("Failed to read {}: {e}", path.display())))?;
    parse_yaml(&content)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_valid_yaml() {
        let yaml = r#"
routes:
  - id: "test-route"
    from: "timer:tick?period=1000"
    steps:
      - set_header:
          key: "source"
          value: "timer"
      - to: "log:info"
"#;
        let defs = parse_yaml(yaml).unwrap();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].route_id(), "test-route");
        assert_eq!(defs[0].from_uri(), "timer:tick?period=1000");
    }

    #[test]
    fn test_parse_missing_id_fails() {
        let yaml = r#"
routes:
  - from: "timer:tick"
    steps:
      - to: "log:info"
"#;
        let result = parse_yaml(yaml);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_empty_id_fails() {
        let yaml = r#"
routes:
  - id: ""
    from: "timer:tick"
"#;
        let result = parse_yaml(yaml);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_multiple_routes() {
        let yaml = r#"
routes:
  - id: "route-a"
    from: "timer:tick"
    steps:
      - to: "log:info"
  - id: "route-b"
    from: "timer:tock"
    auto_startup: false
    startup_order: 10
"#;
        let defs = parse_yaml(yaml).unwrap();
        assert_eq!(defs.len(), 2);
        assert_eq!(defs[1].route_id(), "route-b");
    }

    #[test]
    fn test_parse_defaults() {
        let yaml = r#"
routes:
  - id: "default-route"
    from: "timer:tick"
"#;
        let defs = parse_yaml(yaml).unwrap();
        assert!(defs[0].auto_startup());
        assert_eq!(defs[0].startup_order(), 1000);
    }

    #[test]
    fn test_load_from_file() {
        use std::io::Write;
        let temp_dir = std::env::temp_dir();
        let file_path = temp_dir.join("test_routes.yaml");

        let yaml_content = r#"
routes:
  - id: "file-route"
    from: "timer:tick"
    steps:
      - to: "log:info"
"#;

        let mut file = std::fs::File::create(&file_path).unwrap();
        file.write_all(yaml_content.as_bytes()).unwrap();

        let defs = load_from_file(&file_path).unwrap();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].route_id(), "file-route");

        // Cleanup
        std::fs::remove_file(&file_path).ok();
    }

    #[test]
    fn test_load_from_nonexistent_file() {
        let result = load_from_file(std::path::Path::new("/nonexistent/path/routes.yaml"));
        assert!(result.is_err());
    }
}
