//! Compile-time multi-document source resolution and confinement
//! (openspec change `multidoc`, Task 1.2).
//!
//! [`SourceSelection`] carries the explicit compile inputs — an optional
//! `--config <Camel.toml>` and ordered `--profile <name>` selections.
//! [`resolve`] turns the primary document plus that selection into the
//! typed document set of a v2 virtual store:
//!
//! - the primary document (the logical entry point),
//! - the explicit `Camel.toml`, its ordered includes (top-level,
//!   `[default]`, then each selected profile section — mirroring
//!   `camel-config`'s ordered include walk), and the selected profile
//!   sections as `Profile` fragments in flag order,
//! - route-file patterns resolved from the document's
//!   `routeFiles`/`routeFilesFromRoot` fields and the config's `routes`
//!   patterns, in declared order with each pattern's matches sorted by
//!   normalized logical path.
//!
//! Confinement is fail-closed for EVERY source: names normalize to UTF-8
//! relative `/` paths anchored at the selected root (the config's
//! directory, or the primary document's directory without `--config`),
//! symlink-aware canonicalization must keep each source under that root,
//! and absolute paths, empty/dot/traversal components, non-UTF-8 names,
//! missing sources, duplicate canonical targets, and duplicate
//! normalized paths are all named rejections. The declared-path rules
//! mirror `camel_config::include::resolve_include_path` (relative,
//! no traversal, canonicalization-checked) so one uniform resolver
//! covers includes, route patterns, and operator-named files.
//!
//! Nothing here reads an ambient `Camel.toml` or walks ancestor
//! directories: without `--config` no configuration is resolved at all,
//! and `routeFilesFromRoot` — whose runtime anchor is a discovered
//! Camel.toml root — is rejected instead.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::{Component, Path, PathBuf};

use noyalib::compat::serde_yaml as serde_yml;

use super::CompileError;
use super::store::{StoreDocument, StoreEntryKind, validate_path};
use super::trailer::{self, TrailerKind};

/// Explicit compile-time source selection (the `--config` and
/// `--profile` flags). With neither present the compiler embeds no
/// configuration and selects no profile.
#[derive(Debug, Clone, Default)]
pub struct SourceSelection {
    /// Explicitly selected configuration document (`--config
    /// <Camel.toml>`); its directory becomes the confinement root.
    pub config_path: Option<PathBuf>,
    /// Selected profile names in declaration order (`--profile <name>`,
    /// repeatable).
    pub profiles: Vec<String>,
}

/// The resolved, confined document set for one compile.
#[derive(Debug, Clone)]
pub struct ResolvedSources {
    /// Artifact kind of the primary document.
    pub kind: TrailerKind,
    /// Logical path of the primary document (the store entry point).
    pub entry_point: String,
    /// Every embedded document (normalized bytes included).
    pub documents: Vec<StoreDocument>,
    /// Configuration/include/profile entry paths in resolution order.
    pub config_references: Vec<String>,
    /// Ordered route-source plan: the entry document first, then each
    /// declared pattern's matches sorted by normalized logical path.
    pub source_plan: Vec<String>,
    /// `(logical path, normalized text)` of every route/job document in
    /// plan order, for the asset policy and manifest derivation.
    pub route_documents: Vec<(String, String)>,
}

/// Named source-resolution failures. Every variant carries the operator
///-facing diagnostic text; the compile command maps them all to exit 2
/// before any output byte exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceError {
    /// A declared source path is not a canonical relative name
    /// (absolute, empty/dot/traversal component, backslash, control
    /// byte, or a dynamic `${...}` placeholder).
    InvalidName(String),
    /// A source's on-disk name is not valid UTF-8.
    NonUtf8Name(String),
    /// A source's content is not valid UTF-8.
    NonUtf8Content(String),
    /// A source named by the operator or a literal pattern is missing.
    MissingSource(String),
    /// A source resolves (after symlink-aware canonicalization) outside
    /// the selected root.
    OutsideRoot {
        /// The source as declared (display form).
        declared: String,
        /// The canonical confinement root.
        root: PathBuf,
    },
    /// Two declared sources claim the same canonical file or the same
    /// normalized logical path.
    DuplicateSource {
        /// The colliding normalized logical path.
        logical: String,
    },
    /// `--profile` was supplied without `--config`.
    ProfileRequiresConfig,
    /// The document declares `routeFilesFromRoot` but no `--config`
    /// root was selected (compile performs no ambient discovery).
    RouteFilesFromRootRequiresConfig,
    /// A selected profile section exists nowhere in the configuration.
    UnknownProfile(String),
    /// The configuration or an include is not valid TOML (or a required
    /// field has the wrong shape).
    InvalidConfig(String),
    /// The primary document is not a usable YAML/JSON document.
    InvalidDocument(String),
    /// A route-file pattern or a literal route source is unusable
    /// (reserved document suffix, unsupported extension, malformed
    /// glob).
    UnsupportedRouteSource(String),
    /// Normalization failed for the collected document set (invalid
    /// UTF-8, or the aggregate 16 MiB embedded-byte cap).
    Normalize(CompileError),
}

impl fmt::Display for SourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidName(declared) => write!(
                f,
                "invalid source path '{declared}': source paths must be canonical relative \
                 names (absolute paths, '.', '..', empty, or backslash components, and \
                 dynamic placeholders are rejected)"
            ),
            Self::NonUtf8Name(name) => {
                write!(f, "source name is not valid UTF-8: {name:?}")
            }
            Self::NonUtf8Content(path) => {
                write!(f, "source content is not valid UTF-8: {path:?}")
            }
            Self::MissingSource(declared) => write!(f, "missing source '{declared}'"),
            Self::OutsideRoot { declared, root } => write!(
                f,
                "source '{declared}' resolves outside the selected root '{}' (confinement \
                 rejected; symlink escapes are not embeddable)",
                root.display()
            ),
            Self::DuplicateSource { logical } => write!(
                f,
                "duplicate source '{logical}': two declared sources claim the same file or \
                 logical path; overlapping patterns must be rejected, not silently embedded \
                 twice"
            ),
            Self::ProfileRequiresConfig => write!(
                f,
                "--profile requires --config: profiles are selected from an explicitly \
                 supplied Camel.toml, never discovered"
            ),
            Self::RouteFilesFromRootRequiresConfig => write!(
                f,
                "routeFilesFromRoot requires an explicitly selected --config root: compile \
                 performs no ambient Camel.toml discovery"
            ),
            Self::UnknownProfile(name) => write!(
                f,
                "unknown profile '{name}': the selected profile section must exist in the \
                 explicit configuration"
            ),
            Self::InvalidConfig(reason) => write!(f, "invalid configuration: {reason}"),
            Self::InvalidDocument(reason) => write!(f, "invalid document: {reason}"),
            Self::UnsupportedRouteSource(reason) => write!(f, "unsupported route source: {reason}"),
            Self::Normalize(CompileError::PayloadTooLarge) => write!(
                f,
                "embedded documents exceed the aggregate 16 MiB compile payload limit"
            ),
            Self::Normalize(inner) => write!(f, "{inner}"),
        }
    }
}

impl std::error::Error for SourceError {}

/// One collected raw source before normalization.
struct RawSource {
    path: String,
    kind: StoreEntryKind,
    bytes: Vec<u8>,
}

/// Claim bookkeeping: every canonical file and every normalized logical
/// path may be claimed exactly once.
struct Resolver {
    /// Canonical confinement root.
    root: PathBuf,
    /// Canonical source path -> logical path (duplicate-target guard).
    claimed_files: HashMap<PathBuf, String>,
    /// Normalized logical paths (duplicate-path guard).
    claimed_paths: HashSet<String>,
}

impl Resolver {
    fn new(root: PathBuf) -> Self {
        Self {
            root,
            claimed_files: HashMap::new(),
            claimed_paths: HashSet::new(),
        }
    }

    /// Symlink-aware confinement: `canonical` must stay under the root.
    fn confine(&self, canonical: &Path, declared: &str) -> Result<(), SourceError> {
        if canonical.starts_with(&self.root) {
            Ok(())
        } else {
            Err(SourceError::OutsideRoot {
                declared: declared.to_string(),
                root: self.root.clone(),
            })
        }
    }

    /// Normalized logical path of a canonical source: UTF-8 relative
    /// `/` path anchored at the root. The canonical-path rule of
    /// [`validate_path`] applies as the final gate (control bytes and
    /// other foreign forms never reach the store).
    fn logical_of(&self, canonical: &Path) -> Result<String, SourceError> {
        let rel = canonical
            .strip_prefix(&self.root)
            .map_err(|_| SourceError::OutsideRoot {
                declared: canonical.display().to_string(),
                root: self.root.clone(),
            })?;
        let mut logical = String::new();
        for component in rel.components() {
            let Component::Normal(part) = component else {
                return Err(SourceError::InvalidName(canonical.display().to_string()));
            };
            let part = part
                .to_str()
                .ok_or_else(|| SourceError::NonUtf8Name(canonical.display().to_string()))?;
            if !logical.is_empty() {
                logical.push('/');
            }
            logical.push_str(part);
        }
        validate_path(&logical).map_err(|_| SourceError::InvalidName(logical.clone()))?;
        Ok(logical)
    }

    /// Claim a canonical file and its logical path; a second claim of
    /// either is a named duplicate.
    fn claim_file(&mut self, canonical: PathBuf, logical: String) -> Result<(), SourceError> {
        if self
            .claimed_files
            .insert(canonical, logical.clone())
            .is_some()
        {
            return Err(SourceError::DuplicateSource { logical });
        }
        self.claim_path(logical)
    }

    /// Claim a synthesized logical path (no backing file, e.g. a profile
    /// fragment).
    fn claim_path(&mut self, logical: String) -> Result<(), SourceError> {
        if !self.claimed_paths.insert(logical.clone()) {
            return Err(SourceError::DuplicateSource { logical });
        }
        Ok(())
    }

    /// Resolve an operator-named file (the primary document or the
    /// explicit config): canonicalize as given, confine, and claim. The
    /// operator may name the file through any spellable path; only
    /// confinement and name normalization apply.
    fn resolve_named(&mut self, named: &Path) -> Result<(PathBuf, String), SourceError> {
        let canonical = named
            .canonicalize()
            .map_err(|_| SourceError::MissingSource(named.display().to_string()))?;
        self.confine(&canonical, &named.display().to_string())?;
        let logical = self.logical_of(&canonical)?;
        self.claim_file(canonical.clone(), logical.clone())?;
        Ok((canonical, logical))
    }

    /// Resolve one declared relative source name (an include entry):
    /// validate the declared form, join onto `base`, canonicalize,
    /// confine, and claim. Mirrors the
    /// `camel_config::include::resolve_include_path` rules.
    fn resolve_declared_file(
        &mut self,
        declared: &str,
        base: &Path,
    ) -> Result<(PathBuf, String), SourceError> {
        validate_declared(declared)?;
        let canonical = base
            .join(declared)
            .canonicalize()
            .map_err(|_| SourceError::MissingSource(declared.to_string()))?;
        self.confine(&canonical, declared)?;
        let logical = self.logical_of(&canonical)?;
        self.claim_file(canonical.clone(), logical.clone())?;
        Ok((canonical, logical))
    }

    /// Expand one route-file pattern against `base` and claim its
    /// matches: declared pattern order is the caller's, each pattern's
    /// matches are sorted by normalized logical path, and a literal
    /// pattern that matches nothing is a missing source. Reserved
    /// document suffixes and the JSON explicit-pattern gate follow
    /// filesystem discovery semantics (`camel_dsl::discovery`).
    fn expand_pattern(
        &mut self,
        declared: &str,
        base: &Path,
    ) -> Result<Vec<(PathBuf, String)>, SourceError> {
        validate_declared(declared)?;
        let pattern = base.join(declared);
        let pattern_str = pattern.to_string_lossy().into_owned();
        let literal = camel_dsl::discovery::pattern_is_literal(declared);
        let json_authorized = camel_dsl::discovery::pattern_targets_json(declared);
        let entries = glob::glob(&pattern_str).map_err(|e| {
            SourceError::UnsupportedRouteSource(format!("malformed glob pattern '{declared}': {e}"))
        })?;

        let mut matches: Vec<(PathBuf, String)> = Vec::new();
        for entry in entries {
            let path = entry.map_err(|e| {
                SourceError::UnsupportedRouteSource(format!(
                    "cannot enumerate pattern '{declared}' at '{}': {e}",
                    e.path().display()
                ))
            })?;
            // Reserved-document gate, discovery parity: `.test.*` and
            // `.job.*` documents are never routes. A literal pattern
            // naming one is a named rejection; a wildcard skips them.
            if camel_dsl::discovery::is_reserved_document(&path) {
                if literal {
                    return Err(SourceError::UnsupportedRouteSource(format!(
                        "'{}' is a reserved test/job document, not a route source",
                        path.display()
                    )));
                }
                continue;
            }
            // Extension gate, discovery parity: YAML/YML always, JSON
            // only under an explicit .json pattern, anything else named.
            let ext = path
                .extension()
                .map(|ext| ext.to_string_lossy().to_lowercase());
            match ext.as_deref() {
                Some("yaml") | Some("yml") => {}
                Some("json") if json_authorized => {}
                _ => {
                    return Err(SourceError::UnsupportedRouteSource(format!(
                        "'{}' is not a route document (expected *.yaml, *.yml, or an \
                         explicit *.json pattern)",
                        path.display()
                    )));
                }
            }
            let canonical = path
                .canonicalize()
                .map_err(|_| SourceError::MissingSource(path.display().to_string()))?;
            self.confine(&canonical, &path.display().to_string())?;
            let logical = self.logical_of(&canonical)?;
            matches.push((canonical, logical));
        }
        if matches.is_empty() && literal {
            return Err(SourceError::MissingSource(declared.to_string()));
        }
        matches.sort_by(|a, b| a.1.cmp(&b.1));
        for (canonical, logical) in &matches {
            self.claim_file(canonical.clone(), logical.clone())?;
        }
        Ok(matches)
    }
}

/// Validate a declared source name: non-empty, relative, no empty/`.`/
/// `..` components, no backslash, no control byte, and no dynamic
/// `${...}` placeholder (a placeholder cannot be resolved
/// deterministically at compile time).
fn validate_declared(declared: &str) -> Result<(), SourceError> {
    let invalid = || SourceError::InvalidName(declared.to_string());
    if declared.is_empty()
        || declared.starts_with('/')
        || declared.contains('\\')
        || declared.contains("${")
        || declared.bytes().any(|b| b < 0x20 || b == 0x7F)
    {
        return Err(invalid());
    }
    for segment in declared.split('/') {
        if segment.is_empty() || segment == "." || segment == ".." {
            return Err(invalid());
        }
    }
    Ok(())
}

/// Validate a profile name before it becomes a synthesized logical
/// path (`<name>.profile.toml`).
fn validate_profile_name(name: &str) -> Result<(), SourceError> {
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.contains(['/', '\\'])
        || name.bytes().any(|b| b < 0x20 || b == 0x7F)
    {
        return Err(SourceError::InvalidName(name.to_string()));
    }
    Ok(())
}

/// String list from a YAML value (a sequence of strings or one string).
fn yaml_string_list(owner: &str, value: &serde_yml::Value) -> Result<Vec<String>, SourceError> {
    match value {
        serde_yml::Value::String(s) => Ok(vec![s.clone()]),
        serde_yml::Value::Sequence(seq) => {
            let mut out = Vec::with_capacity(seq.len());
            for item in seq {
                match item.as_str() {
                    Some(s) => out.push(s.to_string()),
                    None => {
                        return Err(SourceError::InvalidDocument(format!(
                            "{owner} must be a list of strings"
                        )));
                    }
                }
            }
            Ok(out)
        }
        _ => Err(SourceError::InvalidDocument(format!(
            "{owner} must be a list of strings"
        ))),
    }
}

/// String list from a TOML value (an array of strings).
fn toml_string_list(owner: &str, value: &toml::Value) -> Result<Vec<String>, SourceError> {
    let toml::Value::Array(items) = value else {
        return Err(SourceError::InvalidConfig(format!(
            "{owner} must be an array of strings"
        )));
    };
    items
        .iter()
        .map(|item| {
            item.as_str().map(str::to_string).ok_or_else(|| {
                SourceError::InvalidConfig(format!("{owner} must be an array of strings"))
            })
        })
        .collect()
}

/// Read a source file's raw bytes.
fn read_source(canonical: &Path) -> Result<Vec<u8>, SourceError> {
    std::fs::read(canonical)
        .map_err(|e| SourceError::MissingSource(format!("{}: {e}", canonical.display())))
}

/// Resolve every compile-time source for one `camel compile` invocation.
///
/// The primary document anchors the artifact: it is always the first
/// plan reference and the store entry point. With `--config`, the
/// config's directory is the confinement root and the configuration
/// chain (config, ordered includes, selected profile fragments) is
/// embedded; without it, the document's directory is the root and no
/// configuration is read.
pub fn resolve(
    document: &Path,
    kind: TrailerKind,
    selection: &SourceSelection,
) -> Result<ResolvedSources, SourceError> {
    if !selection.profiles.is_empty() && selection.config_path.is_none() {
        return Err(SourceError::ProfileRequiresConfig);
    }

    // Confinement root: the explicit config's directory, else the
    // primary document's directory. No ancestor walk, no ambient
    // discovery. A bare file name anchors at the current directory.
    let root_raw = match &selection.config_path {
        Some(config) => parent_or_cwd(config),
        None => parent_or_cwd(document),
    };
    let root = root_raw
        .canonicalize()
        .map_err(|_| SourceError::MissingSource(root_raw.display().to_string()))?;
    let mut resolver = Resolver::new(root.clone());

    // Primary document: entry point and first plan reference.
    let (doc_canonical, doc_logical) = resolver.resolve_named(document)?;
    let doc_base = parent_dir(&doc_canonical)?;
    let mut raws = vec![RawSource {
        path: doc_logical.clone(),
        kind: StoreEntryKind::from(kind),
        bytes: read_source(&doc_canonical)?,
    }];
    let mut plan = vec![doc_logical.clone()];

    // Document-declared route sources. The normalized text (not the raw
    // bytes) is parsed: field extraction sees exactly what embedding
    // will store.
    let entry_text = trailer::normalize_document(&raws[0].bytes).map_err(SourceError::Normalize)?;
    let entry_fields = route_source_fields(&entry_text)?;
    match entry_fields {
        RouteSourceFields::RouteFiles(patterns) => {
            for declared in patterns {
                for (canonical, logical) in resolver.expand_pattern(&declared, &doc_base)? {
                    raws.push(RawSource {
                        path: logical.clone(),
                        kind: StoreEntryKind::Route,
                        bytes: read_source(&canonical)?,
                    });
                    plan.push(logical);
                }
            }
        }
        RouteSourceFields::RouteFilesFromRoot(patterns) => {
            if selection.config_path.is_none() {
                return Err(SourceError::RouteFilesFromRootRequiresConfig);
            }
            for declared in patterns {
                for (canonical, logical) in resolver.expand_pattern(&declared, &root)? {
                    raws.push(RawSource {
                        path: logical.clone(),
                        kind: StoreEntryKind::Route,
                        bytes: read_source(&canonical)?,
                    });
                    plan.push(logical);
                }
            }
        }
        RouteSourceFields::None => {}
    }

    // Explicit configuration chain.
    let mut config_references: Vec<String> = Vec::new();
    if let Some(config_path) = &selection.config_path {
        let (config_canonical, config_logical) = resolver.resolve_named(config_path)?;
        let config_raw = read_source(&config_canonical)?;
        let config_text = String::from_utf8(config_raw.clone())
            .map_err(|_| SourceError::NonUtf8Content(config_canonical.display().to_string()))?;
        let config: toml::Value = toml::from_str(&config_text).map_err(|e| {
            SourceError::InvalidConfig(format!("{}: {e}", config_canonical.display()))
        })?;

        // Ordered include walk, mirroring camel-config's
        // `extract_includes`: top-level, `[default]`, then each selected
        // non-default profile section (in flag order, deduplicated).
        let mut include_decls: Vec<String> = Vec::new();
        if let Some(value) = config.get("include") {
            include_decls.extend(toml_string_list("include", value)?);
        }
        let mut sections: Vec<String> = vec!["default".to_string()];
        for profile in &selection.profiles {
            if profile != "default" && !sections.iter().any(|s| s == profile) {
                sections.push(profile.clone());
            }
        }
        for section in &sections {
            if let Some(toml::Value::Table(table)) = config.get(section)
                && let Some(value) = table.get("include")
            {
                include_decls.extend(toml_string_list(&format!("{section}.include"), value)?);
            }
        }

        // Route patterns from the config, with camel-config overlay
        // semantics: top-level `routes`, then each declaring section
        // ([default], selected profiles) replaces the accumulated list.
        let mut route_patterns: Option<Vec<String>> = None;
        if let Some(value) = config.get("routes") {
            route_patterns = Some(toml_string_list("routes", value)?);
        }
        for section in &sections {
            if let Some(toml::Value::Table(table)) = config.get(section)
                && let Some(value) = table.get("routes")
            {
                route_patterns = Some(toml_string_list(&format!("{section}.routes"), value)?);
            }
        }

        // The config document itself.
        raws.push(RawSource {
            path: config_logical.clone(),
            kind: StoreEntryKind::Config,
            bytes: config_raw,
        });
        config_references.push(config_logical);

        // Ordered includes: claimed, confined, embedded verbatim, and
        // retained for profile-section extraction (camel-config applies
        // profile sections per file, config first).
        let mut include_tables: Vec<toml::Value> = Vec::with_capacity(include_decls.len());
        for declared in include_decls {
            let (canonical, logical) = resolver.resolve_declared_file(&declared, &root)?;
            let bytes = read_source(&canonical)?;
            let text = String::from_utf8(bytes.clone())
                .map_err(|_| SourceError::NonUtf8Content(canonical.display().to_string()))?;
            let table: toml::Value = toml::from_str(&text)
                .map_err(|e| SourceError::InvalidConfig(format!("{}: {e}", canonical.display())))?;
            raws.push(RawSource {
                path: logical.clone(),
                kind: StoreEntryKind::Include,
                bytes,
            });
            config_references.push(logical);
            include_tables.push(table);
        }

        // Selected profile fragments in flag order: the first section
        // (config, then includes in resolution order) declaring the
        // profile becomes a synthesized `<name>.profile.toml` entry.
        // Repeated flags for the same profile are deduplicated (first
        // occurrence preserved) so `--profile prod --profile prod`
        // synthesizes one fragment instead of colliding on its path.
        let mut selected_profiles: Vec<&String> = Vec::new();
        for name in &selection.profiles {
            if !selected_profiles.contains(&name) {
                selected_profiles.push(name);
            }
        }
        for name in selected_profiles {
            validate_profile_name(name)?;
            let mut found = config
                .get(name)
                .filter(|v| matches!(v, toml::Value::Table(_)));
            if found.is_none() {
                for table in &include_tables {
                    if let Some(value) = table
                        .get(name)
                        .filter(|v| matches!(v, toml::Value::Table(_)))
                    {
                        found = Some(value);
                        break;
                    }
                }
            }
            let section = found.ok_or_else(|| SourceError::UnknownProfile(name.clone()))?;
            let mut wrapper = toml::Table::new();
            wrapper.insert(name.clone(), section.clone());
            let text = toml::to_string(&toml::Value::Table(wrapper))
                .map_err(|e| SourceError::InvalidConfig(format!("profile '{name}': {e}")))?;
            let logical = format!("{name}.profile.toml");
            resolver.claim_path(logical.clone())?;
            raws.push(RawSource {
                path: logical.clone(),
                kind: StoreEntryKind::Profile,
                bytes: text.into_bytes(),
            });
            config_references.push(logical);
        }

        // Config route patterns, declared order, matches sorted.
        if let Some(patterns) = route_patterns {
            for declared in patterns {
                for (canonical, logical) in resolver.expand_pattern(&declared, &root)? {
                    raws.push(RawSource {
                        path: logical.clone(),
                        kind: StoreEntryKind::Route,
                        bytes: read_source(&canonical)?,
                    });
                    plan.push(logical);
                }
            }
        }
    }

    // Normalize the whole set with the aggregate 16 MiB cap. The entry
    // text was normalized above for field extraction; normalization is
    // idempotent (BOM removal, CRLF/CR folding), so re-normalizing it
    // here is a no-op.
    let raw_slices: Vec<&[u8]> = raws.iter().map(|raw| raw.bytes.as_slice()).collect();
    let normalized = trailer::normalize_documents(&raw_slices).map_err(SourceError::Normalize)?;
    let documents: Vec<StoreDocument> = raws
        .iter()
        .zip(normalized)
        .map(|(raw, text)| StoreDocument {
            path: raw.path.clone(),
            kind: raw.kind,
            bytes: text.into_bytes(),
        })
        .collect();

    // Route/job documents in plan order for policy + manifest.
    let mut route_documents = Vec::with_capacity(plan.len());
    for path in &plan {
        let text = documents
            .iter()
            .find(|doc| &doc.path == path)
            .map(|doc| std::str::from_utf8(&doc.bytes))
            .expect("every plan reference names a collected document"); // allow-unwrap
        let text = text.map_err(|_| SourceError::NonUtf8Content(path.clone()))?;
        route_documents.push((path.clone(), text.to_string()));
    }

    Ok(ResolvedSources {
        kind,
        entry_point: doc_logical,
        documents,
        config_references,
        source_plan: plan,
        route_documents,
    })
}

/// The document-declared route-source fields.
enum RouteSourceFields {
    RouteFiles(Vec<String>),
    RouteFilesFromRoot(Vec<String>),
    None,
}

/// Extract `routeFiles`/`routeFilesFromRoot` from the normalized primary
/// document. The two forms are mutually exclusive (family rule), the
/// snake-case spellings are rejected as misspellings of the canonical
/// vocabulary, and a non-list shape is a document error.
fn route_source_fields(entry_text: &str) -> Result<RouteSourceFields, SourceError> {
    let value: serde_yml::Value = serde_yml::from_str(entry_text)
        .map_err(|e| SourceError::InvalidDocument(format!("not a YAML/JSON document: {e}")))?;
    for misspelling in ["route_files", "route_files_from_root"] {
        if value.get(misspelling).is_some() {
            return Err(SourceError::InvalidDocument(format!(
                "field '{misspelling}' is not recognized: use the canonical '{}' spelling",
                if misspelling == "route_files" {
                    "routeFiles"
                } else {
                    "routeFilesFromRoot"
                }
            )));
        }
    }
    let route_files = value.get("routeFiles");
    let from_root = value.get("routeFilesFromRoot");
    match (route_files, from_root) {
        (Some(_), Some(_)) => Err(SourceError::InvalidDocument(
            "a document may declare routeFiles or routeFilesFromRoot, not both".to_string(),
        )),
        (Some(value), None) => Ok(RouteSourceFields::RouteFiles(yaml_string_list(
            "routeFiles",
            value,
        )?)),
        (None, Some(value)) => Ok(RouteSourceFields::RouteFilesFromRoot(yaml_string_list(
            "routeFilesFromRoot",
            value,
        )?)),
        (None, None) => Ok(RouteSourceFields::None),
    }
}

/// Parent directory of `path`, named for the input when absent.
fn parent_dir(path: &Path) -> Result<PathBuf, SourceError> {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .ok_or_else(|| SourceError::MissingSource(path.display().to_string()))
}

/// Root-anchoring parent: like [`parent_dir`] but a bare file name
/// anchors at the current directory (`.`), matching how the operator
/// named it.
fn parent_or_cwd(path: &Path) -> PathBuf {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => PathBuf::from("."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Declared source names reject absolute paths, traversal, dot and
    /// empty components, backslashes, control bytes, and dynamic
    /// placeholders; canonical relative names and glob metacharacters
    /// pass.
    #[test]
    fn validate_declared_rejects_noncanonical_forms() {
        for bad in [
            "",
            "/abs/a.yaml",
            "../outside.yaml",
            "routes/../a.yaml",
            "routes/./a.yaml",
            "routes//a.yaml",
            "routes/",
            ".",
            "..",
            "routes\\a.yaml",
            "rou\0te",
            "routes/${env:DIR}/a.yaml",
        ] {
            assert!(
                validate_declared(bad).is_err(),
                "{bad:?} must be rejected as a declared source name"
            );
        }
        for good in [
            "a.yaml",
            "routes/*.yaml",
            "routes/**/a.yaml",
            "conf/base.toml",
        ] {
            assert_eq!(validate_declared(good), Ok(()), "{good:?} must be accepted");
        }
    }

    /// Profile names that cannot become synthesized logical paths are
    /// rejected before any file is touched.
    #[test]
    fn validate_profile_name_rejects_path_shapes() {
        for bad in ["", ".", "..", "a/b", "a\\b", "a\nb"] {
            assert!(
                validate_profile_name(bad).is_err(),
                "{bad:?} must be rejected"
            );
        }
        assert_eq!(validate_profile_name("prod"), Ok(()));
        assert_eq!(validate_profile_name("prod.eu"), Ok(()));
    }
}
