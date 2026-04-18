use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex;

use camel_component_api::{Body, CamelError};
use libxml::parser::Parser as XmlParser;
use libxml::schemas::{SchemaParserContext, SchemaValidationContext};
use serde_yml::Value as YamlValue;

use crate::config::{SchemaType, ValidatorConfig};

pub(crate) struct SendSchemaValidationContext(SchemaValidationContext);

// SAFETY: SchemaValidationContext wraps libxml2's xmlSchemaValidCtxt which is
// NOT thread-safe (requires &mut self for validate_document). However, it is
// safe to move between threads because it owns its heap allocation and has no
// thread-affinity (no thread-local state, no TLS, no OS handles tied to a thread).
// Concurrent access is prevented by wrapping in Mutex<Arc<...>>, so only one
// thread can call validate_document at a time. This matches the pattern used by
// other Rust XML libraries wrapping libxml2 (e.g., the libxml crate's own tests).
unsafe impl Send for SendSchemaValidationContext {}

pub(crate) enum CompiledValidator {
    Xml(Arc<Mutex<SendSchemaValidationContext>>),
    Json(Arc<jsonschema::Validator>),
    Yaml(Arc<jsonschema::Validator>),
}

impl CompiledValidator {
    pub fn compile(config: &ValidatorConfig) -> Result<Self, CamelError> {
        let path = &config.schema_path;

        let content = std::fs::read(path).map_err(|e| {
            CamelError::EndpointCreationFailed(format!(
                "failed to read schema file '{}': {e}",
                path.display()
            ))
        })?;

        match config.schema_type {
            SchemaType::Xml => Self::compile_xsd(path, &content),
            SchemaType::Json => Self::compile_json(&content, path),
            SchemaType::Yaml => Self::compile_yaml_schema(&content, path),
        }
    }

    fn compile_xsd(path: &Path, content: &[u8]) -> Result<Self, CamelError> {
        let mut parser_ctx = SchemaParserContext::from_buffer(content);
        let ctx = SchemaValidationContext::from_parser(&mut parser_ctx).map_err(|errors| {
            let msgs: Vec<String> = errors
                .iter()
                .map(|e| e.message.as_deref().unwrap_or("").to_string())
                .collect();
            CamelError::EndpointCreationFailed(format!(
                "invalid XSD schema '{}': {}",
                path.display(),
                msgs.join("; ")
            ))
        })?;

        Ok(CompiledValidator::Xml(Arc::new(Mutex::new(
            SendSchemaValidationContext(ctx),
        ))))
    }

    fn compile_json(content: &[u8], path: &Path) -> Result<Self, CamelError> {
        let schema_value: serde_json::Value = serde_json::from_slice(content).map_err(|e| {
            CamelError::EndpointCreationFailed(format!(
                "invalid JSON schema '{}': {e}",
                path.display()
            ))
        })?;

        let validator = jsonschema::validator_for(&schema_value).map_err(|e| {
            CamelError::EndpointCreationFailed(format!(
                "failed to compile JSON schema '{}': {e}",
                path.display()
            ))
        })?;

        Ok(CompiledValidator::Json(Arc::new(validator)))
    }

    fn compile_yaml_schema(content: &[u8], path: &Path) -> Result<Self, CamelError> {
        let yaml_str = std::str::from_utf8(content).map_err(|e| {
            CamelError::EndpointCreationFailed(format!(
                "YAML schema '{}' is not valid UTF-8: {e}",
                path.display()
            ))
        })?;

        let yaml_value: YamlValue = serde_yml::from_str(yaml_str).map_err(|e| {
            CamelError::EndpointCreationFailed(format!(
                "invalid YAML schema '{}': {e}",
                path.display()
            ))
        })?;

        let schema_value: serde_json::Value = serde_json::to_value(&yaml_value).map_err(|e| {
            CamelError::EndpointCreationFailed(format!(
                "failed to convert YAML schema to JSON '{}': {e}",
                path.display()
            ))
        })?;

        let validator = jsonschema::validator_for(&schema_value).map_err(|e| {
            CamelError::EndpointCreationFailed(format!(
                "failed to compile YAML schema '{}': {e}",
                path.display()
            ))
        })?;

        Ok(CompiledValidator::Yaml(Arc::new(validator)))
    }

    pub fn validate(&self, body: &Body) -> Result<(), CamelError> {
        match self {
            CompiledValidator::Xml(ctx) => Self::validate_xml(ctx, body),
            CompiledValidator::Json(validator) => Self::validate_json(validator, body),
            CompiledValidator::Yaml(validator) => Self::validate_yaml(validator, body),
        }
    }

    fn validate_xml(
        ctx: &Arc<Mutex<SendSchemaValidationContext>>,
        body: &Body,
    ) -> Result<(), CamelError> {
        let xml_str: std::borrow::Cow<str> = match body {
            Body::Xml(s) => std::borrow::Cow::Borrowed(s.as_str()),
            Body::Text(s) => std::borrow::Cow::Borrowed(s.as_str()),
            Body::Bytes(b) => {
                std::borrow::Cow::Owned(String::from_utf8(b.to_vec()).map_err(|e| {
                    CamelError::ProcessorError(format!("XSD validator: invalid UTF-8 in body: {e}"))
                })?)
            }
            _ => {
                return Err(CamelError::ProcessorError(
                    "XSD validator requires Body::Xml, Body::Text, or Body::Bytes".to_string(),
                ));
            }
        };

        let parser = XmlParser::default();
        let doc = parser.parse_string(xml_str.as_bytes()).map_err(|e| {
            CamelError::ProcessorError(format!("XML parse error during validation: {e}"))
        })?;

        let mut guard = ctx
            .lock()
            .map_err(|e| CamelError::ProcessorError(format!("XSD validator lock poisoned: {e}")))?;
        let result = guard.0.validate_document(&doc);
        match result {
            Ok(()) => Ok(()),
            Err(errors) => {
                let messages: Vec<String> = errors
                    .iter()
                    .map(|e| e.message.as_deref().unwrap_or("").to_string())
                    .collect();
                Err(CamelError::ProcessorError(format!(
                    "XSD validation failed:\n{}",
                    messages.join("\n")
                )))
            }
        }
    }

    fn validate_json(validator: &jsonschema::Validator, body: &Body) -> Result<(), CamelError> {
        let json_value = match body {
            Body::Json(v) => v.clone(),
            Body::Text(s) => serde_json::from_str(s)
                .map_err(|e| CamelError::ProcessorError(format!("body is not valid JSON: {e}")))?,
            Body::Bytes(b) => serde_json::from_slice(b).map_err(|e| {
                CamelError::ProcessorError(format!("body bytes are not valid JSON: {e}"))
            })?,
            _ => {
                return Err(CamelError::ProcessorError(
                    "JSON Schema validator requires Body::Json, Body::Text, or Body::Bytes"
                        .to_string(),
                ));
            }
        };

        let messages: Vec<String> = validator
            .iter_errors(&json_value)
            .map(|e| format!("{e} at {}", e.instance_path()))
            .collect();

        if messages.is_empty() {
            Ok(())
        } else {
            Err(CamelError::ProcessorError(format!(
                "JSON Schema validation failed:\n{}",
                messages.join("\n")
            )))
        }
    }

    fn validate_yaml(validator: &jsonschema::Validator, body: &Body) -> Result<(), CamelError> {
        let yaml_str = match body {
            Body::Text(s) => s.as_str(),
            _ => {
                return Err(CamelError::ProcessorError(
                    "YAML validator requires a text body (Body::Text)".to_string(),
                ));
            }
        };

        let yaml_value: serde_yml::Value = serde_yml::from_str(yaml_str)
            .map_err(|e| CamelError::ProcessorError(format!("body is not valid YAML: {e}")))?;

        let json_value: serde_json::Value = serde_json::to_value(&yaml_value)
            .map_err(|e| CamelError::ProcessorError(format!("YAML→JSON conversion failed: {e}")))?;

        let messages: Vec<String> = validator
            .iter_errors(&json_value)
            .map(|e| format!("{e} at {}", e.instance_path()))
            .collect();

        if messages.is_empty() {
            Ok(())
        } else {
            Err(CamelError::ProcessorError(format!(
                "YAML Schema validation failed:\n{}",
                messages.join("\n")
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{SchemaType, ValidatorConfig};
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn write_temp(content: &str, suffix: &str) -> NamedTempFile {
        let mut f = tempfile::Builder::new().suffix(suffix).tempfile().unwrap();
        f.write_all(content.as_bytes()).unwrap();
        f
    }

    #[test]
    fn compiled_validator_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<CompiledValidator>();
    }

    #[test]
    fn xsd_compile_missing_file_errors() {
        let config = ValidatorConfig {
            schema_path: "/nonexistent/schema.xsd".into(),
            schema_type: SchemaType::Xml,
        };
        assert!(CompiledValidator::compile(&config).is_err());
    }

    #[test]
    fn xsd_valid_xml_passes() {
        let xsd = r#"<?xml version="1.0"?>
<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema">
  <xs:element name="order">
    <xs:complexType>
      <xs:sequence>
        <xs:element name="id" type="xs:string"/>
      </xs:sequence>
    </xs:complexType>
  </xs:element>
</xs:schema>"#;
        let f = write_temp(xsd, ".xsd");
        let config = ValidatorConfig {
            schema_path: f.path().to_path_buf(),
            schema_type: SchemaType::Xml,
        };
        let compiled = CompiledValidator::compile(&config).unwrap();
        let body = Body::Xml("<order><id>123</id></order>".to_string());
        assert!(compiled.validate(&body).is_ok());
    }

    #[test]
    fn xsd_invalid_xml_returns_all_errors() {
        let xsd = r#"<?xml version="1.0"?>
<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema">
  <xs:element name="order">
    <xs:complexType>
      <xs:sequence>
        <xs:element name="id" type="xs:string"/>
        <xs:element name="amount" type="xs:integer"/>
      </xs:sequence>
    </xs:complexType>
  </xs:element>
</xs:schema>"#;
        let f = write_temp(xsd, ".xsd");
        let config = ValidatorConfig {
            schema_path: f.path().to_path_buf(),
            schema_type: SchemaType::Xml,
        };
        let compiled = CompiledValidator::compile(&config).unwrap();
        let body = Body::Xml("<order/>".to_string());
        let err = compiled.validate(&body).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("validation failed"), "got: {msg}");
    }

    #[test]
    fn xsd_detects_missing_required_attribute() {
        let xsd = r#"<?xml version="1.0"?>
<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema">
  <xs:element name="order">
    <xs:complexType>
      <xs:sequence>
        <xs:element name="item">
          <xs:complexType>
            <xs:attribute name="id" type="xs:string" use="required"/>
          </xs:complexType>
        </xs:element>
      </xs:sequence>
    </xs:complexType>
  </xs:element>
</xs:schema>"#;
        let f = write_temp(xsd, ".xsd");
        let config = ValidatorConfig {
            schema_path: f.path().to_path_buf(),
            schema_type: SchemaType::Xml,
        };
        let compiled = CompiledValidator::compile(&config).unwrap();
        let body = Body::Xml("<order><item/></order>".to_string());
        let err = compiled.validate(&body).unwrap_err();
        assert!(
            err.to_string().contains("validation failed"),
            "libxml2 should detect missing required attr"
        );
    }

    #[test]
    fn xsd_detects_wrong_simple_type() {
        let xsd = r#"<?xml version="1.0"?>
<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema">
  <xs:element name="root">
    <xs:complexType>
      <xs:sequence>
        <xs:element name="count" type="xs:positiveInteger"/>
      </xs:sequence>
    </xs:complexType>
  </xs:element>
</xs:schema>"#;
        let f = write_temp(xsd, ".xsd");
        let config = ValidatorConfig {
            schema_path: f.path().to_path_buf(),
            schema_type: SchemaType::Xml,
        };
        let compiled = CompiledValidator::compile(&config).unwrap();
        let body = Body::Xml("<root><count>-5</count></root>".to_string());
        assert!(
            compiled.validate(&body).is_err(),
            "libxml2 should detect negative positiveInteger"
        );
    }

    #[test]
    fn json_valid_passes() {
        let schema =
            r#"{"type":"object","required":["name"],"properties":{"name":{"type":"string"}}}"#;
        let f = write_temp(schema, ".json");
        let config = ValidatorConfig {
            schema_path: f.path().to_path_buf(),
            schema_type: SchemaType::Json,
        };
        let compiled = CompiledValidator::compile(&config).unwrap();
        let body = Body::Json(serde_json::json!({"name": "Alice"}));
        assert!(compiled.validate(&body).is_ok());
    }

    #[test]
    fn json_invalid_returns_errors() {
        let schema = r#"{"type":"object","required":["name"]}"#;
        let f = write_temp(schema, ".json");
        let config = ValidatorConfig {
            schema_path: f.path().to_path_buf(),
            schema_type: SchemaType::Json,
        };
        let compiled = CompiledValidator::compile(&config).unwrap();
        let body = Body::Json(serde_json::json!({"age": 30}));
        let err = compiled.validate(&body).unwrap_err();
        assert!(err.to_string().contains("validation failed"));
    }

    #[test]
    fn json_text_body_parses_and_validates() {
        let schema = r#"{"type":"string"}"#;
        let f = write_temp(schema, ".json");
        let config = ValidatorConfig {
            schema_path: f.path().to_path_buf(),
            schema_type: SchemaType::Json,
        };
        let compiled = CompiledValidator::compile(&config).unwrap();
        let body = Body::Text(r#""hello""#.to_string());
        assert!(compiled.validate(&body).is_ok());
    }

    #[test]
    fn json_empty_body_errors() {
        let schema = r#"{"type":"object"}"#;
        let f = write_temp(schema, ".json");
        let config = ValidatorConfig {
            schema_path: f.path().to_path_buf(),
            schema_type: SchemaType::Json,
        };
        let compiled = CompiledValidator::compile(&config).unwrap();
        let body = Body::Empty;
        assert!(compiled.validate(&body).is_err());
    }

    #[test]
    fn yaml_valid_passes() {
        let schema = "type: object\nrequired: [host]\nproperties:\n  host:\n    type: string\n";
        let f = write_temp(schema, ".yaml");
        let config = ValidatorConfig {
            schema_path: f.path().to_path_buf(),
            schema_type: SchemaType::Yaml,
        };
        let compiled = CompiledValidator::compile(&config).unwrap();
        let body = Body::Text("host: localhost\n".to_string());
        assert!(compiled.validate(&body).is_ok());
    }

    #[test]
    fn yaml_invalid_returns_errors() {
        let schema = "type: object\nrequired: [host]\n";
        let f = write_temp(schema, ".yaml");
        let config = ValidatorConfig {
            schema_path: f.path().to_path_buf(),
            schema_type: SchemaType::Yaml,
        };
        let compiled = CompiledValidator::compile(&config).unwrap();
        let body = Body::Text("port: 8080\n".to_string());
        let err = compiled.validate(&body).unwrap_err();
        assert!(err.to_string().contains("validation failed"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_xsd_validation_stress_test() {
        let xsd = r#"<?xml version="1.0"?>
<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema">
  <xs:element name="order">
    <xs:complexType>
      <xs:sequence>
        <xs:element name="id" type="xs:string"/>
      </xs:sequence>
    </xs:complexType>
  </xs:element>
</xs:schema>"#;
        let f = write_temp(xsd, ".xsd");
        let config = ValidatorConfig {
            schema_path: f.path().to_path_buf(),
            schema_type: SchemaType::Xml,
        };
        let compiled = Arc::new(CompiledValidator::compile(&config).unwrap());

        let mut handles = Vec::new();
        for i in 0..20 {
            let c = Arc::clone(&compiled);
            handles.push(tokio::spawn(async move {
                let body = if i % 2 == 0 {
                    Body::Xml("<order><id>123</id></order>".to_string())
                } else {
                    Body::Xml("<order/>".to_string())
                };
                c.validate(&body)
            }));
        }

        let mut valid = 0;
        let mut invalid = 0;
        for h in handles {
            match h.await.unwrap() {
                Ok(()) => valid += 1,
                Err(_) => invalid += 1,
            }
        }
        assert_eq!(valid, 10);
        assert_eq!(invalid, 10);
    }
}
