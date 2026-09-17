//! Operational manifest embedded next to the payload in compiled artifacts.
//!
//! The manifest is canonical UTF-8 JSON with lexicographically ordered keys
//! and source-ordered arrays. It records the independent `manifest_schema`
//! (trailer version describes framing; manifest schema describes JSON
//! meaning — the two are validated independently), the logical source name,
//! runtime version, artifact kind, embedded components (endpoint URI
//! schemes of the route/job documents plus configuration-declared
//! per-component config blocks), required environment names (`${env:NAME}`
//! tokens WITHOUT defaults across the route/job documents AND the
//! embedded configuration entries — defaulted tokens resolve at runtime
//! and name nothing), listener declarations from the canonical REST/MCP
//! port fields as literal addresses or unresolved expressions plus the
//! configuration-declared observability health/prometheus endpoints, and
//! the `embedded_files`
//! list of every bundled virtual document with its logical path, document
//! kind, byte length, and content digest (hex BLAKE3 of the
//! normalized bytes).
//!
//! Decode is strict for schema 2: only the declared fields are accepted
//! (unknown fields and wrong types rejected, never collapsed to
//! defaults), `embedded_files` entries carry well-formed digests, kinds,
//! lengths, and canonical paths in canonical path order, and the v2
//! trailer decode additionally enforces agreement between the manifest
//! and the embedded store (same entries, same content digests). The
//! schema-less legacy form stays lenient and is accepted only in v1
//! trailers.
//!
//! Derivation runs on the normalized, pre-interpolation document text: the
//! artifact captures authoring text before `${env:}` resolution, so every
//! field here must tolerate unresolved placeholders.

use std::sync::OnceLock;

use noyalib::compat::serde_yaml as serde_yml;
use regex::Regex;

use super::CompileError;
use super::store::StoreEntryKind;
use super::trailer::{FORMAT_VERSION, FORMAT_VERSION_V2, TrailerError, TrailerKind};

/// Runtime version recorded in every manifest built by this crate.
pub const RUNTIME_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Manifest schema written by this crate. Validated independently from the
/// trailer version.
pub const MANIFEST_SCHEMA: u64 = 2;

/// Manifest schema of a legacy v1 artifact, whose JSON carried no
/// `manifest_schema` field at all.
pub const MANIFEST_SCHEMA_LEGACY: u64 = 1;

/// One embedded virtual document as recorded in the manifest: logical path,
/// document kind, byte length, and content digest (hex BLAKE3 of the
/// normalized bytes).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EmbeddedFile {
    /// Content digest as lowercase hex BLAKE3.
    pub digest: String,
    /// Document kind (`route`, `job`, `config`, `include`, `profile`).
    pub kind: StoreEntryKind,
    /// Normalized byte length.
    pub length: u64,
    /// Canonical logical path.
    pub path: String,
}

/// Operational manifest of a compiled artifact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    /// Manifest schema; only [`MANIFEST_SCHEMA`] is written and
    /// [`MANIFEST_SCHEMA_LEGACY`] is tolerated on decode.
    pub manifest_schema: u64,
    /// Logical source name: input path relative to the compile working
    /// directory. Runtime source identity becomes `compiled://<source_name>`.
    pub source_name: String,
    /// Runtime version of the compiling CLI.
    pub runtime_version: String,
    /// Embedded artifact kind.
    pub kind: TrailerKind,
    /// Endpoint URI schemes referenced by the route/job documents, in
    /// source order, followed by configuration-declared per-component
    /// config block names — sorted and deduped per entry, merged in
    /// canonical store order across entries.
    pub components: Vec<String>,
    /// `${env:NAME}` names WITHOUT defaults, in scan order across the
    /// route/job documents and then the embedded configuration entries.
    /// A defaulted token never requires its variable, so it is never
    /// listed.
    pub env_names: Vec<String>,
    /// Listener declarations, in scan order: from the canonical REST/MCP
    /// port fields (`host:port` for REST blocks, the `bind` value
    /// verbatim for MCP blocks — literal ports and unresolved expressions
    /// pass through as written) and from the configuration-declared
    /// observability health/prometheus endpoints (`host:port`, camel-config
    /// defaults filled in).
    pub listeners: Vec<String>,
    /// Every bundled virtual document, in canonical path order.
    pub embedded_files: Vec<EmbeddedFile>,
}

impl Manifest {
    /// Serialize to canonical UTF-8 JSON: compact, lexicographically ordered
    /// keys (serde_json maps are BTreeMaps), source-ordered arrays. This is
    /// the schema-2 form carried by v2 artifacts.
    pub fn to_canonical_json(&self) -> String {
        let value = serde_json::json!({
            "components": self.components,
            "embedded_files": self.embedded_files,
            "env_names": self.env_names,
            "kind": self.kind.as_str(),
            "listeners": self.listeners,
            "manifest_schema": self.manifest_schema,
            "runtime_version": self.runtime_version,
            "source_name": self.source_name,
        });
        serde_json::to_string(&value).expect("json! of strings and arrays cannot fail") // allow-unwrap
    }

    /// Serialize to the legacy schema-less JSON form carried by v1
    /// trailers: the operational fields only — no `manifest_schema` (the
    /// legacy JSON predates the field) and no `embedded_files` (a v1
    /// artifact embeds exactly the one document named by `source_name`).
    /// The v1 compile writer uses this so its artifacts decode under the
    /// version-matched rules; multidoc Task 1.2 switches the writer to the
    /// v2 store with [`Self::to_canonical_json`]. v1-decode path only;
    /// retained for legacy artifact reading.
    pub fn to_legacy_json(&self) -> String {
        let value = serde_json::json!({
            "components": self.components,
            "env_names": self.env_names,
            "kind": self.kind.as_str(),
            "listeners": self.listeners,
            "runtime_version": self.runtime_version,
            "source_name": self.source_name,
        });
        serde_json::to_string(&value).expect("json! of strings and arrays cannot fail") // allow-unwrap
    }

    /// Parse and validate canonical manifest JSON. The schema is validated
    /// independently from any trailer version: [`MANIFEST_SCHEMA`] and the
    /// schema-less legacy v1 form are accepted, anything else is rejected.
    /// The strict schema-2 field rules (`check_schema2_fields`) apply
    /// whenever the declared schema is [`MANIFEST_SCHEMA`].
    pub fn from_canonical_json(bytes: &[u8]) -> Result<Manifest, CompileError> {
        let value: serde_json::Value = serde_json::from_slice(bytes)
            .map_err(|e| CompileError::InvalidDocument(format!("manifest is not JSON: {e}")))?;
        let schema = manifest_schema_of(&value).map_err(|_| {
            CompileError::InvalidDocument("manifest schema is not an integer".to_string())
        })?;
        if schema != MANIFEST_SCHEMA && schema != MANIFEST_SCHEMA_LEGACY {
            return Err(CompileError::InvalidDocument(format!(
                "unsupported manifest schema {schema}"
            )));
        }
        if schema == MANIFEST_SCHEMA {
            check_schema2_fields(&value).map_err(CompileError::InvalidDocument)?;
        }
        let object = |field: &str| {
            value.get(field).and_then(serde_json::Value::as_str).ok_or(
                CompileError::InvalidDocument(format!("manifest carries no {field}")),
            )
        };
        let strings = |field: &str| -> Vec<String> {
            value
                .get(field)
                .and_then(serde_json::Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(serde_json::Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default()
        };
        let kind = TrailerKind::from_name(object("kind")?).ok_or(CompileError::InvalidDocument(
            "manifest carries no recognizable kind".to_string(),
        ))?;
        let embedded_files = match value.get("embedded_files") {
            Some(serde_json::Value::Array(items)) => items
                .iter()
                .map(|item| {
                    serde_json::from_value(item.clone()).map_err(|e| {
                        CompileError::InvalidDocument(format!("invalid embedded_files entry: {e}"))
                    })
                })
                .collect::<Result<Vec<EmbeddedFile>, CompileError>>()?,
            Some(_) => {
                return Err(CompileError::InvalidDocument(
                    "manifest embedded_files is not an array".to_string(),
                ));
            }
            // Legacy v1 manifests predate `embedded_files`; absence is the
            // empty list. For schema 2 the field is required, and
            // `check_schema2_fields` has already rejected its absence.
            None => Vec::new(),
        };
        Ok(Manifest {
            manifest_schema: schema,
            source_name: object("source_name")?.to_string(),
            runtime_version: object("runtime_version")?.to_string(),
            kind,
            components: strings("components"),
            env_names: strings("env_names"),
            listeners: strings("listeners"),
            embedded_files,
        })
    }
}

/// Read the effective manifest schema from parsed manifest JSON: the
/// `manifest_schema` field, defaulting to the legacy schema when absent.
fn manifest_schema_of(value: &serde_json::Value) -> Result<u64, ()> {
    match value.get("manifest_schema") {
        None => Ok(MANIFEST_SCHEMA_LEGACY),
        Some(v) => v.as_u64().ok_or(()),
    }
}

/// Every field a schema-2 manifest may carry. Anything else is rejected:
/// an unrecognized field would silently change meaning without a schema
/// bump.
const SCHEMA2_FIELDS: [&str; 8] = [
    "components",
    "embedded_files",
    "env_names",
    "kind",
    "listeners",
    "manifest_schema",
    "runtime_version",
    "source_name",
];

/// Every field an `embedded_files` entry may carry.
const EMBEDDED_FILE_FIELDS: [&str; 4] = ["digest", "kind", "length", "path"];

/// BLAKE3 hex digest shape: exactly 64 lowercase hex characters.
fn is_blake3_hex(digest: &str) -> bool {
    digest.len() == 64
        && digest
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Strict field rules for a schema-2 manifest, shared by the
/// trailer-decode validator and the canonical manifest parser:
///
/// - only the declared [`SCHEMA2_FIELDS`] may appear (unknown fields
///   rejected);
/// - `kind`, `source_name`, and `runtime_version` are required strings;
/// - `components`, `env_names`, and `listeners` are required arrays of
///   strings (wrong types and non-string entries rejected, never
///   collapsed to defaults);
/// - `embedded_files` is a required array of typed entries with exactly
///   the [`EMBEDDED_FILE_FIELDS`]: a 64-character lowercase-hex BLAKE3
///   `digest`, a known document `kind`, a `length` integer, and a
///   canonical relative `/` `path` (same rule as store entry paths);
/// - entries are in canonical path order (strictly ascending, no
///   duplicates).
fn check_schema2_fields(value: &serde_json::Value) -> Result<(), String> {
    let Some(map) = value.as_object() else {
        return Err("manifest is not a JSON object".to_string());
    };
    for key in map.keys() {
        if !SCHEMA2_FIELDS.contains(&key.as_str()) {
            return Err(format!("unknown schema-2 manifest field {key:?}"));
        }
    }
    for field in ["kind", "source_name", "runtime_version"] {
        if value
            .get(field)
            .and_then(serde_json::Value::as_str)
            .is_none()
        {
            return Err(format!("schema-2 manifest carries no {field}"));
        }
    }
    for field in ["components", "env_names", "listeners"] {
        let Some(items) = value.get(field).and_then(serde_json::Value::as_array) else {
            return Err(format!("schema-2 manifest field {field} is not an array"));
        };
        if items.iter().any(|item| item.as_str().is_none()) {
            return Err(format!(
                "schema-2 manifest array {field} carries a non-string entry"
            ));
        }
    }
    let Some(serde_json::Value::Array(items)) = value.get("embedded_files") else {
        return Err("schema-2 manifest carries no embedded_files array".to_string());
    };
    let mut paths = Vec::with_capacity(items.len());
    for item in items {
        paths.push(check_embedded_file(item)?);
    }
    for window in paths.windows(2) {
        if window[0] >= window[1] {
            return Err("embedded_files are not in canonical path order".to_string());
        }
    }
    Ok(())
}

/// Type- and shape-check one `embedded_files` entry and return its
/// canonical path.
fn check_embedded_file(item: &serde_json::Value) -> Result<String, String> {
    let Some(map) = item.as_object() else {
        return Err("embedded_files entry is not a JSON object".to_string());
    };
    for key in map.keys() {
        if !EMBEDDED_FILE_FIELDS.contains(&key.as_str()) {
            return Err(format!("unknown embedded_files field {key:?}"));
        }
    }
    let digest = map
        .get("digest")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "embedded_files entry carries no digest string".to_string())?;
    if !is_blake3_hex(digest) {
        return Err(format!(
            "embedded_files digest {digest:?} is not 64-character lowercase hex"
        ));
    }
    let kind = map
        .get("kind")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "embedded_files entry carries no kind string".to_string())?;
    if StoreEntryKind::from_name(kind).is_none() {
        return Err(format!("embedded_files entry names unknown kind {kind:?}"));
    }
    if map
        .get("length")
        .and_then(serde_json::Value::as_u64)
        .is_none()
    {
        return Err("embedded_files entry carries no length integer".to_string());
    }
    let path = map
        .get("path")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "embedded_files entry carries no path string".to_string())?;
    super::store::validate_path(path).map_err(|e| format!("embedded_files path invalid: {e}"))?;
    Ok(path.to_string())
}

/// Validate manifest bytes for trailer decode and return the artifact kind.
///
/// Each trailer version accepts exactly its own manifest form: a v1
/// trailer carries the schema-less legacy manifest only, and a v2 trailer
/// requires `manifest_schema: 2` with the strict schema-2 field rules
/// ([`check_schema2_fields`]). Fail closed on unknown schemas, non-JSON
/// bytes, unrecognizable kinds, and incomplete schema-2 manifests.
pub(crate) fn validate_manifest(
    manifest: &[u8],
    trailer_version: u16,
) -> Result<TrailerKind, TrailerError> {
    let value: serde_json::Value =
        serde_json::from_slice(manifest).map_err(|_| TrailerError::InvalidManifest)?;
    let schema = manifest_schema_of(&value).map_err(|_| TrailerError::InvalidManifest)?;
    match trailer_version {
        FORMAT_VERSION => {
            if schema != MANIFEST_SCHEMA_LEGACY {
                return Err(TrailerError::InvalidManifestSchema(schema));
            }
        }
        FORMAT_VERSION_V2 => {
            if schema != MANIFEST_SCHEMA {
                return Err(TrailerError::InvalidManifestSchema(schema));
            }
            check_schema2_fields(&value).map_err(TrailerError::InvalidManifestFields)?;
        }
        _ => return Err(TrailerError::InvalidVersion(trailer_version)),
    }
    let kind = value
        .get("kind")
        .and_then(serde_json::Value::as_str)
        .and_then(TrailerKind::from_name)
        .ok_or(TrailerError::InvalidManifest)?;
    Ok(kind)
}

/// Derive the manifest from a normalized, pre-interpolation document.
/// v1-decode path only; retained for legacy artifact reading.
pub fn derive(
    source_name: &str,
    kind: TrailerKind,
    document: &str,
) -> Result<Manifest, CompileError> {
    let root = parse_document(document)
        .map_err(|e| CompileError::InvalidDocument(format!("not a YAML/JSON document: {e}")))?;
    let mut components = Vec::new();
    let mut listeners = Vec::new();
    walk_document(&root, &mut components, &mut listeners);
    let env_names = scan_env_names(document);
    Ok(Manifest {
        manifest_schema: MANIFEST_SCHEMA,
        source_name: source_name.to_string(),
        runtime_version: RUNTIME_VERSION.to_string(),
        kind,
        components,
        env_names,
        listeners,
        embedded_files: vec![EmbeddedFile {
            digest: blake3::hash(document.as_bytes()).to_hex().to_string(),
            kind: StoreEntryKind::from(kind),
            length: document.len() as u64,
            path: source_name.to_string(),
        }],
    })
}

/// Derive the schema-2 manifest for a built virtual store (multidoc
/// Task 1.2): operational fields are scanned from every route/job
/// document in source-plan order (`route_documents` carries `(logical
/// path, normalized text)` pairs) AND from every embedded
/// config/include/profile entry in canonical store order, and
/// `embedded_files` mirrors the store entries — canonical path order,
/// kinds, lengths, and BLAKE3 content digests — so trailer decode's
/// manifest/store agreement holds by construction.
pub fn derive_for_store(
    store: &super::store::VirtualDocumentStore,
    kind: TrailerKind,
    route_documents: &[(String, String)],
) -> Result<Manifest, CompileError> {
    let mut components = Vec::new();
    let mut listeners = Vec::new();
    let mut env_names = Vec::new();
    for (path, text) in route_documents {
        let root = parse_document(text).map_err(|e| {
            CompileError::InvalidDocument(format!("{path}: not a YAML/JSON document: {e}"))
        })?;
        walk_document(&root, &mut components, &mut listeners);
        for name in scan_env_names(text) {
            if !env_names.contains(&name) {
                env_names.push(name);
            }
        }
    }
    // Configuration entries: the merged config/include/profile chain
    // carries operational fields at runtime too — required env names,
    // configuration-declared listener endpoints, and per-component
    // config blocks. Scanned in the store's canonical entry order; each
    // entry's contribution is deduped (and the config-derived component
    // and listener names sorted) before merging.
    for entry in &store.index.entries {
        if !matches!(
            entry.kind,
            StoreEntryKind::Config | StoreEntryKind::Include | StoreEntryKind::Profile
        ) {
            continue;
        }
        let text = store.read_text(&entry.path).ok_or_else(|| {
            CompileError::InvalidDocument(format!(
                "store entry '{}' is not valid UTF-8",
                entry.path
            ))
        })?;
        let (entry_components, entry_listeners) = scan_config_entry(text);
        for name in scan_env_names(text) {
            if !env_names.contains(&name) {
                env_names.push(name);
            }
        }
        for name in entry_components {
            if !components.contains(&name) {
                components.push(name);
            }
        }
        // Config-declared listeners report the artifact kind's effective
        // runtime listeners: the job boot projection suppresses the
        // observability endpoints, so a job artifact manifest omits them;
        // route artifacts bind them and keep them.
        if kind == TrailerKind::Route {
            for listener in entry_listeners {
                if !listeners.contains(&listener) {
                    listeners.push(listener);
                }
            }
        }
    }
    let mut embedded_files = Vec::with_capacity(store.index.entries.len());
    for entry in &store.index.entries {
        let range = entry.offset as usize..(entry.offset + entry.length) as usize;
        let bytes = store.content.get(range).ok_or_else(|| {
            CompileError::InvalidDocument(format!(
                "store entry '{}' range is out of content bounds",
                entry.path
            ))
        })?;
        embedded_files.push(EmbeddedFile {
            digest: blake3::hash(bytes).to_hex().to_string(),
            kind: entry.kind,
            length: entry.length,
            path: entry.path.clone(),
        });
    }
    Ok(Manifest {
        manifest_schema: MANIFEST_SCHEMA,
        source_name: store.index.entry_point.clone(),
        runtime_version: RUNTIME_VERSION.to_string(),
        kind,
        components,
        env_names,
        listeners,
        embedded_files,
    })
}

/// Operational fields carried by one embedded configuration entry
/// (`config`, `include`, or `profile` TOML text).
///
/// - env names: `${env:NAME}` tokens without defaults, via the same
///   raw-text scan used for route documents ([`scan_env_names`]);
/// - components: the per-component config block keys under every
///   `components` table at any depth (`[components.<name>]`,
///   `[default.components.<name>]`, profile overlays), sorted and
///   deduped;
/// - listeners: the configuration-declared listener endpoints —
///   `[observability.health]` and `[observability.prometheus]` tables
///   with `enabled = true` — as `host:port` with the camel-config host
///   defaults filled in and unresolved `${env:}` port expressions passed
///   through verbatim.
///
/// Derivation stays total: an entry that is not valid TOML contributes
/// only the raw-text env scan. Malformed configuration is named
/// precisely by the runtime configuration assembly
/// (`MalformedVirtualConfig`) before any boot — the manifest must not
/// turn a runtime-diagnosable defect into a compile-side rejection of
/// an otherwise well-formed store.
fn scan_config_entry(text: &str) -> (Vec<String>, Vec<String>) {
    let Ok(root) = toml::from_str::<toml::Value>(text) else {
        return (Vec::new(), Vec::new());
    };
    let mut components = Vec::new();
    let mut listeners = Vec::new();
    walk_config_tables(&root, &mut components, &mut listeners);
    components.sort();
    components.dedup();
    listeners.sort();
    listeners.dedup();
    (components, listeners)
}

/// Recursive walk over a configuration TOML tree collecting component
/// block names and enabled observability listener endpoints — the
/// config-side counterpart of [`walk_document`] (top-level, `[default]`,
/// and profile-overlay sections all reached at any depth).
fn walk_config_tables(
    value: &toml::Value,
    components: &mut Vec<String>,
    listeners: &mut Vec<String>,
) {
    match value {
        toml::Value::Table(table) => {
            for (key, val) in table {
                if key == "components"
                    && let Some(blocks) = val.as_table()
                {
                    for name in blocks.keys() {
                        if !components.iter().any(|s| s == name) {
                            components.push(name.clone());
                        }
                    }
                }
                if key == "observability"
                    && let Some(sections) = val.as_table()
                {
                    if let Some(entry) = sections.get("health") {
                        push_config_listener(listeners, entry, "8081");
                    }
                    if let Some(entry) = sections.get("prometheus") {
                        push_config_listener(listeners, entry, "9090");
                    }
                }
                walk_config_tables(val, components, listeners);
            }
        }
        toml::Value::Array(items) => {
            for item in items {
                walk_config_tables(item, components, listeners);
            }
        }
        _ => {}
    }
}

/// One enabled observability endpoint table -> `host:port`, honoring the
/// camel-config host/port defaults when the table omits them, and passing
/// a disabled or unresolved-port table through verbatim (an unresolved
/// `${env:}` port expression stays an expression, exactly like the
/// REST/MCP block handling).
///
/// SYNC: defaults mirror camel-config `default_health_host`/`port`
/// (`0.0.0.0` / `8081`) and `default_prometheus_host`/`port`
/// (`0.0.0.0` / `9090`); update together.
fn push_config_listener(listeners: &mut Vec<String>, entry: &toml::Value, default_port: &str) {
    let Some(table) = entry.as_table() else {
        return;
    };
    if table.get("enabled").and_then(toml::Value::as_bool) != Some(true) {
        return; // disabled endpoints bind nothing at runtime
    }
    let host = table
        .get("host")
        .and_then(toml::Value::as_str)
        .unwrap_or("0.0.0.0");
    let port = match table.get("port") {
        Some(toml::Value::Integer(n)) => n.to_string(),
        Some(toml::Value::String(s)) => s.clone(),
        _ => default_port.to_string(),
    };
    listeners.push(format!("{host}:{port}"));
}

/// Parser used for the manifest Value walk: the same YAML shim camel-dsl
/// uses, which also accepts JSON documents (YAML superset).
fn parse_document(document: &str) -> Result<serde_yml::Value, String> {
    serde_yml::from_str(document).map_err(|e| e.to_string())
}

/// Collect components and listeners from the document tree.
///
/// Generic recursive walk (not typed-AST): the raw document still carries
/// unresolved `${env:}` tokens, so typed fields like `rest.port: u16` cannot
/// deserialize. Endpoint strings are found under `from`/`to` keys at any
/// depth; listeners under the canonical `rest[]` and `mcp[].server` blocks.
fn walk_document(
    value: &serde_yml::Value,
    components: &mut Vec<String>,
    listeners: &mut Vec<String>,
) {
    match value {
        serde_yml::Value::Mapping(map) => {
            for (key, val) in map {
                let key = key.as_str();
                if (key == "from" || key == "to")
                    && let Some(uri) = val.as_str()
                {
                    push_scheme(components, uri);
                }
                if key == "rest"
                    && let Some(entries) = val.as_sequence()
                {
                    for entry in entries {
                        push_rest_listener(listeners, entry);
                    }
                }
                if key == "mcp"
                    && let Some(entries) = val.as_sequence()
                {
                    for entry in entries {
                        push_mcp_listener(listeners, entry);
                    }
                }
                walk_document(val, components, listeners);
            }
        }
        serde_yml::Value::Sequence(seq) => {
            for item in seq {
                walk_document(item, components, listeners);
            }
        }
        _ => {}
    }
}

/// Record the scheme of one endpoint URI (text before the first `:`).
/// Only RFC 3986 scheme shapes are recorded: a prefix that is not
/// `ALPHA *( ALPHA / DIGIT / "+" / "-" / "." )` carries no derivable scheme
/// pre-interpolation (e.g. a whole `${env:NAME}` token) and is skipped.
fn push_scheme(components: &mut Vec<String>, uri: &str) {
    let Some((scheme, _rest)) = uri.split_once(':') else {
        return;
    };
    if !is_uri_scheme(scheme) {
        return;
    }
    if !components.iter().any(|s| s == scheme) {
        components.push(scheme.to_string());
    }
}

/// RFC 3986 scheme grammar: leading ALPHA, then ALPHA / DIGIT / "+" / "-" / ".".
/// Shared with `compile::policy` for endpoint scheme classification.
pub(crate) fn is_uri_scheme(scheme: &str) -> bool {
    let mut chars = scheme.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphabetic() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

/// REST block listener entry -> `host:port`, honoring the canonical host/port
/// defaults when the raw block omits them.
///
/// SYNC: defaults mirror camel-dsl `default_rest_host`/`default_rest_port`
/// (`0.0.0.0` / `8080`); update together.
fn push_rest_listener(listeners: &mut Vec<String>, entry: &serde_yml::Value) {
    let Some(map) = entry.as_mapping() else {
        return;
    };
    let host = map
        .get("host")
        .and_then(serde_yml::Value::as_str)
        .unwrap_or("0.0.0.0");
    let port = match map.get("port") {
        Some(v) => match v {
            serde_yml::Value::Number(n) => n.to_string(),
            serde_yml::Value::String(s) => s.clone(),
            _ => "8080".to_string(),
        },
        None => "8080".to_string(),
    };
    listeners.push(format!("{host}:{port}"));
}

/// MCP block listener entry -> the `server.bind` value verbatim (an IP:port
/// literal or an unresolved `${env:}` expression, as written).
fn push_mcp_listener(listeners: &mut Vec<String>, entry: &serde_yml::Value) {
    let Some(map) = entry.as_mapping() else {
        return;
    };
    let Some(server) = map.get("server").and_then(serde_yml::Value::as_mapping) else {
        return;
    };
    let Some(bind) = server.get("bind").and_then(serde_yml::Value::as_str) else {
        return;
    };
    listeners.push(bind.to_string());
}

/// Token scanner for `${env:NAME}` / `${env:NAME:-default}` placeholders.
///
/// SYNC: env-only subset of camel-dsl `env_interpolation` `env_regex()` —
/// since jobargs Task 2.1 that grammar also carries `arg:` tokens, which
/// the manifest deliberately does NOT collect (job arguments are not
/// deployment env requirements; `$${arg:...}` still drops its leading
/// `$$` via the escape arm, and bare `${arg:NAME}` simply never matches).
/// Escape forms `$${env:...}` and `$$` are literal text, never
/// substitutions; crate purity forbids the dependency.
fn env_token_re() -> &'static Regex {
    static ENV_RE: OnceLock<Regex> = OnceLock::new();
    ENV_RE.get_or_init(|| {
        Regex::new(r"(\$\$\{env:[^}]*\})|(\$\$)|(\$\{env:([A-Za-z_][A-Za-z0-9_]*)(?::-([^}]*))?\})")
            .unwrap() // allow-unwrap
    })
}

/// Collect `${env:NAME}` names WITHOUT defaults, in source order, deduped.
/// Escaped forms and defaulted tokens are excluded: neither requires the
/// variable at runtime.
fn scan_env_names(document: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for caps in env_token_re().captures_iter(document) {
        if caps.get(1).is_some() || caps.get(2).is_some() {
            continue; // escaped literal text, not a substitution
        }
        if caps.get(5).is_some() {
            continue; // has a default (even empty): not required
        }
        let name = &caps[4];
        if !names.iter().any(|n| n == name) {
            names.push(name.to_string());
        }
    }
    names
}

// ---------------------------------------------------------------------------
// Tests. The blessed manifest test lives at MODULE level (not in a nested
// `mod tests`) so the mandated filter command
// `cargo test -p camel-cli --lib compile::manifest::<name>` matches exactly.
// Extra coverage lives in `mod tests` below.
// ---------------------------------------------------------------------------

#[test]
fn compile_manifest_lists_components_env_and_listeners() {
    let document = "\
routes:
  - id: tick
    from: timer:tick
    steps:
      - to: kafka:events
  - id: ingest
    from: ${env:INPUT}
    steps:
      - to: file:out?dir=${env:OUT_DIR:-/tmp/spool}
rest:
  - port: 8080
    host: 0.0.0.0
    operations:
      - get: /health
        to: direct:health
mcp:
  - server:
      name: tools
      bind: \"127.0.0.1:${env:MCP_PORT}\"
    tools: []
";

    let manifest = derive("routes/app.yaml", TrailerKind::Route, document)
        .expect("supported document must derive a manifest");

    assert_eq!(manifest.source_name, "routes/app.yaml");
    assert_eq!(manifest.kind, TrailerKind::Route);
    assert_eq!(manifest.runtime_version, RUNTIME_VERSION);
    assert_eq!(manifest.manifest_schema, MANIFEST_SCHEMA);

    // The document itself is the one embedded file: its logical path,
    // route kind, normalized length, and BLAKE3 content digest.
    assert_eq!(
        manifest.embedded_files,
        vec![EmbeddedFile {
            digest: blake3::hash(document.as_bytes()).to_hex().to_string(),
            kind: StoreEntryKind::Route,
            length: document.len() as u64,
            path: "routes/app.yaml".to_string(),
        }]
    );

    // Endpoint URI schemes in source order (timer, kafka, file; the
    // `${env:INPUT}` from-URI has no derivable scheme before interpolation
    // and the direct: operation target adds the last one).
    assert_eq!(
        manifest.components,
        vec!["timer", "kafka", "file", "direct"]
    );

    // Required env names, source order; OUT_DIR has a default and is
    // never required.
    assert_eq!(manifest.env_names, vec!["INPUT", "MCP_PORT"]);

    // Listeners: literal REST port and unresolved MCP bind expression.
    assert_eq!(
        manifest.listeners,
        vec!["0.0.0.0:8080", "127.0.0.1:${env:MCP_PORT}"]
    );

    // Canonical JSON: lexicographic keys, compact, deterministic.
    let json = manifest.to_canonical_json();
    assert_eq!(
        json,
        format!(
            "{{\"components\":[\"timer\",\"kafka\",\"file\",\"direct\"],\
\"embedded_files\":[{{\"digest\":\"{}\",\"kind\":\"route\",\"length\":{},\
\"path\":\"routes/app.yaml\"}}],\
\"env_names\":[\"INPUT\",\"MCP_PORT\"],\"kind\":\"route\",\
\"listeners\":[\"0.0.0.0:8080\",\"127.0.0.1:${{env:MCP_PORT}}\"],\
\"manifest_schema\":{MANIFEST_SCHEMA},\
\"runtime_version\":\"{RUNTIME_VERSION}\",\
\"source_name\":\"routes/app.yaml\"}}",
            blake3::hash(document.as_bytes()).to_hex(),
            document.len()
        )
    );
    // The string is itself valid JSON round-tripping to the same values.
    let reparsed: serde_json::Value =
        serde_json::from_str(&json).expect("canonical manifest JSON must parse");
    assert_eq!(
        reparsed.get("kind").and_then(serde_json::Value::as_str),
        Some("route")
    );
    assert_eq!(
        reparsed
            .get("manifest_schema")
            .and_then(serde_json::Value::as_u64),
        Some(MANIFEST_SCHEMA)
    );
    assert!(
        !json.contains("OUT_DIR"),
        "defaulted env tokens must not appear in the manifest"
    );
}

/// The manifest env-name scan stays an env-only subset of the shared
/// interpolation grammar (jobargs Task 3.2, per the jobargs Task 2.1
/// generalization): bare `${arg:NAME}` tokens and escaped `$${arg:NAME}`
/// forms contribute nothing to `env_names`, and the derived manifest of a
/// declared job lists only the genuine `${env:...}` requirement.
#[test]
fn manifest_env_scan_ignores_arg_tokens() {
    let document = "\
args:
  value:
    default: hello
execute:
  mode: one-shot
  timeout: 60s
  send:
    to: direct:transform
    body: \"${arg:value}\"
routes:
  - id: job-arg
    from: direct:lit
    steps:
      - set_body:
          value: \"$${arg:lit} ${env:HOST}\"
";

    let manifest = derive("args.job.yaml", TrailerKind::Job, document)
        .expect("declared job document must derive a manifest");

    assert_eq!(
        manifest.env_names,
        vec!["HOST"],
        "arg tokens (bare and escaped) must contribute nothing to env_names"
    );
}

/// The store-level derivation also scans the embedded configuration
/// chain: a config entry's `${env:HEALTH_PORT}` token without a default
/// joins the required env names, its enabled health endpoint becomes a
/// config-declared listener, and its component config blocks merge into
/// the components list (deduped against the endpoint schemes, sorted).
#[test]
fn compile_manifest_store_derivation_scans_config_entries() {
    use super::store::{StoreDocument, VirtualDocumentStore};

    let route_text = "\
routes:
  - id: a
    from: timer:a
    steps:
      - to: direct:a
";
    let config_text = "\
[components.kafka]
brokers = \"localhost:9092\"

[observability.health]
enabled = true
port = \"${env:HEALTH_PORT}\"
";
    let store = VirtualDocumentStore::build(
        "app.yaml",
        &[
            StoreDocument {
                path: "Camel.toml".to_string(),
                kind: StoreEntryKind::Config,
                bytes: config_text.as_bytes().to_vec(),
            },
            StoreDocument {
                path: "app.yaml".to_string(),
                kind: StoreEntryKind::Route,
                bytes: route_text.as_bytes().to_vec(),
            },
        ],
        &["Camel.toml".to_string()],
        &["app.yaml".to_string()],
    )
    .expect("valid store builds");

    let manifest = derive_for_store(
        &store,
        TrailerKind::Route,
        &[("app.yaml".to_string(), route_text.to_string())],
    )
    .expect("store manifest must derive");

    // Env names: route documents contribute nothing here; the config
    // entry's non-defaulted token is required. A defaulted token would
    // never be listed.
    assert_eq!(manifest.env_names, vec!["HEALTH_PORT"]);

    // Listeners: the enabled health endpoint with the camel-config host
    // default and the unresolved port expression passed through
    // verbatim. A disabled section declares no listener.
    assert_eq!(manifest.listeners, vec!["0.0.0.0:${env:HEALTH_PORT}"]);

    // Components: endpoint schemes keep source order; the
    // config-declared block name merges sorted and deduped.
    assert_eq!(manifest.components, vec!["timer", "direct", "kafka"]);
}

/// Shared fixture for the config-listener kind tests: a store whose
/// embedded configuration enables the health (8081) and prometheus
/// (9090) observability listeners, plus the entry-point source name.
/// The embedded document text and its store kind follow the artifact
/// kind under test, so each test embeds a document of its own shape.
#[cfg(test)]
fn observability_store(
    doc_text: &str,
    kind: TrailerKind,
) -> (super::store::VirtualDocumentStore, String) {
    use super::store::{StoreDocument, VirtualDocumentStore};

    let config_text = "\
[observability.health]
enabled = true
port = 8081

[observability.prometheus]
enabled = true
port = 9090
";
    let store = VirtualDocumentStore::build(
        "app.yaml",
        &[
            StoreDocument {
                path: "Camel.toml".to_string(),
                kind: StoreEntryKind::Config,
                bytes: config_text.as_bytes().to_vec(),
            },
            StoreDocument {
                path: "app.yaml".to_string(),
                kind: StoreEntryKind::from(kind),
                bytes: doc_text.as_bytes().to_vec(),
            },
        ],
        &["Camel.toml".to_string()],
        &["app.yaml".to_string()],
    )
    .expect("valid store builds");
    (store, "app.yaml".to_string())
}

/// The job boot projection suppresses the configuration-declared
/// observability listeners at runtime, so the job artifact manifest must
/// omit them too: the manifest reports the artifact kind's effective
/// runtime listeners, not the raw configuration chain.
#[test]
fn job_artifact_manifest_omits_config_listeners() {
    let job_text = "\
args:
  value:
    default: hello
execute:
  mode: one-shot
  timeout: 60s
  send:
    to: direct:transform
    body: \"hello\"
routes:
  - id: job-arg
    from: direct:lit
    steps:
      - set_body:
          value: \"lit\"
";
    let (store, source_name) = observability_store(job_text, TrailerKind::Job);
    let manifest = derive_for_store(
        &store,
        TrailerKind::Job,
        &[(source_name, job_text.to_string())],
    )
    .expect("store manifest must derive");

    assert!(
        !manifest.listeners.iter().any(|l| l == "0.0.0.0:8081"),
        "job artifact manifest must omit the suppressed health listener"
    );
    assert!(
        !manifest.listeners.iter().any(|l| l == "0.0.0.0:9090"),
        "job artifact manifest must omit the suppressed prometheus listener"
    );
}

/// Route artifacts bind the configuration-declared observability
/// listeners at runtime, so the route artifact manifest keeps them:
/// the config-entry listener merge is unchanged for route kind.
#[test]
fn route_artifact_manifest_keeps_config_listeners() {
    let route_text = "\
routes:
  - id: a
    from: timer:a
    steps:
      - to: direct:a
";
    let (store, source_name) = observability_store(route_text, TrailerKind::Route);
    let manifest = derive_for_store(
        &store,
        TrailerKind::Route,
        &[(source_name, route_text.to_string())],
    )
    .expect("store manifest must derive");

    assert!(
        manifest.listeners.iter().any(|l| l == "0.0.0.0:8081"),
        "route artifact manifest must keep the health listener"
    );
    assert!(
        manifest.listeners.iter().any(|l| l == "0.0.0.0:9090"),
        "route artifact manifest must keep the prometheus listener"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_derive_rejects_unparsable_document() {
        // Unclosed flow sequence: neither valid YAML nor valid JSON.
        let err = derive("x.yaml", TrailerKind::Route, "key: [unclosed")
            .expect_err("unparsable document must be named");
        assert!(matches!(err, CompileError::InvalidDocument(_)));
    }

    #[test]
    fn manifest_rest_defaults_applied_when_fields_absent() {
        let document = "rest:\n  - operations:\n      - get: /x\n        to: direct:a\n";
        let manifest = derive("a.yaml", TrailerKind::Route, document).expect("derives");
        assert_eq!(manifest.listeners, vec!["0.0.0.0:8080"]);
    }

    #[test]
    fn manifest_env_scan_skips_escapes_and_defaults() {
        let names = scan_env_names(
            "$${env:LIT} $$ ${env:REQUIRED} ${env:WITH_DEFAULT:-x} ${env:EMPTY_DEFAULT:-} \
             ${env:REQUIRED}",
        );
        assert_eq!(names, vec!["REQUIRED"]);
    }

    /// A minimal strict-valid schema-2 manifest; every case below mutates
    /// exactly one rule.
    fn schema2_json() -> serde_json::Value {
        serde_json::json!({
            "components": ["direct"],
            "embedded_files": [{
                "digest": blake3::hash(b"doc").to_hex().to_string(),
                "kind": "route",
                "length": 3,
                "path": "routes/a.yaml",
            }],
            "env_names": [],
            "kind": "route",
            "listeners": [],
            "manifest_schema": MANIFEST_SCHEMA,
            "runtime_version": RUNTIME_VERSION,
            "source_name": "routes/a.yaml",
        })
    }

    fn parse(value: &serde_json::Value) -> Result<Manifest, CompileError> {
        Manifest::from_canonical_json(value.to_string().as_bytes())
    }

    /// The strict schema-2 field rules: unknown fields, wrong types, bad
    /// entry shapes, and noncanonical entry order are all rejected by name
    /// — nothing collapses silently to a default.
    #[test]
    fn schema2_manifest_rejects_unknown_fields_wrong_types_and_shapes() {
        // The baseline is strict-valid.
        assert!(parse(&schema2_json()).is_ok());

        // Unknown top-level field.
        let mut value = schema2_json();
        value["signed"] = serde_json::json!(true);
        assert!(matches!(
            parse(&value),
            Err(CompileError::InvalidDocument(reason))
                if reason.contains("unknown schema-2 manifest field \"signed\"")
        ));

        // Wrong type: a string where `components` must be an array.
        let mut value = schema2_json();
        value["components"] = serde_json::json!("direct");
        assert!(matches!(
            parse(&value),
            Err(CompileError::InvalidDocument(reason))
                if reason.contains("field components is not an array")
        ));

        // Wrong element type inside a required string array.
        let mut value = schema2_json();
        value["env_names"] = serde_json::json!(["NAME", 7]);
        assert!(matches!(
            parse(&value),
            Err(CompileError::InvalidDocument(reason))
                if reason.contains("array env_names carries a non-string entry")
        ));

        // Wrong `kind` type.
        let mut value = schema2_json();
        value["kind"] = serde_json::json!(1);
        assert!(matches!(
            parse(&value),
            Err(CompileError::InvalidDocument(reason))
                if reason.contains("carries no kind")
        ));

        // Unknown field inside an `embedded_files` entry.
        let mut value = schema2_json();
        value["embedded_files"][0]["digest_algorithm"] = serde_json::json!("blake3");
        assert!(matches!(
            parse(&value),
            Err(CompileError::InvalidDocument(reason))
                if reason.contains("unknown embedded_files field \"digest_algorithm\"")
        ));

        // Digest shape: not 64 lowercase hex characters.
        for bad in ["ABCD", &"g".repeat(64), &"a".repeat(63)] {
            let mut value = schema2_json();
            value["embedded_files"][0]["digest"] = serde_json::json!(bad);
            assert!(matches!(
                parse(&value),
                Err(CompileError::InvalidDocument(reason))
                    if reason.contains("not 64-character lowercase hex")
            ));
        }

        // Path shape: traversal, backslash, and drive-prefix forms are
        // rejected by the same canonical-path rule as store entry paths.
        for bad in ["../escape.yaml", "routes\\a.yaml", "C:/routes/a.yaml"] {
            let mut value = schema2_json();
            value["embedded_files"][0]["path"] = serde_json::json!(bad);
            assert!(matches!(
                parse(&value),
                Err(CompileError::InvalidDocument(reason))
                    if reason.contains("embedded_files path invalid")
            ));
        }

        // Unknown document kind.
        let mut value = schema2_json();
        value["embedded_files"][0]["path"] = serde_json::json!("routes/a.yaml");
        value["embedded_files"][0]["kind"] = serde_json::json!("asset");
        assert!(matches!(
            parse(&value),
            Err(CompileError::InvalidDocument(reason))
                if reason.contains("unknown kind \"asset\"")
        ));

        // Wrong `length` type.
        let mut value = schema2_json();
        value["embedded_files"][0]["kind"] = serde_json::json!("route");
        value["embedded_files"][0]["length"] = serde_json::json!("3");
        assert!(matches!(
            parse(&value),
            Err(CompileError::InvalidDocument(reason))
                if reason.contains("carries no length integer")
        ));

        // Noncanonical entry order (and, equally, a duplicate path).
        let mut value = schema2_json();
        let second = schema2_json()["embedded_files"][0].clone();
        value["embedded_files"] = serde_json::json!([
            schema2_json()["embedded_files"][0],
            { "digest": blake3::hash(b"other").to_hex().to_string(),
              "kind": "route", "length": 5, "path": "routes/00.yaml" },
            second,
        ]);
        assert!(matches!(
            parse(&value),
            Err(CompileError::InvalidDocument(reason))
                if reason.contains("not in canonical path order")
        ));
    }

    /// The v1 writer emits the legacy schema-less form: no
    /// `manifest_schema`, no `embedded_files`, accepted by the v1 trailer
    /// validation and rejected as a v2 manifest.
    #[test]
    fn legacy_json_omits_schema2_fields_and_validates_as_v1() {
        let document = "routes:\n- id: a\n  from: direct:a\n";
        let manifest = derive("routes/app.yaml", TrailerKind::Route, document).expect("derives");
        let legacy = manifest.to_legacy_json();

        assert!(!legacy.contains("manifest_schema"));
        assert!(!legacy.contains("embedded_files"));
        assert!(legacy.contains(r#""kind":"route""#));
        assert!(legacy.contains(r#""source_name":"routes/app.yaml""#));

        assert_eq!(
            validate_manifest(legacy.as_bytes(), FORMAT_VERSION),
            Ok(TrailerKind::Route),
            "the transitional v1 writer must produce decodable v1 artifacts"
        );
        assert_eq!(
            validate_manifest(legacy.as_bytes(), FORMAT_VERSION_V2),
            Err(TrailerError::InvalidManifestSchema(MANIFEST_SCHEMA_LEGACY))
        );
    }
}
