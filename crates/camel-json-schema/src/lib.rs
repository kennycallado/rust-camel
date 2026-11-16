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

/// A compiled JSON Schema validator.
#[derive(Debug)]
pub struct CompiledSchema {
    validator: jsonschema::Validator,
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
        Ok(validator) => Ok(CompiledSchema { validator }),
        Err(error) => Err(
            take_captured(capture).unwrap_or_else(|| SchemaError::Compile {
                message: error.to_string(),
            }),
        ),
    }
}
