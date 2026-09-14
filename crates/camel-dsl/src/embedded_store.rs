//! Canonical virtual-document store model for compiled artifacts.
//!
//! A [`VirtualDocumentStore`] packs every resolved compile-time document
//! (routes, jobs, `Camel.toml`, includes, selected profiles) into one content
//! blob plus a canonical UTF-8 JSON [`StoreIndex`]. Entry paths are
//! normalized UTF-8 relative paths using `/`; entries are stored and indexed
//! in canonical (lexicographic) path order so identical inputs produce
//! identical bytes, while the [`SourcePlan`] preserves the declared
//! route-source order as normative array order.
//!
//! Decode is fail-closed: an unsupported [`STORE_SCHEMA`], malformed path,
//! duplicate path, noncanonical entry order, out-of-bounds or overlapping
//! range, unreferenced content byte, reference to a missing entry, unknown
//! index/entry/plan field, or missing required field is named by
//! [`StoreError`] and never silently reinterpreted.
//!
//! openspec change `multidoc` (Task 1.1). `camel-cli` re-exports this model
//! verbatim; it never defines a second one.

use std::collections::HashSet;
use std::fmt;

use serde::{Deserialize, Serialize};

/// Store index schema understood by this reader and written by the builder.
/// Any other value fails closed on decode.
pub const STORE_SCHEMA: u64 = 1;

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
            _ => None,
        }
    }
}

/// One embedded document in the store: its logical path, kind, and the
/// half-open byte range `[offset, offset + length)` inside the content blob.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoreEntry {
    /// Document kind.
    pub kind: StoreEntryKind,
    /// Byte length of the normalized document inside the content blob.
    pub length: u64,
    /// Byte offset of the document inside the content blob.
    pub offset: u64,
    /// Normalized UTF-8 relative path using `/` separators.
    pub path: String,
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
/// canonical path order, configuration references, and the ordered
/// source plan.
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
    /// Index schema; only [`STORE_SCHEMA`] is understood.
    pub store_schema: u64,
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
    /// Entry kind is not one of the five known document kinds.
    InvalidEntryKind(String),
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
        let raw: RawIndex =
            serde_json::from_str(text).map_err(|e| StoreError::InvalidJson(e.to_string()))?;

        // Schema first: never reinterpret bytes of an unknown schema.
        if raw.store_schema != STORE_SCHEMA {
            return Err(StoreError::UnsupportedStoreSchema(raw.store_schema));
        }

        // Typed entries: known kind, valid path, no duplicates, canonical order.
        let mut paths = HashSet::with_capacity(raw.entries.len());
        let mut entries = Vec::with_capacity(raw.entries.len());
        for entry in &raw.entries {
            let kind = StoreEntryKind::from_name(&entry.kind)
                .ok_or_else(|| StoreError::InvalidEntryKind(entry.kind.clone()))?;
            validate_path(&entry.path)?;
            if !paths.insert(entry.path.clone()) {
                return Err(StoreError::DuplicatePath(entry.path.clone()));
            }
            entries.push(StoreEntry {
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

        Ok(StoreIndex {
            config_references: raw.config_references,
            entry_point: raw.entry_point,
            entries,
            source_plan: SourcePlan {
                references: raw.source_plan.references,
            },
            store_schema: raw.store_schema,
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
/// Unknown fields are rejected in the index, in every entry, and in the
/// source plan: canonical schema-1 bytes carry exactly the declared fields,
/// and a silent extra field would change meaning without a schema bump.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawIndex {
    config_references: Vec<String>,
    entry_point: String,
    entries: Vec<RawEntry>,
    source_plan: RawSourcePlan,
    store_schema: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawEntry {
    kind: String,
    length: u64,
    offset: u64,
    path: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSourcePlan {
    references: Vec<String>,
}

impl VirtualDocumentStore {
    /// Build a canonical store from unordered documents: paths are
    /// validated, content is laid out in canonical (lexicographic) path
    /// order, and every reference (entry point, configuration, source plan)
    /// must name one of the documents.
    pub fn build(
        entry_point: &str,
        documents: &[StoreDocument],
        config_references: &[String],
        source_plan: &[String],
    ) -> Result<Self, StoreError> {
        let mut sorted: Vec<&StoreDocument> = documents.iter().collect();
        sorted.sort_by(|a, b| a.path.cmp(&b.path));

        let mut entries = Vec::with_capacity(sorted.len());
        let mut paths = HashSet::with_capacity(sorted.len());
        let mut content = Vec::new();
        for document in sorted {
            validate_path(&document.path)?;
            if !paths.insert(document.path.clone()) {
                return Err(StoreError::DuplicatePath(document.path.clone()));
            }
            entries.push(StoreEntry {
                kind: document.kind,
                length: document.bytes.len() as u64,
                offset: content.len() as u64,
                path: document.path.clone(),
            });
            content.extend_from_slice(&document.bytes);
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
    /// `entries`/`entry_point` pair and each entry's `kind`/`length`/
    /// `offset`/`path`), arrays in normative order. Documents are passed in
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
            r#"{"kind":"config","length":4,"offset":0,"path":"Camel.toml"},"#,
            r#"{"kind":"include","length":4,"offset":4,"path":"includes/base.yaml"},"#,
            r#"{"kind":"route","length":4,"offset":8,"path":"routes/a.yaml"},"#,
            r#"{"kind":"route","length":3,"offset":12,"path":"routes/b.yaml"}],"#,
            r#""entry_point":"routes/a.yaml","#,
            r#""source_plan":{"references":["routes/a.yaml","routes/b.yaml"]},"#,
            r#""store_schema":1}"#,
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
