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

/// Classified configuration references of a store (in index order).
struct VirtualConfigRefs {
    /// The `Camel.toml` document path, when the store embeds one.
    config: Option<String>,
    /// Include fragment paths in declaration order.
    includes: Vec<String>,
    /// Selected profile names in selection order, from the synthesized
    /// `<name>.profile.toml` fragment paths.
    profiles: Vec<String>,
}

/// Classify `config_references` by document kind. Every reference must
/// name a config, include, or profile entry; violations surface as the
/// named store errors (missing reference, kind mismatch) and structural
/// breakage (duplicate config document, malformed profile fragment
/// path) as `MalformedVirtualConfig`.
fn classify_virtual_config(
    store: &VirtualDocumentStore,
) -> Result<VirtualConfigRefs, DiscoveryError> {
    let mut refs = VirtualConfigRefs {
        config: None,
        includes: Vec::new(),
        profiles: Vec::new(),
    };
    for path in &store.index.config_references {
        let entry = store
            .index
            .entry(path)
            .ok_or_else(|| StoreError::MissingReference(path.clone()))?;
        match entry.kind {
            StoreEntryKind::Config => {
                if refs.config.replace(path.clone()).is_some() {
                    return Err(DiscoveryError::MalformedVirtualConfig {
                        path: path.clone(),
                        error: "duplicate configuration document".to_string(),
                    });
                }
            }
            StoreEntryKind::Include => refs.includes.push(path.clone()),
            StoreEntryKind::Profile => {
                let Some(name) = path.strip_suffix(".profile.toml").filter(|n| !n.is_empty())
                else {
                    return Err(DiscoveryError::MalformedVirtualConfig {
                        path: path.clone(),
                        error: "profile entry path must be `<name>.profile.toml`".to_string(),
                    });
                };
                refs.profiles.push(name.to_string());
            }
            kind => {
                return Err(StoreError::KindMismatch {
                    path: path.clone(),
                    expected: "config, include, or profile",
                    got: kind.as_str(),
                }
                .into());
            }
        }
    }
    Ok(refs)
}

/// Build the merged configuration value from the indexed
/// config/include/profile texts, mirroring camel-config's
/// `load_includes` + `build_from_toml_value_inner` ordering: includes
/// are pre-sources in declaration order (lowest priority), the
/// configuration document sits above them, and profile-section
/// selection applies per document before merging.
///
/// SYNC: the mirror lives here because the dependency direction
/// (camel-config depends on camel-dsl) forbids sharing the camel-config
/// `pub(crate)` helpers; behavioral changes there must be reflected
/// here and in the compiler's `camel-cli` `compile::sources`.
fn build_virtual_config(store: &VirtualDocumentStore) -> Result<toml::Value, DiscoveryError> {
    let refs = classify_virtual_config(store)?;

    // Includes (lowest priority), in declaration order. Each fragment
    // drops any `include` key (recursive includes are unsupported, as
    // in camel-config's loader) and applies lenient profile-section
    // selection before merging.
    let mut merged = toml::Value::Table(toml::Table::new());
    for path in &refs.includes {
        let text = virtual_config_text(store, path)?;
        let mut value = parse_virtual_config_toml(path, &text)?;
        if let toml::Value::Table(table) = &mut value
            && table.remove("include").is_some()
        {
            tracing::warn!(
                path,
                "embedded include declares 'include'; recursive includes are unsupported — ignoring"
            );
        }
        select_profile_sections(&mut value, &refs.profiles);
        merge_toml_values(&mut merged, &value);
    }

    // The configuration document above the includes: strip `include`
    // keys from every declaring location (top-level, `[default]`, and
    // the selected profile sections), enforce the strict unknown-profile
    // rule (a configuration with `[default]` must carry every selected
    // profile section — the filesystem loader's error), then apply the
    // profile-section selection and merge.
    if let Some(path) = &refs.config {
        let text = virtual_config_text(store, path)?;
        let mut value = parse_virtual_config_toml(path, &text)?;
        strip_include_keys(&mut value, &refs.profiles);
        if let toml::Value::Table(table) = &value {
            let has_structure = table.contains_key("default")
                || refs.profiles.iter().any(|p| table.contains_key(p));
            let selected_present = refs.profiles.iter().any(|p| table.contains_key(p));
            if has_structure
                && !refs.profiles.is_empty()
                && table.contains_key("default")
                && !selected_present
            {
                return Err(DiscoveryError::MalformedVirtualConfig {
                    path: path.clone(),
                    error: format!(
                        "unknown profile: none of the selected profiles ({}) exist in the \
                         configuration",
                        refs.profiles.join(", ")
                    ),
                });
            }
        }
        select_profile_sections(&mut value, &refs.profiles);
        merge_toml_values(&mut merged, &value);
    }

    Ok(merged)
}

/// Read one configuration document as UTF-8 text (named failure for
/// missing-validity).
fn virtual_config_text(store: &VirtualDocumentStore, path: &str) -> Result<String, DiscoveryError> {
    store.read_text(path).map(str::to_string).ok_or_else(|| {
        DiscoveryError::MalformedVirtualConfig {
            path: path.to_string(),
            error: "configuration document is not valid UTF-8".to_string(),
        }
    })
}

/// Parse one configuration document as TOML (named failure).
fn parse_virtual_config_toml(path: &str, text: &str) -> Result<toml::Value, DiscoveryError> {
    toml::from_str(text).map_err(|e| DiscoveryError::MalformedVirtualConfig {
        path: path.to_string(),
        error: e.to_string(),
    })
}

/// Remove `include` keys from the top-level table and from the
/// `[default]` plus selected profile sections, mirroring camel-config's
/// `extract_includes` stripping (the embedded include order already
/// encodes the same walk).
fn strip_include_keys(value: &mut toml::Value, profiles: &[String]) {
    let Some(table) = value.as_table_mut() else {
        return;
    };
    table.remove("include");
    let mut sections: Vec<&str> = vec!["default"];
    sections.extend(profiles.iter().map(String::as_str));
    for section in sections {
        if let Some(toml::Value::Table(section_table)) = table.get_mut(section) {
            section_table.remove("include");
        }
    }
}

/// Apply the filesystem profile-section selection to one document,
/// generalized to the store's ordered selected profiles: the
/// `[default]` section forms the base when present (else the first
/// selected section), every selected profile section overlays it in
/// selection order, and the selected content REPLACES the document
/// root. A document with neither `[default]` nor any selected section
/// stays as-is (flat config).
///
/// SYNC: mirrors camel-config's `apply_profile` and
/// `apply_profile_lenient` (`config.rs`, `pub(crate)`); with exactly one
/// selected profile the selection is byte-for-byte the filesystem
/// behavior.
fn select_profile_sections(value: &mut toml::Value, profiles: &[String]) {
    let Some(table) = value.as_table_mut() else {
        return;
    };
    let mut base = match table.get("default").cloned() {
        Some(default) => default,
        None => match profiles.iter().find(|p| table.contains_key(p.as_str())) {
            Some(first) => match table.get(first.as_str()) {
                Some(section) => section.clone(),
                // `find` proved presence; unreachable in practice.
                None => return,
            },
            // Flat document with no profile structure: keep as-is.
            None => return,
        },
    };
    for profile in profiles {
        if let Some(section) = table.get(profile.as_str()) {
            merge_toml_values(&mut base, section);
        }
    }
    *value = base;
}

/// Deep-merge `overlay` into `base`: tables merge recursively, every
/// other value (arrays included) is replaced by the overlay — array
/// replacement is what gives profile overlays such as `routes` their
/// replace, never concatenate, semantics.
///
/// SYNC: mirrors camel-config's `merge_toml_values` (`config.rs`,
/// `pub(crate)`); the dependency direction forbids sharing the
/// implementation, so behavioral changes there must be mirrored here.
fn merge_toml_values(base: &mut toml::Value, overlay: &toml::Value) {
    match (base, overlay) {
        (toml::Value::Table(base_table), toml::Value::Table(overlay_table)) => {
            for (key, value) in overlay_table {
                if let Some(base_value) = base_table.get_mut(key) {
                    merge_toml_values(base_value, value);
                } else {
                    base_table.insert(key.clone(), value.clone());
                }
            }
        }
        (base, overlay) => *base = overlay.clone(),
    }
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
}
