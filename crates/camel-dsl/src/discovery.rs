//! Route discovery module - finds and loads routes from YAML/JSON files using glob patterns.

use camel_api::template::{RouteTemplateSpec, TemplateError, TemplatedRouteSpec};
use camel_core::route::RouteDefinition;
use glob::glob;
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::hash::{Hash, Hasher};
use std::io;
use std::path::Path;

use crate::embedded_store::{STORE_SCHEMA, StoreEntryKind, StoreError, VirtualDocumentStore};
use crate::env_interpolation::{
    ProvenancePath, interpolate_env_with, interpolate_yaml_source_with_provenance,
};
use crate::json::parse_json_with_threshold_and_security;
use crate::model::SecurityCompileContext;
use crate::template::materializer::materialize_and_compile;
use crate::virtual_config::build_virtual_config;
use crate::yaml::parse_yaml_with_threshold_and_security;

/// Errors that can occur during route discovery.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum DiscoveryError {
    /// Invalid glob pattern.
    #[error("Glob pattern error: {0}")]
    GlobPattern(#[from] glob::PatternError),

    /// Error accessing file while iterating glob.
    #[error("Glob error accessing {path}: {source}")]
    GlobAccess { path: String, source: io::Error },

    /// Error reading a file.
    #[error("IO error reading {path}: {source}")]
    Io { path: String, source: io::Error },

    /// Error parsing YAML content.
    #[error("YAML parse error in {path}: {error}")]
    Yaml { path: String, error: String },

    /// Environment variable not set during interpolation.
    #[error("Environment variable '{var_name}' not set (required by {path})")]
    Env { path: String, var_name: String },

    /// Error parsing JSON content.
    #[error("JSON parse error in {path}: {error}")]
    Json { path: String, error: String },

    /// File has an unsupported extension (not .yaml, .yml, or .json).
    #[error("Unsupported file extension '{extension}' in {path}")]
    UnsupportedExtension { path: String, extension: String },

    /// JSON file matched by a broad glob pattern requires an explicit .json pattern.
    #[error(
        "JSON file {path} matched by broad pattern '{pattern}' — use an explicit .json glob like 'routes/*.json'"
    )]
    JsonRequiresExplicitPattern { path: String, pattern: String },

    /// Route file matched by a literal pattern uses a reserved document
    /// suffix (`.test.yaml`/`.test.yml` names a camel test document;
    /// `.job.yaml`/`.job.yml` names a camel job document) — not a route.
    #[error(
        "Route file {path} uses a reserved document suffix ('.test.yaml'/'.test.yml' names a camel test document; '.job.yaml'/'.job.yml' names a camel job document). Run it with 'camel test {path}' or 'camel job {path}', or rename it if it is a route."
    )]
    ReservedDocumentSuffix { path: String },

    /// A route id was produced more than once across regular + materialized routes.
    #[error("Duplicate route id '{route_id}' in {path}")]
    DuplicateRouteId { path: String, route_id: String },

    /// Template parsing or materialization failed (invalid body, missing params, etc.).
    #[error("Template error in {path}: {source}")]
    MaterializationFailed {
        path: String,
        #[source]
        source: TemplateError,
    },

    /// One or more template materialization failures, aggregated across files.
    /// Rendered as a multi-line report listing every failure with its path.
    #[error(
        "template materialization failed:\n{}",
        failures
            .iter()
            .map(|f| format!("  {f}"))
            .collect::<Vec<_>>()
            .join("\n")
    )]
    MaterializationFailures {
        failures: Vec<MaterializationFailure>,
    },

    /// Duplicate template id across files, or invalid template spec in file.
    #[error("Template error in {path}: {error}")]
    TemplateSpec { path: String, error: String },

    /// Embedded virtual-store failure: unsupported store schema or a
    /// reference rule violation, named by [`StoreError`] (missing
    /// entry-point/configuration/source-plan reference, or a reference
    /// naming an entry of the wrong document kind).
    #[error("Virtual store error: {0}")]
    VirtualStore(#[from] StoreError),

    /// An embedded configuration document (config, include, or profile
    /// fragment) is not valid TOML, is not valid UTF-8, or breaks an
    /// assembly rule (duplicate configuration document, malformed
    /// profile fragment path, unknown profile selection).
    #[error("Malformed embedded configuration in {path}: {error}")]
    MalformedVirtualConfig { path: String, error: String },

    /// A source-plan reference names content that cannot be a route
    /// document (non-UTF-8 bytes). Missing or wrong-kind references
    /// surface as [`DiscoveryError::VirtualStore`] instead.
    #[error("Invalid virtual route source plan entry {path}: {reason}")]
    InvalidVirtualSourcePlan { path: String, reason: String },
}

/// A single template materialization failure, carrying the file path it
/// originated from — Pass 2 iterates specs collected across multiple files.
#[derive(Debug, Clone)]
pub struct MaterializationFailure {
    /// Path of the file declaring the templated route spec.
    pub path: String,
    /// The referenced template id.
    pub template_ref: String,
    /// Optional explicit route id of the failing instance.
    pub route_id: Option<String>,
    /// The classified template error.
    pub error: TemplateError,
}

impl std::fmt::Display for MaterializationFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} (template '{}'): {}",
            self.path, self.template_ref, self.error
        )
    }
}

/// Read a file with a size cap. Stats first, rejects if too large.
fn read_file_capped(path: &Path) -> Result<String, DiscoveryError> {
    let metadata = fs::metadata(path).map_err(|e| DiscoveryError::Io {
        path: path.to_string_lossy().to_string(),
        source: e,
    })?;
    if metadata.len() > crate::MAX_ROUTE_FILE_SIZE {
        return Err(DiscoveryError::Io {
            path: path.to_string_lossy().to_string(),
            source: io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "Route file `{}` is {} bytes, exceeds max {} bytes",
                    path.display(),
                    metadata.len(),
                    crate::MAX_ROUTE_FILE_SIZE
                ),
            ),
        });
    }
    fs::read_to_string(path).map_err(|e| DiscoveryError::Io {
        path: path.to_string_lossy().to_string(),
        source: e,
    })
}

/// Returns true if the glob pattern explicitly targets `.json` files.
///
/// Only patterns whose **file target extension** is `.json` (case-insensitive) return true.
/// A `.json` segment appearing only in a directory path (e.g. `config/.json/routes/*`)
/// does **not** authorize JSON loading.
///
/// Public since multidoc Task 1.2: `camel compile` resolves route-file
/// patterns at compile time and must apply the identical JSON
/// authorization rule as filesystem discovery (additive change, no
/// behavior difference for existing callers).
pub fn pattern_targets_json(pattern: &str) -> bool {
    let lower = pattern.to_lowercase();
    // Extract the last path segment (the file/target portion) and check if it ends with .json
    lower
        .rsplit('/')
        .next()
        .is_some_and(|last_segment| last_segment.ends_with(".json"))
}

/// Returns true if the file name ends with the reserved `.test.yaml` or
/// `.test.yml` suffix. Such files name camel test documents (the
/// `camel test` family), not routes.
pub fn is_test_document(path: &Path) -> bool {
    path.file_name().is_some_and(|name| {
        let name = name.to_string_lossy();
        name.ends_with(".test.yaml") || name.ends_with(".test.yml")
    })
}

/// Returns true if the file name ends with the reserved `.job.yaml` or
/// `.job.yml` suffix. Such files name camel job documents (the
/// `camel job` family), not routes.
pub fn is_job_document(path: &Path) -> bool {
    path.file_name().is_some_and(|name| {
        let name = name.to_string_lossy();
        name.ends_with(".job.yaml") || name.ends_with(".job.yml")
    })
}

/// Returns true if the file name ends with any reserved document suffix:
/// `.test.yaml`/`.test.yml` (owned by `camel test`) or
/// `.job.yaml`/`.job.yml` (owned by `camel job`). Such files are never
/// routes; route discovery skips them under wildcard globs and errors on
/// literal naming.
pub fn is_reserved_document(path: &Path) -> bool {
    is_test_document(path) || is_job_document(path)
}

/// Returns true if the glob pattern contains no metacharacters (`* ? [ ] { }`),
/// i.e. it names exactly one literal path rather than a set of paths.
/// A lone `]` or `}` also counts as a metacharacter, so a pathologically
/// named literal is skipped under wildcard rules rather than erroring
/// (accepted behavior).
///
/// Public since multidoc Task 1.2: `camel compile` distinguishes literal
/// from wildcard route-file patterns to decide whether an empty match is
/// a missing source or an empty set (additive change, no behavior
/// difference for existing callers).
pub fn pattern_is_literal(pattern: &str) -> bool {
    !pattern.contains(['*', '?', '[', ']', '{', '}'])
}

/// Extracts the lowercase file extension from a path, if any.
fn file_extension(path: &Path) -> Option<String> {
    path.extension()
        .map(|ext| ext.to_string_lossy().to_lowercase())
}

/// Discovers routes from YAML/JSON files matching the given glob patterns.
///
/// # Arguments
/// * `patterns` - Slice of glob patterns to match route definition files
///
/// # Returns
/// A vector of all discovered route definitions, or an error.
///
/// # Supported formats
/// - `.yaml` / `.yml` — parsed as YAML
/// - `.json` — parsed as JSON, but only when the source pattern explicitly targets `.json`
///
/// # Example
/// ```ignore
/// let routes = discover_routes(&["routes/*.yaml".to_string(), "routes/*.json".to_string()])?;
/// ```
pub fn discover_routes(patterns: &[String]) -> Result<Vec<RouteDefinition>, DiscoveryError> {
    discover_routes_inner(patterns, None, None, None)
}

/// Discovers routes with a custom stream-cache threshold.
///
/// Same as [`discover_routes`] but uses the given `stream_cache_threshold`
/// instead of the default when compiling routes.
pub fn discover_routes_with_threshold(
    patterns: &[String],
    stream_cache_threshold: usize,
) -> Result<Vec<RouteDefinition>, DiscoveryError> {
    discover_routes_inner(patterns, Some(stream_cache_threshold), None, None)
}

/// Discovers routes with a custom stream-cache threshold and security compile context.
///
/// Same as [`discover_routes_with_threshold`] but also passes a
/// [`SecurityCompileContext`] through to route compilation, allowing
/// permission evaluators and security policy registries to be resolved
/// during DSL compilation.
pub fn discover_routes_with_threshold_and_security(
    patterns: &[String],
    stream_cache_threshold: usize,
    security_ctx: SecurityCompileContext,
) -> Result<Vec<RouteDefinition>, DiscoveryError> {
    discover_routes_inner(
        patterns,
        Some(stream_cache_threshold),
        Some(security_ctx),
        None,
    )
}

/// Discovers routes with a custom stream-cache threshold, a security
/// compile context, and an injected environment lookup.
///
/// Same as [`discover_routes_with_threshold_and_security`] but every
/// `${env:NAME}` placeholder resolves through `env_lookup` instead of the
/// process environment. Hermetic callers (the integration tier) inject
/// their layered environment here; the process environment is never
/// consulted through this entry.
pub fn discover_routes_with_threshold_security_and_env(
    patterns: &[String],
    stream_cache_threshold: usize,
    security_ctx: SecurityCompileContext,
    env_lookup: &dyn Fn(&str) -> Option<String>,
) -> Result<Vec<RouteDefinition>, DiscoveryError> {
    discover_routes_inner(
        patterns,
        Some(stream_cache_threshold),
        Some(security_ctx),
        Some(env_lookup),
    )
}

/// Explicit document kind carried by an embedded document (cli-compile).
///
/// The compiler records the kind in the artifact trailer and the runtime
/// passes it back so the discovery seam knows which document family the
/// text belongs to. Route and job documents share the same route-DSL parse
/// pipeline; the kind steers reserved-document validation and the caller's
/// lifecycle choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbeddedDocumentKind {
    /// A route document (`camel run` artifact).
    Route,
    /// A job document (`camel job` artifact).
    Job,
}

/// Per-document gates shared by every embedded seam (single embedded
/// text and the virtual store): the reserved-document check for route
/// documents and the fail-closed extension check, both naming the
/// virtual `compiled://<source_name>` identity. Returns the accepted
/// lowercase extension for the shared parse pass.
fn embedded_document_gates(
    source_name: &str,
    kind: EmbeddedDocumentKind,
) -> Result<String, DiscoveryError> {
    let path_str = format!("compiled://{source_name}");

    // Reserved-document gate (route kind only): `.test.yaml`/`.job.yaml`
    // names belong to `camel test`/`camel job`, never to route discovery —
    // the same fail-closed rule as a literal filesystem pattern. The job
    // kind keeps the suffix contract of the CLI job-document parser, which
    // runs upstream of this seam.
    if matches!(kind, EmbeddedDocumentKind::Route) && is_reserved_document(Path::new(source_name)) {
        return Err(DiscoveryError::ReservedDocumentSuffix { path: path_str });
    }

    // Extension gate BEFORE parsing — the same fail-closed rule as
    // filesystem discovery: the shared parse pass only handles
    // yaml/yml/json and its fallback arm is unreachable, so an
    // extensionless or unsupported source name must fail with
    // `UnsupportedExtension` (naming the virtual `compiled://` identity),
    // never panic. The embedded seams have no glob pattern, so the JSON
    // explicit-pattern gate does not apply — a `.json` source name is
    // explicitly named in the artifact manifest.
    let ext = file_extension(Path::new(source_name));
    let Some(ext) = ext else {
        return Err(DiscoveryError::UnsupportedExtension {
            path: path_str,
            extension: String::new(),
        });
    };
    match ext.as_str() {
        "yaml" | "yml" | "json" => Ok(ext),
        other => Err(DiscoveryError::UnsupportedExtension {
            path: path_str,
            extension: other.to_string(),
        }),
    }
}

/// Discovers routes from one embedded document without touching the
/// filesystem (cli-compile).
///
/// This is the discovery seam for self-contained compiled artifacts: `text`
/// is the normalized document payload, `source_name` is the logical source
/// name recorded in the artifact manifest, and the virtual identity
/// `compiled://<source_name>` is used for the source-hash context and every
/// diagnostic. `${env:NAME}` placeholders resolve exclusively through
/// `env_lookup` — the process environment is never consulted, and no
/// config file, glob, external route file, or temporary file is read or
/// written.
///
/// The pipeline is the shared discovery path: interpolation (with YAML
/// provenance), typed env probing, template parsing and materialization,
/// provenance-preserving lowering, and reserved-document validation. The
/// parse format follows the extension of `source_name` exactly like
/// filesystem discovery (`.yaml`/`.yml`/`.json`); an extensionless or
/// unsupported source name is rejected with `UnsupportedExtension` before
/// parsing, never reaching the shared parse pass.
pub fn discover_embedded_text(
    text: &str,
    source_name: &str,
    kind: EmbeddedDocumentKind,
    env_lookup: &dyn Fn(&str) -> Option<String>,
) -> Result<Vec<RouteDefinition>, DiscoveryError> {
    let ext = embedded_document_gates(source_name, kind)?;
    let path_str = format!("compiled://{source_name}");

    let mut routes = Vec::new();
    let mut templates: HashMap<String, RouteTemplateSpec> = HashMap::new();
    let mut templated_specs: Vec<(String, TemplatedRouteSpec)> = Vec::new();
    parse_document_routes(
        text,
        &path_str,
        Some(ext.as_str()),
        None,
        None,
        env_lookup,
        &mut routes,
        &mut templates,
        &mut templated_specs,
    )?;
    materialize_templated_routes(&mut routes, &templates, &templated_specs, None, None)?;
    Ok(routes)
}

/// Result of embedded virtual-store discovery (openspec change
/// `multidoc`, Task 2.1): the merged deployment configuration plus
/// every route definition of the ordered source plan.
///
/// `config` is the merged TOML tree of the embedded
/// config/include/profile documents with filesystem-loader semantics:
/// includes are lowest priority in declaration order, the
/// configuration document sits above them, `[default]` merges with the
/// selected profile sections in selection order, and overlays replace
/// arrays (never concatenate). `${env:...}` placeholders stay raw — the
/// caller resolves them against the deployment environment during
/// `CamelConfig` deserialization, preserving camel-config's typed
/// placeholder handling. Route documents, by contrast, resolve
/// placeholders through the injected deployment lookup inside this
/// discovery.
pub struct VirtualStoreDiscovery {
    /// Merged configuration value for `CamelConfig` deserialization.
    pub config: toml::Value,
    /// Route definitions from the store's ordered source plan, in plan
    /// order.
    pub routes: Vec<RouteDefinition>,
}

/// Discovers routes and configuration from an embedded
/// [`VirtualDocumentStore`] without touching the filesystem
/// (openspec change `multidoc`, Task 2.1).
///
/// This is the multi-document seam for v2 compiled artifacts: the store
/// carries the normalized documents and a validated index; the one
/// logical entry point, the ordered configuration references, and the
/// ordered source plan are consumed strictly by index lookup. No glob
/// expansion, filesystem discovery, canonicalization, or temporary-file
/// helper is ever invoked; `${env:NAME}` placeholders in route
/// documents resolve exclusively through `env_lookup`, and every
/// diagnostic names the virtual `compiled://<logical-path>` identity of
/// its document.
///
/// Configuration is assembled first (named `MalformedVirtualConfig`
/// errors for invalid TOML or violated merge rules), then only the
/// source-plan references are parsed through the shared discovery pass
/// (interpolation with provenance, typed env probing, template parsing
/// and cross-document materialization, reserved-document validation,
/// lowering) — the same semantics as filesystem discovery, so
/// templates may be declared in one plan document and instantiated in
/// another.
pub fn discover_virtual_store(
    store: &VirtualDocumentStore,
    env_lookup: &dyn Fn(&str) -> Option<String>,
) -> Result<VirtualStoreDiscovery, DiscoveryError> {
    // Fail-closed store gates. `build`/`decode` already enforce these on
    // the canonical paths; a hand-constructed store re-validates here so
    // the named errors never depend on the construction route.
    if store.index.store_schema != STORE_SCHEMA {
        return Err(StoreError::UnsupportedStoreSchema(store.index.store_schema).into());
    }
    let entry_point = store
        .index
        .entry(&store.index.entry_point)
        .ok_or_else(|| StoreError::MissingReference(store.index.entry_point.clone()))?;
    if !matches!(
        entry_point.kind,
        StoreEntryKind::Route | StoreEntryKind::Job
    ) {
        return Err(StoreError::KindMismatch {
            path: entry_point.path.clone(),
            expected: "route or job",
            got: entry_point.kind.as_str(),
        }
        .into());
    }

    // Configuration assembly first, from the indexed config/include/
    // profile texts in index order.
    let config = build_virtual_config(store)?;

    // Route documents: only source-plan references, only by index
    // lookup. Templates and templated specs accumulate across documents
    // and materialize once at the end, exactly like the filesystem pass.
    let mut routes = Vec::new();
    let mut templates: HashMap<String, RouteTemplateSpec> = HashMap::new();
    let mut templated_specs: Vec<(String, TemplatedRouteSpec)> = Vec::new();
    for path in &store.index.source_plan.references {
        let entry = store
            .index
            .entry(path)
            .ok_or_else(|| StoreError::MissingReference(path.clone()))?;
        if entry.kind != StoreEntryKind::Route {
            return Err(StoreError::KindMismatch {
                path: path.clone(),
                expected: "route",
                got: entry.kind.as_str(),
            }
            .into());
        }
        let text =
            store
                .read_text(path)
                .ok_or_else(|| DiscoveryError::InvalidVirtualSourcePlan {
                    path: path.clone(),
                    reason: "route document is not valid UTF-8".to_string(),
                })?;
        let ext = embedded_document_gates(path, EmbeddedDocumentKind::Route)?;
        let identity = format!("compiled://{path}");
        parse_document_routes(
            text,
            &identity,
            Some(ext.as_str()),
            None,
            None,
            env_lookup,
            &mut routes,
            &mut templates,
            &mut templated_specs,
        )?;
    }
    materialize_templated_routes(&mut routes, &templates, &templated_specs, None, None)?;

    Ok(VirtualStoreDiscovery { config, routes })
}

/// Parse a `TemplateError::InvalidParameter` Display string
/// (`parameter '<name>' declared type <ty> but value '<value>' is not
/// coercible`) back into its fields, preserving the error class through
/// Config-string propagation. Returns `None` for any other shape.
fn parse_invalid_parameter_message(msg: &str) -> Option<(String, String, String)> {
    let rest = msg.strip_prefix("parameter '")?;
    let (name, rest) = rest.split_once("' ")?;
    let rest = rest.strip_prefix("declared type ")?;
    let (ty, rest) = rest.split_once(" but value '")?;
    let value = rest.strip_suffix("' is not coercible")?;
    Some((name.to_string(), ty.to_string(), value.to_string()))
}

/// Injected `${env:NAME}` resolver: returns the value for the variable
/// name, or `None` when unresolved.
type EnvLookup<'a> = &'a dyn Fn(&str) -> Option<String>;

/// Process-environment resolver — the default when no lookup is injected.
fn process_env_lookup(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

/// Env interpolation strategy per parse format (rc-ayke).
///
/// YAML and YML use the shared tree-walk-first seam
/// ([`interpolate_yaml_source`]) so YAML comments are never interpolated;
/// the YAML arm additionally returns interpolation provenance (structural
/// paths of whole-scalar substituted leaves) to feed the loader-layer
/// typed probe (`env_int_probe`). `None` provenance means "no probe": the
/// legacy whole-text splice ran (the document did not survive the YAML
/// round-trip) or the format is not YAML (JSON: the YAML-shim tree
/// re-serializes to YAML, not JSON). An unresolved variable from either
/// path surfaces as `Err(var_name)`.
fn interpolate_for_parse(
    raw: &str,
    ext: Option<&str>,
    lookup: EnvLookup<'_>,
) -> Result<(String, Option<Vec<ProvenancePath>>), String> {
    match ext {
        Some("yaml") | Some("yml") => interpolate_yaml_source_with_provenance(raw, lookup),
        _ => interpolate_env_with(raw, lookup).map(|text| (text, None)),
    }
}

fn discover_routes_inner(
    patterns: &[String],
    stream_cache_threshold: Option<usize>,
    security_ctx: Option<SecurityCompileContext>,
    env_lookup: Option<EnvLookup<'_>>,
) -> Result<Vec<RouteDefinition>, DiscoveryError> {
    let mut routes = Vec::new();
    let mut templates: HashMap<String, RouteTemplateSpec> = HashMap::new();
    // (path_str, templated_spec) — materialized after all files scanned
    let mut templated_specs: Vec<(String, TemplatedRouteSpec)> = Vec::new();

    // With an injected lookup the process environment is never read.
    let fallback_lookup: EnvLookup<'_> = &process_env_lookup;
    let lookup = env_lookup.unwrap_or(fallback_lookup);

    for pattern in patterns {
        let is_json_pattern = pattern_targets_json(pattern);
        let entries = glob(pattern)?;

        for entry in entries {
            let path = entry.map_err(|e| DiscoveryError::GlobAccess {
                path: e.path().to_string_lossy().to_string(),
                source: e.into(),
            })?;
            let path_str = path.to_string_lossy().to_string();

            // Reserved-document gate: `.test.yaml` / `.test.yml` files
            // belong to `camel test` and `.job.yaml` / `.job.yml` files
            // belong to `camel job` — neither is a route. A literal
            // pattern naming one is a user error — fail with guidance.
            // Under a wildcard, reserved documents are simply skipped
            // (never read).
            if is_reserved_document(&path) {
                if pattern_is_literal(pattern) {
                    return Err(DiscoveryError::ReservedDocumentSuffix { path: path_str });
                }
                continue;
            }

            // Validate extension and JSON explicit-pattern gate BEFORE reading or
            // interpolating — rejects must not trigger env lookups.
            let ext = file_extension(&path);
            match ext.as_deref() {
                Some("yaml") | Some("yml") => {}
                Some("json") => {
                    if !is_json_pattern {
                        return Err(DiscoveryError::JsonRequiresExplicitPattern {
                            path: path_str,
                            pattern: pattern.clone(),
                        });
                    }
                }
                Some(other) => {
                    return Err(DiscoveryError::UnsupportedExtension {
                        path: path_str,
                        extension: other.to_string(),
                    });
                }
                None => {
                    return Err(DiscoveryError::UnsupportedExtension {
                        path: path_str,
                        extension: String::new(),
                    });
                }
            }

            // Read file content (only reached for accepted extensions)
            let raw_content = read_file_capped(&path)?;

            parse_document_routes(
                &raw_content,
                &path_str,
                ext.as_deref(),
                stream_cache_threshold,
                security_ctx.as_ref(),
                lookup,
                &mut routes,
                &mut templates,
                &mut templated_specs,
            )?;
        }
    }

    materialize_templated_routes(
        &mut routes,
        &templates,
        &templated_specs,
        stream_cache_threshold,
        security_ctx.as_ref(),
    )?;

    Ok(routes)
}

/// Shared per-document parse pass (filesystem discovery and the embedded
/// text seam): hash the raw content, interpolate `${env:NAME}` through
/// `lookup` (with YAML provenance for the typed probe), then parse routes,
/// templates, and templated route specs into the caller's accumulators so
/// both entry points keep identical discovery semantics.
#[allow(clippy::too_many_arguments)]
fn parse_document_routes(
    raw_content: &str,
    path_str: &str,
    ext: Option<&str>,
    stream_cache_threshold: Option<usize>,
    security_ctx: Option<&SecurityCompileContext>,
    lookup: EnvLookup<'_>,
    routes: &mut Vec<RouteDefinition>,
    templates: &mut HashMap<String, RouteTemplateSpec>,
    templated_specs: &mut Vec<(String, TemplatedRouteSpec)>,
) -> Result<(), DiscoveryError> {
    // Source hash is based on raw content before env interpolation
    let mut hasher = DefaultHasher::new();
    raw_content.hash(&mut hasher);
    let source_hash = hasher.finish();

    // Env interpolation happens before parsing for both YAML and JSON.
    let (content, provenance) =
        interpolate_for_parse(raw_content, ext, lookup).map_err(|var_name| {
            DiscoveryError::Env {
                path: path_str.to_string(),
                var_name,
            }
        })?;

    // Parse based on extension — collect templates, templated specs, and regular routes
    match ext {
        Some("yaml") | Some("yml") => {
            // Typed probe over interpolation provenance
            // (env-int-placeholder-typing): pass 1 parses through a
            // QUIET twin of the threshold/security parser
            // (speculative probe attempts must not log); on final
            // failure the original text re-runs through the LOGGING
            // parser exactly once (today's error log and error
            // text), then maps through the existing error path.
            let threshold = stream_cache_threshold
                .unwrap_or(camel_api::stream_cache::DEFAULT_STREAM_CACHE_THRESHOLD);
            let quiet_probe_parse = |text: &str| {
                crate::yaml::parse_yaml_with_threshold_and_security_quiet(
                    text,
                    threshold,
                    security_ctx.cloned().unwrap_or_default(),
                )
            };
            let file_routes = crate::env_int_probe::parse_with_probe(
                &content,
                provenance.as_deref(),
                quiet_probe_parse,
            )
            .or_else(|_| {
                parse_yaml_with_threshold_and_security(
                    &content,
                    threshold,
                    security_ctx.cloned().unwrap_or_default(),
                )
            })
            .map_err(|e| DiscoveryError::Yaml {
                path: path_str.to_string(),
                error: e.to_string(),
            })?;
            for route in file_routes {
                routes.push(route.with_source_hash(source_hash));
            }

            // Parse templates
            let tpls = crate::template::yaml::parse_yaml_templates(&content).map_err(|e| {
                DiscoveryError::MaterializationFailed {
                    path: path_str.to_string(),
                    source: e,
                }
            })?;
            for tpl in tpls {
                if templates.contains_key(&tpl.id) {
                    return Err(DiscoveryError::TemplateSpec {
                        path: path_str.to_string(),
                        error: format!("duplicate template id '{}'", tpl.id),
                    });
                }
                templates.insert(tpl.id.clone(), tpl);
            }

            // Parse templated route specs for later materialization
            let specs =
                crate::template::yaml::parse_yaml_templated_routes(&content).map_err(|e| {
                    DiscoveryError::MaterializationFailed {
                        path: path_str.to_string(),
                        source: e,
                    }
                })?;
            for spec in specs {
                templated_specs.push((path_str.to_string(), spec));
            }
        }
        Some("json") => {
            // Parse regular routes
            let file_routes = parse_json_with_threshold_and_security(
                &content,
                stream_cache_threshold
                    .unwrap_or(camel_api::stream_cache::DEFAULT_STREAM_CACHE_THRESHOLD),
                security_ctx.cloned().unwrap_or_default(),
            )
            .map_err(|e| DiscoveryError::Json {
                path: path_str.to_string(),
                error: e.to_string(),
            })?;
            for route in file_routes {
                routes.push(route.with_source_hash(source_hash));
            }

            // Parse templates
            let tpls = crate::template::json::parse_json_templates(&content).map_err(|e| {
                DiscoveryError::MaterializationFailed {
                    path: path_str.to_string(),
                    source: e,
                }
            })?;
            for tpl in tpls {
                if templates.contains_key(&tpl.id) {
                    return Err(DiscoveryError::TemplateSpec {
                        path: path_str.to_string(),
                        error: format!("duplicate template id '{}'", tpl.id),
                    });
                }
                templates.insert(tpl.id.clone(), tpl);
            }

            // Parse templated route specs for later materialization
            let specs =
                crate::template::json::parse_json_templated_routes(&content).map_err(|e| {
                    DiscoveryError::MaterializationFailed {
                        path: path_str.to_string(),
                        source: e,
                    }
                })?;
            for spec in specs {
                templated_specs.push((path_str.to_string(), spec));
            }
        }
        // SAFETY: Unreachable. The validation block above returns early for
        // any extension that is not yaml, yml, or json.
        _ => unreachable!(
            "validated extension should be yaml/yml/json but was: {:?}",
            ext
        ),
    }

    Ok(())
}

/// Shared materialization pass (filesystem discovery and the embedded text
/// seam): compile every collected templated route spec against the parsed
/// templates, deduplicate route ids, and attach source hashes. Failures are
/// aggregated — every spec is attempted so the caller sees the full set of
/// broken templates, not just the first one.
fn materialize_templated_routes(
    routes: &mut Vec<RouteDefinition>,
    templates: &HashMap<String, RouteTemplateSpec>,
    templated_specs: &[(String, TemplatedRouteSpec)],
    stream_cache_threshold: Option<usize>,
    security_ctx: Option<&SecurityCompileContext>,
) -> Result<(), DiscoveryError> {
    let mut seen_route_ids: HashSet<String> =
        routes.iter().map(|r| r.route_id().to_string()).collect();
    let mut failures: Vec<MaterializationFailure> = Vec::new();

    for (path_str, spec) in templated_specs {
        let Some(template) = templates.get(&spec.route_template_ref) else {
            failures.push(MaterializationFailure {
                path: path_str.clone(),
                template_ref: spec.route_template_ref.clone(),
                route_id: spec.route_id.clone(),
                error: TemplateError::NotFound(spec.route_template_ref.clone()),
            });
            continue;
        };

        let compiled = match materialize_and_compile(
            template,
            spec,
            stream_cache_threshold
                .unwrap_or(camel_api::stream_cache::DEFAULT_STREAM_CACHE_THRESHOLD),
            security_ctx.cloned().unwrap_or_default(),
        ) {
            Ok(compiled) => compiled,
            Err(e) => {
                let source = match &e {
                    camel_api::CamelError::RouteError(msg)
                        if msg.starts_with("route requires an authenticator") =>
                    {
                        TemplateError::SecurityRequired {
                            template_id: spec.route_template_ref.clone(),
                            detail: msg.clone(),
                        }
                    }
                    // Typed-parameter coercion failures surface as Config
                    // strings in the InvalidParameter display format —
                    // parse the fields back so the class survives to the
                    // aggregated surface instead of flattening to
                    // InvalidBody. Unparseable text falls through below.
                    camel_api::CamelError::Config(msg)
                        if msg.starts_with("parameter '") && msg.contains("declared type") =>
                    {
                        match parse_invalid_parameter_message(msg) {
                            Some((name, ty, value)) => {
                                TemplateError::InvalidParameter(name, ty, value)
                            }
                            None => TemplateError::InvalidBody(msg.clone()),
                        }
                    }
                    camel_api::CamelError::Config(msg) => TemplateError::InvalidBody(msg.clone()),
                    other => TemplateError::InvalidBody(other.to_string()),
                };
                failures.push(MaterializationFailure {
                    path: path_str.clone(),
                    template_ref: spec.route_template_ref.clone(),
                    route_id: spec.route_id.clone(),
                    error: source,
                });
                continue;
            }
        };

        for result in compiled {
            let rid = result.route_def.route_id().to_string();
            if !seen_route_ids.insert(rid.clone()) {
                // Precedence decision: a duplicate id aborts immediately and
                // preempts any materialization failures collected so far —
                // identity conflicts poison the seen-id set, so continuing
                // would attribute later failures to the wrong instance.
                return Err(DiscoveryError::DuplicateRouteId {
                    path: path_str.clone(),
                    route_id: rid,
                });
            }
            let route_def = match result.source_hash {
                Some(h) => result.route_def.with_source_hash(h),
                None => result.route_def,
            };
            routes.push(route_def);
        }
    }

    if !failures.is_empty() {
        return Err(DiscoveryError::MaterializationFailures { failures });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::io::Write;
    use tempfile::NamedTempFile;

    /// Pin the display-format round-trip: `parse_invalid_parameter_message`
    /// must recover the three fields from a REAL `TemplateError::InvalidParameter`
    /// Display string. If the thiserror format string in camel-api changes,
    /// this fails loudly instead of silently downgrading the error class to
    /// InvalidBody at the aggregation surface.
    #[test]
    fn invalid_parameter_display_round_trip_pins_parser() {
        let err = TemplateError::InvalidParameter(
            "delay".to_string(),
            "number".to_string(),
            "abc".to_string(),
        );
        let parsed = parse_invalid_parameter_message(&err.to_string());
        assert_eq!(
            parsed,
            Some(("delay".to_string(), "number".to_string(), "abc".to_string()))
        );
        // Values containing delimiter substrings must still round-trip
        // (split_once binds first, strip_suffix binds last).
        let tricky = TemplateError::InvalidParameter(
            "p".to_string(),
            "string".to_string(),
            "' is not coercible' is not coercible".to_string(),
        );
        assert_eq!(
            parse_invalid_parameter_message(&tricky.to_string()),
            Some((
                "p".to_string(),
                "string".to_string(),
                "' is not coercible' is not coercible".to_string()
            ))
        );
        // Non-matching input yields None (falls back to InvalidBody).
        assert_eq!(parse_invalid_parameter_message("parameter 'x' oops"), None);
    }

    // ── pattern_targets_json ──────────────────────────────────────────

    #[test]
    fn pattern_targets_json_explicit() {
        assert!(pattern_targets_json("routes/*.json"));
    }

    #[test]
    fn pattern_targets_json_recursive() {
        assert!(pattern_targets_json("routes/**/*.json"));
    }

    #[test]
    fn pattern_targets_json_uppercase() {
        assert!(pattern_targets_json("routes/*.JSON"));
    }

    #[test]
    fn pattern_targets_json_with_trailing_slash() {
        // .json in directory name but file targets .json — should still match
        assert!(pattern_targets_json("config/.json/routes/*.json"));
    }

    #[test]
    fn pattern_targets_json_dir_name_only_returns_false() {
        // .json only appears in directory path, not as file extension
        assert!(!pattern_targets_json("config/.json/routes/*"));
    }

    #[test]
    fn pattern_targets_json_dir_name_recursive_returns_false() {
        assert!(!pattern_targets_json("config/.json/routes/**/*"));
    }

    #[test]
    fn pattern_targets_json_brace_expansion() {
        assert!(pattern_targets_json("routes/{a,b}.json"));
    }

    #[test]
    fn pattern_targets_json_uppercase_extension() {
        assert!(pattern_targets_json("routes/*.JSON"));
    }

    #[test]
    fn pattern_targets_json_broad_returns_false() {
        assert!(!pattern_targets_json("routes/*"));
    }

    #[test]
    fn pattern_targets_json_broad_recursive_returns_false() {
        assert!(!pattern_targets_json("routes/**/*"));
    }

    // ── YAML discovery ───────────────────────────────────────────────

    #[test]
    fn discovers_route_with_env_var_in_uri_yaml() {
        unsafe { env::set_var("TEST_DISC_TIMER_NAME", "my-tick") };

        let mut f = NamedTempFile::with_suffix(".yaml").unwrap();
        writeln!(f, "routes:").unwrap();
        writeln!(f, "  - id: \"disc-route-1\"").unwrap();
        writeln!(f, "    from: \"timer:${{env:TEST_DISC_TIMER_NAME}}\"").unwrap();
        writeln!(f, "    steps:").unwrap();
        writeln!(f, "      - to: \"log:out\"").unwrap();

        let pattern = f.path().to_string_lossy().to_string();
        let routes = discover_routes(&[pattern]).unwrap();
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].from_uri(), "timer:my-tick");

        unsafe { env::remove_var("TEST_DISC_TIMER_NAME") };
    }

    #[test]
    fn discover_fails_when_env_var_missing_yaml() {
        unsafe { env::remove_var("TEST_DISC_MISSING_VAR") };

        let mut f = NamedTempFile::with_suffix(".yaml").unwrap();
        writeln!(f, "routes:").unwrap();
        writeln!(f, "  - id: \"disc-route-missing\"").unwrap();
        writeln!(f, "    from: \"timer:${{env:TEST_DISC_MISSING_VAR}}\"").unwrap();
        writeln!(f, "    steps: []").unwrap();

        let pattern = f.path().to_string_lossy().to_string();
        let err = match discover_routes(&[pattern]) {
            Ok(_) => panic!("expected error"),
            Err(e) => e,
        };
        match &err {
            DiscoveryError::Env { path: _, var_name } => {
                assert_eq!(var_name, "TEST_DISC_MISSING_VAR");
            }
            other => panic!("expected Env error, got: {other:?}"),
        }
    }

    #[test]
    fn discovers_yml_extension() {
        let mut f = NamedTempFile::with_suffix(".yml").unwrap();
        writeln!(f, "routes:").unwrap();
        writeln!(f, "  - id: \"yml-route\"").unwrap();
        writeln!(f, "    from: \"timer:tick\"").unwrap();
        writeln!(f, "    steps:").unwrap();
        writeln!(f, "      - to: \"log:info\"").unwrap();

        let pattern = f.path().to_string_lossy().to_string();
        let routes = discover_routes(&[pattern]).unwrap();
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].route_id(), "yml-route");
    }

    // ── JSON discovery ───────────────────────────────────────────────

    #[test]
    fn discovers_explicit_json_route() {
        let mut f = NamedTempFile::with_suffix(".json").unwrap();
        write!(
            f,
            r#"{{
  "routes": [
    {{
      "id": "json-route-1",
      "from": "timer:tick?period=1000",
      "steps": [
        {{ "to": "log:info" }}
      ]
    }}
  ]
}}"#
        )
        .unwrap();

        let pattern = f.path().to_string_lossy().to_string();
        let routes = discover_routes(&[pattern]).unwrap();
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].route_id(), "json-route-1");
        assert_eq!(routes[0].from_uri(), "timer:tick?period=1000");
    }

    #[test]
    fn discovers_json_with_glob_pattern() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("route.json");
        fs::write(
            &file_path,
            r#"{"routes":[{"id":"glob-json","from":"direct:start","steps":[{"to":"log:out"}]}]}"#,
        )
        .unwrap();

        let pattern = dir.path().join("*.json").to_string_lossy().to_string();
        let routes = discover_routes(&[pattern]).unwrap();
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].route_id(), "glob-json");
    }

    // ── Extension/gate validation before env interpolation ────────────

    #[test]
    fn unsupported_extension_with_env_var_returns_unsupported_not_env() {
        // .xml file containing a real env var reference must fail with
        // UnsupportedExtension, NOT Env — env interpolation must not run.
        unsafe { env::remove_var("TASK3_SHOULD_NOT_READ_ENV") };

        let f = NamedTempFile::with_suffix(".xml").unwrap();
        let content = "content: ${env:TASK3_SHOULD_NOT_READ_ENV}";
        fs::write(f.path(), content).unwrap();

        let pattern = f.path().to_string_lossy().to_string();
        let err = match discover_routes(&[pattern]) {
            Ok(_) => panic!("expected error"),
            Err(e) => e,
        };
        match &err {
            DiscoveryError::UnsupportedExtension { path: _, extension } => {
                assert_eq!(extension, "xml");
            }
            other => panic!(
                "expected UnsupportedExtension, got: {:?} — env interpolation ran before extension check",
                other
            ),
        }
    }

    #[test]
    fn broad_glob_json_with_missing_env_returns_gate_not_env() {
        // Broad glob matching .json with missing env var must fail with
        // JsonRequiresExplicitPattern, NOT Env — gate must fire before interpolation.
        unsafe { env::remove_var("TASK3_SHOULD_NOT_READ_ENV") };

        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("route.json");
        fs::write(
            &file_path,
            r#"{"routes":[{"id":"x","from":"timer:${env:TASK3_SHOULD_NOT_READ_ENV}","steps":[]}]}"#,
        )
        .unwrap();

        let pattern = dir.path().join("*").to_string_lossy().to_string();
        let err = match discover_routes(&[pattern]) {
            Ok(_) => panic!("expected error"),
            Err(e) => e,
        };
        match &err {
            DiscoveryError::JsonRequiresExplicitPattern {
                path: p,
                pattern: pat,
            } => {
                assert!(p.ends_with("route.json"), "path was: {p}");
                assert!(!pat.contains(".json"), "pattern was: {pat}");
            }
            other => panic!(
                "expected JsonRequiresExplicitPattern, got: {:?} — gate did not fire before env interpolation",
                other
            ),
        }
    }

    // ── Unsupported extension ────────────────────────────────────────

    #[test]
    fn unsupported_extension_returns_error() {
        let mut f = NamedTempFile::with_suffix(".xml").unwrap();
        writeln!(f, "<routes/>").unwrap();

        let pattern = f.path().to_string_lossy().to_string();
        let err = match discover_routes(&[pattern]) {
            Ok(_) => panic!("expected error"),
            Err(e) => e,
        };
        match &err {
            DiscoveryError::UnsupportedExtension { path: _, extension } => {
                assert_eq!(extension, "xml");
            }
            other => panic!("expected UnsupportedExtension, got: {other:?}"),
        }
    }

    #[test]
    fn no_extension_returns_error() {
        let mut f = NamedTempFile::new().unwrap();
        writeln!(f, "routes:").unwrap();

        let pattern = f.path().to_string_lossy().to_string();
        let err = match discover_routes(&[pattern]) {
            Ok(_) => panic!("expected error"),
            Err(e) => e,
        };
        match &err {
            DiscoveryError::UnsupportedExtension { path: _, extension } => {
                assert!(extension.is_empty());
            }
            other => panic!("expected UnsupportedExtension, got: {other:?}"),
        }
    }

    // ── Broad glob rejects JSON ──────────────────────────────────────

    #[test]
    fn broad_glob_rejects_json_with_explicit_pattern_error() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("route.json");
        fs::write(
            &file_path,
            r#"{"routes":[{"id":"broad-json","from":"direct:start","steps":[]}]}"#,
        )
        .unwrap();

        // Use a broad pattern that matches .json files but doesn't explicitly target .json
        let pattern = dir.path().join("*").to_string_lossy().to_string();
        let err = match discover_routes(&[pattern]) {
            Ok(_) => panic!("expected error"),
            Err(e) => e,
        };
        match &err {
            DiscoveryError::JsonRequiresExplicitPattern {
                path: p,
                pattern: pat,
            } => {
                assert!(p.ends_with("route.json"), "path was: {p}");
                assert!(pat.ends_with('*'), "pattern was: {pat}");
                assert!(!pat.contains(".json"), "pattern was: {pat}");
            }
            other => panic!("expected JsonRequiresExplicitPattern, got: {other:?}"),
        }
    }

    // ── JSON env interpolation ────────────────────────────────────────────

    #[test]
    fn json_env_interpolation_with_unescaped_quote_returns_json_error() {
        // An env var containing a raw double-quote will break JSON parsing
        // because interpolation is textual — the quote is injected verbatim
        // into the JSON string, producing invalid JSON.
        unsafe { env::set_var("TEST_JSON_BAD_QUOTE", r#"has"quote"#) };

        let mut f = NamedTempFile::with_suffix(".json").unwrap();
        write!(
            f,
            r#"{{
  "routes": [
    {{
      "id": "bad-quote",
      "from": "timer:${{env:TEST_JSON_BAD_QUOTE}}",
      "steps": []
    }}
  ]
}}"#
        )
        .unwrap();

        // The temp file path IS the pattern (already ends in .json)
        let pattern = f.path().to_string_lossy().to_string();
        let err = match discover_routes(&[pattern]) {
            Ok(_) => panic!("expected JSON parse error"),
            Err(e) => e,
        };
        match &err {
            DiscoveryError::Json { path: _, error } => {
                // Error should mention the parse failure (caused by unescaped quote)
                assert!(
                    !error.is_empty(),
                    "JSON parse error should describe the issue"
                );
            }
            other => panic!("expected DiscoveryError::Json, got: {:?}", other),
        }

        unsafe { env::remove_var("TEST_JSON_BAD_QUOTE") };
    }

    #[test]
    fn json_env_interpolation_with_valid_value_succeeds() {
        unsafe { env::set_var("TEST_JSON_GOOD_VAL", "tick") };

        let mut f = NamedTempFile::with_suffix(".json").unwrap();
        write!(
            f,
            r#"{{
  "routes": [
    {{
      "id": "good-env",
      "from": "timer:${{env:TEST_JSON_GOOD_VAL}}",
      "steps": []
    }}
  ]
}}"#
        )
        .unwrap();

        let pattern = f.path().to_string_lossy().to_string();
        let routes = discover_routes(&[pattern]).unwrap();
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].from_uri(), "timer:tick");

        unsafe { env::remove_var("TEST_JSON_GOOD_VAL") };
    }

    // ── Template-aware discovery ─────────────────────────────────────

    #[test]
    fn discovers_yaml_template_and_materializes() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("routes.yaml");
        fs::write(
            &file_path,
            r#"
routes: []
templates:
  - id: http-route
    parameters:
      - name: path
    routes:
      - id: "materialized-http"
        from: "rest:{{path}}"
        steps:
          - to: "log:info"
templated_routes:
  - route_template_ref: http-route
    route_id: "my-http"
    parameters:
      path: /api/users
"#,
        )
        .unwrap();

        let pattern = file_path.to_string_lossy().to_string();
        let routes = discover_routes(&[pattern]).unwrap();
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].route_id(), "my-http");
        assert_eq!(routes[0].from_uri(), "rest:/api/users");
    }

    #[test]
    fn discovers_json_template_and_materializes() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("routes.json");
        fs::write(
            &file_path,
            r#"{
  "routes": [],
  "templates": [
    {
      "id": "timer-route",
      "parameters": [{"name": "period"}],
      "routes": [
        {
          "id": "materialized-timer",
          "from": "timer:tick?period={{period}}",
          "steps": []
        }
      ]
    }
  ],
  "templated_routes": [
    {
      "route_template_ref": "timer-route",
      "parameters": {"period": "5000"}
    }
  ]
}"#,
        )
        .unwrap();

        let pattern = file_path.to_string_lossy().to_string();
        let routes = discover_routes(&[pattern]).unwrap();
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].route_id(), "materialized-timer");
        assert_eq!(routes[0].from_uri(), "timer:tick?period=5000");
    }

    #[test]
    fn discovers_mixed_regular_routes_and_templates() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("mixed.yaml");
        fs::write(
            &file_path,
            r#"
routes:
  - id: regular-route
    from: direct:start
    steps:
      - to: log:info
templates:
  - id: log-route
    parameters:
      - name: level
    routes:
      - id: "materialized-log"
        from: "direct:log"
        steps:
          - to: "log:{{level}}"
templated_routes:
  - route_template_ref: log-route
    parameters:
      level: warn
"#,
        )
        .unwrap();

        let pattern = file_path.to_string_lossy().to_string();
        let routes = discover_routes(&[pattern]).unwrap();
        assert_eq!(routes.len(), 2);
        let ids: Vec<&str> = routes.iter().map(|r| r.route_id()).collect();
        assert!(ids.contains(&"regular-route"));
        assert!(ids.contains(&"materialized-log"));
    }

    #[test]
    fn discovers_cross_file_template_reference() {
        let dir = tempfile::tempdir().unwrap();
        // File A: defines the template
        let file_a = dir.path().join("templates.yaml");
        fs::write(
            &file_a,
            r#"
routes: []
templates:
  - id: shared-http
    parameters:
      - name: path
    routes:
      - id: "shared-route"
        from: "rest:{{path}}"
        steps:
          - to: "log:shared"
"#,
        )
        .unwrap();

        // File B: instantiates the template
        let file_b = dir.path().join("instances.yaml");
        fs::write(
            &file_b,
            r#"
routes: []
templated_routes:
  - route_template_ref: shared-http
    parameters:
      path: /cross-file
"#,
        )
        .unwrap();

        let pattern = dir.path().join("*.yaml").to_string_lossy().to_string();
        let routes = discover_routes(&[pattern]).unwrap();
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].route_id(), "shared-route");
        assert_eq!(routes[0].from_uri(), "rest:/cross-file");
    }

    #[test]
    fn missing_template_ref_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("missing.yaml");
        fs::write(
            &file_path,
            r#"
routes: []
templated_routes:
  - route_template_ref: nonexistent-template
    parameters:
      path: /test
"#,
        )
        .unwrap();

        let pattern = file_path.to_string_lossy().to_string();
        let err = match discover_routes(&[pattern]) {
            Ok(_) => panic!("expected error"),
            Err(e) => e,
        };
        match &err {
            DiscoveryError::MaterializationFailures { failures } => {
                assert_eq!(failures.len(), 1, "expected exactly one failure: {err:?}");
                let failure = &failures[0];
                assert_eq!(failure.template_ref, "nonexistent-template");
                match &failure.error {
                    TemplateError::NotFound(ref_) => {
                        assert_eq!(ref_, "nonexistent-template");
                    }
                    other => panic!("expected NotFound, got: {other:?}"),
                }
            }
            other => panic!("expected MaterializationFailures, got: {other:?}"),
        }
    }

    #[test]
    fn duplicate_template_ids_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        // File A: defines template "dup-tpl"
        let file_a = dir.path().join("a.yaml");
        fs::write(
            &file_a,
            r#"
routes: []
templates:
  - id: dup-tpl
    routes:
      - id: "route-a"
        from: "direct:a"
"#,
        )
        .unwrap();

        // File B: also defines template "dup-tpl"
        let file_b = dir.path().join("b.yaml");
        fs::write(
            &file_b,
            r#"
routes: []
templates:
  - id: dup-tpl
    routes:
      - id: "route-b"
        from: "direct:b"
"#,
        )
        .unwrap();

        let pattern = dir.path().join("*.yaml").to_string_lossy().to_string();
        let err = match discover_routes(&[pattern]) {
            Ok(_) => panic!("expected error"),
            Err(e) => e,
        };
        match &err {
            DiscoveryError::TemplateSpec { path: _, error } => {
                assert!(error.contains("dup-tpl"));
                assert!(error.contains("duplicate"));
            }
            other => panic!("expected TemplateSpec error, got: {other:?}"),
        }
    }

    #[test]
    fn materialized_routes_preserve_source_hash() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("hash-test.yaml");
        fs::write(
            &file_path,
            r#"
routes: []
templates:
  - id: hash-tpl
    routes:
      - id: "hash-route"
        from: "direct:hash"
        steps: []
templated_routes:
  - route_template_ref: hash-tpl
    parameters: {}
"#,
        )
        .unwrap();

        let pattern = file_path.to_string_lossy().to_string();
        let routes = discover_routes(&[pattern]).unwrap();
        assert_eq!(routes.len(), 1);
        let hash = routes[0].source_hash();
        assert!(hash.is_some(), "materialized route should have source_hash");
        assert_ne!(hash.unwrap(), 0, "source_hash should be non-zero");
    }

    #[test]
    fn materialized_source_hash_is_instance_sensitive() {
        let dir = tempfile::tempdir().unwrap();

        let template_body = vec![serde_json::json!({
            "id": "same-route",
            "from": "direct:x",
            "steps": []
        })];
        let empty_params = std::collections::BTreeMap::new();
        let hash_a = crate::template::materializer::compute_instance_source_hash(
            &template_body,
            &empty_params,
            "inst-a",
        );
        let hash_b = crate::template::materializer::compute_instance_source_hash(
            &template_body,
            &empty_params,
            "inst-b",
        );

        let file_path = dir.path().join("two-instances.yaml");
        fs::write(
            &file_path,
            r#"
routes: []
templates:
  - id: shared-tpl
    routes:
      - id: "same-route"
        from: "direct:x"
        steps: []
templated_routes:
  - route_template_ref: shared-tpl
    route_id: "inst-a"
    parameters: {}
  - route_template_ref: shared-tpl
    route_id: "inst-b"
    parameters: {}
"#,
        )
        .unwrap();

        let pattern = file_path.to_string_lossy().to_string();
        let routes = discover_routes(&[pattern]).unwrap();
        assert_eq!(routes.len(), 2);

        let mut hashes_by_id = std::collections::HashMap::new();
        for route in &routes {
            let hash = route.source_hash().expect("should have source_hash");
            assert_ne!(hash, 0, "source_hash should be non-zero");
            hashes_by_id.insert(route.route_id().to_string(), hash);
        }
        assert_eq!(
            hashes_by_id.get("inst-a"),
            Some(&hash_a),
            "inst-a hash must reflect body + params + its effective id"
        );
        assert_eq!(
            hashes_by_id.get("inst-b"),
            Some(&hash_b),
            "inst-b hash must reflect body + params + its effective id"
        );
        assert_ne!(
            hash_a, hash_b,
            "instances differing only in override id must hash distinctly"
        );
    }

    #[test]
    fn template_only_file_without_routes_key() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("tpl-only.yaml");
        fs::write(
            &file_path,
            r#"
templates:
  - id: solo-tpl
    parameters:
      - name: target
    routes:
      - id: "solo-{{target}}"
        from: "direct:start"
        steps:
          - to: "{{target}}"
templated_routes:
  - route_template_ref: solo-tpl
    parameters:
      target: "log:info"
"#,
        )
        .unwrap();

        let pattern = file_path.to_string_lossy().to_string();
        let routes = discover_routes(&[pattern]).unwrap();
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].from_uri(), "direct:start");
    }

    #[test]
    fn duplicate_route_ids_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("dup-rid.yaml");
        fs::write(
            &file_path,
            r#"
routes:
  - id: "shared-id"
    from: "direct:a"
    steps: []
templates:
  - id: tpl
    routes:
      - id: "tpl-route"
        from: "direct:b"
        steps: []
templated_routes:
  - route_template_ref: tpl
    route_id: "shared-id"
    parameters: {}
"#,
        )
        .unwrap();

        let pattern = file_path.to_string_lossy().to_string();
        let err = match discover_routes(&[pattern]) {
            Ok(_) => panic!("expected duplicate route id error"),
            Err(e) => e,
        };
        let msg = err.to_string();
        assert!(
            msg.contains("shared-id"),
            "expected duplicate route id error, got: {msg}"
        );
        match &err {
            DiscoveryError::DuplicateRouteId { route_id, .. } => {
                assert_eq!(route_id, "shared-id");
            }
            other => panic!("expected DuplicateRouteId error, got: {other:?}"),
        }
    }

    #[test]
    fn discovers_multi_route_template() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("multi.yaml");
        fs::write(
            &file_path,
            r#"
routes: []
templates:
  - id: chain
    parameters:
      - name: PROV
    routes:
      - id: "step1-{{PROV}}"
        from: "direct:start"
        steps:
          - to: "controlbus:route?routeId=step2-{{PROV}}&action=start"
      - id: "step2-{{PROV}}"
        from: "direct:step2"
        steps:
          - to: "log:done"
templated_routes:
  - route_template_ref: chain
    parameters:
      PROV: granada
"#,
        )
        .unwrap();

        let pattern = file_path.to_string_lossy().to_string();
        let routes = discover_routes(&[pattern]).unwrap();
        assert_eq!(routes.len(), 2);
        assert_eq!(routes[0].route_id(), "step1-granada");
        assert_eq!(routes[1].route_id(), "step2-granada");
    }

    // ── threshold-less discovery threads security context ────────────

    struct TestAuthenticator;

    #[async_trait::async_trait]
    impl camel_auth::TokenAuthenticator for TestAuthenticator {
        async fn authenticate_bearer(
            &self,
            _token: &str,
        ) -> Result<camel_api::security_policy::Principal, camel_api::CamelError> {
            Ok(camel_api::security_policy::Principal {
                subject: "test-user".into(),
                issuer: "test-issuer".into(),
                audience: vec![],
                scopes: vec!["read:api".into()],
                roles: vec!["admin".into()],
                claims: serde_json::Value::Null,
            })
        }
    }

    #[test]
    fn threshold_less_discovery_threads_security_context() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("route.yaml");
        fs::write(
            &file_path,
            r#"
routes:
  - id: sec-route
    from: direct:start
    security_policy:
      roles: ["admin"]
    steps:
      - to: log:info
"#,
        )
        .unwrap();
        let pattern = file_path.to_string_lossy().to_string();

        let auth = std::sync::Arc::new(TestAuthenticator)
            as std::sync::Arc<dyn camel_auth::TokenAuthenticator>;
        let ctx = SecurityCompileContext::new(Some(auth), None);

        // (None-threshold, Some-ctx) — only reachable through the private fn.
        let routes =
            discover_routes_inner(std::slice::from_ref(&pattern), None, Some(ctx), None).unwrap();
        assert_eq!(routes.len(), 1);
        assert!(routes[0].security_authenticator().is_some());

        // Fail-closed pin: public path (None ctx) must reject the secured route.
        let err = match discover_routes_inner(&[pattern], None, None, None) {
            Ok(_) => panic!("expected error for secured route without authenticator"),
            Err(e) => e,
        };
        let msg = err.to_string();
        assert!(
            msg.contains("route requires an authenticator"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn security_required_error_classified() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("secured-tpl.yaml");
        fs::write(
            &file_path,
            r#"
routes: []
templates:
  - id: secured-tpl
    parameters: []
    routes:
      - id: "secured-route"
        from: "direct:start"
        security_policy:
          roles: ["admin"]
        steps:
          - to: "log:info"
templated_routes:
  - route_template_ref: secured-tpl
    parameters: {}
"#,
        )
        .unwrap();

        let pattern = file_path.to_string_lossy().to_string();

        // Fail-closed: default security ctx (no authenticator) must classify
        // the failure as SecurityRequired, not InvalidBody.
        let err = match discover_routes_inner(&[pattern], None, None, None) {
            Ok(_) => panic!("expected secured templated route to fail closed"),
            Err(e) => e,
        };
        match &err {
            DiscoveryError::MaterializationFailures { failures } => {
                assert_eq!(failures.len(), 1, "expected exactly one failure: {err:?}");
                match &failures[0].error {
                    TemplateError::SecurityRequired { template_id, .. } => {
                        assert_eq!(template_id, "secured-tpl");
                    }
                    other => panic!("expected SecurityRequired, got: {other:?}"),
                }
            }
            other => panic!("expected MaterializationFailures, got: {other:?}"),
        }
    }

    // ── Reserved document suffixes (.test.yaml / .job.yaml families) ──

    #[test]
    fn test_doc_skipped_under_wildcard_pattern() {
        let dir = tempfile::tempdir().unwrap();
        let routes_dir = dir.path().join("routes");
        fs::create_dir_all(&routes_dir).unwrap();
        fs::write(
            routes_dir.join("demo.yaml"),
            r#"
routes:
  - id: "demo-route"
    from: "timer:tick"
    steps:
      - to: "log:info"
"#,
        )
        .unwrap();
        // Any bytes — a test doc must never be read by route discovery.
        fs::write(routes_dir.join("demo.test.yaml"), "not: [a route document").unwrap();

        let pattern = routes_dir.join("*.yaml").to_string_lossy().to_string();
        let routes = discover_routes(&[pattern]).unwrap();
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].route_id(), "demo-route");
    }

    #[test]
    fn test_doc_literal_pattern_hard_errors() {
        let dir = tempfile::tempdir().unwrap();
        let routes_dir = dir.path().join("routes");
        fs::create_dir_all(&routes_dir).unwrap();
        fs::write(routes_dir.join("demo.test.yaml"), "not: [a route document").unwrap();

        let pattern = routes_dir
            .join("demo.test.yaml")
            .to_string_lossy()
            .to_string();
        let err = match discover_routes(std::slice::from_ref(&pattern)) {
            Ok(_) => panic!("expected ReservedDocumentSuffix error"),
            Err(e) => e,
        };
        match &err {
            DiscoveryError::ReservedDocumentSuffix { path } => {
                assert_eq!(path, &pattern);
            }
            other => panic!("expected ReservedDocumentSuffix, got: {other:?}"),
        }
        let msg = err.to_string();
        assert!(msg.contains("camel test"), "display was: {msg}");
    }

    #[test]
    fn yml_test_doc_also_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let routes_dir = dir.path().join("routes");
        fs::create_dir_all(&routes_dir).unwrap();
        fs::write(
            routes_dir.join("demo.yml"),
            r#"
routes:
  - id: "yml-demo-route"
    from: "timer:tick"
    steps:
      - to: "log:info"
"#,
        )
        .unwrap();
        fs::write(routes_dir.join("demo.test.yml"), "not: [a route document").unwrap();

        let pattern = routes_dir.join("*.yml").to_string_lossy().to_string();
        let routes = discover_routes(&[pattern]).unwrap();
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].route_id(), "yml-demo-route");
    }

    #[test]
    fn wildcard_over_only_test_docs_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        let routes_dir = dir.path().join("routes");
        fs::create_dir_all(&routes_dir).unwrap();
        fs::write(routes_dir.join("demo.test.yaml"), "not: [a route document").unwrap();

        let pattern = routes_dir.join("*.test.yaml").to_string_lossy().to_string();
        let routes = discover_routes(&[pattern]).unwrap();
        assert!(routes.is_empty());
    }

    #[test]
    fn test_json_name_not_test_suffixed() {
        // `.test.json` is NOT a camel test document — the JSON explicit-pattern
        // gate must still fire for it under a broad wildcard.
        let dir = tempfile::tempdir().unwrap();
        let routes_dir = dir.path().join("routes");
        fs::create_dir_all(&routes_dir).unwrap();
        fs::write(routes_dir.join("x.test.json"), "{}").unwrap();

        let pattern = routes_dir.join("*").to_string_lossy().to_string();
        let err = match discover_routes(&[pattern]) {
            Ok(_) => panic!("expected JsonRequiresExplicitPattern"),
            Err(e) => e,
        };
        match &err {
            DiscoveryError::JsonRequiresExplicitPattern { path, .. } => {
                assert!(path.ends_with("x.test.json"), "path was: {path}");
            }
            other => panic!("expected JsonRequiresExplicitPattern, got: {other:?}"),
        }
    }

    #[test]
    fn test_json_name_under_explicit_json_glob_parses_as_route() {
        // `.test.json` is not a camel test document (the reserved suffix is
        // YAML-only), so an explicit `.json` glob loads it as a route. Spec
        // scenario: `routes/x.test.json` matched by `routes/*.json`.
        let dir = tempfile::tempdir().unwrap();
        let routes_dir = dir.path().join("routes");
        fs::create_dir_all(&routes_dir).unwrap();
        fs::write(
            routes_dir.join("x.test.json"),
            r#"{"routes":[{"id":"j","from":"direct:s","steps":[{"to":"mock:m"}]}]}"#,
        )
        .unwrap();

        let pattern = routes_dir.join("*.json").to_string_lossy().to_string();
        let routes = discover_routes(&[pattern]).unwrap();
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].route_id(), "j");
    }

    #[test]
    fn is_test_document_predicate() {
        assert!(is_test_document(Path::new("a.test.yaml")));
        assert!(is_test_document(Path::new("a.test.yml")));
        assert!(!is_test_document(Path::new("atest.yaml")));
        assert!(!is_test_document(Path::new("a.yaml")));
        assert!(!is_test_document(Path::new("x.test.json")));
    }

    #[test]
    fn is_job_document_predicate() {
        assert!(is_job_document(Path::new("a.job.yaml")));
        assert!(is_job_document(Path::new("a.job.yml")));
        assert!(!is_job_document(Path::new("ajob.yaml")));
        assert!(!is_job_document(Path::new("a.yaml")));
        assert!(!is_job_document(Path::new("x.job.json")));
    }

    #[test]
    fn is_reserved_document_predicate() {
        assert!(is_reserved_document(Path::new("a.test.yaml")));
        assert!(is_reserved_document(Path::new("a.test.yml")));
        assert!(is_reserved_document(Path::new("a.job.yaml")));
        assert!(is_reserved_document(Path::new("a.job.yml")));
        assert!(!is_reserved_document(Path::new("a.yaml")));
        assert!(!is_reserved_document(Path::new("a.test.json")));
    }

    #[test]
    fn job_doc_skipped_under_wildcard_pattern() {
        let dir = tempfile::tempdir().unwrap();
        let routes_dir = dir.path().join("routes");
        fs::create_dir_all(&routes_dir).unwrap();
        fs::write(
            routes_dir.join("demo.yaml"),
            r#"
routes:
  - id: "demo-route"
    from: "timer:tick"
    steps:
      - to: "log:info"
"#,
        )
        .unwrap();
        // Any bytes — a job doc must never be read by route discovery.
        fs::write(routes_dir.join("demo.job.yaml"), "not: [a route document").unwrap();

        let pattern = routes_dir.join("*.yaml").to_string_lossy().to_string();
        let routes = discover_routes(&[pattern]).unwrap();
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].route_id(), "demo-route");
    }

    #[test]
    fn job_doc_literal_pattern_hard_errors() {
        let dir = tempfile::tempdir().unwrap();
        let routes_dir = dir.path().join("routes");
        fs::create_dir_all(&routes_dir).unwrap();
        fs::write(routes_dir.join("demo.job.yaml"), "not: [a route document").unwrap();

        let pattern = routes_dir
            .join("demo.job.yaml")
            .to_string_lossy()
            .to_string();
        let err = match discover_routes(std::slice::from_ref(&pattern)) {
            Ok(_) => panic!("expected ReservedDocumentSuffix error"),
            Err(e) => e,
        };
        match &err {
            DiscoveryError::ReservedDocumentSuffix { path } => {
                assert_eq!(path, &pattern);
            }
            other => panic!("expected ReservedDocumentSuffix, got: {other:?}"),
        }
        let msg = err.to_string();
        assert!(msg.contains("camel job"), "display was: {msg}");
    }

    // ── Env-lookup-injected discovery entry ──────────────────────────

    #[test]
    fn env_injected_entry_resolves_through_lookup() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("tier.yaml");
        fs::write(
            &file_path,
            r#"
routes:
  - id: tier-route
    from: "direct:${env:RC_TIER_ONLY}"
    steps:
      - to: "log:info"
"#,
        )
        .unwrap();
        let pattern = file_path.to_string_lossy().to_string();

        let routes = discover_routes_with_threshold_security_and_env(
            &[pattern],
            4096,
            SecurityCompileContext::default(),
            &|n| (n == "RC_TIER_ONLY").then(|| "start".into()),
        )
        .unwrap();
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].from_uri(), "direct:start");
    }

    #[test]
    fn env_injected_entry_never_reads_process_env() {
        unsafe { env::set_var("RC_6BSF_PROC_ONLY", "leak") };

        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("proc.yaml");
        fs::write(
            &file_path,
            r#"
routes:
  - id: proc-route
    from: "direct:${env:RC_6BSF_PROC_ONLY}"
    steps:
      - to: "log:info"
"#,
        )
        .unwrap();
        let pattern = file_path.to_string_lossy().to_string();

        let err = match discover_routes_with_threshold_security_and_env(
            &[pattern],
            4096,
            SecurityCompileContext::default(),
            &|_| None,
        ) {
            Ok(_) => panic!("expected Env error, process env leaked through"),
            Err(e) => e,
        };
        match &err {
            DiscoveryError::Env { var_name, .. } => {
                assert_eq!(var_name, "RC_6BSF_PROC_ONLY");
            }
            other => panic!("expected Env error, got: {other:?}"),
        }

        unsafe { env::remove_var("RC_6BSF_PROC_ONLY") };
    }

    #[test]
    fn env_injected_entry_materializes_templates() {
        let dir = tempfile::tempdir().unwrap();

        // Env-injected file: template parameter values carry ${env:} placeholders.
        let env_file = dir.path().join("tpl-env.yaml");
        fs::write(
            &env_file,
            r#"
routes: []
templates:
  - id: direct-tpl
    parameters:
      - name: target
    routes:
      - id: "tpl-body-route"
        from: "{{target}}"
        steps: []
templated_routes:
  - route_template_ref: direct-tpl
    route_id: "inst-a"
    parameters:
      target: "direct:${env:RC_TPL}"
  - route_template_ref: direct-tpl
    route_id: "inst-b"
    parameters:
      target: "direct:${env:RC_TPL}"
"#,
        )
        .unwrap();
        let env_pattern = env_file.to_string_lossy().to_string();
        let injected = discover_routes_with_threshold_security_and_env(
            &[env_pattern],
            4096,
            SecurityCompileContext::default(),
            &|n| (n == "RC_TPL").then(|| "shared".into()),
        )
        .unwrap();

        // Baseline: identical file with placeholders pre-substituted, run
        // through the process-environment entry (existing comparison
        // convention — RouteDefinition has no PartialEq).
        let plain_file = dir.path().join("tpl-plain.yaml");
        fs::write(
            &plain_file,
            r#"
routes: []
templates:
  - id: direct-tpl
    parameters:
      - name: target
    routes:
      - id: "tpl-body-route"
        from: "{{target}}"
        steps: []
templated_routes:
  - route_template_ref: direct-tpl
    route_id: "inst-a"
    parameters:
      target: "direct:shared"
  - route_template_ref: direct-tpl
    route_id: "inst-b"
    parameters:
      target: "direct:shared"
"#,
        )
        .unwrap();
        let plain_pattern = plain_file.to_string_lossy().to_string();
        let baseline = discover_routes_with_threshold_and_security(
            &[plain_pattern],
            4096,
            SecurityCompileContext::default(),
        )
        .unwrap();

        assert_eq!(injected.len(), 2);
        assert_eq!(injected.len(), baseline.len());
        let injected_ids: Vec<&str> = injected.iter().map(|r| r.route_id()).collect();
        let baseline_ids: Vec<&str> = baseline.iter().map(|r| r.route_id()).collect();
        assert_eq!(injected_ids, baseline_ids);
        let injected_uris: Vec<&str> = injected.iter().map(|r| r.from_uri()).collect();
        let baseline_uris: Vec<&str> = baseline.iter().map(|r| r.from_uri()).collect();
        assert_eq!(injected_uris, baseline_uris);
    }

    #[test]
    fn env_injected_entry_equivalent_output_at_same_threshold() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("threshold.yaml");
        let raw = r#"
routes:
  - id: threshold-route
    from: "direct:${env:RC_TH}"
    steps:
      - stream_cache: {}
      - to: "log:info"
"#;
        fs::write(&file_path, raw).unwrap();
        let pattern = file_path.to_string_lossy().to_string();

        let injected = discover_routes_with_threshold_security_and_env(
            &[pattern],
            777,
            SecurityCompileContext::default(),
            &|n| (n == "RC_TH").then(|| "th".into()),
        )
        .unwrap();

        let pre_interpolated = raw.replace("${env:RC_TH}", "th");
        let parsed = crate::yaml::parse_yaml_with_threshold(&pre_interpolated, 777).unwrap();

        assert_eq!(injected.len(), 1);
        assert_eq!(injected.len(), parsed.len());
        assert_eq!(injected[0].route_id(), parsed[0].route_id());
        assert_eq!(injected[0].from_uri(), parsed[0].from_uri());
    }

    // ── Typed probe through discovery (env-int-placeholder-typing) ────

    fn assert_discovered_throttle(routes: &[RouteDefinition], expected: usize) {
        match &routes[0].steps()[0] {
            camel_core::route::BuilderStep::Throttle { config, .. } => {
                assert_eq!(config.max_requests, expected);
            }
            other => panic!("expected throttle step, got: {other:?}"),
        }
    }

    #[test]
    fn discovery_int_placeholder_loads() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("int-placeholder.yaml");
        fs::write(
            &file_path,
            "routes:\n  - id: disc-int\n    from: \"direct:start\"\n    steps:\n      - throttle:\n          max_requests: ${env:DWD_WARM_MAX_REQUESTS:-2}\n          period_secs: 1\n",
        )
        .unwrap();
        let pattern = file_path.to_string_lossy().to_string();

        let routes = discover_routes_with_threshold_security_and_env(
            &[pattern],
            4096,
            SecurityCompileContext::default(),
            &|n| (n == "DWD_WARM_MAX_REQUESTS").then(|| "5".into()),
        )
        .unwrap();
        assert_discovered_throttle(&routes, 5);
    }

    #[test]
    fn discovery_int_placeholder_default() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("int-placeholder-default.yaml");
        fs::write(
            &file_path,
            "routes:\n  - id: disc-int-default\n    from: \"direct:start\"\n    steps:\n      - throttle:\n          max_requests: ${env:DWD_WARM_MAX_REQUESTS:-2}\n          period_secs: 1\n",
        )
        .unwrap();
        let pattern = file_path.to_string_lossy().to_string();

        let routes = discover_routes_with_threshold_security_and_env(
            &[pattern],
            4096,
            SecurityCompileContext::default(),
            &|_| None,
        )
        .unwrap();
        assert_discovered_throttle(&routes, 2);
    }

    #[test]
    fn templates_and_int_placeholder_route_coexist() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("tpl-int-coexist.yaml");
        fs::write(
            &file_path,
            r#"
routes:
  - id: coexisting-direct
    from: "direct:start"
    steps:
      - throttle:
          max_requests: ${env:COEXIST_MAX_REQUESTS:-2}
          period_secs: 1
templates:
  - id: coexist-tpl
    parameters:
      - name: target
    routes:
      - id: "coexist-tpl-body"
        from: "{{target}}"
        steps: []
templated_routes:
  - route_template_ref: coexist-tpl
    route_id: "coexist-materialized"
    parameters:
      target: "direct:materialized"
"#,
        )
        .unwrap();
        let pattern = file_path.to_string_lossy().to_string();

        let routes = discover_routes_with_threshold_security_and_env(
            &[pattern],
            4096,
            SecurityCompileContext::default(),
            &|n| (n == "COEXIST_MAX_REQUESTS").then(|| "5".into()),
        )
        .unwrap();

        assert_eq!(routes.len(), 2);

        // The probed direct route carries the injected integer.
        let direct = routes
            .iter()
            .find(|r| r.route_id() == "coexisting-direct")
            .expect("direct route discovered");
        match &direct.steps()[0] {
            camel_core::route::BuilderStep::Throttle { config, .. } => {
                assert_eq!(config.max_requests, 5);
            }
            other => panic!("expected throttle step, got: {other:?}"),
        }

        // The templated route materialized and compiled in the same pass.
        let templated = routes
            .iter()
            .find(|r| r.route_id() == "coexist-materialized")
            .expect("templated route materialized");
        assert_eq!(templated.from_uri(), "direct:materialized");
    }

    #[test]
    fn json_int_placeholder_still_fails() {
        // Non-goal pin: JSON route files never probe — a string at an
        // integer position stays a discovery error.
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("int-placeholder.json");
        fs::write(
            &file_path,
            r#"{"routes":[{"id":"json-int","from":"direct:start","steps":[{"throttle":{"max_requests":"${env:J:-2}","period_secs":1}}]}]}"#,
        )
        .unwrap();
        let pattern = file_path.to_string_lossy().to_string();

        let err = match discover_routes_with_threshold_security_and_env(
            &[pattern],
            4096,
            SecurityCompileContext::default(),
            &|_| None,
        ) {
            Ok(_) => panic!("expected JSON int placeholder to stay rejected"),
            Err(e) => e,
        };
        match &err {
            DiscoveryError::Json { error, .. } => {
                assert!(
                    error.contains("did not match any variant") || error.contains("invalid type"),
                    "expected a typed parse error, got: {error}"
                );
            }
            other => panic!("expected DiscoveryError::Json, got: {other:?}"),
        }
    }

    // ── Virtual-store config parity goldens (openspec change
    //    `configunify`, Task 1.2) ───────────────────────────────────────
    //
    // These tests lock the PRE-refactor behavior of the virtual-store
    // config assembly over a representative matrix: `build_virtual_config`
    // output serialized with `toml::to_string_pretty` is compared
    // byte-for-byte against committed goldens under
    // `tests/goldens/virtual_config/`, as are the recursive-include WARN
    // transcript and the unknown-profile error `Display` string (error
    // message strings are observable behavior). Regenerate after an
    // intentional behavior change:
    // `UPDATE_GOLDENS=1 cargo test -p camel-dsl virtual_config_`.

    use crate::StoreDocument;
    use std::sync::{Arc, Mutex};

    /// Regeneration switch: when `UPDATE_GOLDENS=1` is set, write the
    /// golden files instead of comparing them.
    fn update_goldens() -> bool {
        std::env::var("UPDATE_GOLDENS").is_ok_and(|v| v == "1")
    }

    fn golden_path(name: &str) -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/goldens/virtual_config")
            .join(name)
    }

    /// Byte-for-byte golden lock for text artifacts (pretty-printed
    /// merged TOML, warn and error transcripts).
    fn lock_text_golden(name: &str, actual: &str) {
        let path = golden_path(name);
        if update_goldens() {
            std::fs::create_dir_all(path.parent().expect("golden parent dir"))
                .expect("create goldens dir");
            std::fs::write(&path, actual).unwrap_or_else(|e| panic!("write golden {name}: {e}"));
        } else {
            let expected = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("read golden {name}: {e} (capture: UPDATE_GOLDENS=1)"));
            assert_eq!(expected, actual, "golden {name} drifted");
        }
    }

    /// Serialize one merged configuration value exactly as the goldens
    /// store it.
    fn lock_toml_golden(name: &str, merged: &toml::Value) {
        let text = toml::to_string_pretty(merged).expect("serialize merged config");
        lock_text_golden(name, &text);
    }

    // ── store fixtures ───────────────────────────────────────────────

    /// Inert route document every fixture store carries as entry point
    /// and sole source-plan reference (config assembly never reads it).
    const PARITY_ROUTE_TEXT: &str =
        "routes:\n  - id: parity-store\n    from: \"direct:start\"\n    steps: []\n";

    fn store_doc(path: &str, kind: StoreEntryKind, text: &str) -> StoreDocument {
        StoreDocument {
            path: path.to_string(),
            kind,
            bytes: text.as_bytes().to_vec(),
        }
    }

    /// Pack fixture documents as a virtual store the way the compiler
    /// embeds them: the same inert route document serves as entry point
    /// and sole source-plan reference, and `config_references` encodes
    /// the config/include/profile declaration order under test.
    fn fixture_store(
        config_references: &[&str],
        mut documents: Vec<StoreDocument>,
    ) -> VirtualDocumentStore {
        documents.push(store_doc(
            "routes/main.yaml",
            StoreEntryKind::Route,
            PARITY_ROUTE_TEXT,
        ));
        VirtualDocumentStore::build(
            "routes/main.yaml",
            &documents,
            &config_references
                .iter()
                .map(|path| (*path).to_string())
                .collect::<Vec<_>>(),
            &["routes/main.yaml".to_string()],
        )
        .expect("fixture store must build")
    }

    // ── WARN capture (recursive-include case) ─────────────────────────

    struct WarnMessageVisitor<'a>(&'a mut Option<String>);

    impl tracing::field::Visit for WarnMessageVisitor<'_> {
        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            if field.name() == "message" {
                *self.0 = Some(format!("{value:?}"));
            }
        }
    }

    /// `tracing_subscriber` layer recording every WARN event message
    /// emitted under the installing thread's default subscriber.
    struct WarnCaptureLayer {
        events: Arc<Mutex<Vec<String>>>,
    }

    impl<S> tracing_subscriber::Layer<S> for WarnCaptureLayer
    where
        S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
    {
        fn on_event(
            &self,
            event: &tracing::Event<'_>,
            _ctx: tracing_subscriber::layer::Context<'_, S>,
        ) {
            if *event.metadata().level() != tracing::Level::WARN {
                return;
            }
            let mut slot = None;
            event.record(&mut WarnMessageVisitor(&mut slot));
            if let Some(message) = slot {
                self.events
                    .lock()
                    .expect("capture lock")
                    .push(format!("{}: {message}", event.metadata().level()));
            }
        }
    }

    /// Run `body` under a thread-local subscriber recording WARN
    /// messages. `set_default` is thread-local, so concurrent tests
    /// neither pollute this capture nor observe it.
    fn capture_warns<T>(body: impl FnOnce() -> T) -> (T, Vec<String>) {
        use tracing_subscriber::prelude::*;
        let events = Arc::new(Mutex::new(Vec::new()));
        let guard = tracing_subscriber::registry()
            .with(WarnCaptureLayer {
                events: Arc::clone(&events),
            })
            .set_default();
        let out = body();
        drop(guard);
        let captured = events.lock().expect("capture lock").clone();
        (out, captured)
    }

    // ── the matrix ───────────────────────────────────────────────────

    /// Case (a): one flat config document assembles as-is.
    #[test]
    fn virtual_config_flat_matches_golden() {
        let store = fixture_store(
            &["Camel.toml"],
            vec![store_doc(
                "Camel.toml",
                StoreEntryKind::Config,
                r#"
timeout_ms = 15000
log_level = "debug"
watch = true
routes = ["routes/a.yaml"]

[components.http]
max_connections = 25
"#,
            )],
        );
        let merged = build_virtual_config(&store).expect("flat store config assembles");
        lock_toml_golden("case_01.toml", &merged);
    }

    /// Case (b): `[default]` + `[production]` with `production` selected
    /// deep-merges tables recursively and REPLACES the routes array.
    #[test]
    fn virtual_config_profile_selection_merges_and_replaces() {
        let store = fixture_store(
            &["Camel.toml", "production.profile.toml"],
            vec![
                store_doc(
                    "Camel.toml",
                    StoreEntryKind::Config,
                    r#"
[default]
timeout_ms = 30000
log_level = "info"
watch = false
routes = ["routes/base.yaml"]

[default.components.http]
max_connections = 10

[production]
timeout_ms = 5000
routes = ["routes/prod-main.yaml", "routes/prod-orders.yaml"]

[production.components.http]
max_connections = 99
"#,
                ),
                // The compiler synthesizes the fragment from the selected
                // section; assembly reads only the profile NAME from the
                // fragment path.
                store_doc(
                    "production.profile.toml",
                    StoreEntryKind::Profile,
                    r#"
[production]
timeout_ms = 5000
routes = ["routes/prod-main.yaml", "routes/prod-orders.yaml"]

[production.components.http]
max_connections = 99
"#,
                ),
            ],
        );
        let merged = build_virtual_config(&store).expect("profiled store config assembles");
        assert_eq!(
            merged
                .get("routes")
                .and_then(toml::Value::as_array)
                .map(Vec::len),
            Some(2),
            "the [production] routes array must REPLACE the [default] array"
        );
        lock_toml_golden("case_02.toml", &merged);
    }

    /// Case (c): two includes in declaration order sit below the config
    /// document; the config wins conflicts and the later include wins
    /// over the earlier one.
    #[test]
    fn virtual_config_includes_below_config() {
        let store = fixture_store(
            &["conf/a.toml", "conf/b.toml", "Camel.toml"],
            vec![
                store_doc(
                    "Camel.toml",
                    StoreEntryKind::Config,
                    r#"
include = ["conf/a.toml", "conf/b.toml"]

[default]
timeout_ms = 1000
routes = ["routes/config.yaml"]

[default.components.http]
max_connections = 30
"#,
                ),
                store_doc(
                    "conf/a.toml",
                    StoreEntryKind::Include,
                    r#"
[default]
timeout_ms = 111
routes = ["routes/a.yaml"]

[default.components.http]
max_connections = 11
base_url = "http://from-a"
"#,
                ),
                store_doc(
                    "conf/b.toml",
                    StoreEntryKind::Include,
                    r#"
[default]
timeout_ms = 222
routes = ["routes/b.yaml"]

[default.components.http]
max_connections = 22
base_url = "http://from-b"
"#,
                ),
            ],
        );
        let merged = build_virtual_config(&store).expect("include store config assembles");
        assert_eq!(
            merged.get("timeout_ms").and_then(toml::Value::as_integer),
            Some(1000),
            "the configuration document must outrank both includes"
        );
        assert_eq!(
            merged
                .get("components")
                .and_then(|c| c.get("http"))
                .and_then(|h| h.get("base_url"))
                .and_then(toml::Value::as_str),
            Some("http://from-b"),
            "the later include must win over the earlier one"
        );
        assert!(
            merged.get("include").is_none(),
            "config-declared include keys are stripped before merging"
        );
        lock_toml_golden("case_03.toml", &merged);
    }

    /// Case (d): a fragment declaring `include` has the key stripped
    /// (recursive includes are unsupported — the declared document is
    /// not even embedded) and the loader emits exactly one WARN
    /// diagnostic, locked in `case_04_warn.txt`.
    #[test]
    fn virtual_config_recursive_include_warns_and_strips() {
        let store = fixture_store(
            &["conf/base.toml", "Camel.toml"],
            vec![
                store_doc(
                    "Camel.toml",
                    StoreEntryKind::Config,
                    r#"
[default]
timeout_ms = 1000
log_level = "info"
"#,
                ),
                store_doc(
                    "conf/base.toml",
                    StoreEntryKind::Include,
                    r#"
include = ["conf/nested.toml"]

[default]
log_level = "debug"

[default.components.http]
base_url = "http://from-base"
"#,
                ),
            ],
        );
        let (merged, warns) = capture_warns(|| build_virtual_config(&store));
        let merged = merged.expect("assembly succeeds despite the recursive declaration");
        assert!(
            merged.get("include").is_none(),
            "the recursive include declaration must be stripped from the merged output"
        );
        assert_eq!(warns.len(), 1, "exactly one WARN expected, got: {warns:?}");
        assert!(
            warns[0].contains("recursive includes are unsupported"),
            "unexpected diagnostic: {}",
            warns[0]
        );
        lock_toml_golden("case_04.toml", &merged);
        lock_text_golden("case_04_warn.txt", &warns.join("\n"));
    }

    /// Case (e): ordered selection `profiles=["production","qa"]` with
    /// both sections present — `qa` overlays `production` in selection
    /// order and wins conflicts.
    #[test]
    fn virtual_config_multi_profile_ordered_overlay() {
        let store = fixture_store(
            &["Camel.toml", "production.profile.toml", "qa.profile.toml"],
            vec![
                store_doc(
                    "Camel.toml",
                    StoreEntryKind::Config,
                    r#"
[default]
timeout_ms = 30000
log_level = "info"
watch = false
routes = ["routes/base.yaml"]

[default.components.http]
max_connections = 10

[production]
timeout_ms = 5000
routes = ["routes/prod.yaml"]

[production.components.http]
max_connections = 50

[qa]
timeout_ms = 7000
watch = true
routes = ["routes/qa.yaml", "routes/qa-extra.yaml"]

[qa.components.http]
base_url = "http://qa"
"#,
                ),
                store_doc(
                    "production.profile.toml",
                    StoreEntryKind::Profile,
                    r#"
[production]
timeout_ms = 5000
routes = ["routes/prod.yaml"]

[production.components.http]
max_connections = 50
"#,
                ),
                store_doc(
                    "qa.profile.toml",
                    StoreEntryKind::Profile,
                    r#"
[qa]
timeout_ms = 7000
watch = true
routes = ["routes/qa.yaml", "routes/qa-extra.yaml"]

[qa.components.http]
base_url = "http://qa"
"#,
                ),
            ],
        );
        let merged = build_virtual_config(&store).expect("multi-profile store config assembles");
        assert_eq!(
            merged.get("timeout_ms").and_then(toml::Value::as_integer),
            Some(7000),
            "the later-selected qa section must win the conflict"
        );
        assert_eq!(
            merged.get("log_level").and_then(toml::Value::as_str),
            Some("info"),
            "values only [default] speaks must survive"
        );
        lock_toml_golden("case_05.toml", &merged);
    }

    /// Case (f): partial absence `profiles=["production","qa"]` with only
    /// `production` present merges `production` and does NOT error — the
    /// store backstop fires only when NO selected section exists.
    #[test]
    fn virtual_config_partial_absence_merges_present() {
        let store = fixture_store(
            &["Camel.toml", "production.profile.toml", "qa.profile.toml"],
            vec![
                store_doc(
                    "Camel.toml",
                    StoreEntryKind::Config,
                    r#"
[default]
timeout_ms = 30000
log_level = "info"
watch = false
routes = ["routes/base.yaml"]

[default.components.http]
max_connections = 10

[production]
timeout_ms = 5000
routes = ["routes/prod.yaml"]

[production.components.http]
max_connections = 50
"#,
                ),
                store_doc(
                    "production.profile.toml",
                    StoreEntryKind::Profile,
                    r#"
[production]
timeout_ms = 5000
routes = ["routes/prod.yaml"]

[production.components.http]
max_connections = 50
"#,
                ),
                store_doc(
                    "qa.profile.toml",
                    StoreEntryKind::Profile,
                    r#"
[qa]
timeout_ms = 7000
"#,
                ),
            ],
        );
        let merged = build_virtual_config(&store).expect("partial absence must merge, not fail");
        assert_eq!(
            merged.get("timeout_ms").and_then(toml::Value::as_integer),
            Some(5000),
            "the present production section must apply"
        );
        lock_toml_golden("case_06.toml", &merged);
    }

    /// Case (g): `[default]` present with every selected section absent
    /// fails with the strict unknown-profile backstop; the full
    /// `MalformedVirtualConfig` Display string is locked.
    #[test]
    fn virtual_config_unknown_profile_backstop_error_locked() {
        let store = fixture_store(
            &["Camel.toml", "staging.profile.toml"],
            vec![
                store_doc(
                    "Camel.toml",
                    StoreEntryKind::Config,
                    r#"
[default]
timeout_ms = 1000
watch = false
"#,
                ),
                store_doc(
                    "staging.profile.toml",
                    StoreEntryKind::Profile,
                    r#"
[staging]
timeout_ms = 9000
"#,
                ),
            ],
        );
        let err =
            build_virtual_config(&store).expect_err("unknown profile must fail the store backstop");
        match &err {
            DiscoveryError::MalformedVirtualConfig { .. } => {}
            other => panic!("expected MalformedVirtualConfig, got: {other:?}"),
        }
        let display = err.to_string();
        assert!(
            display.contains("unknown profile: none of the selected profiles (staging)"),
            "unexpected error spelling: {display}"
        );
        lock_text_golden("case_07_error.txt", &display);
    }

    /// Case (h): `${env:PARITY_STORE_VAR:-fallback}` passes through
    /// unresolved into the merged value — assembly never resolves env
    /// placeholders (that happens at typed-load time).
    #[test]
    fn virtual_config_env_placeholder_passes_through() {
        let store = fixture_store(
            &["Camel.toml"],
            vec![store_doc(
                "Camel.toml",
                StoreEntryKind::Config,
                r#"
log_level = "${env:PARITY_STORE_VAR:-fallback}"
timeout_ms = 1000
"#,
            )],
        );
        let merged = build_virtual_config(&store).expect("env placeholder store config assembles");
        assert_eq!(
            merged.get("log_level").and_then(toml::Value::as_str),
            Some("${env:PARITY_STORE_VAR:-fallback}"),
            "assembly must leave env placeholders raw"
        );
        lock_toml_golden("case_08.toml", &merged);
    }
}
