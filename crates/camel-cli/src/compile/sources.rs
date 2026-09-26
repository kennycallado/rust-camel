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
//!   `[default]`, then each selected profile section — walk order
//!   defined by `camel_dsl::config_semantics`), and the selected
//!   profile sections as `Profile` fragments in flag order,
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

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;
use std::path::{Component, Path, PathBuf};

use noyalib::compat::serde_yaml as serde_yml;

use super::CompileError;
use super::policy::{self, AssetRef};
use super::store::{
    StoreAsset, StoreDocument, StoreEntryKind, SubstitutionContext, SubstitutionEntry,
    SubstitutionSpan, validate_path,
};
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
    /// Configured aggregate payload cap in bytes (`--max-payload-bytes`):
    /// normalized document bytes plus verbatim asset bytes must fit under
    /// it. Always positive.
    pub max_payload_bytes: u64,
    /// Secret-embed opt-in (`--embed-secrets`): without it, a collected
    /// secret-family asset (exactly the private-key family) fails the
    /// compile closed; with it, secret material embeds normally.
    pub embed_secrets: bool,
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
    /// Confined deploy-time assets (r2embed Task 1.2): one entry per
    /// canonical target, bytes embedded verbatim.
    pub assets: Vec<StoreAsset>,
    /// The compile-time substitution table: every (document, declared
    /// string) spelling pair with its byte spans and asset logical path.
    pub substitutions: Vec<SubstitutionEntry>,
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
    /// Selected profile sections exist only in includes while the
    /// configuration document carries `[default]`: the strict mirror of
    /// camel-config's `apply_profile` and the virtual-store
    /// `MalformedVirtualConfig` backstop rejects the selection at
    /// compile time.
    IncludeOnlyProfiles { names: Vec<String> },
    /// The selected `--config` document has profile structure and
    /// carries root-level `CamelConfig` keys (other than the `routes`
    /// pattern accumulator, which keeps its documented overlay role)
    /// that strict profile selection would silently discard. The
    /// compile mirror of camel-config's root-key discard guard.
    RootKeysDiscarded(Vec<String>),
    /// The selected `--config` document has profile structure and
    /// carries root-level TABLES whose names misspell known
    /// `CamelConfig` keys (per the shared
    /// `camel_config::root_key_policy::near_miss_root_table`
    /// predicate) — the same tables the loader rejects naming the
    /// probable intended key. Pairs are `(table, intended)`.
    RootTableMisspelling(Vec<(String, String)>),
    /// The configuration or an include is not valid TOML (or a required
    /// field has the wrong shape).
    InvalidConfig(String),
    /// The primary document is not a usable YAML/JSON document.
    InvalidDocument(String),
    /// A route-file pattern or a literal route source is unusable
    /// (reserved document suffix, unsupported extension, malformed
    /// glob).
    UnsupportedRouteSource(String),
    /// A job-kind entry document declared a file-form route source
    /// (`routeFiles`/`routeFilesFromRoot`) whose patterns resolved zero
    /// route files. `camel job` rejects the same declaration set with
    /// its job-safety rule, so compiling would embed a dead artifact.
    JobRouteSourceEmpty {
        /// Logical path of the job document.
        document: String,
        /// The declared route-file patterns.
        patterns: Vec<String>,
    },
    /// A collected asset reference names a file that does not exist
    /// under the selected root (reject-missing at compile time).
    AssetMissing {
        /// Document field or URI parameter the reference was declared in.
        field: String,
        /// Asset class of the reference.
        class: String,
        /// The declared path.
        declared: String,
    },
    /// A collected asset reference resolves (after symlink-aware
    /// canonicalization) outside the selected root.
    AssetOutsideRoot {
        /// Document field or URI parameter the reference was declared in.
        field: String,
        /// Asset class of the reference.
        class: String,
        /// The declared path.
        declared: String,
        /// The canonical confinement root.
        root: PathBuf,
    },
    /// A collected asset reference is not a canonical relative name
    /// (traversal, backslash, control byte, glob metacharacters, …).
    AssetInvalidName {
        /// Document field or URI parameter the reference was declared in.
        field: String,
        /// Asset class of the reference.
        class: String,
        /// The declared path.
        declared: String,
        /// The specific shape violation.
        reason: String,
    },
    /// A symlink was encountered inside a `static_dir` tree: static
    /// trees embed regular files only and fail closed on symlinks
    /// (a silent skip could hide a pivot out of the tree).
    StaticTreeSymlink {
        /// The declared directory path.
        declared: String,
        /// The symlink, relative to the confinement root.
        link: String,
    },
    /// A declared asset spelling does not occur verbatim in its site
    /// entry's normalized bytes, so no substitution span can be
    /// recorded (fail-closed rather than shipping an invalid index).
    AssetSpanMissing {
        /// The declared path.
        declared: String,
        /// The site entry logical path.
        site: String,
    },
    /// Two substitution entries claim overlapping byte regions of one
    /// site entry (one spelling occurring inside another's site): the
    /// runtime rewrite walks spans last-offset-first assuming
    /// disjointness, so overlap would double-rewrite a region.
    AssetSpanOverlap {
        /// The site entry logical path.
        site: String,
        /// The first overlapping declared spelling.
        first: String,
        /// The second overlapping declared spelling.
        second: String,
    },
    /// Normalization failed for the collected document set (invalid
    /// UTF-8, or the configured aggregate payload cap breached by the
    /// normalized bytes).
    Normalize(CompileError),
    /// Compile policy rejected the collected configuration or document
    /// set (fail-closed bean-plugin / WASM-security config assets, or
    /// unusable document asset declarations). The wrapped error carries
    /// the operator-facing diagnostic text unchanged.
    Policy(CompileError),
    /// A secret-family asset is present but `--embed-secrets` was not
    /// passed: secret material never embeds without explicit opt-in.
    SecretEmbedOptIn {
        /// Document field or URI parameter the secret was declared in.
        field: String,
        /// The secret asset class name (always `private key`).
        class: &'static str,
    },
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
            Self::IncludeOnlyProfiles { names } => write!(
                f,
                "profiles {} exist only in includes: a configuration with a [default] section \
                 must declare at least one selected profile itself; move at least one selected \
                 profile section into the configuration document",
                names.join(", ")
            ),
            Self::RootKeysDiscarded(keys) => write!(
                f,
                "configuration key(s) {} sit at the top level of a profile-structured document \
                 and would be silently discarded by profile selection: move each key under \
                 [default] (overlaid by the selected profile section), or remove the profile \
                 sections to use a flat document",
                keys.join(", ")
            ),
            Self::RootTableMisspelling(pairs) => {
                let names: Vec<&str> = pairs.iter().map(|(name, _)| name.as_str()).collect();
                let misspellings: Vec<String> = pairs
                    .iter()
                    .map(|(name, target)| {
                        format!("'{name}' looks like a misspelling of '{target}'")
                    })
                    .collect();
                write!(
                    f,
                    "top-level table(s) {} would be silently discarded by profile selection: \
                     {} — move the table under [default] (overlaid by the selected profile \
                     section), or remove the profile sections to use a flat document",
                    names.join(", "),
                    misspellings.join("; ")
                )
            }
            Self::InvalidConfig(reason) => write!(f, "invalid configuration: {reason}"),
            Self::InvalidDocument(reason) => write!(f, "invalid document: {reason}"),
            Self::UnsupportedRouteSource(reason) => write!(f, "unsupported route source: {reason}"),
            Self::JobRouteSourceEmpty { document, patterns } => write!(
                f,
                "job route source resolved zero route definitions: document '{document}' \
                 declares route-file patterns [{}] that matched no route files",
                patterns.join(", ")
            ),
            Self::AssetMissing {
                field,
                class,
                declared,
            } => write!(
                f,
                "asset field '{field}' ({class}): missing file '{declared}' under the \
                 selected root (asset references resolve embedded-only and reject missing \
                 at compile time)"
            ),
            Self::AssetOutsideRoot {
                field,
                class,
                declared,
                root,
            } => write!(
                f,
                "asset field '{field}' ({class}): '{declared}' resolves outside the selected \
                 root '{}' (confinement rejected; symlink escapes are not embeddable)",
                root.display()
            ),
            Self::AssetInvalidName {
                field,
                class,
                declared,
                reason,
            } => write!(
                f,
                "asset field '{field}' ({class}): invalid path '{declared}': {reason}"
            ),
            Self::StaticTreeSymlink { declared, link } => write!(
                f,
                "static directory '{declared}': symlink '{link}' inside the tree — static \
                 trees embed regular files only and fail closed on symlinks instead of \
                 silently skipping or following them"
            ),
            Self::AssetSpanMissing { declared, site } => write!(
                f,
                "asset '{declared}' in document '{site}': the declared spelling does not \
                 occur verbatim in the normalized entry, so no substitution span can be \
                 recorded"
            ),
            Self::AssetSpanOverlap {
                site,
                first,
                second,
            } => write!(
                f,
                "assets '{first}' and '{second}' in document '{site}': their substitution \
                 spans overlap — one spelling occurs inside the other's site, and a \
                 last-offset-first rewrite would corrupt the entry"
            ),
            Self::Normalize(inner) => write!(f, "{inner}"),
            Self::Policy(inner) => write!(f, "{inner}"),
            // allow-secret: names the field and class, never a secret value.
            Self::SecretEmbedOptIn { field, class } => write!(
                f,
                "asset field '{field}' ({class}): embedding secret material requires \
                 --embed-secrets (re-run with the flag to opt in)"
            ),
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
    // r3jobs Task 1.2: remember a file-form declaration (with its
    // declared patterns) for the zero-entry job check after plan
    // construction — `camel job` rejects a job whose route source
    // resolves zero route definitions, and so must compile.
    let job_file_patterns = match &entry_fields {
        RouteSourceFields::RouteFiles(patterns)
        | RouteSourceFields::RouteFilesFromRoot(patterns) => Some(patterns.clone()),
        RouteSourceFields::None => None,
    };
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

        // r2embed Task 1.2: the selected Camel.toml itself must not
        // declare bean plugins, WASM security permissions, or WASM
        // security-policy modules — at the root and in every profile
        // section that merges into the effective root (all fail closed
        // before any output exists; recorded R2 deferrals).
        policy::reject_config_assets(&config_logical, &config, &selection.profiles)
            .map_err(SourceError::Policy)?;

        // cfgdrop2 Task 2.1: compile mirror of camel-config's
        // root-key discard guard. When the document has profile
        // structure, root-level CamelConfig keys (except the `routes`
        // pattern accumulator, whose overlay walk below is untouched)
        // would be silently dropped by strict profile selection, and
        // root tables that misspell a known key would vanish the same
        // way — reject both here exactly as the loader rejects them at
        // boot, so one document gets one disposition on both front
        // doors. Without `--config` this block is never reached.
        if camel_dsl::config_semantics::has_profile_structure(&config, &selection.profiles)
            && let toml::Value::Table(ref table) = config
        {
            let classes = camel_config::root_key_policy::classify_root_entries(
                table.iter().map(|(k, v)| (k.as_str(), v)),
            );
            if !classes.discarded_keys.is_empty() {
                return Err(SourceError::RootKeysDiscarded(classes.discarded_keys));
            }
            if !classes.misspelled_tables.is_empty() {
                let near_miss = classes
                    .misspelled_tables
                    .into_iter()
                    .map(|(name, target)| (name, target.to_string()))
                    .collect();
                return Err(SourceError::RootTableMisspelling(near_miss));
            }
        }

        // Ordered include walk in canonical order, collected by
        // `camel_dsl::config_semantics::include_declarations`: top-level,
        // `[default]`, then each selected non-default profile section
        // (in flag order, deduplicated).
        let mut include_decls: Vec<String> = Vec::new();
        for (label, value) in
            camel_dsl::config_semantics::include_declarations(&config, &selection.profiles)
        {
            // Empty section name (pathological empty profile + [""] table)
            // maps to top-level "include" wording; pre-refactor emitted
            // ".include" — intentional divergence (rc-io2zl).
            let owner = if label.is_empty() {
                "include".to_string()
            } else {
                format!("{label}.include")
            };
            include_decls.extend(toml_string_list(&owner, value)?);
        }

        // Route patterns from the config, with camel-config overlay
        // semantics: top-level `routes`, then each declaring section
        // ([default], selected profiles) replaces the accumulated list.
        // The section order is the canonical
        // `camel_dsl::config_semantics::section_walk`.
        let mut route_patterns: Option<Vec<String>> = None;
        if let Some(value) = config.get("routes") {
            route_patterns = Some(toml_string_list("routes", value)?);
        }
        for section in camel_dsl::config_semantics::section_walk(&selection.profiles) {
            if let Some(toml::Value::Table(table)) = config.get(section.as_str())
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
        // retained (with their logical paths) for profile-section
        // extraction (camel-config applies profile sections per file,
        // config first) and for the r2embed config-asset rejection.
        let mut include_tables: Vec<(String, toml::Value)> =
            Vec::with_capacity(include_decls.len());
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
            config_references.push(logical.clone());
            include_tables.push((logical, table));
        }

        // r2embed Task 1.2: every include in the chain obeys the same
        // fail-closed bean-plugin / WASM-security rule, over its root
        // and its runtime-merged profile sections alike.
        for (path, table) in &include_tables {
            policy::reject_config_assets(path, table, &selection.profiles)
                .map_err(SourceError::Policy)?;
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
        let selected_names: Vec<String> = selected_profiles.iter().map(|n| n.to_string()).collect();
        for name in selected_profiles {
            validate_profile_name(name)?;
            let mut found = config
                .get(name)
                .filter(|v| matches!(v, toml::Value::Table(_)));
            if found.is_none() {
                for (_, table) in &include_tables {
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

        // Strict-at-compile mirror of camel-config's `apply_profile`
        // and the virtual-store `MalformedVirtualConfig` backstop: when
        // the configuration document carries profile structure but
        // declares none of the selected profiles itself, the selection
        // is rejected here — exactly what the runtime would reject at
        // boot. The loop above has already errored on chain-wide
        // absence, so this fires only when every selected profile is
        // include-only. The empty-profiles guard keeps every
        // profile-less compile of a `[default]`-carrying configuration
        // green (the runtime mirror guards the same).
        if camel_dsl::config_semantics::has_profile_structure(&config, &selection.profiles)
            && !selection.profiles.is_empty()
            && !camel_dsl::config_semantics::has_selected_profile(&config, &selection.profiles)
        {
            return Err(SourceError::IncludeOnlyProfiles {
                names: selected_names,
            });
        }

        // Config route patterns, declared order, matches sorted. Route
        // kind only: a job's route set comes exclusively from its own
        // `routeFiles`/`routeFilesFromRoot` declarations — `camel job`
        // never consults configuration `routes` patterns, so seeding
        // the plan from them here would reject document sets the CLI
        // accepts (`duplicate source` when a pattern overlaps a
        // declared file). Parity: every document set `camel job`
        // accepts must compile.
        if kind == TrailerKind::Route
            && let Some(patterns) = route_patterns
        {
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

    // r3jobs Task 1.2: a job whose declared file-form route source
    // resolved zero route files is a named rejection, mirroring the
    // `camel job` job-safety rule ("job route source resolved zero
    // route definitions") — compiling would embed a dead artifact.
    // Count-of-entries check only: route document contents are never
    // parsed here. Config `routes` patterns never seed a job plan (see
    // above), so the plan is final. Inline-`routes:` job documents
    // (no file form) keep compiling.
    if kind == TrailerKind::Job
        && let Some(patterns) = job_file_patterns
        && raws
            .iter()
            .filter(|raw| raw.kind == StoreEntryKind::Route)
            .count()
            == 0
    {
        return Err(SourceError::JobRouteSourceEmpty {
            document: doc_logical.clone(),
            patterns,
        });
    }

    // Normalize the whole set under the configured aggregate cap. The
    // entry text was normalized above for field extraction; normalization
    // is idempotent (BOM removal, CRLF/CR folding), so re-normalizing it
    // here is a no-op.
    let raw_slices: Vec<&[u8]> = raws.iter().map(|raw| raw.bytes.as_slice()).collect();
    let normalized = trailer::normalize_documents(&raw_slices, selection.max_payload_bytes)
        .map_err(SourceError::Normalize)?;
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

    // r2embed Task 1.2: collect the revised asset matrix from every
    // embedded document, then confine, deduplicate, and expand it into
    // store assets plus the substitution table — all before any output
    // byte exists.
    let mut refs: Vec<AssetRef> = Vec::new();
    for (path, text) in &route_documents {
        refs.extend(policy::collect_document_assets(path, text).map_err(SourceError::Policy)?);
    }

    // Secret opt-in gate (r2embed Task 2.1): a secret-family asset —
    // exactly the private-key family — never embeds without an explicit
    // `--embed-secrets` flag. The offending reference is identified up
    // front, but the gate verdict is returned only after resolution:
    // a set that fails resolution for another named reason (missing,
    // escaping, or placeholder asset) surfaces THAT rejection first,
    // and the gate then fails closed before any output byte exists,
    // naming the field and the secret class.
    let secret = if selection.embed_secrets {
        None
    } else {
        refs.iter()
            .find(|reference| reference.class == crate::compile::policy::SECRET_ASSET_CLASS)
            .map(|reference| (reference.field.clone(), reference.class))
    };
    let AssetSet {
        assets,
        substitutions,
    } = resolve_assets(refs, &root, &route_documents)?;
    if let Some((field, class)) = secret {
        return Err(SourceError::SecretEmbedOptIn { field, class });
    }

    // The single aggregation point of the payload cap (r2embed Task
    // 2.1): normalized document bytes plus verbatim asset bytes must
    // together fit the configured `--max-payload-bytes` cap. The reader
    // enforces no cap of its own.
    let doc_total = documents.iter().fold(0u64, |acc, document| {
        acc.saturating_add(u64::try_from(document.bytes.len()).unwrap_or(u64::MAX))
    });
    let asset_total = assets.iter().fold(0u64, |acc, asset| {
        acc.saturating_add(u64::try_from(asset.bytes.len()).unwrap_or(u64::MAX))
    });
    trailer::enforce_payload_cap(
        doc_total.saturating_add(asset_total),
        selection.max_payload_bytes,
    )
    .map_err(SourceError::Normalize)?;

    Ok(ResolvedSources {
        kind,
        entry_point: doc_logical,
        documents,
        config_references,
        source_plan: plan,
        route_documents,
        assets,
        substitutions,
    })
}

/// The confined asset set for one compile: store assets (one per
/// canonical target) plus the substitution table over every declared
/// spelling.
struct AssetSet {
    assets: Vec<StoreAsset>,
    substitutions: Vec<SubstitutionEntry>,
}

/// Resolve every collected asset reference through the R1 confinement
/// rules (r2embed Task 1.2).
///
/// Anchored at the selected root; symlink-aware canonicalization must
/// keep each target under it; missing targets are named rejections.
/// `static directory` references expand to a deterministic sorted walk
/// of regular files (a symlink inside the tree fails closed; other
/// non-regular entries are skipped). References deduplicate by canonical
/// target into one shared store asset — the substitution table keeps
/// every alias spelling of a FILE target pointing at that single entry,
/// while a tree declaration itself records no substitution site (F4:
/// dir-level substitution semantics are deferred to the Phase-3
/// registry review). Substitution sites are boundary-valid occurrences
/// grouped per `(document, declared, context)`, and cross-entry span
/// overlap is a named rejection (F1).
fn resolve_assets(
    refs: Vec<AssetRef>,
    root: &Path,
    documents: &[(String, String)],
) -> Result<AssetSet, SourceError> {
    // Canonical target -> `assets/`-prefixed store path. Deduplicating
    // by canonical target keeps one shared entry per file; the first
    // reference's class names the entry.
    let mut claimed: HashMap<PathBuf, String> = HashMap::with_capacity(refs.len());
    let mut assets: Vec<StoreAsset> = Vec::with_capacity(refs.len());
    let mut resolved: Vec<(AssetRef, String)> = Vec::with_capacity(refs.len());

    for reference in refs {
        validate_asset_name(&reference)?;
        let canonical = root.join(&reference.declared).canonicalize().map_err(|_| {
            SourceError::AssetMissing {
                field: reference.field.clone(),
                class: reference.class.to_string(),
                declared: reference.declared.clone(),
            }
        })?;
        if !canonical.starts_with(root) {
            return Err(SourceError::AssetOutsideRoot {
                field: reference.field.clone(),
                class: reference.class.to_string(),
                declared: reference.declared.clone(),
                root: root.to_path_buf(),
            });
        }

        let store_path = if let Some(existing) = claimed.get(&canonical) {
            existing.clone()
        } else if reference.class == "static directory" {
            if !canonical.is_dir() {
                return Err(SourceError::AssetInvalidName {
                    field: reference.field.clone(),
                    class: reference.class.to_string(),
                    declared: reference.declared.clone(),
                    reason: "not a directory".to_string(),
                });
            }
            let mut files = Vec::new();
            walk_static_dir(&canonical, root, &reference, &mut files)?;
            // The `claimed` set is consulted per walked FILE (review
            // F3): nested or alias tree declarations share files, and
            // each file becomes exactly one store asset.
            for file in files {
                if claimed.contains_key(&file) {
                    continue;
                }
                let logical = logical_asset_path(root, &file, &reference)?;
                let bytes = std::fs::read(&file).map_err(|_| SourceError::AssetMissing {
                    field: reference.field.clone(),
                    class: reference.class.to_string(),
                    declared: reference.declared.clone(),
                })?;
                claimed.insert(file, format!("assets/{logical}"));
                assets.push(StoreAsset {
                    path: logical,
                    class: Some(reference.class.to_string()),
                    bytes,
                });
            }
            // The tree declaration itself records NO substitution site
            // (review F4): dir-level substitution semantics are deferred
            // to the Phase-3 registry review — the expanded files'
            // own references resolve through that registry.
            continue;
        } else {
            if !canonical.is_file() {
                return Err(SourceError::AssetInvalidName {
                    field: reference.field.clone(),
                    class: reference.class.to_string(),
                    declared: reference.declared.clone(),
                    reason: "not a regular file".to_string(),
                });
            }
            let logical = logical_asset_path(root, &canonical, &reference)?;
            let bytes = std::fs::read(&canonical).map_err(|_| SourceError::AssetMissing {
                field: reference.field.clone(),
                class: reference.class.to_string(),
                declared: reference.declared.clone(),
            })?;
            let path = format!("assets/{logical}");
            claimed.insert(canonical, path.clone());
            assets.push(StoreAsset {
                path: logical,
                class: Some(reference.class.to_string()),
                bytes,
            });
            path
        };
        resolved.push((reference, store_path));
    }

    // Group by (site document, declared string, site context): one
    // substitution entry per distinct declared spelling per context —
    // a mixed literal/uri declaration of one string keeps two entries
    // instead of a first-seen-wins merge (review F1). Spans cover the
    // boundary-valid sites of that context in the site entry's
    // normalized bytes.
    let mut groups: BTreeMap<
        (String, String, SubstitutionContext),
        (String, Vec<SubstitutionSpan>),
    > = BTreeMap::new();
    for (reference, store_path) in &resolved {
        let entry = groups
            .entry((
                reference.site.clone(),
                reference.declared.clone(),
                reference.context,
            ))
            .or_insert_with(|| (store_path.clone(), Vec::new()));
        let text = documents
            .iter()
            .find(|(path, _)| path == &reference.site)
            .map(|(_, text)| text)
            .ok_or_else(|| SourceError::AssetSpanMissing {
                declared: reference.declared.clone(),
                site: reference.site.clone(),
            })?;
        let mut spans = find_spans(text, &reference.declared, reference.context);
        if spans.is_empty() {
            return Err(SourceError::AssetSpanMissing {
                declared: reference.declared.clone(),
                site: reference.site.clone(),
            });
        }
        spans.append(&mut entry.1);
        spans.sort_by_key(|span| span.start);
        spans.dedup();
        entry.1 = spans;
    }
    let substitutions: Vec<SubstitutionEntry> = groups
        .into_iter()
        .map(
            |((document, declared, context), (asset, spans))| SubstitutionEntry {
                asset,
                context,
                declared,
                document,
                spans,
            },
        )
        .collect();

    // Cross-entry span overlap is a named rejection (review F1): the
    // runtime rewrite walks each site's spans last-offset-first, so two
    // entries claiming one byte region would double-rewrite it.
    for (i, a) in substitutions.iter().enumerate() {
        for b in &substitutions[i + 1..] {
            if a.document != b.document {
                continue;
            }
            let overlap = a
                .spans
                .iter()
                .any(|s| b.spans.iter().any(|t| s.start < t.end && t.start < s.end));
            if overlap {
                return Err(SourceError::AssetSpanOverlap {
                    site: a.document.clone(),
                    first: a.declared.clone(),
                    second: b.declared.clone(),
                });
            }
        }
    }

    Ok(AssetSet {
        assets,
        substitutions,
    })
}

/// Validate a declared asset path: relative (absolute paths are already
/// a policy rejection), no traversal or backslash, no control bytes, no
/// dynamic placeholder, and no glob metacharacters — an asset names one
/// compile-known file or directory. A leading `./` spelling is allowed:
/// canonicalization normalizes it and the substitution table keeps the
/// alias spelling verbatim.
fn validate_asset_name(reference: &AssetRef) -> Result<(), SourceError> {
    let invalid = |reason: &str| SourceError::AssetInvalidName {
        field: reference.field.clone(),
        class: reference.class.to_string(),
        declared: reference.declared.clone(),
        reason: reason.to_string(),
    };
    let declared = &reference.declared;
    if declared.is_empty() {
        return Err(invalid("empty path"));
    }
    if declared.starts_with('/') {
        return Err(invalid("absolute path"));
    }
    if declared.contains('\\') {
        return Err(invalid("backslash separator"));
    }
    if declared.contains("${") {
        return Err(invalid("dynamic placeholder"));
    }
    if declared.contains('*')
        || declared.contains('?')
        || (declared.contains('[') && declared.contains(']'))
    {
        return Err(invalid("glob metacharacters"));
    }
    if declared.bytes().any(|b| b < 0x20 || b == 0x7F) {
        return Err(invalid("control byte"));
    }
    if declared.split('/').any(|segment| segment == "..") {
        return Err(invalid("traversal '..'"));
    }
    Ok(())
}

/// Deterministic sorted walk of a static directory tree: depth-first
/// with entries sorted by file name at each level, regular files only.
/// A symlink inside the tree fails closed with a named diagnostic; other
/// non-regular entries (FIFOs, sockets, devices) are skipped.
fn walk_static_dir(
    dir: &Path,
    root: &Path,
    reference: &AssetRef,
    out: &mut Vec<PathBuf>,
) -> Result<(), SourceError> {
    let missing = || SourceError::AssetMissing {
        field: reference.field.clone(),
        class: reference.class.to_string(),
        declared: reference.declared.clone(),
    };
    let mut entries: Vec<std::fs::DirEntry> = std::fs::read_dir(dir)
        .map_err(|_| missing())?
        .collect::<Result<_, _>>()
        .map_err(|_| missing())?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let file_type = entry.file_type().map_err(|_| missing())?;
        let path = entry.path();
        if file_type.is_symlink() {
            let link = path.strip_prefix(root).map_or_else(
                |_| path.display().to_string(),
                |rel| rel.display().to_string(),
            );
            return Err(SourceError::StaticTreeSymlink {
                declared: reference.declared.clone(),
                link,
            });
        }
        if file_type.is_dir() {
            walk_static_dir(&path, root, reference, out)?;
        } else if file_type.is_file() {
            out.push(path);
        }
    }
    Ok(())
}

/// Root-anchored logical asset path of a canonical file: UTF-8 relative
/// `/` path, validated by the store path rules.
fn logical_asset_path(
    root: &Path,
    canonical: &Path,
    reference: &AssetRef,
) -> Result<String, SourceError> {
    let invalid = |reason: &str| SourceError::AssetInvalidName {
        field: reference.field.clone(),
        class: reference.class.to_string(),
        declared: canonical.display().to_string(),
        reason: reason.to_string(),
    };
    let rel = canonical
        .strip_prefix(root)
        .map_err(|_| SourceError::AssetOutsideRoot {
            field: reference.field.clone(),
            class: reference.class.to_string(),
            declared: reference.declared.clone(),
            root: root.to_path_buf(),
        })?;
    let mut logical = String::new();
    for component in rel.components() {
        let Component::Normal(part) = component else {
            return Err(invalid("non-canonical component"));
        };
        let part = part.to_str().ok_or_else(|| invalid("non-UTF-8 name"))?;
        if !logical.is_empty() {
            logical.push('/');
        }
        logical.push_str(part);
    }
    validate_path(&logical).map_err(|_| invalid("not a canonical store path"))?;
    Ok(logical)
}

/// Path-token continuation byte (review F1): when the byte before or
/// after a match would extend a path token, the match is a substring of
/// a longer spelling (`certs/ca.pem` inside `./certs/ca.pem`, the tail
/// of `client_ca.pem`), not a site of the declared string.
fn is_path_continuation(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'/' | b'%' | b'+' | b':' | b'-')
}

/// Substitution sites of `needle` in `haystack` as ascending,
/// non-overlapping byte spans, filtered by token boundaries and the
/// site's context (review F1). A `uri` site sits immediately after a
/// scheme colon or a `param=` separator; a `literal` site follows
/// ordinary text — so one declared string occurring in both a URI and a
/// literal field of one document yields two disjoint site sets, never a
/// shared occurrence.
fn find_spans(haystack: &str, needle: &str, context: SubstitutionContext) -> Vec<SubstitutionSpan> {
    let mut spans = Vec::new();
    if needle.is_empty() {
        return spans;
    }
    let bytes = haystack.as_bytes();
    let mut from = 0;
    while let Some(position) = haystack[from..].find(needle) {
        let start = from + position;
        let end = start + needle.len();
        let preceded = |expected: fn(u8) -> bool| start > 0 && expected(bytes[start - 1]);
        let site = match context {
            SubstitutionContext::Uri => preceded(|b| b == b':' || b == b'='),
            SubstitutionContext::Literal => {
                start == 0 || preceded(|b| !is_path_continuation(b) && b != b':' && b != b'=')
            }
        };
        let terminated = end >= bytes.len() || !is_path_continuation(bytes[end]);
        if site && terminated {
            spans.push(SubstitutionSpan {
                start: start as u64,
                end: end as u64,
            });
        }
        from = end;
    }
    spans
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

    /// The include-only diagnostic names the selected profiles, the
    /// rule, and the remedy.
    #[test]
    fn include_only_profiles_display_names_rule_and_remedy() {
        let err = SourceError::IncludeOnlyProfiles {
            names: vec!["prod".into(), "canary".into()],
        };
        assert_eq!(
            err.to_string(),
            "profiles prod, canary exist only in includes: a configuration with a [default] \
             section must declare at least one selected profile itself; move at least one \
             selected profile section into the configuration document"
        );
    }

    /// Boundary-aware substitution sites (review F1): a match whose
    /// neighboring byte continues a path token is a substring of a
    /// longer spelling, not a site — the `certs/ca.pem` nested inside
    /// `./certs/ca.pem` and the tail of `client_ca.pem` are not sites.
    /// `uri` sites sit after a scheme colon or a `param=` separator;
    /// `literal` sites follow ordinary text.
    #[test]
    fn find_spans_respects_token_boundaries_and_site_context() {
        let text = "cert: ./certs/ca.pem\nclient_ca: certs/ca.pem\nnote: client_ca.pem\n";
        let spans = find_spans(text, "certs/ca.pem", SubstitutionContext::Literal);
        assert_eq!(
            spans.len(),
            1,
            "nested alias and token-substring matches are not sites: {spans:?}"
        );
        assert_eq!(
            &text[spans[0].start as usize..spans[0].end as usize],
            "certs/ca.pem"
        );
        let dotted = find_spans(text, "./certs/ca.pem", SubstitutionContext::Literal);
        assert_eq!(dotted.len(), 1, "the longer alias spelling is a site");

        let uri_text = "steps:\n- to: 'xslt:transform.xslt'\nxslt: transform.xslt\n";
        let uri = find_spans(uri_text, "transform.xslt", SubstitutionContext::Uri);
        assert_eq!(uri.len(), 1, "a uri site follows the scheme colon: {uri:?}");
        assert_eq!(
            &uri_text[uri[0].start as usize..uri[0].end as usize],
            "transform.xslt"
        );
        let literal = find_spans(uri_text, "transform.xslt", SubstitutionContext::Literal);
        assert_eq!(
            literal.len(),
            1,
            "a literal site follows ordinary text: {literal:?}"
        );
        assert!(
            uri[0].end <= literal[0].start || literal[0].end <= uri[0].start,
            "the two context sites never share a byte"
        );
    }

    /// Cross-entry span overlap is a named compile rejection (review
    /// F1): one spelling occurring inside another's site would corrupt
    /// a last-offset-first rewrite.
    #[test]
    fn resolve_assets_rejects_cross_entry_span_overlap() {
        let root = tempfile::tempdir().expect("tempdir");
        std::fs::write(root.path().join("my cert.pem"), "x").expect("write overlapping file");
        std::fs::write(root.path().join("cert.pem"), "x").expect("write substring file");
        let docs = vec![(
            "app.yaml".to_string(),
            "cert: \"my cert.pem\"\nclient_ca: cert.pem\n".to_string(),
        )];
        let refs = vec![
            AssetRef {
                class: "certificate",
                declared: "my cert.pem".to_string(),
                field: "cert".to_string(),
                site: "app.yaml".to_string(),
                context: SubstitutionContext::Literal,
            },
            AssetRef {
                class: "client CA",
                declared: "cert.pem".to_string(),
                field: "client_ca".to_string(),
                site: "app.yaml".to_string(),
                context: SubstitutionContext::Literal,
            },
        ];
        let Err(err) = resolve_assets(refs, root.path(), &docs) else {
            panic!("a spelling inside another's site must be rejected");
        };
        let SourceError::AssetSpanOverlap {
            site,
            first,
            second,
        } = err
        else {
            panic!("unexpected error variant: {err:?}");
        };
        assert_eq!(site, "app.yaml");
        for declared in [first, second] {
            assert!(
                declared == "my cert.pem" || declared == "cert.pem",
                "diagnostic must name both spellings: {declared:?}"
            );
        }
    }
}
