//! Canonical virtual-document store model for compiled artifacts.
//!
//! A [`VirtualDocumentStore`] packs every resolved compile-time document
//! (routes, jobs, `Camel.toml`, includes, selected profiles) plus every
//! embedded deploy-time asset into one content blob and a canonical UTF-8
//! JSON [`StoreIndex`]. Entry paths are normalized UTF-8 relative paths
//! using `/`; documents keep their logical paths while assets are keyed
//! under the `assets/` namespace (`assets/<normalized-relative-path>`), so
//! the two namespaces cannot collide. Entries are stored and indexed in
//! canonical (lexicographic) path order so identical inputs produce
//! identical bytes, while the [`SourcePlan`] preserves the declared
//! route-source order as normative array order.
//!
//! Asset entries ([`StoreEntryKind::Asset`]) embed their bytes verbatim:
//! no BOM removal, no newline conversion, no UTF-8 requirement — text
//! normalization applies to document entries only. An asset entry may
//! carry an `asset_class` name (documents always carry none). A schema-2
//! index additionally carries the compile-time `substitutions` table:
//! ordered `(document, declared string) → asset logical path` entries,
//! each recording the declared string's exact byte span(s) inside its
//! site entry and the site context (`literal` or `uri`). The table is
//! absent from schema-1 indexes.
//!
//! Decode is fail-closed: an unsupported store schema (this reader
//! understands schemas 1 and 2), malformed path, duplicate path,
//! noncanonical entry order, out-of-bounds or overlapping range,
//! unreferenced content byte, reference to a missing entry, malformed
//! substitution table (missing document or asset target, span outside
//! its site entry, unknown site context, noncanonical ordering), an
//! asset class on a non-asset entry, unknown index/entry/substitution
//! field, or missing required field is named by [`StoreError`] and never
//! silently reinterpreted.
//!
//! openspec change `multidoc` (Task 1.1) and `r2embed` (Task 1.1).
//! `camel-cli` re-exports this model verbatim; it never defines a second
//! one.

use std::collections::HashSet;
use std::fmt;

use serde::{Deserialize, Serialize};

/// Store index schema written by the builder and understood by this
/// reader. The reader also accepts schema 1 (the legacy document-only
/// shape, with no asset classes and no substitution table); any other
/// value fails closed on decode.
pub const STORE_SCHEMA: u64 = 2;

/// The document kind of one store entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StoreEntryKind {
    /// A route document (`*.yaml`/`*.json`).
    Route,
    /// A job document (`*.job.yaml`).
    Job,
    /// A `Camel.toml` configuration document.
    Config,
    /// An include fragment referenced by `Camel.toml`.
    Include,
    /// A selected profile section of `Camel.toml`.
    Profile,
    /// An embedded deploy-time asset (certificate, stylesheet, schema,
    /// SQL file, static file), keyed under the `assets/` namespace and
    /// embedded verbatim.
    Asset,
}

impl StoreEntryKind {
    /// Canonical lowercase name used in the index and manifest JSON.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Route => "route",
            Self::Job => "job",
            Self::Config => "config",
            Self::Include => "include",
            Self::Profile => "profile",
            Self::Asset => "asset",
        }
    }

    /// Inverse of [`StoreEntryKind::as_str`].
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "route" => Some(Self::Route),
            "job" => Some(Self::Job),
            "config" => Some(Self::Config),
            "include" => Some(Self::Include),
            "profile" => Some(Self::Profile),
            "asset" => Some(Self::Asset),
            _ => None,
        }
    }
}

/// One embedded document or asset in the store: its logical path, kind,
/// and the half-open byte range `[offset, offset + length)` inside the
/// content blob.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoreEntry {
    /// Asset class name (`certificate`, `static file`, …) for asset
    /// entries; always `None` for document entries.
    pub asset_class: Option<String>,
    /// Document or asset kind.
    pub kind: StoreEntryKind,
    /// Byte length of the entry inside the content blob.
    pub length: u64,
    /// Byte offset of the entry inside the content blob.
    pub offset: u64,
    /// Normalized UTF-8 relative path using `/` separators.
    pub path: String,
}

/// Site context of a substitution entry: where the declared string sits
/// in its site entry, which decides how the runtime rewrites it.
///
/// The declaration order is the canonical order: a shared
/// `(document, declared)` pair sorts its literal entry before its uri
/// entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SubstitutionContext {
    /// Ordinary document or configuration text; the site receives the
    /// raw confined path.
    Literal,
    /// Inside a URI; the site receives the percent-encoded confined path.
    Uri,
}

/// Half-open byte span `[start, end)` inside a site entry's bytes,
/// relative to the entry's first byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SubstitutionSpan {
    /// Exclusive end offset, relative to the site entry's first byte.
    pub end: u64,
    /// Inclusive start offset, relative to the site entry's first byte.
    pub start: u64,
}

/// One compile-time substitution site: a declared string's exact byte
/// span(s) inside a site entry, and the asset logical path that replaces
/// them at runtime. Entries are canonically ordered by
/// `(document, declared, context)`: a declared string occurring in both
/// a literal field and a URI of the same document carries two entries,
/// one per context, so the context is never a first-seen-wins guess.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubstitutionEntry {
    /// Asset logical path (`assets/…`) that replaces the declared string.
    pub asset: String,
    /// Site context (literal text or URI value).
    pub context: SubstitutionContext,
    /// The declared string exactly as written in the site entry.
    pub declared: String,
    /// Site entry logical path — a document entry, never an asset entry.
    pub document: String,
    /// Exact byte spans of the declared string, ascending and
    /// non-overlapping, each inside the site entry.
    pub spans: Vec<SubstitutionSpan>,
}

/// Ordered route-source plan. The `references` array order is normative: it
/// preserves the declared pattern order with each pattern's matches sorted
/// by normalized logical path. Every reference must name a store entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourcePlan {
    /// Entry paths in execution order.
    pub references: Vec<String>,
}

/// Canonical store index: schema, logical entry point, typed entries in
/// canonical path order, configuration references, the ordered source
/// plan, and — schema 2 — the ordered substitution table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoreIndex {
    /// Configuration/include/profile entry paths, in resolution order.
    pub config_references: Vec<String>,
    /// Logical entry point; must name exactly one store entry.
    pub entry_point: String,
    /// All embedded entries in canonical (lexicographic) path order.
    pub entries: Vec<StoreEntry>,
    /// Ordered route-source plan.
    pub source_plan: SourcePlan,
    /// Index schema; this reader understands schemas 1 and 2.
    pub store_schema: u64,
    /// Compile-time substitution sites in canonical
    /// `(document, declared, context)` order; empty for schema-1
    /// indexes.
    pub substitutions: Vec<SubstitutionEntry>,
}

/// One input document for [`VirtualDocumentStore::build`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreDocument {
    /// Normalized UTF-8 relative path using `/` separators.
    pub path: String,
    /// Document kind.
    pub kind: StoreEntryKind,
    /// Normalized document bytes.
    pub bytes: Vec<u8>,
}

/// One input asset for [`VirtualDocumentStore::build_with_assets`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreAsset {
    /// Verbatim asset bytes: embedded without BOM removal, newline
    /// conversion, or any UTF-8 requirement.
    pub bytes: Vec<u8>,
    /// Asset class name (`certificate`, `static file`, …); `None` when
    /// the class is not tracked.
    pub class: Option<String>,
    /// Normalized relative path (no `assets/` prefix); the entry is
    /// keyed `assets/<path>` in the store.
    pub path: String,
}

/// A decoded virtual-document store: the content blob plus its validated
/// canonical index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VirtualDocumentStore {
    /// Concatenated document bytes, laid out in canonical path order.
    pub content: Vec<u8>,
    /// Validated index over `content`.
    pub index: StoreIndex,
}

/// Store codec or validation failure. Every variant names the violated
/// format rule; decode never falls back to a different interpretation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreError {
    /// Index bytes are not valid UTF-8.
    InvalidUtf8,
    /// Index bytes are not JSON with the required shape.
    InvalidJson(String),
    /// Index declares a `store_schema` this reader does not support.
    UnsupportedStoreSchema(u64),
    /// Entry kind is not one of the six known entry kinds.
    InvalidEntryKind(String),
    /// A substitution-table entry violates a format rule: unknown site
    /// context, empty declared string, an empty, inverted, out-of-bounds,
    /// overlapping, or non-ascending span list, or noncanonical
    /// `(document, declared)` entry ordering.
    InvalidSubstitution(String),
    /// A non-asset entry declares an asset class.
    UnexpectedAssetClass(String),
    /// Entry path is not a canonical relative `/` path.
    InvalidPath(String),
    /// Two entries declare the same logical path.
    DuplicatePath(String),
    /// Entries are not in lexicographic path order (noncanonical bytes).
    NoncanonicalOrder,
    /// An entry range reaches past the end of the content blob.
    RangeOutOfBounds,
    /// Two entry ranges overlap.
    OverlappingRange,
    /// Content bytes exist outside every declared entry range.
    UnreferencedContent,
    /// The entry point, a configuration reference, or a source-plan
    /// reference names no store entry.
    MissingReference(String),
    /// A typed reference names an existing entry whose document kind does
    /// not satisfy the reference's rule (entry point, configuration, or
    /// source-plan kind agreement).
    KindMismatch {
        /// The reference path.
        path: String,
        /// The required document kind (description).
        expected: &'static str,
        /// The actual document kind name of the targeted entry.
        got: &'static str,
    },
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidUtf8 => write!(f, "store index is not valid UTF-8"),
            Self::InvalidJson(reason) => write!(f, "store index is not valid JSON: {reason}"),
            Self::UnsupportedStoreSchema(schema) => {
                write!(f, "unsupported store schema {schema}")
            }
            Self::InvalidEntryKind(kind) => write!(f, "invalid store entry kind {kind:?}"),
            Self::InvalidSubstitution(reason) => {
                write!(f, "invalid substitution table entry: {reason}")
            }
            Self::UnexpectedAssetClass(path) => write!(
                f,
                "store entry {path:?} declares an asset class but is not an asset entry"
            ),
            Self::InvalidPath(path) => {
                write!(
                    f,
                    "invalid store entry path {path:?}: must be a canonical relative / path \
                     without empty, '.', or '..' components, backslashes, control bytes, or \
                     drive-prefix components"
                )
            }
            Self::DuplicatePath(path) => write!(f, "duplicate store entry path {path:?}"),
            Self::NoncanonicalOrder => {
                write!(f, "store entries are not in canonical path order")
            }
            Self::RangeOutOfBounds => write!(f, "store entry range is out of content bounds"),
            Self::OverlappingRange => write!(f, "store entry ranges overlap"),
            Self::UnreferencedContent => {
                write!(f, "store content contains bytes outside every entry range")
            }
            Self::MissingReference(path) => write!(f, "store reference to missing entry {path:?}"),
            Self::KindMismatch {
                path,
                expected,
                got,
            } => write!(
                f,
                "store reference {path:?} names a {got} entry, expected {expected}"
            ),
        }
    }
}

impl std::error::Error for StoreError {}

/// Validate a canonical logical store path: a relative UTF-8 `/` path with
/// no empty, `.`, or `..` components, no backslash, no control bytes
/// (including NUL), and no drive-prefix component (`C:` shape — absolute or
/// drive-relative). Public so the schema-2 manifest validator (`camel-cli`
/// `compile::manifest`) reuses the exact same canonical-path rule instead
/// of defining a second one.
pub fn validate_path(path: &str) -> Result<(), StoreError> {
    if path.is_empty() {
        return Err(StoreError::InvalidPath(path.to_string()));
    }
    // Backslash separators and raw control bytes (NUL included) never
    // appear in canonical logical paths; their presence means a foreign
    // path form this format must reject, not reinterpret.
    if path.bytes().any(|b| b == b'\\' || b < 0x20 || b == 0x7F) {
        return Err(StoreError::InvalidPath(path.to_string()));
    }
    for component in path.split('/') {
        match component {
            "" | "." | ".." => return Err(StoreError::InvalidPath(path.to_string())),
            _ if is_drive_prefix(component) => {
                return Err(StoreError::InvalidPath(path.to_string()));
            }
            _ => {}
        }
    }
    Ok(())
}

/// A component shaped like a Windows drive prefix (`C:`, `c:rest`): an
/// absolute or drive-relative form that is never a canonical relative
/// store path.
fn is_drive_prefix(component: &str) -> bool {
    let bytes = component.as_bytes();
    bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic()
}

/// Rewrite `value` with every object's keys inserted in lexicographic
/// order, recursing through arrays and nested objects. Inserting in sorted
/// order is canonical under both `serde_json` map representations: the
/// default `BTreeMap` (order-insensitive) and `preserve_order`'s
/// insertion-ordered map.
fn canonicalize_value(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let mut pairs: Vec<(String, serde_json::Value)> = map.into_iter().collect();
            pairs.sort_unstable_by(|a, b| a.0.cmp(&b.0));
            let mut canonical = serde_json::Map::with_capacity(pairs.len());
            for (key, value) in pairs {
                canonical.insert(key, canonicalize_value(value));
            }
            serde_json::Value::Object(canonical)
        }
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.into_iter().map(canonicalize_value).collect())
        }
        other => other,
    }
}

impl StoreIndex {
    /// Encode to canonical UTF-8 JSON: compact, lexicographically ordered
    /// object keys at every depth, arrays in normative order.
    pub fn encode_canonical(&self) -> Result<Vec<u8>, StoreError> {
        // Canonicalization is explicit and recursive: object keys are
        // re-inserted in lexicographic order at every depth. The bytes
        // therefore never depend on serde_json's `preserve_order` feature
        // (which makes `Value` insertion-ordered) or on struct field
        // declaration order.
        let value =
            serde_json::to_value(self).map_err(|e| StoreError::InvalidJson(e.to_string()))?;
        serde_json::to_vec(&canonicalize_value(value))
            .map_err(|e| StoreError::InvalidJson(e.to_string()))
    }

    /// Decode and validate an index against a content blob of
    /// `content_len` bytes. See the module docs for the fail-closed rules.
    pub fn decode(bytes: &[u8], content_len: usize) -> Result<StoreIndex, StoreError> {
        let text = std::str::from_utf8(bytes).map_err(|_| StoreError::InvalidUtf8)?;

        // Schema first: probe only the schema field, pick the exact index
        // shape for the declared schema, and never shape-parse (let alone
        // reinterpret) bytes of an unknown schema.
        let probe: serde_json::Value =
            serde_json::from_str(text).map_err(|e| StoreError::InvalidJson(e.to_string()))?;
        let schema = probe
            .get("store_schema")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| {
                StoreError::InvalidJson("missing or non-integer store_schema".to_string())
            })?;
        if !matches!(schema, 1 | 2) {
            return Err(StoreError::UnsupportedStoreSchema(schema));
        }

        // Shape-match the declared schema exactly: schema-1 bytes carry no
        // asset classes and no substitution table, and their presence in a
        // schema-1 index is rejected as an unknown field.
        let raw = if schema == 1 {
            let raw: RawIndexV1 =
                serde_json::from_str(text).map_err(|e| StoreError::InvalidJson(e.to_string()))?;
            ParsedIndex {
                config_references: raw.config_references,
                entry_point: raw.entry_point,
                entries: raw
                    .entries
                    .into_iter()
                    .map(|entry| ParsedEntry {
                        asset_class: None,
                        kind: entry.kind,
                        length: entry.length,
                        offset: entry.offset,
                        path: entry.path,
                    })
                    .collect(),
                source_plan: raw.source_plan,
                store_schema: raw.store_schema,
                substitutions: Vec::new(),
            }
        } else {
            let raw: RawIndexV2 =
                serde_json::from_str(text).map_err(|e| StoreError::InvalidJson(e.to_string()))?;
            ParsedIndex {
                config_references: raw.config_references,
                entry_point: raw.entry_point,
                entries: raw
                    .entries
                    .into_iter()
                    .map(|entry| ParsedEntry {
                        asset_class: entry.asset_class,
                        kind: entry.kind,
                        length: entry.length,
                        offset: entry.offset,
                        path: entry.path,
                    })
                    .collect(),
                source_plan: raw.source_plan,
                store_schema: raw.store_schema,
                substitutions: raw.substitutions,
            }
        };

        // Typed entries: known kind, valid path, no duplicates, canonical
        // order, and asset classes only on asset entries.
        let mut paths = HashSet::with_capacity(raw.entries.len());
        let mut entries = Vec::with_capacity(raw.entries.len());
        for entry in &raw.entries {
            let kind = StoreEntryKind::from_name(&entry.kind)
                .ok_or_else(|| StoreError::InvalidEntryKind(entry.kind.clone()))?;
            // Schema 1 is the legacy document-only shape: it has no asset
            // classes and no substitution table, so an asset entry there
            // fails closed rather than decoding with a forced `None` class.
            if raw.store_schema == 1 && kind == StoreEntryKind::Asset {
                return Err(StoreError::InvalidEntryKind(entry.kind.clone()));
            }
            validate_path(&entry.path)?;
            if kind != StoreEntryKind::Asset && entry.asset_class.is_some() {
                return Err(StoreError::UnexpectedAssetClass(entry.path.clone()));
            }
            if !paths.insert(entry.path.clone()) {
                return Err(StoreError::DuplicatePath(entry.path.clone()));
            }
            entries.push(StoreEntry {
                asset_class: entry.asset_class.clone(),
                kind,
                length: entry.length,
                offset: entry.offset,
                path: entry.path.clone(),
            });
        }
        for window in entries.windows(2) {
            if window[0].path >= window[1].path {
                return Err(StoreError::NoncanonicalOrder);
            }
        }

        // Ranges: in-bounds, non-overlapping, and exactly covering the
        // content blob in canonical path order.
        let mut cursor = 0u64;
        for entry in &entries {
            if entry.offset < cursor {
                return Err(StoreError::OverlappingRange);
            }
            if entry.offset > cursor {
                return Err(StoreError::UnreferencedContent);
            }
            let end = entry
                .offset
                .checked_add(entry.length)
                .ok_or(StoreError::RangeOutOfBounds)?;
            if end > content_len as u64 {
                return Err(StoreError::RangeOutOfBounds);
            }
            cursor = end;
        }
        if cursor != content_len as u64 {
            return Err(StoreError::UnreferencedContent);
        }

        // References: entry point, configuration, and source plan must all
        // name existing entries.
        if !paths.contains(&raw.entry_point) {
            return Err(StoreError::MissingReference(raw.entry_point.clone()));
        }
        for path in &raw.config_references {
            if !paths.contains(path) {
                return Err(StoreError::MissingReference(path.clone()));
            }
        }
        for path in &raw.source_plan.references {
            if !paths.contains(path) {
                return Err(StoreError::MissingReference(path.clone()));
            }
        }

        // Substitution table (schema 2 only): typed contexts plus the full
        // validated-entry rules.
        let mut substitutions = Vec::with_capacity(raw.substitutions.len());
        for sub in &raw.substitutions {
            let context = match sub.context.as_str() {
                "literal" => SubstitutionContext::Literal,
                "uri" => SubstitutionContext::Uri,
                other => {
                    return Err(StoreError::InvalidSubstitution(format!(
                        "unknown site context {other:?}"
                    )));
                }
            };
            substitutions.push(SubstitutionEntry {
                asset: sub.asset.clone(),
                context,
                declared: sub.declared.clone(),
                document: sub.document.clone(),
                spans: sub
                    .spans
                    .iter()
                    .map(|span| SubstitutionSpan {
                        end: span.end,
                        start: span.start,
                    })
                    .collect(),
            });
        }
        validate_substitutions(&substitutions, &entries)?;

        Ok(StoreIndex {
            config_references: raw.config_references,
            entry_point: raw.entry_point,
            entries,
            source_plan: SourcePlan {
                references: raw.source_plan.references,
            },
            store_schema: raw.store_schema,
            substitutions,
        })
    }

    /// Entry for a logical path, if the index names one. Shared lookup
    /// seam for the discovery pass (openspec change `multidoc`,
    /// Task 2.1): route resolution and configuration assembly both
    /// resolve references through this single walk.
    pub fn entry(&self, path: &str) -> Option<&StoreEntry> {
        self.entries.iter().find(|entry| entry.path == path)
    }
}

/// Shape-checked raw index used during decode before semantic validation.
/// Unknown fields are rejected in the index, in every entry, in the
/// substitution table, and in the source plan: canonical bytes carry
/// exactly the declared fields, and a silent extra field would change
/// meaning without a schema bump.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawIndexV1 {
    config_references: Vec<String>,
    entry_point: String,
    entries: Vec<RawEntryV1>,
    source_plan: RawSourcePlan,
    store_schema: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawEntryV1 {
    kind: String,
    length: u64,
    offset: u64,
    path: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawIndexV2 {
    config_references: Vec<String>,
    entry_point: String,
    entries: Vec<RawEntryV2>,
    source_plan: RawSourcePlan,
    store_schema: u64,
    substitutions: Vec<RawSubstitution>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawEntryV2 {
    asset_class: Option<String>,
    kind: String,
    length: u64,
    offset: u64,
    path: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSubstitution {
    asset: String,
    context: String,
    declared: String,
    document: String,
    spans: Vec<RawSpan>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSpan {
    end: u64,
    start: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSourcePlan {
    references: Vec<String>,
}

/// Schema-independent raw index shape shared by the schema-1 and schema-2
/// parse paths once the declared shape has been matched exactly.
struct ParsedIndex {
    config_references: Vec<String>,
    entry_point: String,
    entries: Vec<ParsedEntry>,
    source_plan: RawSourcePlan,
    store_schema: u64,
    substitutions: Vec<RawSubstitution>,
}

struct ParsedEntry {
    asset_class: Option<String>,
    kind: String,
    length: u64,
    offset: u64,
    path: String,
}

/// Validate the substitution table against the typed entries: canonical
/// `(document, declared, context)` ordering, then the per-entry rules
/// (existing document and asset targets with the right kinds, non-empty
/// declared string, ascending non-overlapping spans inside the site
/// entry).
fn validate_substitutions(
    substitutions: &[SubstitutionEntry],
    entries: &[StoreEntry],
) -> Result<(), StoreError> {
    for window in substitutions.windows(2) {
        let a = (&window[0].document, &window[0].declared, window[0].context);
        let b = (&window[1].document, &window[1].declared, window[1].context);
        if a >= b {
            return Err(StoreError::InvalidSubstitution(
                "entries are not in canonical (document, declared, context) order".to_string(),
            ));
        }
    }
    for substitution in substitutions {
        if substitution.declared.is_empty() {
            return Err(StoreError::InvalidSubstitution(
                "declared string is empty".to_string(),
            ));
        }
        if substitution.spans.is_empty() {
            return Err(StoreError::InvalidSubstitution(format!(
                "no byte spans recorded for declared string {:?} in {:?}",
                substitution.declared, substitution.document
            )));
        }
        let site = entries
            .iter()
            .find(|entry| entry.path == substitution.document)
            .ok_or_else(|| StoreError::MissingReference(substitution.document.clone()))?;
        if site.kind == StoreEntryKind::Asset {
            return Err(StoreError::KindMismatch {
                path: substitution.document.clone(),
                expected: "document",
                got: site.kind.as_str(),
            });
        }
        let asset = entries
            .iter()
            .find(|entry| entry.path == substitution.asset);
        match asset {
            None => return Err(StoreError::MissingReference(substitution.asset.clone())),
            Some(asset) if asset.kind != StoreEntryKind::Asset => {
                return Err(StoreError::KindMismatch {
                    path: substitution.asset.clone(),
                    expected: "asset",
                    got: asset.kind.as_str(),
                });
            }
            Some(_) => {}
        }
        let mut previous_end = 0u64;
        for span in &substitution.spans {
            if span.start >= span.end {
                return Err(StoreError::InvalidSubstitution(format!(
                    "span {}..{} of {:?} in {:?} is empty or inverted",
                    span.start, span.end, substitution.declared, substitution.document
                )));
            }
            if span.start < previous_end {
                return Err(StoreError::InvalidSubstitution(format!(
                    "spans of {:?} in {:?} overlap or are not ascending",
                    substitution.declared, substitution.document
                )));
            }
            if span.end > site.length {
                return Err(StoreError::InvalidSubstitution(format!(
                    "span {}..{} lies outside site entry {:?}",
                    span.start, span.end, substitution.document
                )));
            }
            previous_end = span.end;
        }
    }
    Ok(())
}

impl VirtualDocumentStore {
    /// Build a canonical store from unordered documents: paths are
    /// validated, content is laid out in canonical (lexicographic) path
    /// order, and every reference (entry point, configuration, source plan)
    /// must name one of the documents. Document-only form of
    /// [`VirtualDocumentStore::build_with_assets`].
    pub fn build(
        entry_point: &str,
        documents: &[StoreDocument],
        config_references: &[String],
        source_plan: &[String],
    ) -> Result<Self, StoreError> {
        Self::build_with_assets(
            entry_point,
            documents,
            &[],
            config_references,
            source_plan,
            &[],
        )
    }

    /// Build a canonical store from unordered documents and verbatim
    /// assets. Documents keep their logical paths; assets are keyed under
    /// the `assets/` namespace (`assets/<normalized-relative-path>`) and
    /// embed their bytes without any normalization. All entries —
    /// documents and assets interleaved — are laid out in canonical
    /// (lexicographic) path order, duplicate logical paths are rejected,
    /// and asset bytes join the aggregate content accounting. The
    /// substitution table is ordered canonically by
    /// `(document, declared, context)` and validated against the built
    /// entries.
    pub fn build_with_assets(
        entry_point: &str,
        documents: &[StoreDocument],
        assets: &[StoreAsset],
        config_references: &[String],
        source_plan: &[String],
        substitutions: &[SubstitutionEntry],
    ) -> Result<Self, StoreError> {
        // Combined inputs in canonical path order: documents keep their
        // logical path, assets gain the `assets/` prefix.
        let mut items: Vec<(String, StoreEntryKind, Option<String>, &[u8])> = documents
            .iter()
            .map(|document| {
                (
                    document.path.clone(),
                    document.kind,
                    None,
                    document.bytes.as_slice(),
                )
            })
            .collect();
        items.extend(assets.iter().map(|asset| {
            (
                format!("assets/{}", asset.path),
                StoreEntryKind::Asset,
                asset.class.clone(),
                asset.bytes.as_slice(),
            )
        }));
        items.sort_by(|a, b| a.0.cmp(&b.0));

        let mut entries = Vec::with_capacity(items.len());
        let mut paths = HashSet::with_capacity(items.len());
        let mut content = Vec::new();
        for (path, kind, asset_class, bytes) in items {
            validate_path(&path)?;
            if !paths.insert(path.clone()) {
                return Err(StoreError::DuplicatePath(path));
            }
            entries.push(StoreEntry {
                asset_class,
                kind,
                length: bytes.len() as u64,
                offset: content.len() as u64,
                path,
            });
            content.extend_from_slice(bytes);
        }

        if !paths.contains(entry_point) {
            return Err(StoreError::MissingReference(entry_point.to_string()));
        }
        for path in config_references {
            if !paths.contains(path) {
                return Err(StoreError::MissingReference(path.clone()));
            }
        }
        for path in source_plan {
            if !paths.contains(path) {
                return Err(StoreError::MissingReference(path.clone()));
            }
        }

        let mut substitutions = substitutions.to_vec();
        substitutions.sort_by(|a, b| {
            (&a.document, &a.declared, a.context).cmp(&(&b.document, &b.declared, b.context))
        });
        validate_substitutions(&substitutions, &entries)?;

        Ok(Self {
            content,
            index: StoreIndex {
                config_references: config_references.to_vec(),
                entry_point: entry_point.to_string(),
                entries,
                source_plan: SourcePlan {
                    references: source_plan.to_vec(),
                },
                store_schema: STORE_SCHEMA,
                substitutions,
            },
        })
    }

    /// Decode a store from its content blob and canonical index bytes,
    /// validating the index against the content length.
    pub fn decode(content: Vec<u8>, index_bytes: &[u8]) -> Result<Self, StoreError> {
        let index = StoreIndex::decode(index_bytes, content.len())?;
        Ok(Self { content, index })
    }

    /// Document bytes for a logical path, if the index names one.
    pub fn read(&self, path: &str) -> Option<&[u8]> {
        let entry = self.index.entry(path)?;
        self.content
            .get(entry.offset as usize..entry.offset as usize + entry.length as usize)
    }

    /// Document text for a logical path: the indexed bytes when they
    /// are present AND valid UTF-8, `None` otherwise. A `None` for an
    /// indexed path names non-UTF-8 bytes — normalized compile-time
    /// documents are always UTF-8, so this is a fail-closed signal for
    /// the discovery pass, not a tolerated state.
    pub fn read_text(&self, path: &str) -> Option<&str> {
        std::str::from_utf8(self.read(path)?).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The canonical index encoding is pinned byte-for-byte: compact
    /// separators, object keys lexicographic at every depth (including the
    /// `entries`/`entry_point` pair, each entry's `asset_class`/`kind`/
    /// `length`/`offset`/`path`, and the `store_schema`/`substitutions`
    /// pair), arrays in normative order. Documents are passed in
    /// non-sorted order to prove canonical layout. The compiler pins the
    /// same string against its re-exported types
    /// (`camel-cli` `compile::store::canonical_index_json_matches_embedded_store_bytes`).
    #[test]
    fn canonical_index_json_is_byte_pinned_lexicographic() {
        let documents = [
            StoreDocument {
                path: "routes/b.yaml".to_string(),
                kind: StoreEntryKind::Route,
                bytes: b"bbb".to_vec(),
            },
            StoreDocument {
                path: "Camel.toml".to_string(),
                kind: StoreEntryKind::Config,
                bytes: b"aaaa".to_vec(),
            },
            StoreDocument {
                path: "includes/base.yaml".to_string(),
                kind: StoreEntryKind::Include,
                bytes: b"cccc".to_vec(),
            },
            StoreDocument {
                path: "routes/a.yaml".to_string(),
                kind: StoreEntryKind::Route,
                bytes: b"dddd".to_vec(),
            },
        ];
        let config_references = vec!["Camel.toml".to_string(), "includes/base.yaml".to_string()];
        let source_plan = vec!["routes/a.yaml".to_string(), "routes/b.yaml".to_string()];
        let store = VirtualDocumentStore::build(
            "routes/a.yaml",
            &documents,
            &config_references,
            &source_plan,
        )
        .expect("store builds");

        let bytes = store.index.encode_canonical().expect("index encodes");
        let expected = concat!(
            r#"{"config_references":["Camel.toml","includes/base.yaml"],"#,
            r#""entries":["#,
            r#"{"asset_class":null,"kind":"config","length":4,"offset":0,"path":"Camel.toml"},"#,
            r#"{"asset_class":null,"kind":"include","length":4,"offset":4,"path":"includes/base.yaml"},"#,
            r#"{"asset_class":null,"kind":"route","length":4,"offset":8,"path":"routes/a.yaml"},"#,
            r#"{"asset_class":null,"kind":"route","length":3,"offset":12,"path":"routes/b.yaml"}],"#,
            r#""entry_point":"routes/a.yaml","#,
            r#""source_plan":{"references":["routes/a.yaml","routes/b.yaml"]},"#,
            r#""store_schema":2,"#,
            r#""substitutions":[]}"#,
        );
        assert_eq!(std::str::from_utf8(&bytes).expect("utf-8"), expected);

        // The pinned bytes decode back to the identical index.
        assert_eq!(
            StoreIndex::decode(&bytes, store.content.len()).expect("decodes"),
            store.index
        );
    }

    /// The canonical-path rule rejects noncanonical forms — backslash
    /// separators, control bytes (NUL included), drive-prefix components,
    /// and the traversal/absolute/empty-component shapes — while accepting
    /// UTF-8 relative slash paths.
    #[test]
    fn validate_path_rejects_noncanonical_forms() {
        for bad in [
            "",                 // empty
            ".",                // dot component
            "..",               // dot-dot component
            "a/./b.yaml",       // interior dot component
            "a/../b.yaml",      // interior traversal
            "/abs/a.yaml",      // absolute (empty first component)
            "trailing/",        // empty last component
            "a//b.yaml",        // empty interior component
            "routes\\a.yaml",   // backslash separator
            "rou\\te/a.yaml",   // backslash anywhere
            "a\0b.yaml",        // NUL byte
            "a\nb.yaml",        // other control byte
            "\u{7f}.yaml",      // DEL byte
            "C:/routes/a.yaml", // drive prefix
            "C:routes/a.yaml",  // drive-relative
            "x:/a.yaml",        // lowercase drive letter
        ] {
            assert!(
                matches!(validate_path(bad), Err(StoreError::InvalidPath(_))),
                "{bad:?} must be rejected as noncanonical"
            );
        }

        for good in [
            "a.yaml",
            "Camel.toml",
            "routes/a.yaml",
            "a/b/c/d.yaml",
            "routes/café.yaml",
        ] {
            assert_eq!(validate_path(good), Ok(()), "{good:?} must be accepted");
        }
    }

    /// Canonical baseline JSON for one route entry over a 4-byte blob.
    fn baseline(entries: &str, entry_point: &str, plan: &str) -> String {
        format!(
            "{{\"config_references\":[],\"entry_point\":\"{entry_point}\",\
             \"entries\":[{entries}],\
             \"source_plan\":{{\"references\":[{plan}]}},\
             \"store_schema\":1}}"
        )
    }

    const ONE_ENTRY: &str = r#"{"kind":"route","length":4,"offset":0,"path":"a.yaml"}"#;

    /// All byte spans of `needle` inside `haystack`, ascending and
    /// non-overlapping — the substitution-table span shape.
    fn spans_of(haystack: &[u8], needle: &str) -> Vec<SubstitutionSpan> {
        let mut spans = Vec::new();
        let mut cursor = 0usize;
        while let Some(found) = haystack[cursor..]
            .windows(needle.len())
            .position(|window| window == needle.as_bytes())
        {
            let start = cursor + found;
            spans.push(SubstitutionSpan {
                end: (start + needle.len()) as u64,
                start: start as u64,
            });
            cursor = start + needle.len();
        }
        spans
    }

    /// Schema-2 index JSON over the given entries, substitutions, and
    /// schema value. Mirrors the canonical key order.
    fn schema2_baseline(
        entries: &str,
        entry_point: &str,
        plan: &str,
        substitutions: &str,
        schema: u64,
    ) -> String {
        format!(
            "{{\"config_references\":[],\"entry_point\":\"{entry_point}\",\
             \"entries\":[{entries}],\
             \"source_plan\":{{\"references\":[{plan}]}},\
             \"store_schema\":{schema},\
             \"substitutions\":{substitutions}}}"
        )
    }

    /// Schema-2 codec round trip (r2embed Task 1.1): one document entry
    /// and two asset entries (classes `certificate` and `static file`)
    /// survive `encode_canonical` → `decode` with identical paths, kinds,
    /// classes, substitution entries, ranges, and ordering, and the
    /// decoded schema is 2.
    #[test]
    fn store_schema2_codec_round_trips_asset_entries() {
        let document = b"routes:\n- from: direct:tls/ca.pem\n  to: log:tls/ca.pem\n".to_vec();
        let documents = [StoreDocument {
            path: "routes/tls.yaml".to_string(),
            kind: StoreEntryKind::Route,
            bytes: document.clone(),
        }];
        let assets = [
            StoreAsset {
                path: "tls/ca.pem".to_string(),
                class: Some("certificate".to_string()),
                bytes: b"-----BEGIN CERTIFICATE-----\nCA\n".to_vec(),
            },
            StoreAsset {
                path: "static/logo.bin".to_string(),
                class: Some("static file".to_string()),
                bytes: vec![0x00, 0xFF, 0x0D, 0x0A],
            },
        ];
        let declared = "tls/ca.pem";
        let substitutions = [SubstitutionEntry {
            asset: "assets/tls/ca.pem".to_string(),
            context: SubstitutionContext::Literal,
            declared: declared.to_string(),
            document: "routes/tls.yaml".to_string(),
            spans: spans_of(&document, declared),
        }];
        assert_eq!(substitutions[0].spans.len(), 2, "two declared sites");

        let store = VirtualDocumentStore::build_with_assets(
            "routes/tls.yaml",
            &documents,
            &assets,
            &[],
            &["routes/tls.yaml".to_string()],
            &substitutions,
        )
        .expect("schema-2 store builds");

        // Assets join the canonical path ordering ahead of the document;
        // classes ride on the entries and documents carry none.
        let paths: Vec<&str> = store
            .index
            .entries
            .iter()
            .map(|e| e.path.as_str())
            .collect();
        assert_eq!(
            paths,
            [
                "assets/static/logo.bin",
                "assets/tls/ca.pem",
                "routes/tls.yaml"
            ]
        );
        let kinds: Vec<StoreEntryKind> = store.index.entries.iter().map(|e| e.kind).collect();
        assert_eq!(
            kinds,
            [
                StoreEntryKind::Asset,
                StoreEntryKind::Asset,
                StoreEntryKind::Route
            ]
        );
        let classes: Vec<Option<&str>> = store
            .index
            .entries
            .iter()
            .map(|e| e.asset_class.as_deref())
            .collect();
        assert_eq!(classes, [Some("static file"), Some("certificate"), None]);

        assert_eq!(store.index.store_schema, 2);

        let bytes = store.index.encode_canonical().expect("index encodes");
        let decoded = StoreIndex::decode(&bytes, store.content.len()).expect("index decodes");
        assert_eq!(decoded, store.index, "round trip is identity");
        assert_eq!(decoded.substitutions, substitutions);
    }

    /// Two entries may share one `(document, declared)` pair when their
    /// site contexts differ: canonical substitution order is
    /// `(document, declared, context)` with literal before uri
    /// (r2embed review F1), and a mixed literal/uri declaration never
    /// collapses into a first-seen-wins single entry.
    #[test]
    fn substitution_table_orders_by_document_declared_and_context() {
        let document = b"steps:\n- to: 'xslt:t.xslt'\nxslt: t.xslt\n".to_vec();
        let documents = [StoreDocument {
            path: "app.yaml".to_string(),
            kind: StoreEntryKind::Route,
            bytes: document.clone(),
        }];
        let assets = [StoreAsset {
            path: "t.xslt".to_string(),
            class: Some("xslt stylesheet".to_string()),
            bytes: b"x".to_vec(),
        }];
        let spans = spans_of(&document, "t.xslt");
        assert_eq!(spans.len(), 2, "one uri site and one literal site");
        // Deliberately non-canonical input order: build canonicalizes.
        let substitutions = [
            SubstitutionEntry {
                asset: "assets/t.xslt".to_string(),
                context: SubstitutionContext::Uri,
                declared: "t.xslt".to_string(),
                document: "app.yaml".to_string(),
                spans: vec![spans[0]],
            },
            SubstitutionEntry {
                asset: "assets/t.xslt".to_string(),
                context: SubstitutionContext::Literal,
                declared: "t.xslt".to_string(),
                document: "app.yaml".to_string(),
                spans: vec![spans[1]],
            },
        ];
        let store = VirtualDocumentStore::build_with_assets(
            "app.yaml",
            &documents,
            &assets,
            &[],
            &["app.yaml".to_string()],
            &substitutions,
        )
        .expect("distinct contexts of one declared string build");
        assert_eq!(store.index.substitutions.len(), 2);
        assert_eq!(
            store.index.substitutions[0].context,
            SubstitutionContext::Literal
        );
        assert_eq!(
            store.index.substitutions[1].context,
            SubstitutionContext::Uri
        );
    }

    /// Unknown schemas and unknown entry kinds fail closed with named
    /// errors and are never reinterpreted (r2embed Task 1.1).
    #[test]
    fn store_decoder_rejects_unknown_schema_and_kind() {
        // Schema 3: well-formed otherwise — rejected before any
        // reinterpretation, with the named schema error.
        let entry = r#"{"asset_class":null,"kind":"route","length":4,"offset":0,"path":"a.yaml"}"#;
        let json = schema2_baseline(entry, "a.yaml", r#""a.yaml""#, "[]", 3);
        assert_eq!(
            StoreIndex::decode(json.as_bytes(), 4),
            Err(StoreError::UnsupportedStoreSchema(3))
        );

        // Unknown entry kind in a schema-2 index: named kind error.
        let entry = r#"{"asset_class":null,"kind":"widget","length":4,"offset":0,"path":"a.yaml"}"#;
        let json = schema2_baseline(entry, "a.yaml", r#""a.yaml""#, "[]", 2);
        assert_eq!(
            StoreIndex::decode(json.as_bytes(), 4),
            Err(StoreError::InvalidEntryKind("widget".to_string()))
        );

        // Asset kind in a schema-1 index: schema 1 is the legacy
        // document-only shape, so an asset entry fails closed with the
        // named kind error rather than decoding with a forced `None` class.
        let entry = r#"{"kind":"asset","length":4,"offset":0,"path":"a.yaml"}"#;
        let json = baseline(entry, "a.yaml", r#""a.yaml""#);
        assert_eq!(
            StoreIndex::decode(json.as_bytes(), 4),
            Err(StoreError::InvalidEntryKind("asset".to_string()))
        );
    }

    /// Substitution-table entries referencing a missing document or a
    /// missing asset entry fail decode with the named missing-reference
    /// error (r2embed Task 1.1).
    #[test]
    fn store_decoder_rejects_dangling_substitution_entries() {
        let entries = concat!(
            r#"{"asset_class":null,"kind":"route","length":6,"offset":0,"path":"a.yaml"},"#,
            r#"{"asset_class":"certificate","kind":"asset","length":4,"offset":6,"path":"assets/ca.pem"}"#,
        );
        // Missing document site.
        let json = schema2_baseline(
            entries,
            "a.yaml",
            r#""a.yaml""#,
            r#"[{"asset":"assets/ca.pem","context":"literal","declared":"ca.pem","document":"routes/missing.yaml","spans":[{"end":6,"start":0}]}]"#,
            2,
        );
        assert_eq!(
            StoreIndex::decode(json.as_bytes(), 10),
            Err(StoreError::MissingReference(
                "routes/missing.yaml".to_string()
            ))
        );

        // Missing asset target.
        let json = schema2_baseline(
            entries,
            "a.yaml",
            r#""a.yaml""#,
            r#"[{"asset":"assets/missing.pem","context":"literal","declared":"ca.pem","document":"a.yaml","spans":[{"end":6,"start":0}]}]"#,
            2,
        );
        assert_eq!(
            StoreIndex::decode(json.as_bytes(), 10),
            Err(StoreError::MissingReference(
                "assets/missing.pem".to_string()
            ))
        );

        // Control: the same shape with a complete table decodes.
        let json = schema2_baseline(
            entries,
            "a.yaml",
            r#""a.yaml""#,
            r#"[{"asset":"assets/ca.pem","context":"literal","declared":"ca.pem","document":"a.yaml","spans":[{"end":6,"start":0}]}]"#,
            2,
        );
        assert!(
            StoreIndex::decode(json.as_bytes(), 10).is_ok(),
            "complete table must decode"
        );
    }

    /// Unknown fields are rejected at every level of the index: top level,
    /// entry objects, and the source plan. The canonical encoder never
    /// writes them, so their presence means noncanonical or future bytes
    /// this reader must not reinterpret.
    #[test]
    fn store_decoder_rejects_unknown_fields_and_missing_references_field() {
        // Unknown top-level field.
        let json = format!(
            "{{\"compression\":null,\"config_references\":[],\"entry_point\":\"a.yaml\",\
             \"entries\":[{ONE_ENTRY}],\
             \"source_plan\":{{\"references\":[\"a.yaml\"]}},\"store_schema\":1}}"
        );
        assert!(matches!(
            StoreIndex::decode(json.as_bytes(), 4),
            Err(StoreError::InvalidJson(_))
        ));

        // Unknown field inside an entry object.
        let json = baseline(
            r#"{"digest":"00","kind":"route","length":4,"offset":0,"path":"a.yaml"}"#,
            "a.yaml",
            r#""a.yaml""#,
        );
        assert!(matches!(
            StoreIndex::decode(json.as_bytes(), 4),
            Err(StoreError::InvalidJson(_))
        ));

        // Unknown field inside the source plan.
        let json = format!(
            "{{\"config_references\":[],\"entry_point\":\"a.yaml\",\
             \"entries\":[{ONE_ENTRY}],\
             \"source_plan\":{{\"declared_order\":true,\"references\":[\"a.yaml\"]}},\
             \"store_schema\":1}}"
        );
        assert!(matches!(
            StoreIndex::decode(json.as_bytes(), 4),
            Err(StoreError::InvalidJson(_))
        ));

        // Missing `config_references`: canonical bytes always carry the
        // field, so its absence is malformed, not an empty list.
        let json = format!(
            "{{\"entry_point\":\"a.yaml\",\"entries\":[{ONE_ENTRY}],\
             \"source_plan\":{{\"references\":[\"a.yaml\"]}},\"store_schema\":1}}"
        );
        assert!(matches!(
            StoreIndex::decode(json.as_bytes(), 4),
            Err(StoreError::InvalidJson(_))
        ));

        // Wrong field types stay named errors too (length as string).
        let json = baseline(
            r#"{"kind":"route","length":"4","offset":0,"path":"a.yaml"}"#,
            "a.yaml",
            r#""a.yaml""#,
        );
        assert!(matches!(
            StoreIndex::decode(json.as_bytes(), 4),
            Err(StoreError::InvalidJson(_))
        ));
    }
}
