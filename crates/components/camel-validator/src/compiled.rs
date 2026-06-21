use std::path::Path;
use std::sync::Arc;

use camel_component_api::{Body, CamelError};
// serde_yml migrated to noyalib (compat-serde-yaml shim) — closes RUSTSEC-2025-0068.
// Module alias preserves call-site paths byte-for-byte.
use noyalib::compat::serde_yaml as serde_yml;
use serde_yml::Value as YamlValue;

use crate::config::{SchemaType, ValidatorConfig};
use crate::error::ValidatorError;
use crate::resolver::{FilesystemResolver, ResourceResolver};
use crate::xsd_bridge::XsdBridge;

/// Returns an approximate byte-length for a [`Body`] variant.
///
/// This is a rough estimate used only for backpressure (rejecting obviously
/// oversized payloads). `Body::Stream` is not measurable and returns `None`.
pub(crate) fn approx_body_byte_len(body: &Body) -> Option<usize> {
    match body {
        Body::Empty => Some(0),
        Body::Bytes(b) => Some(b.len()),
        Body::Text(s) => Some(s.len()),
        Body::Xml(s) => Some(s.len()),
        Body::Json(v) => serde_json::to_vec(v).ok().map(|v| v.len()),
        Body::Stream(_) => None,
    }
}

pub(crate) enum CompiledValidator {
    Xml {
        /// Raw XSD bytes. `register` is called lazily on first validation so
        /// bridge startup happens in an async context (avoiding runtime issues
        /// caused by block_on's temporary runtime).
        xsd_bytes: Vec<u8>,
        backend: Arc<dyn XsdBridge>,
        max_payload_bytes: Option<usize>,
    },
    Json {
        validator: Arc<jsonschema::Validator>,
        max_payload_bytes: Option<usize>,
    },
    Yaml {
        validator: Arc<jsonschema::Validator>,
        max_payload_bytes: Option<usize>,
    },
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

        let resolver = FilesystemResolver;
        let path_str = path.to_str().ok_or_else(|| {
            CamelError::EndpointCreationFailed("schema path contains non-UTF-8 characters".into())
        })?;
        let content = resolver.resolve(path_str)?;

        match config.schema_type {
            SchemaType::Xml => Ok(Self::compile_xsd(
                &content,
                xsd_backend,
                config.max_payload_bytes,
            )),
            SchemaType::Json => Self::compile_json(&content, path, config.max_payload_bytes),
            SchemaType::Yaml => Self::compile_yaml_schema(&content, path, config.max_payload_bytes),
            SchemaType::RelaxNg | SchemaType::Schematron => Err(CamelError::Config(format!(
                "schema type {:?} is not yet supported; supported: Xml, Json, Yaml",
                config.schema_type
            ))),
        }
    }

    fn compile_xsd(
        content: &[u8],
        backend: Arc<dyn XsdBridge>,
        max_payload_bytes: Option<usize>,
    ) -> Self {
        // Bridge startup and schema registration are deferred to the first validate()
        // call, which runs in a proper async context. This avoids runtime issues
        // caused by block_on's temporary runtime (channel I/O tasks would die with it).
        CompiledValidator::Xml {
            xsd_bytes: content.to_vec(),
            backend,
            max_payload_bytes,
        }
    }

    fn compile_json(
        content: &[u8],
        path: &Path,
        max_payload_bytes: Option<usize>,
    ) -> Result<Self, CamelError> {
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

        Ok(CompiledValidator::Json {
            validator: Arc::new(validator),
            max_payload_bytes,
        })
    }

    fn compile_yaml_schema(
        content: &[u8],
        path: &Path,
        max_payload_bytes: Option<usize>,
    ) -> Result<Self, CamelError> {
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

        Ok(CompiledValidator::Yaml {
            validator: Arc::new(validator),
            max_payload_bytes,
        })
    }

    pub async fn validate(&self, body: &Body) -> Result<(), CamelError> {
        if let Some(limit) = self.max_payload_bytes()
            && let Some(actual) = approx_body_byte_len(body)
            && actual > limit
        {
            return Err(ValidatorError::PayloadTooLarge { actual, limit }.to_processor_error());
        }

        match self {
            CompiledValidator::Xml {
                xsd_bytes, backend, ..
            } => Self::validate_xml(xsd_bytes, backend, body).await,
            CompiledValidator::Json { validator, .. } => Self::validate_json(validator, body),
            CompiledValidator::Yaml { validator, .. } => Self::validate_yaml(validator, body),
        }
    }

    fn max_payload_bytes(&self) -> Option<usize> {
        match self {
            CompiledValidator::Xml {
                max_payload_bytes, ..
            } => *max_payload_bytes,
            CompiledValidator::Json {
                max_payload_bytes, ..
            } => *max_payload_bytes,
            CompiledValidator::Yaml {
                max_payload_bytes, ..
            } => *max_payload_bytes,
        }
    }

    async fn validate_xml(
        xsd_bytes: &[u8],
        backend: &Arc<dyn XsdBridge>,
        body: &Body,
    ) -> Result<(), CamelError> {
        // Register lazily (idempotent: no-op if already registered).
        let schema_id = backend
            .register(xsd_bytes.to_vec())
            .await
            .map_err(|e| CamelError::Config(format!("XSD registration failed: {e}")))?;

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
    use crate::config::{DEFAULT_SCHEMA_CACHE_MAX_ENTRIES, SchemaType, ValidatorConfig};
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
            max_payload_bytes: None,
            schema_cache_max_entries: DEFAULT_SCHEMA_CACHE_MAX_ENTRIES,
            fail_on_null_body: true,
            header_name: None,
            fail_on_null_header: true,
        };

        let bridge = Arc::new(MockBridge {
            register_err: Some(ValidatorError::CompilationFailed {
                message: "COMPILATION_FAILED".to_string(),
                source: None,
            }),
        });

        // compile() is now sync and always succeeds for XSD (deferred registration)
        let compiled = CompiledValidator::compile(&cfg, bridge).expect("compile should succeed");

        // The error surfaces on the first validate() call when register() is attempted
        let err = compiled
            .validate(&Body::Xml("<order/>".to_string()))
            .await
            .expect_err("expected validate to fail due to registration error");
        assert!(matches!(err, CamelError::Config(_)));
        assert!(err.to_string().contains("COMPILATION_FAILED"));
    }

    #[test]
    fn approx_body_byte_len_variants() {
        assert_eq!(approx_body_byte_len(&Body::Empty), Some(0));
        assert_eq!(
            approx_body_byte_len(&Body::Text("hello".to_string())),
            Some(5)
        );
        assert_eq!(
            approx_body_byte_len(&Body::Xml("<a/>".to_string())),
            Some(4)
        );
        // Json is serialized to measure
        let json_len = approx_body_byte_len(&Body::Json(serde_json::json!({"id": 1})));
        assert!(json_len.is_some());
        assert!(json_len.unwrap() > 0);
    }
}
