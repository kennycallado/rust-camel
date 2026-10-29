use std::collections::BTreeMap;
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
use crate::schema_set::{SchemaSet, discover_schema_set};
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
        // Stream and future variants are not measurably-sized.
        _ => None,
    }
}

/// Compile a JSON Schema with `format` keyword validation controlled by
/// `validate_formats`.
///
/// Format validation is enabled by default for every draft, while the schema's
/// `$schema` key keeps selecting draft semantics (draft 2020-12 when absent).
///
/// Twin of the helper in `crates/camel-processor/src/json_schema_validate.rs`:
/// no common crate exists that both consumers could share without a new
/// dependency edge.
fn compile_json_schema(
    schema: &serde_json::Value,
    validate_formats: bool,
) -> Result<jsonschema::Validator, jsonschema::ValidationError<'static>> {
    jsonschema::options()
        .should_validate_formats(validate_formats)
        .build(schema)
}

pub(crate) enum CompiledValidator {
    Xml {
        /// Discovered schema set (root bytes, transitive imports/includes, and
        /// the supplied alias map). `register` is called lazily on first
        /// validation so bridge startup happens in an async context (avoiding
        /// runtime issues caused by block_on's temporary runtime).
        schema_set: Arc<SchemaSet>,
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
        // Defense-in-depth: `from_uri` caps aliases at parse time, but
        // `ValidatorConfig` fields are public and a programmatic constructor
        // can bypass it. Run before any schema-type dispatch or filesystem
        // read so the bounded cap error wins over a generic missing-schema
        // error.
        config.validate_schema_aliases()?;
        let path = &config.schema_path;

        match config.schema_type {
            SchemaType::Xml => Self::compile_xsd(
                path,
                &config.schema_aliases,
                xsd_backend,
                config.max_payload_bytes,
            ),
            SchemaType::Json | SchemaType::Yaml => {
                let resolver = FilesystemResolver;
                let path_str = path.to_str().ok_or_else(|| {
                    CamelError::EndpointCreationFailed(
                        "schema path contains non-UTF-8 characters".into(),
                    )
                })?;
                let content = resolver.resolve(path_str)?;
                match config.schema_type {
                    SchemaType::Json => Self::compile_json(
                        &content,
                        path,
                        config.max_payload_bytes,
                        config.validate_formats,
                    ),
                    SchemaType::Yaml => Self::compile_yaml_schema(
                        &content,
                        path,
                        config.max_payload_bytes,
                        config.validate_formats,
                    ),
                    _ => unreachable!("outer arm restricts this branch to JSON and YAML"),
                }
            }
            SchemaType::RelaxNg | SchemaType::Schematron => Err(CamelError::Config(format!(
                "schema type {:?} is not yet supported; supported: Xml, Json, Yaml",
                config.schema_type
            ))),
        }
    }

    /// Discovers the XSD schema set for `path` under `aliases` exactly once.
    ///
    /// Bridge startup and schema registration are deferred to the first
    /// validate() call, which runs in a proper async context. Discovery is
    /// fail-closed here so an unresolvable root or import surfaces at endpoint
    /// creation as a domain error rather than a silent empty registration.
    fn compile_xsd(
        path: &Path,
        aliases: &BTreeMap<String, String>,
        backend: Arc<dyn XsdBridge>,
        max_payload_bytes: Option<usize>,
    ) -> Result<Self, CamelError> {
        let set = discover_schema_set(path, aliases)
            .map_err(|e| CamelError::EndpointCreationFailed(e.to_string()))?;
        Ok(CompiledValidator::Xml {
            schema_set: Arc::new(set),
            backend,
            max_payload_bytes,
        })
    }

    fn compile_json(
        content: &[u8],
        path: &Path,
        max_payload_bytes: Option<usize>,
        validate_formats: bool,
    ) -> Result<Self, CamelError> {
        let schema_value: serde_json::Value = serde_json::from_slice(content).map_err(|e| {
            CamelError::EndpointCreationFailed(format!(
                "invalid JSON schema '{}': {e}",
                path.display()
            ))
        })?;

        let validator = compile_json_schema(&schema_value, validate_formats).map_err(|e| {
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
        validate_formats: bool,
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

        let validator = compile_json_schema(&schema_value, validate_formats).map_err(|e| {
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
                schema_set,
                backend,
                ..
            } => Self::validate_xml(schema_set, backend, body).await,
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
        schema_set: &SchemaSet,
        backend: &Arc<dyn XsdBridge>,
        body: &Body,
    ) -> Result<(), CamelError> {
        // Register lazily (idempotent: no-op if already registered).
        let schema_id = backend
            .register(&schema_set.cache_key(), schema_set)
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
    use crate::config::{
        DEFAULT_SCHEMA_CACHE_MAX_ENTRIES, DEFAULT_VALIDATE_FORMATS, SchemaType, ValidatorConfig,
    };
    use async_trait::async_trait;

    #[derive(Debug, Clone)]
    struct MockBridge {
        register_err: Option<ValidatorError>,
    }

    #[async_trait]
    impl XsdBridge for MockBridge {
        async fn register(
            &self,
            _schema_id: &str,
            _set: &crate::schema_set::SchemaSet,
        ) -> Result<String, ValidatorError> {
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
            validate_formats: DEFAULT_VALIDATE_FORMATS,
            schema_aliases: BTreeMap::new(),
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

    fn write_temp_schema(suffix: &str, content: &str) -> tempfile::NamedTempFile {
        use std::io::Write;
        let mut file = tempfile::Builder::new().suffix(suffix).tempfile().unwrap();
        file.write_all(content.as_bytes()).unwrap();
        file
    }

    fn compile_temp_schema(path: &std::path::Path, validate_formats: bool) -> CompiledValidator {
        let mut cfg = ValidatorConfig::from_uri(&format!("validator:{}", path.display())).unwrap();
        cfg.validate_formats = validate_formats;
        let bridge = Arc::new(MockBridge { register_err: None });
        CompiledValidator::compile(&cfg, bridge).expect("compile should succeed")
    }

    const JSON_EMAIL_SCHEMA: &str = r#"{"type":"object","properties":{"email":{"type":"string","format":"email"}},"required":["email"]}"#;
    const YAML_EMAIL_SCHEMA: &str = "type: object\nproperties:\n  email:\n    type: string\n    format: email\nrequired:\n  - email\n";

    #[tokio::test]
    async fn json_format_email_rejected_by_default() {
        let schema = write_temp_schema(".json", JSON_EMAIL_SCHEMA);
        let compiled = compile_temp_schema(schema.path(), DEFAULT_VALIDATE_FORMATS);
        let body = Body::Text("{\"email\":\"not-an-email\"}".into());
        assert!(compiled.validate(&body).await.is_err());
    }

    #[tokio::test]
    async fn json_format_email_accepted_when_opted_out() {
        let schema = write_temp_schema(".json", JSON_EMAIL_SCHEMA);
        let compiled = compile_temp_schema(schema.path(), false);
        let body = Body::Text("{\"email\":\"not-an-email\"}".into());
        assert!(compiled.validate(&body).await.is_ok());
    }

    #[tokio::test]
    async fn yaml_format_enforced_by_default() {
        let schema = write_temp_schema(".yaml", YAML_EMAIL_SCHEMA);
        let compiled = compile_temp_schema(schema.path(), DEFAULT_VALIDATE_FORMATS);
        let body = Body::Text("email: not-an-email".into());
        assert!(compiled.validate(&body).await.is_err());
    }

    #[tokio::test]
    async fn yaml_format_opt_out_accepts() {
        let schema = write_temp_schema(".yaml", YAML_EMAIL_SCHEMA);
        let compiled = compile_temp_schema(schema.path(), false);
        let body = Body::Text("email: not-an-email".into());
        assert!(compiled.validate(&body).await.is_ok());
    }

    #[tokio::test]
    async fn json_unknown_format_is_annotation() {
        let schema = write_temp_schema(".json", r#"{"type":"string","format":"unknown-format-x"}"#);
        let compiled = compile_temp_schema(schema.path(), DEFAULT_VALIDATE_FORMATS);
        assert!(
            compiled
                .validate(&Body::Text("\"anything\"".into()))
                .await
                .is_ok()
        );
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
