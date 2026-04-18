use std::path::{Path, PathBuf};

use camel_component_api::CamelError;

#[derive(Debug, Clone, PartialEq)]
pub enum SchemaType {
    Xml,
    Json,
    Yaml,
}

#[derive(Debug, Clone)]
pub struct ValidatorConfig {
    pub schema_path: PathBuf,
    pub schema_type: SchemaType,
}

impl ValidatorConfig {
    pub fn from_uri(uri: &str) -> Result<Self, CamelError> {
        let without_scheme = uri.strip_prefix("validator:").ok_or_else(|| {
            CamelError::EndpointCreationFailed(format!(
                "invalid validator URI: must start with 'validator:' — got '{uri}'"
            ))
        })?;

        let (path_str, query) = match without_scheme.find('?') {
            Some(idx) => (&without_scheme[..idx], Some(&without_scheme[idx + 1..])),
            None => (without_scheme, None),
        };

        if path_str.is_empty() {
            return Err(CamelError::EndpointCreationFailed(
                "validator URI must specify a schema path".to_string(),
            ));
        }

        let schema_path = PathBuf::from(path_str);

        let schema_type = if let Some(q) = query {
            let type_val = q.split('&').find_map(|kv| kv.strip_prefix("type="));
            match type_val {
                Some("xml") | Some("xml-schema") | Some("xsd") => SchemaType::Xml,
                Some("json") | Some("json-schema") => SchemaType::Json,
                Some("yaml") | Some("yaml-schema") => SchemaType::Yaml,
                Some(other) => {
                    return Err(CamelError::EndpointCreationFailed(format!(
                        "unknown schema type '{other}'; expected xml, json, or yaml"
                    )));
                }
                None => detect_type_from_extension(&schema_path)?,
            }
        } else {
            detect_type_from_extension(&schema_path)?
        };

        Ok(ValidatorConfig {
            schema_path,
            schema_type,
        })
    }
}

fn detect_type_from_extension(path: &Path) -> Result<SchemaType, CamelError> {
    match path.extension().and_then(|e| e.to_str()) {
        Some("xsd") => Ok(SchemaType::Xml),
        Some("json") => Ok(SchemaType::Json),
        Some("yaml") | Some("yml") => Ok(SchemaType::Yaml),
        ext => Err(CamelError::EndpointCreationFailed(format!(
            "cannot infer schema type from extension {ext:?}; use ?type=xml|json|yaml"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_xml_from_xsd_extension() {
        let cfg = ValidatorConfig::from_uri("validator:schemas/order.xsd").unwrap();
        assert_eq!(cfg.schema_path, PathBuf::from("schemas/order.xsd"));
        assert_eq!(cfg.schema_type, SchemaType::Xml);
    }

    #[test]
    fn detects_json_from_json_extension() {
        let cfg = ValidatorConfig::from_uri("validator:schemas/order.json").unwrap();
        assert_eq!(cfg.schema_type, SchemaType::Json);
    }

    #[test]
    fn detects_yaml_from_yaml_extension() {
        let cfg = ValidatorConfig::from_uri("validator:schemas/order.yaml").unwrap();
        assert_eq!(cfg.schema_type, SchemaType::Yaml);
    }

    #[test]
    fn detects_yaml_from_yml_extension() {
        let cfg = ValidatorConfig::from_uri("validator:schemas/order.yml").unwrap();
        assert_eq!(cfg.schema_type, SchemaType::Yaml);
    }

    #[test]
    fn type_param_overrides_extension() {
        let cfg = ValidatorConfig::from_uri("validator:schemas/order.xsd?type=json").unwrap();
        assert_eq!(cfg.schema_type, SchemaType::Json);
    }

    #[test]
    fn wrong_scheme_errors() {
        assert!(ValidatorConfig::from_uri("timer:tick").is_err());
    }

    #[test]
    fn empty_path_errors() {
        assert!(ValidatorConfig::from_uri("validator:").is_err());
    }

    #[test]
    fn unknown_type_param_errors() {
        assert!(ValidatorConfig::from_uri("validator:schema.xsd?type=csv").is_err());
    }

    #[test]
    fn no_extension_no_type_param_errors() {
        assert!(ValidatorConfig::from_uri("validator:schema").is_err());
    }
}
