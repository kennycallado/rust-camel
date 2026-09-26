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
//! list of every bundled virtual document AND asset with its logical
//! path (withheld for secret-class entries), document/asset kind, asset
//! class name, secret-material class, byte length, and content digest
//! (hex BLAKE3 of the exact embedded bytes). Schema 3 additionally
//! carries the `total_embedded_bytes` aggregate and the top-level
//! `artifact_kind` (`job` | `server`). Schema 4 (signed artifacts,
//! r4sign) additionally carries the `signing` block: exactly the
//! algorithm name, the `blake3:` fingerprint of the signing key's
//! verifying key, and the required bit — never any key material.
//!
//! Decode is strict per schema: only the declared fields are accepted
//! (unknown fields and wrong types rejected, never collapsed to
//! defaults), `embedded_files` entries carry well-formed digests, kinds,
//! lengths, classes, and canonical paths (secret-class entries carry a
//! null path), and the v2 trailer decode additionally enforces the
//! schema pairing (manifest 3 or 4 ⇔ store 2, manifest 2 ⇔ store 1) and
//! agreement between the manifest and the embedded store (same entries
//! by position, same content digests, path exposed exactly when the
//! class is public). The schema-less legacy form stays lenient and is
//! accepted only in v1 trailers.
//!
//! Derivation runs on the normalized, pre-interpolation document text: the
//! artifact captures authoring text before `${env:}` resolution, so every
//! field here must tolerate unresolved placeholders.

use std::sync::OnceLock;

use noyalib::compat::serde_yaml as serde_yml;
use regex::Regex;

use super::CompileError;
use super::signature;
use super::store::StoreEntryKind;
use super::trailer::{FORMAT_VERSION, FORMAT_VERSION_V2, TrailerError, TrailerKind};

/// Runtime version recorded in every manifest built by this crate.
pub const RUNTIME_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Manifest schema written by this crate for unsigned artifacts.
/// Validated independently from the trailer version. Schema 3 is the
/// asset-aware form: `embedded_files` entries carry `asset_class` and
/// the secret-material `class`, secret entries withhold their path, and
/// the manifest carries `total_embedded_bytes` and `artifact_kind`.
/// Valid only paired with a schema-2 store index.
pub const MANIFEST_SCHEMA: u64 = 3;

/// Manifest schema of a signed artifact (r4sign): the schema-3 fields
/// plus the mandatory `signing` block — exactly the algorithm name, the
/// `blake3:` fingerprint of the signing key's verifying key, and the
/// required bit. Valid only paired with a schema-2 store index.
pub const MANIFEST_SCHEMA_V4: u64 = 4;

/// Manifest schema of an R1-era v2 artifact (the operational fields and
/// 4-field `embedded_files` entries only). Valid only paired with a
/// schema-1 store index.
pub const MANIFEST_SCHEMA_V2: u64 = 2;

/// Manifest schema value of a legacy v1 artifact, whose JSON carried no
/// `manifest_schema` field at all.
pub const MANIFEST_SCHEMA_LEGACY: u64 = 1;

/// Secret-material class of one `embedded_files` entry: `public` for
/// documents and every non-private-key asset, `secret` for exactly the
/// private-key family. Distinct from `asset_class` (the class NAME, e.g.
/// `certificate`); sealed ruling bd rc-p823t.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ManifestClass {
    /// Ordinary content: the logical path is exposed.
    #[default]
    Public,
    /// Private-key family: the logical path is withheld (null) in the
    /// manifest body and `--manifest` output.
    Secret,
}

impl ManifestClass {
    /// Canonical lowercase name used in the manifest JSON.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Public => "public",
            Self::Secret => "secret",
        }
    }

    /// Inverse of [`ManifestClass::as_str`].
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "public" => Some(Self::Public),
            "secret" => Some(Self::Secret),
            _ => None,
        }
    }
}

/// The private-key family: the only secret-material class (sealed ruling
/// bd rc-p823t). Exactly the asset classes `certificate`-style names map
/// away from — an asset whose class NAME is `private key`.
fn is_secret_class(asset_class: Option<&str>) -> bool {
    asset_class == Some(crate::compile::policy::SECRET_ASSET_CLASS)
}

/// Top-level `artifact_kind` for a trailer kind: the trailer route kind
/// maps to `server`, job to `job` (bd rc-p823t: the sealed R3 tripwire
/// that lets an operator tell a bounded-run artifact from a listener
/// artifact without booting).
fn artifact_kind_for(kind: TrailerKind) -> &'static str {
    match kind {
        TrailerKind::Route => "server",
        TrailerKind::Job => "job",
    }
}

/// One embedded virtual document or asset as recorded in the manifest:
/// logical path (withheld for secret-class entries), document/asset kind,
/// asset class name, secret-material class, byte length, and content
/// digest (hex BLAKE3 of the exact embedded bytes).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EmbeddedFile {
    /// Asset class name (`certificate`, `private key`, `xslt`, …) for
    /// asset entries; always null for document entries.
    pub asset_class: Option<String>,
    /// Secret-material class: `path` is null if and only if this is
    /// [`ManifestClass::Secret`].
    #[serde(default)]
    pub class: ManifestClass,
    /// Content digest as lowercase hex BLAKE3.
    pub digest: String,
    /// Entry kind (`route`, `job`, `config`, `include`, `profile`,
    /// `asset`).
    pub kind: StoreEntryKind,
    /// Byte length (normalized for documents, verbatim for assets).
    pub length: u64,
    /// Canonical logical path; withheld (null) exactly for secret-class
    /// entries. The store index retains the logical path for
    /// substitution at runtime.
    pub path: Option<String>,
}

/// Signing block of a schema-4 manifest (r4sign): present exactly in
/// signed artifacts. Records the canonical algorithm name, the
/// `blake3:` fingerprint of the signing key's verifying key, and the
/// required bit — never any key material.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SigningBlock {
    /// Canonical algorithm name (currently always `ed25519ph`).
    pub algorithm: String,
    /// `blake3:` + lowercase hex BLAKE3 over the 32-byte verifying key —
    /// the identity operators pin; the private seed never appears
    /// anywhere.
    pub key_fingerprint: String,
    /// Whether a boot without the detached envelope fails closed.
    pub required: bool,
}

/// Operational manifest of a compiled artifact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    /// Manifest schema; only [`MANIFEST_SCHEMA`] is written, and
    /// [`MANIFEST_SCHEMA_V2`] and [`MANIFEST_SCHEMA_LEGACY`] are
    /// tolerated on decode (each under its own store-schema pairing).
    pub manifest_schema: u64,
    /// Logical source name: input path relative to the compile working
    /// directory. Runtime source identity becomes `compiled://<source_name>`.
    pub source_name: String,
    /// Runtime version of the compiling CLI.
    pub runtime_version: String,
    /// Embedded artifact kind.
    pub kind: TrailerKind,
    /// Top-level artifact kind for operators (`"job"` | `"server"`):
    /// derived from `kind` (route → `server`, job → `job`).
    pub artifact_kind: String,
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
    /// Every bundled virtual document and asset, in the same canonical
    /// order as the store index.
    pub embedded_files: Vec<EmbeddedFile>,
    /// Sum of all content-entry byte lengths.
    pub total_embedded_bytes: u64,
    /// Signing block: present exactly in schema-4 manifests (signed
    /// artifacts). Unsigned manifests carry `None` and serialize without
    /// the field, keeping the schema-3 form byte-identical.
    pub signing: Option<SigningBlock>,
}

impl Manifest {
    /// Serialize to canonical UTF-8 JSON: compact, lexicographically ordered
    /// keys (serde_json maps are BTreeMaps), source-ordered arrays. This is
    /// the schema-3 form carried by v2 artifacts; a signed manifest (schema
    /// 4) additionally serializes the `signing` block.
    pub fn to_canonical_json(&self) -> String {
        let mut value = serde_json::json!({
            "artifact_kind": self.artifact_kind,
            "components": self.components,
            "embedded_files": self.embedded_files,
            "env_names": self.env_names,
            "kind": self.kind.as_str(),
            "listeners": self.listeners,
            "manifest_schema": self.manifest_schema,
            "runtime_version": self.runtime_version,
            "source_name": self.source_name,
            "total_embedded_bytes": self.total_embedded_bytes,
        });
        if let Some(signing) = &self.signing {
            value["signing"] = serde_json::json!({
                "algorithm": signing.algorithm,
                "key_fingerprint": signing.key_fingerprint,
                "required": signing.required,
            });
        }
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
    /// independently from any trailer version: [`MANIFEST_SCHEMA`] (3),
    /// [`MANIFEST_SCHEMA_V2`] (2), [`MANIFEST_SCHEMA_V4`] (4), and the
    /// schema-less legacy v1 form are accepted, anything else is rejected.
    /// The strict per-schema field rules apply whenever the declared
    /// schema is known: `check_schema3_fields` for [`MANIFEST_SCHEMA`],
    /// `check_schema4_fields` for [`MANIFEST_SCHEMA_V4`], and
    /// `check_schema2_fields` for [`MANIFEST_SCHEMA_V2`]. The
    /// store-schema pairing is enforced where both schemas are visible —
    /// in the trailer decode (`decode_artifact`), never here.
    pub fn from_canonical_json(bytes: &[u8]) -> Result<Manifest, CompileError> {
        let value: serde_json::Value = serde_json::from_slice(bytes)
            .map_err(|e| CompileError::InvalidDocument(format!("manifest is not JSON: {e}")))?;
        let schema = manifest_schema_of(&value).map_err(|_| {
            CompileError::InvalidDocument("manifest schema is not an integer".to_string())
        })?;
        if schema != MANIFEST_SCHEMA
            && schema != MANIFEST_SCHEMA_V2
            && schema != MANIFEST_SCHEMA_V4
            && schema != MANIFEST_SCHEMA_LEGACY
        {
            return Err(CompileError::InvalidDocument(format!(
                "unsupported manifest schema {schema}"
            )));
        }
        match schema {
            MANIFEST_SCHEMA => {
                check_schema3_fields(&value).map_err(CompileError::InvalidDocument)?;
            }
            MANIFEST_SCHEMA_V4 => {
                check_schema4_fields(&value).map_err(CompileError::InvalidDocument)?;
            }
            MANIFEST_SCHEMA_V2 => {
                check_schema2_fields(&value).map_err(CompileError::InvalidDocument)?;
            }
            _ => {}
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
            // empty list. For schemas 2 and 3 the field is required, and
            // the per-schema field check has already rejected its absence.
            None => Vec::new(),
        };
        // Schemas 3 and 4 carry `artifact_kind` and
        // `total_embedded_bytes` explicitly (validated by the per-schema
        // field checks); older schemas predate both, so they are derived
        // to keep the parsed struct total: the artifact kind from the
        // trailer kind, and the aggregate from the entry lengths.
        let (artifact_kind, total_embedded_bytes) =
            if schema == MANIFEST_SCHEMA || schema == MANIFEST_SCHEMA_V4 {
                (
                    object("artifact_kind")?.to_string(),
                    value
                        .get("total_embedded_bytes")
                        .and_then(serde_json::Value::as_u64)
                        .ok_or(CompileError::InvalidDocument(
                            "manifest carries no total_embedded_bytes integer".to_string(),
                        ))?,
                )
            } else {
                (
                    artifact_kind_for(kind).to_string(),
                    embedded_files.iter().map(|f| f.length).sum(),
                )
            };
        // The signing block exists only in schema-4 manifests; the
        // per-schema field checks have already rejected its presence in
        // schemas 2/3 (unknown field) and enforced its shape in schema 4.
        // Extraction is gated on schema 4 so the lenient legacy form
        // never gains a signing block it did not carry.
        let signing = if schema == MANIFEST_SCHEMA_V4 {
            match value.get("signing") {
                Some(block) => Some(
                    serde_json::from_value::<SigningBlock>(block.clone()).map_err(|e| {
                        CompileError::InvalidDocument(format!("invalid signing block: {e}"))
                    })?,
                ),
                None => None,
            }
        } else {
            None
        };
        Ok(Manifest {
            manifest_schema: schema,
            source_name: object("source_name")?.to_string(),
            runtime_version: object("runtime_version")?.to_string(),
            kind,
            artifact_kind,
            components: strings("components"),
            env_names: strings("env_names"),
            listeners: strings("listeners"),
            embedded_files,
            total_embedded_bytes,
            signing,
        })
    }
}

/// Read the effective manifest schema from parsed manifest JSON: the
/// `manifest_schema` field, defaulting to the legacy schema when absent.
pub(crate) fn manifest_schema_of(value: &serde_json::Value) -> Result<u64, ()> {
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

/// Every field a schema-3 manifest may carry. Anything else is rejected:
/// an unrecognized field would silently change meaning without a schema
/// bump.
const SCHEMA3_FIELDS: [&str; 10] = [
    "artifact_kind",
    "components",
    "embedded_files",
    "env_names",
    "kind",
    "listeners",
    "manifest_schema",
    "runtime_version",
    "source_name",
    "total_embedded_bytes",
];

/// Every field a schema-4 manifest may carry: the schema-3 set plus the
/// `signing` block (r4sign). Anything else is rejected.
const SCHEMA4_FIELDS: [&str; 11] = [
    "artifact_kind",
    "components",
    "embedded_files",
    "env_names",
    "kind",
    "listeners",
    "manifest_schema",
    "runtime_version",
    "signing",
    "source_name",
    "total_embedded_bytes",
];

/// Every field the schema-4 `signing` block may carry.
const SIGNING_BLOCK_FIELDS: [&str; 3] = ["algorithm", "key_fingerprint", "required"];

/// Every field a schema-3 `embedded_files` entry may carry.
const EMBEDDED_FILE_FIELDS3: [&str; 6] =
    ["asset_class", "class", "digest", "kind", "length", "path"];

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
    // The manifest mirrors every store entry 1:1 (r2embed Task 1.2):
    // the schema-2 store carries typed asset entries, so embedded_files
    // entries accept `kind: "asset"` too. Per-asset `asset_class`/`
    // `class` manifest fields are the schema-3 form (Task 2.2); schema-2
    // entries keep the R1-era 4-field shape.
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

/// Type- and shape-check one schema-3 `embedded_files` entry and return
/// its byte length (for the aggregate check):
///
/// - exactly the [`EMBEDDED_FILE_FIELDS3`] fields;
/// - `digest`: 64-character lowercase-hex BLAKE3;
/// - `kind`: a known document/asset kind;
/// - `asset_class`: null for document entries, a class name for asset
///   entries;
/// - `class`: `"public"` | `"secret"` — `secret` exactly when
///   `asset_class` is the private-key family (sealed ruling bd rc-p823t);
/// - `path`: null if and only if `class` is `"secret"`; a present path
///   follows the same canonical-path rule as store entry paths.
///
/// Entry ORDER is not re-checked here: secret entries carry a null path,
/// so path ordering is not expressible from the manifest alone. The
/// canonical store order is enforced positionally by the trailer decode's
/// manifest/store agreement check, which compares every entry against the
/// store index.
fn check_embedded_file3(item: &serde_json::Value) -> Result<u64, String> {
    let Some(map) = item.as_object() else {
        return Err("embedded_files entry is not a JSON object".to_string());
    };
    for key in map.keys() {
        if !EMBEDDED_FILE_FIELDS3.contains(&key.as_str()) {
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
    let kind = StoreEntryKind::from_name(kind)
        .ok_or_else(|| format!("embedded_files entry names unknown kind {kind:?}"))?;
    let asset_class = map
        .get("asset_class")
        .map(serde_json::Value::as_str)
        .ok_or_else(|| {
            "embedded_files entry carries no asset_class (string or null)".to_string()
        })?;
    if kind != StoreEntryKind::Asset && asset_class.is_some() {
        return Err(format!(
            "embedded_files {} entry must carry a null asset_class",
            kind.as_str()
        ));
    }
    if kind == StoreEntryKind::Asset && asset_class.is_none() {
        return Err("embedded_files asset entry carries no asset_class string".to_string());
    }
    let class = map
        .get("class")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "embedded_files entry carries no class string".to_string())?;
    let class = ManifestClass::from_name(class)
        .ok_or_else(|| format!("embedded_files entry names unknown class {class:?}"))?;
    // The secret-material class is exactly the private-key family: any
    // other classification is a lie and fails closed.
    if is_secret_class(asset_class) != (class == ManifestClass::Secret) {
        // allow-secret: the diagnostic names the class, never a value.
        return Err(format!(
            "embedded_files entry class {:?} does not match its asset_class {:?}: secret is exactly the private-key family",
            class.as_str(),
            asset_class
        ));
    }
    let length = map
        .get("length")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| "embedded_files entry carries no length integer".to_string())?;
    match map.get("path") {
        Some(serde_json::Value::Null) => {
            if class == ManifestClass::Public {
                return Err(
                    "embedded_files public entry must carry its path (found null)".to_string(),
                );
            }
        }
        Some(serde_json::Value::String(path)) => {
            if class == ManifestClass::Secret {
                // allow-secret: names the withheld path's presence, not a key value.
                return Err(format!(
                    "embedded_files secret entry must withhold its path (found {path:?})"
                ));
            }
            super::store::validate_path(path)
                .map_err(|e| format!("embedded_files path invalid: {e}"))?;
        }
        _ => {
            return Err("embedded_files entry path must be a string or null".to_string());
        }
    }
    Ok(length)
}

/// Strict field rules for a schema-3 manifest: the schema-2 rules plus
/// the asset-aware additions —
///
/// - only the declared [`SCHEMA3_FIELDS`] may appear;
/// - `kind`, `source_name`, and `runtime_version` are required strings;
/// - `components`, `env_names`, and `listeners` are required arrays of
///   strings;
/// - `artifact_kind` is a required `"job"` | `"server"` string that
///   agrees with the manifest `kind` (route → `server`, job → `job`);
/// - `total_embedded_bytes` is a required integer equal to the sum of the
///   `embedded_files` entry lengths;
/// - `embedded_files` is a required array of entries passing
///   [`check_embedded_file3`].
fn check_schema3_fields(value: &serde_json::Value) -> Result<(), String> {
    check_schema34_fields(value, false)
}

/// Strict field rules for a schema-4 manifest (r4sign): exactly the
/// schema-3 rules plus the mandatory signing block.
fn check_schema4_fields(value: &serde_json::Value) -> Result<(), String> {
    check_schema34_fields(value, true)
}

/// Shared schema-3/schema-4 field rules; `schema4` selects the field
/// whitelist (the schema-3 set plus `signing`) and appends the mandatory
/// [`check_signing_block`] validation.
fn check_schema34_fields(value: &serde_json::Value, schema4: bool) -> Result<(), String> {
    let fields: &[&str] = if schema4 {
        &SCHEMA4_FIELDS
    } else {
        &SCHEMA3_FIELDS
    };
    let label = if schema4 { "schema-4" } else { "schema-3" };
    let Some(map) = value.as_object() else {
        return Err("manifest is not a JSON object".to_string());
    };
    for key in map.keys() {
        if !fields.contains(&key.as_str()) {
            return Err(format!("unknown {label} manifest field {key:?}"));
        }
    }
    for field in ["kind", "source_name", "runtime_version"] {
        if value
            .get(field)
            .and_then(serde_json::Value::as_str)
            .is_none()
        {
            return Err(format!("{label} manifest carries no {field}"));
        }
    }
    for field in ["components", "env_names", "listeners"] {
        let Some(items) = value.get(field).and_then(serde_json::Value::as_array) else {
            return Err(format!("{label} manifest field {field} is not an array"));
        };
        if items.iter().any(|item| item.as_str().is_none()) {
            return Err(format!(
                "{label} manifest array {field} carries a non-string entry"
            ));
        }
    }
    let artifact_kind = value
        .get("artifact_kind")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| format!("{label} manifest carries no artifact_kind string"))?;
    let kind = value
        .get("kind")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| format!("{label} manifest carries no kind"))?;
    if artifact_kind_for(
        TrailerKind::from_name(kind)
            .ok_or_else(|| format!("{label} manifest carries no recognizable kind"))?,
    ) != artifact_kind
    {
        return Err(format!(
            "{label} manifest artifact_kind {artifact_kind:?} does not match kind {kind:?}"
        ));
    }
    let Some(serde_json::Value::Array(items)) = value.get("embedded_files") else {
        return Err(format!("{label} manifest carries no embedded_files array"));
    };
    let mut total = 0u64;
    for item in items {
        total = total.saturating_add(check_embedded_file3(item)?);
    }
    let declared = value
        .get("total_embedded_bytes")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| format!("{label} manifest carries no total_embedded_bytes integer"))?;
    if declared != total {
        return Err(format!(
            "{label} manifest total_embedded_bytes {declared} does not equal the entry length sum {total}"
        ));
    }
    if schema4 {
        check_signing_block(value)?;
    }
    Ok(())
}

/// Type- and shape-check the mandatory `signing` block of a schema-4
/// manifest (r4sign):
///
/// - exactly the [`SIGNING_BLOCK_FIELDS`] fields;
/// - `algorithm` is the supported algorithm name (`ed25519ph`);
/// - `key_fingerprint` is `blake3:` + 64 lowercase hex characters;
/// - `required` is a boolean.
fn check_signing_block(value: &serde_json::Value) -> Result<(), String> {
    let Some(signing) = value.get("signing").and_then(serde_json::Value::as_object) else {
        return Err("schema-4 manifest carries no signing block".to_string());
    };
    for key in signing.keys() {
        if !SIGNING_BLOCK_FIELDS.contains(&key.as_str()) {
            return Err(format!("unknown signing block field {key:?}"));
        }
    }
    let algorithm = signing
        .get("algorithm")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "signing block carries no algorithm string".to_string())?;
    if algorithm != signature::ALGORITHM_NAME_ED25519PH {
        return Err(format!(
            "signing block names unsupported algorithm {algorithm:?}"
        ));
    }
    let fingerprint = signing
        .get("key_fingerprint")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "signing block carries no key_fingerprint string".to_string())?;
    let fingerprint_shape = "blake3:<64 lowercase hex>";
    let Some(hex) = fingerprint.strip_prefix("blake3:") else {
        return Err(format!(
            "signing block key_fingerprint {fingerprint:?} is not {fingerprint_shape}"
        ));
    };
    if !is_blake3_hex(hex) {
        return Err(format!(
            "signing block key_fingerprint {fingerprint:?} is not {fingerprint_shape}"
        ));
    }
    if signing
        .get("required")
        .and_then(serde_json::Value::as_bool)
        .is_none()
    {
        return Err("signing block carries no required boolean".to_string());
    }
    Ok(())
}

/// Validate manifest bytes for trailer decode and return the artifact kind.
///
/// Each trailer version accepts exactly its own manifest forms: a v1
/// trailer carries the schema-less legacy manifest only, and a v2 trailer
/// requires `manifest_schema: 3` (unsigned) or `manifest_schema: 4`
/// (signed, r4sign) with the strict schema-3/schema-4 field rules
/// ([`check_schema3_fields`]/[`check_schema4_fields`]) or the R1-era
/// `manifest_schema: 2` with the strict schema-2 field rules
/// ([`check_schema2_fields`]) — which of these is valid for THIS
/// artifact is decided by the store-schema pairing check in
/// `decode_artifact` (manifest 3 or 4 ⇔ store 2, manifest 2 ⇔ store 1),
/// the only site that sees both schemas. Fail closed on unknown schemas,
/// non-JSON bytes, unrecognizable kinds, and incomplete manifests; the
/// unknown-schema check runs first, so an unknown manifest schema is
/// named before any pairing consideration.
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
        FORMAT_VERSION_V2 => match schema {
            MANIFEST_SCHEMA => {
                check_schema3_fields(&value).map_err(TrailerError::InvalidManifestFields)?;
            }
            MANIFEST_SCHEMA_V4 => {
                check_schema4_fields(&value).map_err(TrailerError::InvalidManifestFields)?;
            }
            MANIFEST_SCHEMA_V2 => {
                check_schema2_fields(&value).map_err(TrailerError::InvalidManifestFields)?;
            }
            _ => return Err(TrailerError::InvalidManifestSchema(schema)),
        },
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
    let length = document.len() as u64;
    Ok(Manifest {
        manifest_schema: MANIFEST_SCHEMA,
        source_name: source_name.to_string(),
        runtime_version: RUNTIME_VERSION.to_string(),
        kind,
        artifact_kind: artifact_kind_for(kind).to_string(),
        components,
        env_names,
        listeners,
        total_embedded_bytes: length,
        embedded_files: vec![EmbeddedFile {
            asset_class: None,
            class: ManifestClass::Public,
            digest: blake3::hash(document.as_bytes()).to_hex().to_string(),
            kind: StoreEntryKind::from(kind),
            length,
            path: Some(source_name.to_string()),
        }],
        signing: None,
    })
}

/// Derive the schema-3 manifest for a built virtual store (r2embed Task
/// 2.2): operational fields are scanned from every route/job
/// document in source-plan order (`route_documents` carries `(logical
/// path, normalized text)` pairs) AND from every embedded
/// config/include/profile entry in canonical store order, and
/// `embedded_files` mirrors the store entries — the SAME canonical order
/// as the store index, so registry verification can match manifest
/// entries to store entries by position. Every entry carries its kind,
/// byte length, and BLAKE3 content digest (the same digest rule for
/// assets and documents); asset entries additionally carry their
/// `asset_class` name and secret-material `class`, and secret-class
/// entries (exactly the private-key family, bd rc-p823t) withhold their
/// logical path. The manifest also records the `total_embedded_bytes`
/// aggregate and the top-level `artifact_kind`. The result is always
/// unsigned (schema 3, `signing: None`); the signed form (schema 4 plus
/// the signing block) is applied by the compile call site.
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
    let mut total_embedded_bytes = 0u64;
    for entry in &store.index.entries {
        let range = entry.offset as usize..(entry.offset + entry.length) as usize;
        let bytes = store.content.get(range).ok_or_else(|| {
            CompileError::InvalidDocument(format!(
                "store entry '{}' range is out of content bounds",
                entry.path
            ))
        })?;
        let class = if is_secret_class(entry.asset_class.as_deref()) {
            ManifestClass::Secret
        } else {
            ManifestClass::Public
        };
        // Secret-class entries withhold their logical path from the
        // operator-facing manifest; the store index retains it for the
        // runtime substitution table.
        let path = match class {
            ManifestClass::Secret => None,
            ManifestClass::Public => Some(entry.path.clone()),
        };
        total_embedded_bytes = total_embedded_bytes.saturating_add(entry.length);
        embedded_files.push(EmbeddedFile {
            asset_class: entry.asset_class.clone(),
            class,
            digest: blake3::hash(bytes).to_hex().to_string(),
            kind: entry.kind,
            length: entry.length,
            path,
        });
    }
    Ok(Manifest {
        manifest_schema: MANIFEST_SCHEMA,
        source_name: store.index.entry_point.clone(),
        runtime_version: RUNTIME_VERSION.to_string(),
        kind,
        artifact_kind: artifact_kind_for(kind).to_string(),
        components,
        env_names,
        listeners,
        embedded_files,
        total_embedded_bytes,
        signing: None,
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
            asset_class: None,
            class: ManifestClass::Public,
            digest: blake3::hash(document.as_bytes()).to_hex().to_string(),
            kind: StoreEntryKind::Route,
            length: document.len() as u64,
            path: Some("routes/app.yaml".to_string()),
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
            "{{\"artifact_kind\":\"server\",\
\"components\":[\"timer\",\"kafka\",\"file\",\"direct\"],\
\"embedded_files\":[{{\"asset_class\":null,\"class\":\"public\",\"digest\":\"{}\",\"kind\":\"route\",\"length\":{},\
\"path\":\"routes/app.yaml\"}}],\
\"env_names\":[\"INPUT\",\"MCP_PORT\"],\"kind\":\"route\",\
\"listeners\":[\"0.0.0.0:8080\",\"127.0.0.1:${{env:MCP_PORT}}\"],\
\"manifest_schema\":{MANIFEST_SCHEMA},\
\"runtime_version\":\"{RUNTIME_VERSION}\",\
\"source_name\":\"routes/app.yaml\",\
\"total_embedded_bytes\":{}}}",
            blake3::hash(document.as_bytes()).to_hex(),
            document.len(),
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

/// r2embed Task 2.2: the schema-3 manifest records every asset entry's
/// `asset_class`, secret-material `class` (secret = exactly the
/// private-key family), byte length, and BLAKE3 digest over the exact
/// embedded bytes, plus the aggregate `total_embedded_bytes` and the
/// top-level `artifact_kind` (`server` for a route compile, `job` for a
/// job compile).
#[test]
fn manifest_schema3_records_class_kind_digest_and_total() {
    use super::store::{StoreAsset, StoreDocument, VirtualDocumentStore};

    let route_text =
        "routes:\n- id: a\n  from: timer:a\n  steps:\n    - to: 'xslt:tls/transform.xslt'\n";
    let cert = b"-----BEGIN CERTIFICATE-----\nroute-leg\n".to_vec();
    let key = b"-----BEGIN PRIVATE KEY-----\nroute-leg\n".to_vec();
    let data = b"static-bytes".to_vec();
    let documents = [StoreDocument {
        path: "routes/app.yaml".to_string(),
        kind: StoreEntryKind::Route,
        bytes: route_text.as_bytes().to_vec(),
    }];
    let assets = [
        StoreAsset {
            bytes: cert.clone(),
            class: Some("certificate".to_string()),
            path: "tls/server.crt".to_string(),
        },
        StoreAsset {
            bytes: key.clone(),
            class: Some("private key".to_string()),
            path: "tls/server.key".to_string(),
        },
        StoreAsset {
            bytes: data.clone(),
            class: Some("static directory".to_string()),
            path: "www/data.txt".to_string(),
        },
    ];
    let store = VirtualDocumentStore::build_with_assets(
        "routes/app.yaml",
        &documents,
        &assets,
        &[],
        &["routes/app.yaml".to_string()],
        &[],
    )
    .expect("valid asset store builds");

    let manifest = derive_for_store(
        &store,
        TrailerKind::Route,
        &[("routes/app.yaml".to_string(), route_text.to_string())],
    )
    .expect("store manifest must derive");

    assert_eq!(manifest.manifest_schema, MANIFEST_SCHEMA);
    assert_eq!(manifest.artifact_kind, "server");

    // Every entry mirrors its store entry by position: the length matches
    // and the digest is the BLAKE3 of the exact embedded bytes,
    // independently recomputed here from the store content.
    let total: u64 = manifest.embedded_files.iter().map(|f| f.length).sum();
    assert_eq!(manifest.total_embedded_bytes, total);
    assert_eq!(manifest.embedded_files.len(), store.index.entries.len());
    for (file, entry) in manifest.embedded_files.iter().zip(&store.index.entries) {
        assert_eq!(file.length, entry.length, "length mirrors {entry:?}");
        let range = entry.offset as usize..(entry.offset + entry.length) as usize;
        let bytes = store.content.get(range).expect("entry range in bounds");
        assert_eq!(
            file.digest,
            blake3::hash(bytes).to_hex().to_string(),
            "digest of the exact embedded bytes for {entry:?}"
        );
    }

    // The private-key asset is the secret class with its path withheld.
    let secret = manifest
        .embedded_files
        .iter()
        .find(|f| f.class == ManifestClass::Secret)
        .expect("the private-key asset must be class secret");
    assert_eq!(secret.asset_class.as_deref(), Some("private key"));
    assert_eq!(secret.path, None, "secret entries withhold their path");
    assert_eq!(secret.length, key.len() as u64);
    assert_eq!(secret.digest, blake3::hash(&key).to_hex().to_string());

    // Public assets keep class public, their asset class name, and path.
    let cert_file = manifest
        .embedded_files
        .iter()
        .find(|f| f.asset_class.as_deref() == Some("certificate"))
        .expect("the certificate asset must be listed");
    assert_eq!(cert_file.class, ManifestClass::Public);
    assert_eq!(cert_file.path.as_deref(), Some("assets/tls/server.crt"));
    assert_eq!(cert_file.digest, blake3::hash(&cert).to_hex().to_string());

    // Documents are implicitly public with a null asset class.
    let doc = manifest
        .embedded_files
        .iter()
        .find(|f| f.kind == StoreEntryKind::Route)
        .expect("the route document must be listed");
    assert_eq!(doc.class, ManifestClass::Public);
    assert_eq!(doc.asset_class, None);
    assert_eq!(doc.path.as_deref(), Some("routes/app.yaml"));

    // The job-kind leg: artifact_kind is `job`.
    let job_text = "args:\n  v:\n    default: x\nexecute:\n  mode: one-shot\n  timeout: 60s\n  send:\n    to: direct:a\n    body: \"x\"\nroutes:\n- id: j\n  from: direct:lit\n  steps:\n    - set_body:\n        value: \"lit\"\n";
    let job_documents = [StoreDocument {
        path: "jobs/nightly.job.yaml".to_string(),
        kind: StoreEntryKind::Job,
        bytes: job_text.as_bytes().to_vec(),
    }];
    let job_store = VirtualDocumentStore::build_with_assets(
        "jobs/nightly.job.yaml",
        &job_documents,
        &[],
        &[],
        &["jobs/nightly.job.yaml".to_string()],
        &[],
    )
    .expect("valid job store builds");
    let job_manifest = derive_for_store(
        &job_store,
        TrailerKind::Job,
        &[("jobs/nightly.job.yaml".to_string(), job_text.to_string())],
    )
    .expect("job manifest must derive");
    assert_eq!(job_manifest.artifact_kind, "job");
    assert_eq!(job_manifest.manifest_schema, MANIFEST_SCHEMA);
    let job_total: u64 = job_manifest.embedded_files.iter().map(|f| f.length).sum();
    assert_eq!(job_manifest.total_embedded_bytes, job_total);
}

/// r2embed Task 2.2 (sealed ruling bd rc-p823t): manifest entries for
/// secret-class assets expose ONLY the classification fields, byte length,
/// and BLAKE3 digest — the logical path is withheld and no key material
/// appears in the manifest body (the exact bytes `--manifest` prints).
#[test]
fn secret_manifest_entries_expose_digest_and_length_only() {
    use super::store::{StoreAsset, StoreDocument, VirtualDocumentStore};

    let route_text = "routes:\n- id: a\n  from: timer:a\n  steps:\n    - to: direct:a\n";
    let key = b"-----BEGIN PRIVATE KEY-----\nSECRET-KEY-MATERIAL\n".to_vec();
    let documents = [StoreDocument {
        path: "routes/app.yaml".to_string(),
        kind: StoreEntryKind::Route,
        bytes: route_text.as_bytes().to_vec(),
    }];
    let assets = [StoreAsset {
        bytes: key.clone(),
        class: Some("private key".to_string()),
        path: "tls/server.key".to_string(),
    }];
    let store = VirtualDocumentStore::build_with_assets(
        "routes/app.yaml",
        &documents,
        &assets,
        &[],
        &["routes/app.yaml".to_string()],
        &[],
    )
    .expect("valid asset store builds");

    let manifest = derive_for_store(
        &store,
        TrailerKind::Route,
        &[("routes/app.yaml".to_string(), route_text.to_string())],
    )
    .expect("store manifest must derive");

    let secret = manifest
        .embedded_files
        .iter()
        .find(|f| f.class == ManifestClass::Secret)
        .expect("the private-key asset must be class secret");
    assert_eq!(secret.path, None, "the logical path is withheld");
    assert_eq!(secret.length, key.len() as u64);
    assert_eq!(secret.digest, blake3::hash(&key).to_hex().to_string());

    // The canonical body — the exact bytes `--manifest` prints — carries
    // class, length, and digest only: neither the logical path nor any
    // key material appears anywhere.
    let json = manifest.to_canonical_json();
    assert!(
        json.contains(r#""class":"secret""#),
        "class exposed: {json}"
    );
    assert!(
        !json.contains("server.key"),
        "the logical path must be withheld: {json}"
    );
    assert!(
        !json.contains("SECRET-KEY-MATERIAL"),
        "key material must never appear: {json}"
    );
    assert!(
        !json.contains("BEGIN PRIVATE KEY"),
        "key material must never appear: {json}"
    );
}

/// r2embed Task 2.2 (pairing widened by r4sign Task 1.2): the manifest
/// reader is trailer-version aware and the v2 pairing is enforced in both
/// directions — the R1-era pair (store 1 + manifest 2) decodes with null
/// asset classes, an unknown manifest schema fails with the
/// unknown-schema diagnostic (never a pairing error), a schema-4 body
/// without its signing block fails under the schema-4 field rules, and
/// BOTH mismatched pairings (store 2 + manifest 2, store 1 + manifest 3)
/// fail closed with the pairing diagnostic naming the accepted set. The
/// pairing check itself runs in `decode_artifact` after the store decode;
/// the cases are exercised end-to-end here through `decode_artifact`.
#[test]
fn manifest_reader_accepts_schema2_and_rejects_unknown_and_unpaired() {
    use super::store::{StoreDocument, VirtualDocumentStore};
    use super::trailer::{self, DecodedArtifact, TrailerV2};

    let route_text = "routes:\n- id: r\n  from: direct:r\n";
    let content = route_text.as_bytes().to_vec();
    let digest = blake3::hash(&content).to_hex().to_string();
    let length = content.len();

    // Schema-1 index (R1 era): document-only shape, no asset classes, no
    // substitution table.
    let v1_index = format!(
        "{{\"config_references\":[],\"entry_point\":\"app.yaml\",\
\"entries\":[{{\"kind\":\"route\",\"length\":{length},\"offset\":0,\"path\":\"app.yaml\"}}],\
\"source_plan\":{{\"references\":[\"app.yaml\"]}},\
\"store_schema\":1}}"
    );
    // Schema-2 manifest (R1 era): the 8 operational fields and 4-field
    // entries only.
    let schema2_manifest = format!(
        "{{\"components\":[],\
\"embedded_files\":[{{\"digest\":\"{digest}\",\"kind\":\"route\",\"length\":{length},\"path\":\"app.yaml\"}}],\
\"env_names\":[],\"kind\":\"route\",\"listeners\":[],\"manifest_schema\":2,\
\"runtime_version\":\"{rt}\",\"source_name\":\"app.yaml\"}}",
        rt = RUNTIME_VERSION
    );
    // Schema-3 manifest: the full paired form for a store-2 index.
    let schema3_manifest = format!(
        "{{\"artifact_kind\":\"server\",\"components\":[],\
\"embedded_files\":[{{\"asset_class\":null,\"class\":\"public\",\"digest\":\"{digest}\",\"kind\":\"route\",\"length\":{length},\"path\":\"app.yaml\"}}],\
\"env_names\":[],\"kind\":\"route\",\"listeners\":[],\"manifest_schema\":3,\
\"runtime_version\":\"{rt}\",\"source_name\":\"app.yaml\",\"total_embedded_bytes\":{length}}}",
        rt = RUNTIME_VERSION
    );

    // A real store-2 store for the paired and unknown-schema cases.
    let store = VirtualDocumentStore::build(
        "app.yaml",
        &[StoreDocument {
            path: "app.yaml".to_string(),
            kind: StoreEntryKind::Route,
            bytes: content.clone(),
        }],
        &[],
        &["app.yaml".to_string()],
    )
    .expect("store builds");
    let store2_index = store.index.encode_canonical().expect("index encodes");

    let decode = |index: &[u8], manifest: &str| {
        trailer::decode_artifact(&trailer::encode_v2(&TrailerV2 {
            kind: TrailerKind::Route,
            content: content.clone(),
            index: index.to_vec(),
            manifest: manifest.as_bytes().to_vec(),
        }))
    };

    // 1. The R1-era pair (store 1 + manifest 2) still decodes; its
    // entries carry null asset classes and implicitly public class.
    let decoded = decode(v1_index.as_bytes(), &schema2_manifest)
        .expect("the R1 pair must decode")
        .expect("terminal magic must mark the trailer present");
    let DecodedArtifact::V2(v2) = decoded else {
        panic!("the R1 pair must decode as DecodedArtifact::V2");
    };
    let parsed = Manifest::from_canonical_json(&v2.manifest).expect("manifest must validate");
    assert_eq!(parsed.embedded_files[0].asset_class, None);
    assert_eq!(parsed.embedded_files[0].class, ManifestClass::Public);
    assert_eq!(parsed.embedded_files[0].path.as_deref(), Some("app.yaml"));

    // 2. Unknown manifest schema (5): the unknown-schema diagnostic runs
    // FIRST — never a pairing error — even against a store-2 index.
    let schema5 = schema3_manifest.replacen("\"manifest_schema\":3", "\"manifest_schema\":5", 1);
    assert!(
        matches!(
            decode(&store2_index, &schema5),
            Err(TrailerError::InvalidManifestSchema(5))
        ),
        "schema 5 must fail with the unknown-schema diagnostic"
    );

    // 3. Schema 4 without the mandatory signing block: named by the
    // schema-4 field rules before any pairing consideration.
    let schema4_unsigned =
        schema3_manifest.replacen("\"manifest_schema\":3", "\"manifest_schema\":4", 1);
    let Err(TrailerError::InvalidManifestFields(reason)) = decode(&store2_index, &schema4_unsigned)
    else {
        panic!("a schema-4 manifest without a signing block must fail closed");
    };
    assert!(
        reason.contains("signing"),
        "the diagnostic must name the missing signing block: {reason}"
    );

    // 4. store 2 + manifest 2: the un-paired legacy pairing fails closed,
    // naming the accepted set (manifest 3 or 4).
    assert!(
        matches!(
            decode(&store2_index, &schema2_manifest),
            Err(TrailerError::InvalidSchemaPairing {
                store_schema: 2,
                manifest_schema: 2,
                accepted_manifest_schemas: [3, 4],
            })
        ),
        "store 2 with manifest 2 must fail the pairing check"
    );

    // 5. The reverse pair, manifest 3 with store 1, fails closed too.
    assert!(
        matches!(
            decode(v1_index.as_bytes(), &schema3_manifest),
            Err(TrailerError::InvalidSchemaPairing {
                store_schema: 1,
                manifest_schema: 3,
                accepted_manifest_schemas: [2],
            })
        ),
        "store 1 with manifest 3 must fail the pairing check"
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
            "manifest_schema": MANIFEST_SCHEMA_V2,
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

        // Unknown kind (asset is known since r2embed Task 1.2 — schema-2
        // manifests mirror the schema-2 store's typed asset entries).
        let mut value = schema2_json();
        value["embedded_files"][0]["path"] = serde_json::json!("routes/a.yaml");
        value["embedded_files"][0]["kind"] = serde_json::json!("asset");
        assert!(
            parse(&value).is_ok(),
            "asset entries mirror store entries since r2embed Task 1.2"
        );
        let mut value = schema2_json();
        value["embedded_files"][0]["kind"] = serde_json::json!("galaxy");
        assert!(matches!(
            parse(&value),
            Err(CompileError::InvalidDocument(reason))
                if reason.contains("unknown kind \"galaxy\"")
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
