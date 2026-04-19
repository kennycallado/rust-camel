use std::path::Path;
use std::sync::Arc;

use camel_component_api::{Body, CamelError};
use serde_yml::Value as YamlValue;

use crate::config::{SchemaType, ValidatorConfig};
use crate::error::ValidatorError;
use crate::xsd_bridge::XsdBridge;

pub(crate) enum CompiledValidator {
    Xml {
        /// Raw XSD bytes. `register` is called lazily on first validation so
        /// bridge startup happens in an async context (avoiding runtime issues
        /// caused by block_on's temporary runtime).
        xsd_bytes: Vec<u8>,
        backend: Arc<dyn XsdBridge>,
    },
    Json(Arc<jsonschema::Validator>),
    Yaml(Arc<jsonschema::Validator>),
}

impl std::fmt::Debug for CompiledValidator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompiledValidator").finish_non_exhaustive()
    }
}

impl CompiledValidator {
    pub fn compile(
        config: &ValidatorConfig,
        xsd_backend: Arc<dyn XsdBridge>,
    ) -> Result<Self, CamelError> {
        let path = &config.schema_path;

        let content = std::fs::read(path).map_err(|e| {
            CamelError::EndpointCreationFailed(format!(
                "failed to read schema file '{}': {e}",
                path.display()
            ))
        })?;

        match config.schema_type {
            SchemaType::Xml => Ok(Self::compile_xsd(&content, xsd_backend)),
            SchemaType::Json => Self::compile_json(&content, path),
            SchemaType::Yaml => Self::compile_yaml_schema(&content, path),
            SchemaType::RelaxNg | SchemaType::Schematron => Err(ValidatorError::UnsupportedMode(
                "RelaxNG/Schematron require a future xml-bridge update",
            )
            .to_endpoint_error()),
        }
    }

    fn compile_xsd(content: &[u8], backend: Arc<dyn XsdBridge>) -> Self {
        // Bridge startup and schema registration are deferred to the first validate()
        // call, which runs in a proper async context. This avoids runtime issues
        // caused by block_on's temporary runtime (channel I/O tasks would die with it).
        CompiledValidator::Xml {
            xsd_bytes: content.to_vec(),
            backend,
        }
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

    pub async fn validate(&self, body: &Body) -> Result<(), CamelError> {
        match self {
            CompiledValidator::Xml { xsd_bytes, backend } => {
                Self::validate_xml(xsd_bytes, backend, body).await
            }
            CompiledValidator::Json(validator) => Self::validate_json(validator, body),
            CompiledValidator::Yaml(validator) => Self::validate_yaml(validator, body),
        }
    }

    async fn validate_xml(
        xsd_bytes: &[u8],
        backend: &Arc<dyn XsdBridge>,
        body: &Body,
    ) -> Result<(), CamelError> {
        // Register lazily (idempotent: no-op if already registered).
        let schema_id = backend.register(xsd_bytes.to_vec()).await.map_err(|e| {
            CamelError::EndpointCreationFailed(format!("XSD schema registration failed: {e}"))
        })?;

        let xml_bytes = match body {
            Body::Xml(s) => s.as_bytes().to_vec(),
            Body::Text(s) => s.as_bytes().to_vec(),
            Body::Bytes(b) => b.to_vec(),
            _ => {
                return Err(CamelError::ProcessorError(
                    "XSD validator requires Body::Xml, Body::Text, or Body::Bytes".to_string(),
                ));
            }
        };

        backend
            .validate(&schema_id, xml_bytes)
            .await
            .map_err(|e| e.to_processor_error())
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
    use async_trait::async_trait;

    #[derive(Debug, Clone)]
    struct MockBridge {
        register_err: Option<ValidatorError>,
    }

    #[async_trait]
    impl XsdBridge for MockBridge {
        async fn register(&self, _xsd_bytes: Vec<u8>) -> Result<String, ValidatorError> {
            if let Some(err) = &self.register_err {
                return Err(err.clone());
            }
            Ok("xsd-mock".to_string())
        }

        async fn validate(
            &self,
            _schema_id: &str,
            _doc_bytes: Vec<u8>,
        ) -> Result<(), ValidatorError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn xsd_bridge_register_error_propagates_on_validate() {
        let mut schema = tempfile::Builder::new().suffix(".xsd").tempfile().unwrap();
        use std::io::Write;
        schema.write_all(b"<xs:schema/>").unwrap();

        let cfg = ValidatorConfig {
            schema_path: schema.path().to_path_buf(),
            schema_type: SchemaType::Xml,
        };

        let bridge = Arc::new(MockBridge {
            register_err: Some(ValidatorError::CompilationFailed(
                "COMPILATION_FAILED".to_string(),
            )),
        });

        // compile() is now sync and always succeeds for XSD (deferred registration)
        let compiled = CompiledValidator::compile(&cfg, bridge).expect("compile should succeed");

        // The error surfaces on the first validate() call when register() is attempted
        let err = compiled
            .validate(&Body::Xml("<order/>".to_string()))
            .await
            .expect_err("expected validate to fail due to registration error");
        assert!(err.to_string().contains("COMPILATION_FAILED"));
    }
}
