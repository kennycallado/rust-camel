//! Confined JSON Schema compilation for rust-camel.
//!
//! This crate is the only place in the workspace that compiles JSON Schema
//! `$ref`s (ADR-0100). Confinement means compilation never fetches anything
//! a schema did not ship with:
//!
//! - Inline sources ([`SchemaSource::Inline`]) resolve only same-document
//!   fragment references. Every external reference fails closed with a named
//!   [`SchemaError`]: `file` and the synthetic inline-base scheme are
//!   [`SchemaError::ExternalRefRejected`], every other scheme is
//!   [`SchemaError::RemoteRefRejected`].
//! - File sources ([`SchemaSource::File`]) are parsed by [`SchemaSyntax`] and
//!   compiled with their canonical path as base URI. Every external reference
//!   is read by a confined retriever that percent-decodes the target, resolves
//!   it with `std::fs::canonicalize`, and requires the result to stay inside
//!   the root schema's directory as a regular file. Remote schemes are
//!   rejected before any I/O.
//!
//! Remote schemes are never fetched: JSON Schema references resolve from
//! local files only (ADR-0100). Draft selection is never forced — the root
//! document's `$schema` selects the draft, and 2020-12 is the default.

mod error;
mod retriever;

pub use error::SchemaError;

use retriever::Budget;
use retriever::ConfinedFileRetriever;
use retriever::read_bounded;
use std::collections::BTreeSet;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;

use noyalib::compat::serde_yaml as serde_yml;
use serde_json::Value;

/// Opaque inline base URI.
///
/// Inline sources have no filesystem root. The synthetic scheme keeps plain
/// `#` fragment references resolving from the root document while making any
/// external reference visible to the fail-closed retriever.
const INLINE_BASE_URI: &str = "inline-schema:/";

/// Scheme of [`INLINE_BASE_URI`]; external references rebased onto it are
/// classified as file-local rejections rather than remote ones.
const INLINE_SCHEME: &str = "inline-schema";

/// Largest schema file one compilation reads, in bytes (ADR-0100).
///
/// Reads are bounded, never metadata-only: the retriever asks for
/// `MAX_FILE_BYTES + 1` bytes and rejects the file once the extra byte
/// arrives, so a file that grows after any earlier check cannot slip through.
pub const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;

/// Largest number of distinct schema resources one compilation may retrieve,
/// counting the root document as resource 1 (ADR-0100).
pub const MAX_RESOURCES: usize = 256;

/// Largest total number of schema bytes one compilation may retrieve,
/// counting the root document's bytes (ADR-0100).
pub const MAX_TOTAL_BYTES: u64 = 32 * 1024 * 1024;

/// Where the root schema document comes from.
#[derive(Debug, Clone)]
pub enum SchemaSource {
    /// A schema file on disk. External `$ref`s resolve relative to its
    /// directory under the confinement rules of ADR-0100.
    File(PathBuf),
    /// An already-parsed schema document. External `$ref`s are rejected.
    Inline(Value),
}

/// Parser used for the root schema document of a [`SchemaSource::File`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaSyntax {
    /// Follow the file extension: `.yaml`/`.yml` are YAML, everything else is
    /// JSON.
    Auto,
    /// Parse the root document as JSON.
    Json,
    /// Parse the root document as YAML.
    Yaml,
}

/// How the `format` keyword is handled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormatMode {
    /// Leave the decision to upstream: draft-07 and format-assertion
    /// vocabularies enforce formats, 2020-12 without them annotates. This is
    /// exactly what `jsonschema::validator_for` does.
    DraftDefault,
    /// Always validate formats.
    Enforce,
    /// Never validate formats; treat them as annotations.
    Annotate,
}

/// One validation failure: the instance location and the error text.
///
/// Consumers format it as `"{message} at {instance_path}"`, which is
/// byte-identical to the pre-ADR-0100 validator message shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    /// JSON pointer to the failing value inside the instance.
    pub instance_path: String,
    /// Human-readable description of the failure.
    pub message: String,
}

/// One validation failure reduced to what is safe to project to a caller.
///
/// Unlike [`Violation`], the message is replaced by the upstream keyword and
/// the instance path is redacted: a segment that is not a declared property
/// name or an array index becomes a placeholder, so instance member names and
/// values never reach the projection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedactedViolation {
    /// Upstream keyword that failed, for example `falseSchema` or `type`.
    pub keyword: String,
    /// Instance pointer with undeclared member names replaced by `<prop>`.
    pub instance_path: String,
}

/// Placeholder that replaces a member name the schema does not declare.
const REDACTED_PROPERTY: &str = "<prop>";

/// Depth cap for the declared-property walk over the root schema document.
///
/// The schema is trusted operator configuration (ADR-0032), so the walk only
/// needs to be bounded, not adversarial-proof.
const MAX_SCHEMA_DEPTH: usize = 32;

/// A compiled JSON Schema validator.
#[derive(Debug)]
pub struct CompiledSchema {
    validator: jsonschema::Validator,
    /// Names declared under `properties` anywhere in the root document,
    /// resolved once at compile time for redaction.
    declared_properties: BTreeSet<String>,
}

impl CompiledSchema {
    /// Validate `instance` and collect every violation.
    ///
    /// The returned order is the upstream validator traversal order.
    #[must_use]
    pub fn violations(&self, instance: &Value) -> Vec<Violation> {
        self.validator
            .iter_errors(instance)
            .map(|error| Violation {
                instance_path: error.instance_path().to_string(),
                message: error.to_string(),
            })
            .collect()
    }

    /// Validate `instance` and collect redacted violations.
    ///
    /// Each upstream [`jsonschema::ValidationError`] is reduced to the failing
    /// keyword and an instance pointer in which only paths the schema declares
    /// (`properties` names at any depth) and array indices survive; any other
    /// member name becomes `<prop>`. The returned order is the upstream
    /// validator traversal order.
    #[must_use]
    pub fn redacted_violations(&self, instance: &Value) -> Vec<RedactedViolation> {
        self.validator
            .iter_errors(instance)
            .map(|error| RedactedViolation {
                keyword: error.kind().keyword().to_string(),
                instance_path: redact_pointer(
                    &error.instance_path().to_string(),
                    instance,
                    &self.declared_properties,
                ),
            })
            .collect()
    }
}

/// Collect every key of every `properties` object in `schema` into `out`.
///
/// Objects and arrays are both traversed (subschemas live under keys such as
/// `items` and `prefixItems`); other values carry no properties and are
/// skipped. The walk stops at [`MAX_SCHEMA_DEPTH`].
fn declared_property_names(schema: &Value, out: &mut BTreeSet<String>) {
    walk_declared(schema, out, 0);
}

/// Recursive half of [`declared_property_names`].
fn walk_declared(schema: &Value, out: &mut BTreeSet<String>, depth: usize) {
    if depth >= MAX_SCHEMA_DEPTH {
        return;
    }
    match schema {
        Value::Object(map) => {
            if let Some(Value::Object(properties)) = map.get("properties") {
                out.extend(properties.keys().cloned());
            }
            for value in map.values() {
                walk_declared(value, out, depth + 1);
            }
        }
        Value::Array(items) => {
            for item in items {
                walk_declared(item, out, depth + 1);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}

/// Decode one RFC 6901 pointer segment (`~1` then `~0`).
fn decode_segment(segment: &str) -> String {
    segment.replace("~1", "/").replace("~0", "~")
}

/// Redact an instance pointer, keeping only declared names and array indices.
///
/// The walk is instance-aware: a segment survives when the node it addresses
/// is an array and the segment is an in-bounds index, or when the segment
/// decodes to a name the root schema declares under `properties`. Everything
/// else renders as `REDACTED_PROPERTY` and stops the descent, so a malformed
/// pointer can only over-redact.
///
/// `~0`/`~1` unescaping is applied ONLY for the declared-set match; output
/// preserves the original escaped spelling. Over-redaction is the safe
/// direction.
fn redact_pointer(path: &str, instance: &Value, declared: &BTreeSet<String>) -> String {
    if path.is_empty() {
        return String::new();
    }

    let mut segments: Vec<&str> = Vec::new();
    let mut node = Some(instance);
    for segment in path.split('/').skip(1) {
        match node {
            Some(Value::Array(items)) => {
                let indexed = segment
                    .parse::<usize>()
                    .ok()
                    .and_then(|index| items.get(index).map(|element| (segment, element)));
                match indexed {
                    Some((original, element)) => {
                        segments.push(original);
                        node = Some(element);
                    }
                    None => {
                        segments.push(REDACTED_PROPERTY);
                        node = None;
                    }
                }
            }
            Some(Value::Object(map)) => {
                let key = decode_segment(segment);
                segments.push(kept_segment(segment, &key, declared));
                node = map.get(&key);
            }
            Some(Value::Bool(_) | Value::Number(_) | Value::String(_) | Value::Null) | None => {
                let key = decode_segment(segment);
                segments.push(kept_segment(segment, &key, declared));
                node = None;
            }
        }
    }

    let mut pointer = String::new();
    for segment in segments {
        pointer.push('/');
        pointer.push_str(segment);
    }
    pointer
}

/// Keep `segment` verbatim when `key` is declared, otherwise mask it.
fn kept_segment<'a>(segment: &'a str, key: &str, declared: &BTreeSet<String>) -> &'a str {
    if declared.contains(key) {
        segment
    } else {
        REDACTED_PROPERTY
    }
}

/// Compile `source` into a reusable validator.
///
/// The root document is passed through unchanged, so its `$schema` selects
/// the draft and 2020-12 is used when absent. `syntax` applies to file roots
/// only; for inline roots it is ignored except that [`SchemaSyntax::Yaml`] is
/// rejected because a parsed [`Value`] cannot be YAML-parsed.
///
/// # Errors
///
/// Returns [`SchemaError`] when the root document cannot be read or parsed,
/// when compilation fails, or when a reference is rejected by the confinement
/// rules of ADR-0100.
pub fn compile(
    source: &SchemaSource,
    syntax: SchemaSyntax,
    format: FormatMode,
) -> Result<CompiledSchema, SchemaError> {
    match source {
        SchemaSource::File(path) => compile_file(path, syntax, format),
        SchemaSource::Inline(value) => compile_inline(value, syntax, format),
    }
}

/// Shared slot that stores the typed retrieval failure before it is boxed.
///
/// Upstream wraps retrieval failures in `referencing::Error::Unretrievable`
/// and exposes no `source()` chain on the build error, so the boxed value is
/// lost. The retriever stores the [`SchemaError`] here first; [`compile`]
/// drains the slot when the build fails and returns it verbatim.
type CaptureSlot = Arc<Mutex<Option<SchemaError>>>;

/// Take the typed retrieval failure captured during a failed build, if any.
fn take_captured(capture: &CaptureSlot) -> Option<SchemaError> {
    capture.lock().ok().and_then(|mut captured| captured.take())
}

/// Fail-closed retriever for inline sources.
///
/// Upstream resolves relative references against the base URI before this
/// method is called, so `defs.json` against [`INLINE_BASE_URI`] arrives as
/// `inline-schema:/defs.json` and is classified by its scheme.
struct InlineFailClosedRetriever {
    capture: CaptureSlot,
}

impl jsonschema::Retrieve for InlineFailClosedRetriever {
    fn retrieve(
        &self,
        uri: &jsonschema::Uri<String>,
    ) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
        let error = classify_inline_reference(uri);
        if let Ok(mut captured) = self.capture.lock() {
            *captured = Some(error.clone());
        }
        Err(Box::new(error))
    }
}

/// Classify a resolved inline reference into its named rejection.
///
/// Inline sources have no filesystem root, so [`INLINE_BASE_URI`] doubles as
/// the `root` field: it is the boundary the reference escaped.
fn classify_inline_reference(uri: &jsonschema::Uri<String>) -> SchemaError {
    let resolved = uri.as_str().to_string();
    match uri.scheme().as_str() {
        INLINE_SCHEME | "file" => SchemaError::ExternalRefRejected {
            uri: resolved,
            root: INLINE_BASE_URI.to_string(),
        },
        _ => SchemaError::RemoteRefRejected { uri: resolved },
    }
}

/// Compile an already-parsed inline document with inline-only retrieval.
fn compile_inline(
    schema: &Value,
    syntax: SchemaSyntax,
    format: FormatMode,
) -> Result<CompiledSchema, SchemaError> {
    if syntax == SchemaSyntax::Yaml {
        return Err(SchemaError::Compile {
            message: "inline JSON Schema sources cannot be parsed as YAML".to_string(),
        });
    }

    let capture: CaptureSlot = Arc::new(Mutex::new(None));
    let options = jsonschema::options()
        .with_base_uri(INLINE_BASE_URI)
        .with_retriever(InlineFailClosedRetriever {
            capture: Arc::clone(&capture),
        });
    finish(options, schema, &capture, format)
}

/// Compile a file-backed root document.
///
/// The root is canonicalized and its parent directory becomes the confined
/// resolution root. Every external reference — including one rebased by a
/// `$id` — is resolved by [`ConfinedFileRetriever`], which rejects remote
/// schemes and anything whose canonical path leaves `root_dir`.
fn compile_file(
    path: &Path,
    syntax: SchemaSyntax,
    format: FormatMode,
) -> Result<CompiledSchema, SchemaError> {
    let canonical = std::fs::canonicalize(path).map_err(|error| SchemaError::Open {
        path: path.display().to_string(),
        message: error.to_string(),
    })?;
    // Resolution root and base URI derive from the same canonical path so the
    // containment check matches what upstream resolves references against.
    let root_dir = canonical
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| canonical.clone());

    let bytes = read_bounded(&canonical, MAX_FILE_BYTES)
        .map_err(|error| root_scope(error, &path.display().to_string()))?;
    let schema = parse_document(path, &bytes, syntax)?;

    // The root is resource 1 and its bytes count toward the total budget.
    let mut budget = Budget::default();
    budget.record(&canonical, bytes.len() as u64)?;

    let capture: CaptureSlot = Arc::new(Mutex::new(None));
    let options = jsonschema::options()
        .with_base_uri(file_uri(&canonical))
        .with_retriever(ConfinedFileRetriever::new(
            root_dir,
            Arc::clone(&capture),
            budget,
        ));
    finish(options, &schema, &capture, format)
}

/// Report root-document I/O failures against the configured path.
///
/// [`read_bounded`] opens the canonical path and names it in its errors, but
/// the root document is user-configured: keep `RootTooLarge`/`Read`/`Open`
/// naming the path the caller passed, as it always has. Include-scope errors
/// are untouched and keep naming the decoded candidate path.
fn root_scope(error: SchemaError, configured: &str) -> SchemaError {
    let path = configured.to_string();
    match error {
        SchemaError::FileTooLarge { limit, .. } => SchemaError::RootTooLarge { path, limit },
        SchemaError::Read { message, .. } => SchemaError::Read { path, message },
        SchemaError::Open { message, .. } => SchemaError::Open { path, message },
        other => other,
    }
}

/// Bytes percent-encoded inside one path segment of a `file:` base URI.
///
/// Only characters that would change URI structure (`#`, `?`, `%`, …) or are
/// not URI-legal (space, `"`, `<`, `>`, backtick, braces) are escaped; `/` is
/// the segment separator, so it never reaches this set.
const URI_PATH_SEGMENT: percent_encoding::AsciiSet = percent_encoding::CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'<')
    .add(b'>')
    .add(b'`')
    .add(b'#')
    .add(b'?')
    .add(b'{')
    .add(b'}')
    .add(b'%')
    .add(b'\\');

/// Build the `file:` base URI for the canonical `path`.
///
/// Every path segment is percent-encoded, so a directory named `my schemas`
/// produces `file:///…/my%20schemas/root.json` and stays parseable upstream;
/// [`retriever::resolve_confined`] decodes it back to the real path.
fn file_uri(path: &Path) -> String {
    let mut uri = String::from("file://");
    for component in path.components() {
        if let Component::Normal(segment) = component {
            uri.push('/');
            uri.extend(percent_encoding::utf8_percent_encode(
                &segment.to_string_lossy(),
                &URI_PATH_SEGMENT,
            ));
        }
    }
    uri
}

/// Parse the root document according to `syntax`, following the extension for
/// [`SchemaSyntax::Auto`].
fn parse_document(path: &Path, bytes: &[u8], syntax: SchemaSyntax) -> Result<Value, SchemaError> {
    let display_path = path.display().to_string();
    let yaml = match syntax {
        SchemaSyntax::Auto => matches!(
            path.extension().and_then(|extension| extension.to_str()),
            Some("yaml" | "yml")
        ),
        SchemaSyntax::Json => false,
        SchemaSyntax::Yaml => true,
    };

    if yaml {
        let text = std::str::from_utf8(bytes).map_err(|error| SchemaError::Utf8 {
            path: display_path.clone(),
            message: error.to_string(),
        })?;
        let document: serde_yml::Value =
            serde_yml::from_str(text).map_err(|error| SchemaError::Parse {
                path: display_path.clone(),
                message: format!("invalid YAML schema '{display_path}': {error}"),
            })?;
        serde_json::to_value(&document).map_err(|error| SchemaError::Parse {
            path: display_path.clone(),
            message: format!("failed to convert YAML schema to JSON '{display_path}': {error}"),
        })
    } else {
        serde_json::from_slice(bytes).map_err(|error| SchemaError::Parse {
            path: display_path.clone(),
            message: format!("invalid JSON schema '{display_path}': {error}"),
        })
    }
}

/// Apply [`FormatMode`], build the validator, and recover a captured
/// retrieval failure when the build fails for a reference reason.
fn finish(
    options: jsonschema::ValidationOptions<'_>,
    schema: &Value,
    capture: &CaptureSlot,
    format: FormatMode,
) -> Result<CompiledSchema, SchemaError> {
    let options = match format {
        FormatMode::DraftDefault => options,
        FormatMode::Enforce => options.should_validate_formats(true),
        FormatMode::Annotate => options.should_validate_formats(false),
    };

    match options.build(schema) {
        Ok(validator) => {
            let mut declared_properties = BTreeSet::new();
            declared_property_names(schema, &mut declared_properties);
            Ok(CompiledSchema {
                validator,
                declared_properties,
            })
        }
        Err(error) => Err(
            take_captured(capture).unwrap_or_else(|| SchemaError::Compile {
                message: error.to_string(),
            }),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::CompiledSchema;
    use super::FormatMode;
    use super::SchemaSource;
    use super::SchemaSyntax;
    use super::compile;
    use serde_json::json;

    fn compile_inline(schema: serde_json::Value) -> CompiledSchema {
        compile(
            &SchemaSource::Inline(schema),
            SchemaSyntax::Auto,
            FormatMode::DraftDefault,
        )
        .expect("inline schema compiles")
    }

    #[test]
    fn keyword_comes_from_kind_not_schema_path() {
        let compiled = compile_inline(json!({"properties": {"value": false}}));

        let violations = compiled.redacted_violations(&json!({"value": 1}));

        assert_eq!(violations.len(), 1, "one violation: {violations:?}");
        assert_eq!(violations[0].keyword, "falseSchema");
        assert_eq!(violations[0].instance_path, "/value");
    }

    #[test]
    fn undeclared_member_segment_is_masked() {
        let compiled = compile_inline(
            json!({"type": "object", "patternProperties": {"^x-": {"type": "string"}}}),
        );

        let violations = compiled.redacted_violations(&json!({"x-secret_marker_123": 5}));

        assert_eq!(violations.len(), 1, "one violation: {violations:?}");
        assert_eq!(violations[0].keyword, "type");
        assert_eq!(violations[0].instance_path, "/<prop>");
        assert!(!violations[0].instance_path.contains("secret_marker_123"));
    }

    #[test]
    fn declared_names_and_array_indices_pass_through() {
        let compiled = compile_inline(json!({"type": "array", "items": {"type": "string"}}));

        let violations = compiled.redacted_violations(&json!(["ok", 5]));

        assert_eq!(violations.len(), 1, "one violation: {violations:?}");
        assert_eq!(violations[0].keyword, "type");
        assert_eq!(violations[0].instance_path, "/1");
    }

    #[test]
    fn numeric_object_keys_are_redacted_but_array_indices_survive() {
        let compiled = compile_inline(
            json!({"type": "object", "patternProperties": {"^[0-9]+$": {"type": "string"}}}),
        );

        let violations = compiled.redacted_violations(&json!({"123456789": 5}));

        assert_eq!(violations.len(), 1, "one violation: {violations:?}");
        assert_eq!(violations[0].instance_path, "/<prop>");
    }

    #[test]
    fn escaped_declared_segment_passes_through() {
        let compiled =
            compile_inline(json!({"type": "object", "properties": {"a/b": {"type": "string"}}}));

        let violations = compiled.redacted_violations(&json!({"a/b": 5}));

        assert_eq!(violations.len(), 1, "one violation: {violations:?}");
        assert_eq!(violations[0].keyword, "type");
        assert_eq!(
            violations[0].instance_path, "/a~1b",
            "declared name survives with its original escaped spelling"
        );
    }

    #[test]
    fn escaped_undeclared_segment_is_masked() {
        let compiled = compile_inline(
            json!({"type": "object", "patternProperties": {"~0x": {"type": "string"}}}),
        );

        let violations = compiled.redacted_violations(&json!({"~0x": 5}));

        assert_eq!(violations.len(), 1, "one violation: {violations:?}");
        assert_eq!(violations[0].keyword, "type");
        assert_eq!(violations[0].instance_path, "/<prop>");
        assert!(
            !violations[0].instance_path.contains("~0x"),
            "escaped caller-invented name must not survive: {}",
            violations[0].instance_path
        );
    }

    #[test]
    fn missing_intermediate_node_masks_remainder() {
        let compiled = compile_inline(
            json!({"type": "array", "items": {"type": "object", "properties": {"n": {"type": "string"}}}}),
        );

        let violations = compiled.redacted_violations(&json!([{"n": 1}, 7]));

        assert_eq!(violations.len(), 2, "two violations: {violations:?}");
        let paths: Vec<&str> = violations
            .iter()
            .map(|violation| violation.instance_path.as_str())
            .collect();
        assert!(
            paths.contains(&"/0/n"),
            "declared name below an array index survives verbatim: {paths:?}"
        );
        assert!(
            paths.contains(&"/1"),
            "array index survives verbatim: {paths:?}"
        );
    }

    #[test]
    fn redaction_never_contains_instance_value() {
        let compiled = compile_inline(
            json!({"type": "object", "properties": {"id": {"type": "string", "maxLength": 10}}}),
        );
        let secret = "A".repeat(1_048_576);
        let instance = json!({"id": secret});

        let violations = compiled.redacted_violations(&instance);

        assert!(!violations.is_empty(), "the oversized value fails");
        let needle = "A".repeat(16);
        for violation in &violations {
            assert!(!violation.keyword.contains(&needle), "keyword leaks");
            assert!(!violation.instance_path.contains(&needle), "path leaks");
        }
    }
}
