use std::borrow::Cow;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use arc_swap::ArcSwap;
use async_trait::async_trait;
use camel_component_api::template_reload::{TemplateReloadStaged, TemplateReloadTarget};
use camel_component_api::{Body, CamelError};
use camel_json_schema::CompiledSchema;
use camel_json_schema::FormatMode;
use camel_json_schema::SchemaError;
use camel_json_schema::SchemaSource;
use camel_json_schema::SchemaSyntax;
use prost_reflect::MessageDescriptor;
// serde_yml migrated to noyalib (compat-serde-yaml shim) — closes RUSTSEC-2025-0068.
// Module alias preserves call-site paths byte-for-byte.
use noyalib::compat::serde_yaml as serde_yml;
use serde_yml::Value as YamlValue;

use crate::config::{SchemaType, ValidatorConfig};
use crate::error::ValidatorError;
use crate::resolver::MAX_SCHEMA_BYTES;
use crate::schema_set::{
    SchemaSet, discover_embedded_schema_set, discover_schema_set, embedded_ref_bytes,
    embedded_ref_name, is_embedded_ref,
};
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
        validator: Arc<CompiledSchema>,
        max_payload_bytes: Option<usize>,
    },
    Yaml {
        validator: Arc<CompiledSchema>,
        max_payload_bytes: Option<usize>,
    },
    Protobuf {
        /// Hot-swappable descriptor set. The protobuf validation arm loads the
        /// current state per call, so a committed reload is visible to the next
        /// validation without an endpoint rebuild.
        state: SharedProtobufState,
        max_payload_bytes: Option<usize>,
    },
}

/// Committed protobuf descriptor plus the absolute files that produced it.
pub(crate) struct ProtobufReloadState {
    descriptor: MessageDescriptor,
    /// Absolute schema path followed by its local import closure.
    source_files: Vec<PathBuf>,
}

type SharedProtobufState = Arc<ArcSwap<ProtobufReloadState>>;

impl TemplateReloadStaged for ProtobufReloadState {
    fn into_any(self: Box<Self>) -> Box<dyn std::any::Any> {
        self
    }
}

/// One [`TemplateReloadTarget`] per live protobuf validator producer.
///
/// The target shares the [`ArcSwap`] with the compiled validator, so a commit
/// swaps the descriptor the producer reads on its next validation call.
pub(crate) struct ProtobufReloadTarget {
    route_id: String,
    message: String,
    state: SharedProtobufState,
    schema_path: PathBuf,
    generation: AtomicU64,
}

impl ProtobufReloadTarget {
    pub(crate) fn new(
        route_id: impl Into<String>,
        message: impl Into<String>,
        state: SharedProtobufState,
        schema_path: PathBuf,
    ) -> Self {
        Self {
            route_id: route_id.into(),
            message: message.into(),
            state,
            schema_path,
            generation: AtomicU64::new(0),
        }
    }
}

#[async_trait]
impl TemplateReloadTarget for ProtobufReloadTarget {
    fn route_id(&self) -> &str {
        &self.route_id
    }

    fn reload_timeout(&self) -> Duration {
        // Mirrors camel-template's `reload_timeout_ms` default of 5000
        // (`ResolvedExternalTemplateLimits`), so a route that watches protobuf
        // and template sources keeps one shared deadline.
        Duration::from_millis(5000)
    }

    fn current_generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    fn watch_sources(&self) -> Vec<PathBuf> {
        self.state.load_full().source_files.clone()
    }

    async fn build(&self) -> Result<(Box<dyn TemplateReloadStaged>, u64), CamelError> {
        let schema_path = self.schema_path.clone();
        let message = self.message.clone();
        // Read the generation before the build so the registry's stale check
        // compares against the snapshot this staged set was derived from.
        let read_generation = self.current_generation();
        let staged =
            tokio::task::spawn_blocking(move || compile_protobuf_state(&schema_path, &message))
                .await
                .map_err(|e| {
                    CamelError::TemplateReload(format!("protobuf reload task failed: {e}"))
                })?
                .map_err(|e| {
                    CamelError::TemplateReload(format!("protobuf recompile failed: {e}"))
                })?;
        Ok((Box::new(staged), read_generation))
    }

    fn commit(&self, staged: Box<dyn TemplateReloadStaged>) {
        match staged.into_any().downcast::<ProtobufReloadState>() {
            Ok(state) => {
                self.state.store(Arc::new(*state));
                self.generation.fetch_add(1, Ordering::SeqCst);
            }
            Err(_) => {
                // Unreachable: `build` is the only staged producer and always
                // yields a `ProtobufReloadState`. Leave the prior state intact.
                // log-policy: system-broken
                tracing::error!(
                    route_id = %self.route_id,
                    "protobuf reload: unexpected staged payload; keeping prior descriptor"
                );
            }
        }
    }
}

/// Absolute form of `path`, falling back to the owned path on failure.
fn absolute_path(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Absolute, sorted, deduplicated schema + local import closure from the SAME
/// snapshot that produced the descriptor.
fn protobuf_source_files(schema_path: &Path, imports: &[PathBuf]) -> Vec<PathBuf> {
    let mut sources = Vec::with_capacity(imports.len() + 1);
    sources.push(absolute_path(schema_path));
    sources.extend(imports.iter().map(|path| absolute_path(path)));
    sources.sort();
    sources.dedup();
    sources
}

/// Compile the proto schema, select the named message, and snapshot sources.
///
/// The descriptor and its source list come from one read of the schema graph:
/// `compile_proto_with_sources` preloads the import closure once and compiles
/// only from that snapshot, so an edit landing mid-build cannot commit a
/// descriptor and a watch list that disagree about which imports exist.
fn compile_protobuf_state(
    schema_path: &Path,
    message: &str,
) -> Result<ProtobufReloadState, CamelError> {
    let (pool, imports) =
        camel_proto_compiler::compile_proto_with_sources(schema_path).map_err(|e| {
            CamelError::EndpointCreationFailed(format!(
                "failed to compile proto schema '{}': {e}",
                schema_path.display()
            ))
        })?;

    let descriptor = pool
        .get_message_by_name(message)
        .ok_or_else(|| CamelError::Config(format!("message descriptor not found: {message}")))?;

    Ok(ProtobufReloadState {
        descriptor,
        source_files: protobuf_source_files(schema_path, &imports),
    })
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
                let path_str = path.to_str().ok_or_else(|| {
                    CamelError::EndpointCreationFailed(
                        "schema path contains non-UTF-8 characters".into(),
                    )
                })?;
                // Embedded references resolve from the process-global registry
                // before any filesystem attempt; the shared cap keeps registry
                // bytes bounded exactly like disk bytes.
                let (source, syntax) =
                    match embedded_ref_bytes(path_str).map_err(|e| e.to_endpoint_error())? {
                        Some(bytes) => {
                            if bytes.len() as u64 > MAX_SCHEMA_BYTES {
                                return Err(CamelError::EndpointCreationFailed(format!(
                                    "schema file '{path_str}' exceeds {MAX_SCHEMA_BYTES} bytes"
                                )));
                            }
                            // Registry content has no filesystem root: parse it
                            // here and compile it as an inline document. Syntax
                            // is meaningless once the document is a `Value`.
                            (
                                SchemaSource::Inline(Self::parse_embedded_schema(
                                    &bytes,
                                    path,
                                    &config.schema_type,
                                )?),
                                SchemaSyntax::Auto,
                            )
                        }
                        // File-backed schemas are compiled in place, never
                        // pre-parsed, so sibling references resolve under the
                        // crate's confinement rules.
                        None => (
                            SchemaSource::File(path.clone()),
                            match config.schema_type {
                                SchemaType::Json => SchemaSyntax::Json,
                                _ => SchemaSyntax::Yaml,
                            },
                        ),
                    };
                match config.schema_type {
                    SchemaType::Json => Self::compile_json(
                        &source,
                        syntax,
                        path,
                        config.max_payload_bytes,
                        config.validate_formats,
                    ),
                    SchemaType::Yaml => Self::compile_yaml_schema(
                        &source,
                        syntax,
                        path,
                        config.max_payload_bytes,
                        config.validate_formats,
                    ),
                    _ => unreachable!("outer arm restricts this branch to JSON and YAML"),
                }
            }
            SchemaType::Protobuf => {
                Self::compile_protobuf(path, config.message.as_deref(), config.max_payload_bytes)
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
        // A non-UTF-8 path can never carry the ASCII prefix, so it keeps the
        // existing filesystem behavior.
        let set = if let Some(reference) = path.to_str() {
            match embedded_ref_bytes(reference).map_err(|e| e.to_endpoint_error())? {
                Some(bytes) => discover_embedded_schema_set(&bytes, reference, aliases)
                    .map_err(|e| e.to_endpoint_error())?,
                None => discover_schema_set(path, aliases).map_err(|e| e.to_endpoint_error())?,
            }
        } else {
            discover_schema_set(path, aliases).map_err(|e| e.to_endpoint_error())?
        };
        Ok(CompiledValidator::Xml {
            schema_set: Arc::new(set),
            backend,
            max_payload_bytes,
        })
    }

    fn compile_json(
        source: &SchemaSource,
        syntax: SchemaSyntax,
        path: &Path,
        max_payload_bytes: Option<usize>,
        validate_formats: bool,
    ) -> Result<Self, CamelError> {
        let validator = Self::compile_document(source, syntax, "JSON", path, validate_formats)?;

        Ok(CompiledValidator::Json {
            validator: Arc::new(validator),
            max_payload_bytes,
        })
    }

    fn compile_yaml_schema(
        source: &SchemaSource,
        syntax: SchemaSyntax,
        path: &Path,
        max_payload_bytes: Option<usize>,
        validate_formats: bool,
    ) -> Result<Self, CamelError> {
        let validator = Self::compile_document(source, syntax, "YAML", path, validate_formats)?;

        Ok(CompiledValidator::Yaml {
            validator: Arc::new(validator),
            max_payload_bytes,
        })
    }

    /// Compiles `source` while keeping every pre-migration literal byte-exact.
    ///
    /// `kind` names the schema type in the compile-failure literal (`JSON` or
    /// `YAML`) and `path` is the configured path that literal names.
    fn compile_document(
        source: &SchemaSource,
        syntax: SchemaSyntax,
        kind: &str,
        path: &Path,
        validate_formats: bool,
    ) -> Result<CompiledSchema, CamelError> {
        let format = if validate_formats {
            FormatMode::Enforce
        } else {
            FormatMode::Annotate
        };

        camel_json_schema::compile(source, syntax, format).map_err(|error| match error {
            // The parse text already carries the complete final message.
            SchemaError::Parse { message, .. } => CamelError::EndpointCreationFailed(message),
            SchemaError::Compile { message } => CamelError::EndpointCreationFailed(format!(
                "failed to compile {kind} schema '{}': {message}",
                path.display()
            )),
            other => CamelError::EndpointCreationFailed(other.to_string()),
        })
    }

    /// Parses registry bytes into the `Value` an inline source compiles.
    ///
    /// Embedded content has no filesystem path the shared crate could re-read,
    /// so the wrapper parses it here and keeps the pre-migration parse-failure
    /// literals byte for byte.
    fn parse_embedded_schema(
        content: &[u8],
        path: &Path,
        schema_type: &SchemaType,
    ) -> Result<serde_json::Value, CamelError> {
        match schema_type {
            SchemaType::Json => serde_json::from_slice(content).map_err(|e| {
                CamelError::EndpointCreationFailed(format!(
                    "invalid JSON schema '{}': {e}",
                    path.display()
                ))
            }),
            SchemaType::Yaml => {
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

                serde_json::to_value(&yaml_value).map_err(|e| {
                    CamelError::EndpointCreationFailed(format!(
                        "failed to convert YAML schema to JSON '{}': {e}",
                        path.display()
                    ))
                })
            }
            _ => unreachable!("caller restricts this branch to JSON and YAML"),
        }
    }

    /// Canonicalizes an embedded protobuf reference for the proto compiler.
    ///
    /// The proto compiler resolves `camel-embedded:` references from its own
    /// process-global registry. Boot registers proto assets in both
    /// registries, but a caller that populated only the shared
    /// `camel-component-api` registry must still resolve: the bytes are
    /// forwarded (idempotently) before compiling. A prefixed value with no
    /// registration in either registry fails closed with the shared wording.
    /// Prefix-less paths are returned unchanged.
    fn embedded_proto_schema_path(path: &Path) -> Result<PathBuf, CamelError> {
        let Some(value) = path.to_str() else {
            return Ok(path.to_path_buf());
        };
        if !is_embedded_ref(value) {
            return Ok(path.to_path_buf());
        }
        let missing =
            || ValidatorError::endpoint(format!("no embedded schema registered under '{value}'"));
        let Some(name) = embedded_ref_name(value) else {
            return Err(missing().to_endpoint_error());
        };
        let canonical = || {
            PathBuf::from(format!(
                "{}{name}",
                camel_proto_compiler::EMBEDDED_REF_PREFIX
            ))
        };
        if let Some(bytes) = camel_component_api::embedded_assets::get(&name) {
            camel_proto_compiler::embedded::register(&name, bytes.to_vec());
            return Ok(canonical());
        }
        if camel_proto_compiler::embedded::contains(&name) {
            return Ok(canonical());
        }
        Err(missing().to_endpoint_error())
    }

    /// Compiles the `.proto` descriptor and stores the requested message
    /// descriptor once, at endpoint creation.
    ///
    /// Any compile failure (missing file or invalid proto source) is a
    /// route-build error: it can only be fixed by changing operator config.
    /// The initial state is hot-swappable so a reload target can replace it.
    fn compile_protobuf(
        path: &Path,
        message: Option<&str>,
        max_payload_bytes: Option<usize>,
    ) -> Result<Self, CamelError> {
        let message = message
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .ok_or_else(|| {
                CamelError::Config(
                    "validator protobuf requires a 'message' query parameter naming the \
                     fully-qualified message"
                        .to_string(),
                )
            })?;

        let schema_path = Self::embedded_proto_schema_path(path)?;
        let state = compile_protobuf_state(&schema_path, message)?;

        Ok(CompiledValidator::Protobuf {
            state: Arc::new(ArcSwap::from_pointee(state)),
            max_payload_bytes,
        })
    }

    /// The shared protobuf reload state for a protobuf validator.
    pub(crate) fn protobuf_state(&self) -> Option<&SharedProtobufState> {
        match self {
            CompiledValidator::Protobuf { state, .. } => Some(state),
            _ => None,
        }
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
            CompiledValidator::Protobuf { state, .. } => {
                // Load the current committed state per call: a reload commit is
                // visible to the very next validation without an endpoint
                // rebuild. The guard keeps the snapshot alive for this call.
                let current = state.load_full();
                Self::validate_protobuf(&current.descriptor, body)
            }
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
            CompiledValidator::Protobuf {
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

    fn validate_json(validator: &CompiledSchema, body: &Body) -> Result<(), CamelError> {
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

        let messages = violation_messages(validator, &json_value);

        if messages.is_empty() {
            Ok(())
        } else {
            Err(CamelError::ProcessorError(format!(
                "JSON Schema validation failed:\n{}",
                messages.join("\n")
            )))
        }
    }

    fn validate_yaml(validator: &CompiledSchema, body: &Body) -> Result<(), CamelError> {
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

        let messages = violation_messages(validator, &json_value);

        if messages.is_empty() {
            Ok(())
        } else {
            Err(CamelError::ProcessorError(format!(
                "YAML Schema validation failed:\n{}",
                messages.join("\n")
            )))
        }
    }

    /// Validates a JSON-shaped body against a protobuf message descriptor.
    ///
    /// This only deserializes into a [`prost_reflect::DynamicMessage`] to prove
    /// the payload matches the descriptor, then drops it. The body is never
    /// mutated (not even normalized), and no protobuf encode/decode happens.
    fn validate_protobuf(descriptor: &MessageDescriptor, body: &Body) -> Result<(), CamelError> {
        let json: Cow<'_, str> = match body {
            Body::Json(value) => Cow::Owned(serde_json::to_string(value).map_err(|e| {
                CamelError::ValidationError(format!("protobuf validation failed: {e}"))
            })?),
            Body::Text(text) => Cow::Borrowed(text.as_str()),
            Body::Bytes(bytes) => Cow::Borrowed(std::str::from_utf8(bytes).map_err(|e| {
                CamelError::ValidationError(format!(
                    "protobuf validation failed: body bytes are not valid UTF-8: {e}"
                ))
            })?),
            _ => {
                return Err(CamelError::ValidationError(
                    "protobuf validator requires Body::Json, Body::Text, or Body::Bytes"
                        .to_string(),
                ));
            }
        };

        // `DeserializeOptions::default()` denies unknown fields, so any
        // unrecognized key, wrong scalar/enum shape, or bad oneof shape fails.
        let mut de = serde_json::Deserializer::from_str(json.as_ref());
        let parsed = prost_reflect::DynamicMessage::deserialize(descriptor.clone(), &mut de);
        let tail = de.end();
        parsed
            .map_err(|e| CamelError::ValidationError(format!("protobuf validation failed: {e}")))?;
        tail.map_err(|e| CamelError::ValidationError(format!("protobuf validation failed: {e}")))?;
        Ok(())
    }
}

/// Format every violation as `"{message} at {instance_path}"`, the exact shape
/// this component has always reported.
fn violation_messages(validator: &CompiledSchema, instance: &serde_json::Value) -> Vec<String> {
    validator
        .violations(instance)
        .into_iter()
        .map(|violation| format!("{} at {}", violation.message, violation.instance_path))
        .collect()
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
            message: None,
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

    /// Sibling `$ref`s of a file-backed schema resolve next to the root, and
    /// the compiled validator enforces them.
    #[tokio::test]
    async fn validator_sibling_ref_resolves_from_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("defs.json"),
            r#"{"$defs":{"int":{"type":"integer"}}}"#,
        )
        .expect("write defs.json");
        let root = dir.path().join("root.json");
        std::fs::write(
            &root,
            r#"{"type":"object","properties":{"x":{"$ref":"defs.json#/$defs/int"}}}"#,
        )
        .expect("write root.json");

        let compiled = compile_temp_schema(&root, DEFAULT_VALIDATE_FORMATS);
        assert!(
            compiled
                .validate(&Body::Text(r#"{"x":1}"#.into()))
                .await
                .is_ok(),
            "an integer must satisfy the sibling-ref schema"
        );
        assert!(
            compiled
                .validate(&Body::Text(r#"{"x":"s"}"#.into()))
                .await
                .is_err(),
            "a string must fail the sibling-ref schema"
        );
    }

    /// `schemaType` beats the file extension: `root.json` is parsed as YAML and
    /// its sibling reference still resolves inside the confined root.
    #[tokio::test]
    async fn schema_type_override_yaml_root_with_sibling_ref() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("defs.yaml"),
            "definitions:\n  int:\n    type: integer\n",
        )
        .expect("write defs.yaml");
        let root = dir.path().join("root.json");
        std::fs::write(
            &root,
            "type: object\nproperties:\n  x:\n    $ref: defs.yaml#/definitions/int\n",
        )
        .expect("write root.json");

        let cfg =
            ValidatorConfig::from_uri(&format!("validator:{}?schemaType=yaml", root.display()))
                .expect("validator URI must parse");
        assert_eq!(cfg.schema_type, SchemaType::Yaml);

        let compiled =
            CompiledValidator::compile(&cfg, Arc::new(MockBridge { register_err: None }))
                .expect("a YAML root with a .json extension must compile");
        assert!(matches!(compiled, CompiledValidator::Yaml { .. }));
        assert!(
            compiled.validate(&Body::Text("x: 1".into())).await.is_ok(),
            "an integer must satisfy the YAML sibling-ref schema"
        );
        assert!(
            compiled.validate(&Body::Text("x: s".into())).await.is_err(),
            "a string must fail the YAML sibling-ref schema"
        );
    }

    /// Byte-identity guard for the JSON validation-failure message.
    ///
    /// The migrated `camel-json-schema` path must reproduce the pre-migration
    /// `CamelError` text exactly: header line, upstream error text, `at`, and
    /// the instance pointer.
    #[tokio::test]
    async fn validation_failure_message_byte_identical() {
        let schema = write_temp_schema(
            ".json",
            r#"{"type":"object","properties":{"x":{"type":"integer"}}}"#,
        );
        let compiled = compile_temp_schema(schema.path(), DEFAULT_VALIDATE_FORMATS);
        let err = compiled
            .validate(&Body::Text(r#"{"x":"s"}"#.into()))
            .await
            .expect_err("a string must fail an integer schema");
        assert_eq!(
            err.to_string(),
            concat!(
                "Processor error: JSON Schema validation failed:\n",
                "\"s\" is not of type \"integer\" at /x"
            )
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

    const PROTO_PERSON: &[u8] = br#"
syntax = "proto3";
package test.v1;

enum Role {
  ROLE_UNSPECIFIED = 0;
  ROLE_ADMIN = 1;
  ROLE_USER = 2;
}

message Person {
  string name = 1;
  int32 age = 2;
  Role role = 3;
  oneof contact {
    string email = 4;
    string phone = 5;
  }
}
"#;

    #[tokio::test]
    async fn protobuf_compile_stores_descriptor_and_validates() {
        use std::io::Write;

        let mut f = tempfile::Builder::new()
            .suffix(".proto")
            .tempfile()
            .unwrap();
        f.write_all(PROTO_PERSON).unwrap();

        let cfg = ValidatorConfig {
            schema_path: f.path().to_path_buf(),
            schema_type: SchemaType::Protobuf,
            message: Some("test.v1.Person".to_string()),
            max_payload_bytes: None,
            schema_cache_max_entries: DEFAULT_SCHEMA_CACHE_MAX_ENTRIES,
            fail_on_null_body: true,
            header_name: None,
            fail_on_null_header: true,
            validate_formats: DEFAULT_VALIDATE_FORMATS,
            schema_aliases: BTreeMap::new(),
        };

        let bridge = Arc::new(MockBridge { register_err: None });
        let compiled = CompiledValidator::compile(&cfg, bridge).expect("compile should succeed");

        assert!(
            compiled
                .validate(&Body::Text(r#"{"name":"Ann"}"#.to_string()))
                .await
                .is_ok(),
            "a valid protobuf JSON body must pass"
        );
        assert!(
            compiled
                .validate(&Body::Text("{oops".to_string()))
                .await
                .is_err(),
            "malformed JSON must fail protobuf validation"
        );
    }

    #[test]
    fn protobuf_state_keeps_descriptor_and_sources_in_step() {
        let dir = tempfile::tempdir().expect("tempdir");
        let common = dir.path().join("common.proto");
        std::fs::write(
            &common,
            "syntax = \"proto3\";\npackage reload;\nmessage C { string v = 1; }\n",
        )
        .expect("write common proto");
        let root = dir.path().join("root.proto");
        std::fs::write(
            &root,
            "syntax = \"proto3\";\npackage reload;\nmessage M { string a = 1; }\n",
        )
        .expect("write root proto");

        let before = compile_protobuf_state(&root, "reload.M").expect("initial state");
        assert!(before.descriptor.get_field_by_name("c").is_none());
        assert!(
            !before
                .source_files
                .iter()
                .any(|p| p.file_name() == Some(std::ffi::OsStr::new("common.proto"))),
            "pre-edit sources must match the pre-edit descriptor: {:?}",
            before.source_files
        );

        std::fs::write(
            &root,
            "syntax = \"proto3\";\npackage reload;\nimport \"common.proto\";\nmessage M { string a = 1; C c = 2; }\n",
        )
        .expect("grow the import closure");

        let after = compile_protobuf_state(&root, "reload.M").expect("reloaded state");
        assert!(
            after.descriptor.get_field_by_name("c").is_some(),
            "the descriptor must include the new import"
        );
        let expected_common = std::path::absolute(&common).expect("absolute common path");
        let expected_root = std::path::absolute(&root).expect("absolute root path");
        let mut expected_sources = vec![expected_common, expected_root];
        expected_sources.sort();
        assert_eq!(
            after.source_files, expected_sources,
            "the watch sources must be exactly the descriptor inputs"
        );
    }

    // ── embedded registry seams (fixture51 family) ──────────────────────

    const F51_STANDALONE_XSD: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema">
  <xs:complexType name="Order">
    <xs:sequence>
      <xs:element name="id" type="xs:string"/>
    </xs:sequence>
  </xs:complexType>
</xs:schema>
"#;

    const F51_XSD_WITH_IMPORT: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema">
  <xs:import schemaLocation="common.xsd"/>
</xs:schema>
"#;

    fn register_embedded(key: &str, bytes: &[u8]) -> Vec<u8> {
        camel_component_api::embedded_assets::register(key, bytes.to_vec());
        bytes.to_vec()
    }

    /// Pins the validator's encoded-spelling literal to the registry's
    /// canonical prefix (the same cross-crate guard pattern camel-xslt uses):
    /// drift would silently stop recognizing one of the two reserved
    /// spellings.
    #[test]
    fn encoded_embedded_prefix_literal_matches_registry_prefix() {
        assert_eq!(
            crate::schema_set::ENCODED_EMBEDDED_REF_PREFIX,
            camel_component_api::embedded_assets::EMBEDDED_REF_PREFIX.replace(':', "%3A"),
            "the encoded embedded-reference spelling must track the registry constant"
        );
    }

    fn compile_embedded(uri: &str) -> Result<CompiledValidator, CamelError> {
        let cfg = ValidatorConfig::from_uri(uri).expect("embedded validator URI must parse");
        CompiledValidator::compile(&cfg, Arc::new(MockBridge { register_err: None }))
    }

    fn alias_query(aliases: &[(&str, &str)]) -> String {
        let mut obj = serde_json::Map::new();
        for (key, value) in aliases {
            obj.insert(
                (*key).to_string(),
                serde_json::Value::String((*value).to_string()),
            );
        }
        let json = serde_json::Value::Object(obj).to_string();
        percent_encoding::utf8_percent_encode(&json, percent_encoding::NON_ALPHANUMERIC).to_string()
    }

    fn expect_xml(compiled: CompiledValidator) -> Arc<SchemaSet> {
        match compiled {
            CompiledValidator::Xml { schema_set, .. } => schema_set,
            _ => panic!("expected an XML compiled validator"),
        }
    }

    #[test]
    fn embedded_standalone_xsd_compiles_from_registry() {
        let key = "assets/xsd/fixture51/root.xsd";
        let bytes = register_embedded(key, F51_STANDALONE_XSD.as_bytes());
        let compiled = compile_embedded(
            "validator:camel-embedded%3Aassets%2Fxsd%2Ffixture51%2Froot.xsd?schemaType=xml",
        )
        .expect("a registered standalone XSD must compile");
        let set = expect_xml(compiled);
        assert_eq!(set.root_system_id, "root.xsd");
        assert_eq!(
            set.resources.get("root.xsd").map(Vec::as_slice),
            Some(bytes.as_slice()),
            "the root entry must carry the exact registered bytes"
        );
        assert_eq!(set.resources.len(), 1);
    }

    #[test]
    fn embedded_xsd_root_alias_succeeds_and_counts_bytes() {
        let key = "assets/xsd/fixture52/root.xsd";
        let mut bytes = F51_STANDALONE_XSD.as_bytes().to_vec();
        bytes.resize(2 * 1024 * 1024, b' ');
        let bytes = register_embedded(key, &bytes);
        let aliases = alias_query(&[("urn:t", "root.xsd")]);
        let uri = format!(
            "validator:camel-embedded%3Aassets%2Fxsd%2Ffixture52%2Froot.xsd\
             ?schemaType=xml&schemaAliases={aliases}"
        );
        let compiled = compile_embedded(&uri).expect("an alias to the root must compile");
        let set = expect_xml(compiled);
        assert_eq!(set.root_system_id, "root.xsd");
        assert_eq!(
            set.resources.get("root.xsd").map(Vec::as_slice),
            Some(bytes.as_slice())
        );
        assert_eq!(
            set.resources.get("urn:t").map(Vec::as_slice),
            Some(bytes.as_slice()),
            "the alias URI entry must carry the same root bytes"
        );
        assert_eq!(set.resources.len(), 2);
        assert_eq!(
            set.aliases.get("urn:t").map(String::as_str),
            Some("root.xsd")
        );
    }

    #[test]
    fn embedded_xsd_with_import_fails_typed() {
        let key = "assets/xsd/fixture53/root.xsd";
        register_embedded(key, F51_XSD_WITH_IMPORT.as_bytes());
        let err = compile_embedded(
            "validator:camel-embedded%3Aassets%2Fxsd%2Ffixture53%2Froot.xsd?schemaType=xml",
        )
        .expect_err("an import must fail: only the root is embedded");
        let msg = err.to_string();
        assert!(
            msg.contains("common.xsd"),
            "must name the missing import: {msg}"
        );
        assert!(
            msg.contains("not embedded"),
            "must state it is not embedded: {msg}"
        );
        assert!(
            msg.contains("camel-embedded:assets/xsd/fixture53/root.xsd"),
            "must name the root reference: {msg}"
        );
    }

    #[test]
    fn embedded_xsd_alias_to_missing_resource_fails_typed() {
        let key = "assets/xsd/fixture54/root.xsd";
        register_embedded(key, F51_STANDALONE_XSD.as_bytes());
        let aliases = alias_query(&[("urn:o", "other.xsd")]);
        let uri = format!(
            "validator:camel-embedded%3Aassets%2Fxsd%2Ffixture54%2Froot.xsd\
             ?schemaType=xml&schemaAliases={aliases}"
        );
        let err = compile_embedded(&uri).expect_err("an alias to another resource must fail");
        let msg = err.to_string();
        assert!(
            msg.contains("other.xsd"),
            "must name the alias target: {msg}"
        );
        assert!(
            msg.contains("not embedded"),
            "must state it is not embedded: {msg}"
        );
    }

    #[test]
    fn embedded_xsd_oversized_root_fails_budget() {
        use crate::schema_set::MAX_SCHEMA_BYTES;
        let key = "assets/xsd/fixture55/root.xsd";
        let bytes = vec![b'x'; MAX_SCHEMA_BYTES + 1];
        register_embedded(key, &bytes);
        let err = compile_embedded(
            "validator:camel-embedded%3Aassets%2Fxsd%2Ffixture55%2Froot.xsd?schemaType=xml",
        )
        .expect_err("a root past the aggregate byte cap must fail");
        let msg = err.to_string();
        assert!(msg.contains("8 MiB"), "must name the aggregate cap: {msg}");
        assert!(
            !msg.contains("well-formed"),
            "the budget error must win before XML parsing: {msg}"
        );
    }

    #[test]
    fn embedded_xsd_alias_byte_overflow_fails_aggregate() {
        let key = "assets/xsd/fixture56/root.xsd";
        let mut bytes = F51_STANDALONE_XSD.as_bytes().to_vec();
        bytes.resize(5 * 1024 * 1024, b' ');
        register_embedded(key, &bytes);
        let aliases = alias_query(&[("urn:t", "root.xsd")]);
        let uri = format!(
            "validator:camel-embedded%3Aassets%2Fxsd%2Ffixture56%2Froot.xsd\
             ?schemaType=xml&schemaAliases={aliases}"
        );
        let err = compile_embedded(&uri)
            .expect_err("root + alias bytes past 8 MiB must fail the aggregate budget");
        let msg = err.to_string();
        assert!(msg.contains("8 MiB"), "must name the aggregate cap: {msg}");
    }

    #[test]
    fn embedded_xsd_alias_count_overflow_fails() {
        use crate::schema_set::MAX_SCHEMA_ALIASES;
        let key = "assets/xsd/fixture57/root.xsd";
        register_embedded(key, F51_STANDALONE_XSD.as_bytes());
        let mut cfg = ValidatorConfig::from_uri(
            "validator:camel-embedded%3Aassets%2Fxsd%2Ffixture57%2Froot.xsd?schemaType=xml",
        )
        .expect("URI parses");
        for i in 0..=MAX_SCHEMA_ALIASES {
            cfg.schema_aliases
                .insert(format!("urn:a{i}"), "root.xsd".to_string());
        }
        let err = CompiledValidator::compile(&cfg, Arc::new(MockBridge { register_err: None }))
            .expect_err("an over-cap alias map must fail");
        let msg = err.to_string();
        assert!(msg.contains("128"), "must name the alias cap: {msg}");
    }

    #[test]
    fn embedded_xsd_unknown_reference_fails_closed() {
        let err = compile_embedded(
            "validator:camel-embedded:assets/xsd/never-registered-51?schemaType=xml",
        )
        .expect_err("an unregistered embedded reference must fail closed");
        let msg = err.to_string();
        assert!(
            msg.contains("never-registered-51"),
            "must name the full reference: {msg}"
        );
        assert!(
            msg.contains("no embedded schema registered"),
            "must use the fail-closed wording: {msg}"
        );
    }

    #[tokio::test]
    async fn embedded_json_schema_resolves_from_registry() {
        let key = "assets/fixture51/schema.json";
        register_embedded(
            key,
            br#"{"type":"object","properties":{"id":{"type":"integer"}},"required":["id"]}"#,
        );
        let compiled = compile_embedded(
            "validator:camel-embedded%3Aassets%2Ffixture51%2Fschema.json?schemaType=json",
        )
        .expect("a registered JSON schema must compile");
        assert!(
            compiled
                .validate(&Body::Text(r#"{"id":1}"#.into()))
                .await
                .is_ok()
        );
        assert!(
            compiled
                .validate(&Body::Text(r#"{"id":"x"}"#.into()))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn embedded_yaml_schema_resolves_from_registry() {
        let key = "assets/fixture51/schema.yaml";
        register_embedded(
            key,
            b"type: object\nproperties:\n  id:\n    type: integer\n",
        );
        let compiled = compile_embedded(
            "validator:camel-embedded%3Aassets%2Ffixture51%2Fschema.yaml?schemaType=yaml",
        )
        .expect("a registered YAML schema must compile");
        assert!(matches!(compiled, CompiledValidator::Yaml { .. }));
        assert!(compiled.validate(&Body::Text("id: 1".into())).await.is_ok());
    }

    /// Embedded schemas are inline: they have no filesystem root, so a sibling
    /// `$ref` must fail closed with the named rejection (not a parse error).
    #[test]
    fn embedded_schema_external_ref_rejected() {
        let key = "assets/fixture58/schema.json";
        register_embedded(
            key,
            br#"{"type":"object","properties":{"x":{"$ref":"defs.json#/$defs/x"}}}"#,
        );
        let err = compile_embedded(
            "validator:camel-embedded%3Aassets%2Ffixture58%2Fschema.json?schemaType=json",
        )
        .expect_err("an external $ref must be rejected for an embedded schema");
        let msg = err.to_string();
        assert!(
            msg.contains("defs.json"),
            "must name the rejected reference: {msg}"
        );
        assert!(msg.contains("rejected"), "must name the rejection: {msg}");
    }

    /// Protobuf seam: the validator forwards a shared-registry registration
    /// into the proto compiler's own registry and compiles the canonical
    /// reference. A prefixed-but-unregistered proto fails closed with the same
    /// "no embedded schema registered" wording.
    #[tokio::test]
    async fn embedded_protobuf_resolves_from_registry() {
        let key = "assets/fixture51/person.proto";
        register_embedded(key, PROTO_PERSON);
        let compiled = compile_embedded(
            "validator:camel-embedded%3Aassets%2Ffixture51%2Fperson.proto\
             ?schemaType=protobuf&message=test.v1.Person",
        )
        .expect("a registered proto must compile");
        assert!(matches!(compiled, CompiledValidator::Protobuf { .. }));
        assert!(
            compiled
                .validate(&Body::Text(r#"{"name":"Ann"}"#.to_string()))
                .await
                .is_ok()
        );

        let err = compile_embedded(
            "validator:camel-embedded%3Aassets%2Ffixture51%2Fnever.proto\
             ?schemaType=protobuf&message=test.v1.Person",
        )
        .expect_err("an unregistered embedded proto must fail closed");
        assert!(
            err.to_string().contains("no embedded schema registered"),
            "must use the fail-closed wording: {err}"
        );
    }
}
