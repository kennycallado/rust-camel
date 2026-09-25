//! Virtual-document store bridge for compiled artifacts (openspec change
//! `multidoc`).
//!
//! [`camel_dsl::embedded_store`] owns the canonical model —
//! [`VirtualDocumentStore`], [`StoreIndex`], [`StoreEntry`],
//! [`StoreEntryKind`], [`SourcePlan`] — and its fail-closed codec. This
//! module re-exports it for the compile pipeline and adds the
//! `TrailerKind`/`StoreEntryKind` conversions; no second model is defined
//! here.

use super::trailer::TrailerKind;

pub use camel_dsl::embedded_store::{
    STORE_SCHEMA, SourcePlan, StoreAsset, StoreDocument, StoreEntry, StoreEntryKind, StoreError,
    StoreIndex, SubstitutionContext, SubstitutionEntry, SubstitutionSpan, VirtualDocumentStore,
    validate_path,
};

/// Artifact kind (`1=route`, `2=job`) becomes the store document kind.
impl From<TrailerKind> for StoreEntryKind {
    fn from(kind: TrailerKind) -> Self {
        match kind {
            TrailerKind::Route => Self::Route,
            TrailerKind::Job => Self::Job,
        }
    }
}

/// Inverse of `From<TrailerKind> for StoreEntryKind`; the configuration
/// kinds have no artifact kind.
impl TryFrom<StoreEntryKind> for TrailerKind {
    type Error = ();

    fn try_from(kind: StoreEntryKind) -> Result<Self, Self::Error> {
        match kind {
            StoreEntryKind::Route => Ok(Self::Route),
            StoreEntryKind::Job => Ok(Self::Job),
            _ => Err(()),
        }
    }
}

/// Enforce the typed reference invariants of a store artifact against its
/// trailer kind: the entry point names a `route`/`job` entry of the
/// artifact's own kind, every configuration reference names a `config`,
/// `include`, or `profile` entry, and every source-plan reference names a
/// `route` or `job` entry. `StoreIndex::decode` already guarantees that
/// every reference names an existing entry; only the kind agreements are
/// checked here.
pub(crate) fn validate_typed_references(
    index: &StoreIndex,
    kind: TrailerKind,
) -> Result<(), StoreError> {
    let kind_of = |path: &str| -> Result<StoreEntryKind, StoreError> {
        index
            .entries
            .iter()
            .find(|entry| entry.path == path)
            .map(|entry| entry.kind)
            .ok_or_else(|| StoreError::MissingReference(path.to_string()))
    };
    let mismatch =
        |path: &str, expected: &'static str, got: StoreEntryKind| StoreError::KindMismatch {
            expected,
            got: got.as_str(),
            path: path.to_string(),
        };

    let artifact_kind = StoreEntryKind::from(kind);
    let entry_kind = kind_of(&index.entry_point)?;
    if entry_kind != artifact_kind {
        return Err(mismatch(
            &index.entry_point,
            artifact_kind.as_str(),
            entry_kind,
        ));
    }
    for path in &index.config_references {
        let got = kind_of(path)?;
        if !matches!(
            got,
            StoreEntryKind::Config | StoreEntryKind::Include | StoreEntryKind::Profile
        ) {
            return Err(mismatch(path, "config, include, or profile", got));
        }
    }
    for path in &index.source_plan.references {
        let got = kind_of(path)?;
        if !matches!(got, StoreEntryKind::Route | StoreEntryKind::Job) {
            return Err(mismatch(path, "route or job", got));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests. The blessed test lives at MODULE level (not in a nested `mod
// tests`) so the mandated filter command
// `cargo test -p camel-cli --lib compile::store::store_decoder_rejects_schema_ranges_references_and_order`
// matches exactly.
// ---------------------------------------------------------------------------

/// Assemble a store index JSON with the given entries array text and
/// reference fields, over a 15-byte content blob.
#[cfg(test)]
fn index_json(entries: &str, store_schema: u64, entry_point: &str, plan: &str) -> String {
    format!(
        "{{\"config_references\":[],\"entry_point\":\"{entry_point}\",\
         \"entries\":[{entries}],\
         \"source_plan\":{{\"references\":[{plan}]}},\
         \"store_schema\":{store_schema}}}"
    )
}

/// One canonical JSON entry object.
#[cfg(test)]
fn entry_json(kind: &str, offset: u64, length: u64, path: &str) -> String {
    format!("{{\"kind\":\"{kind}\",\"length\":{length},\"offset\":{offset},\"path\":\"{path}\"}}")
}

/// Schema-2 index JSON (the [`STORE_SCHEMA`] shape): the schema-1 layout
/// plus the substitution table field the schema-2 reader requires.
#[cfg(test)]
fn index_json_v2(entries: &str, entry_point: &str, plan: &str) -> String {
    format!(
        "{{\"config_references\":[],\"entry_point\":\"{entry_point}\",\
         \"entries\":[{entries}],\
         \"source_plan\":{{\"references\":[{plan}]}},\
         \"store_schema\":{},\"substitutions\":[]}}",
        STORE_SCHEMA
    )
}

#[cfg(test)]
const CONTENT_LEN: usize = 15;

#[test]
fn store_decoder_rejects_schema_ranges_references_and_order() {
    // Canonical baseline: two route entries in path order, exactly covering
    // the 15-byte content, entry point and plan referencing existing
    // entries. Every case below mutates exactly one rule.
    let two_routes = format!(
        "{},{}",
        entry_json("route", 0, 5, "routes/a.yaml"),
        entry_json("route", 5, 10, "routes/b.yaml")
    );
    let canonical = index_json_v2(
        &two_routes,
        "routes/a.yaml",
        "\"routes/a.yaml\",\"routes/b.yaml\"",
    );
    assert!(StoreIndex::decode(canonical.as_bytes(), CONTENT_LEN).is_ok());

    // Unknown store schema: fail closed, never reinterpret.
    let json = index_json(&two_routes, 99, "routes/a.yaml", "\"routes/a.yaml\"");
    assert_eq!(
        StoreIndex::decode(json.as_bytes(), CONTENT_LEN),
        Err(StoreError::UnsupportedStoreSchema(99))
    );

    // Out-of-bounds range: second entry runs past the content end.
    let json = index_json_v2(
        &format!(
            "{},{}",
            entry_json("route", 0, 5, "routes/a.yaml"),
            entry_json("route", 5, 99, "routes/b.yaml")
        ),
        "routes/a.yaml",
        "\"routes/a.yaml\"",
    );
    assert_eq!(
        StoreIndex::decode(json.as_bytes(), CONTENT_LEN),
        Err(StoreError::RangeOutOfBounds)
    );

    // Overlapping ranges: second entry starts before the first ends.
    let json = index_json_v2(
        &format!(
            "{},{}",
            entry_json("route", 0, 10, "routes/a.yaml"),
            entry_json("route", 4, 11, "routes/b.yaml")
        ),
        "routes/a.yaml",
        "\"routes/a.yaml\"",
    );
    assert_eq!(
        StoreIndex::decode(json.as_bytes(), CONTENT_LEN),
        Err(StoreError::OverlappingRange)
    );

    // Duplicate paths: two entries with the same logical path.
    let json = index_json_v2(
        &format!(
            "{},{}",
            entry_json("route", 0, 5, "routes/a.yaml"),
            entry_json("route", 5, 10, "routes/a.yaml")
        ),
        "routes/a.yaml",
        "\"routes/a.yaml\"",
    );
    assert_eq!(
        StoreIndex::decode(json.as_bytes(), CONTENT_LEN),
        Err(StoreError::DuplicatePath("routes/a.yaml".into()))
    );

    // Missing source-plan target: the plan names a nonexistent entry.
    let json = index_json_v2(&two_routes, "routes/a.yaml", "\"routes/missing.yaml\"");
    assert_eq!(
        StoreIndex::decode(json.as_bytes(), CONTENT_LEN),
        Err(StoreError::MissingReference("routes/missing.yaml".into()))
    );

    // Unreferenced content: entries cover only the first 5 bytes.
    let json = index_json_v2(
        &entry_json("route", 0, 5, "routes/a.yaml"),
        "routes/a.yaml",
        "\"routes/a.yaml\"",
    );
    assert_eq!(
        StoreIndex::decode(json.as_bytes(), CONTENT_LEN),
        Err(StoreError::UnreferencedContent)
    );

    // Noncanonical order: entries not in lexicographic path order.
    let json = index_json_v2(
        &format!(
            "{},{}",
            entry_json("route", 0, 5, "routes/b.yaml"),
            entry_json("route", 5, 10, "routes/a.yaml")
        ),
        "routes/b.yaml",
        "\"routes/b.yaml\",\"routes/a.yaml\"",
    );
    assert_eq!(
        StoreIndex::decode(json.as_bytes(), CONTENT_LEN),
        Err(StoreError::NoncanonicalOrder)
    );
}

/// The compiler constructs `StoreIndex` through the re-exported
/// `camel_dsl` types; its canonical index bytes must be exactly the same
/// lexicographically-keyed string pinned by the standalone model test
/// (`camel_dsl::embedded_store` `canonical_index_json_is_byte_pinned_lexicographic`),
/// independent of serde_json feature unification.
#[test]
fn canonical_index_json_matches_embedded_store_bytes() {
    let index = StoreIndex {
        config_references: vec!["Camel.toml".into(), "includes/base.yaml".into()],
        entry_point: "routes/a.yaml".into(),
        entries: vec![
            StoreEntry {
                asset_class: None,
                kind: StoreEntryKind::Config,
                length: 4,
                offset: 0,
                path: "Camel.toml".into(),
            },
            StoreEntry {
                asset_class: None,
                kind: StoreEntryKind::Include,
                length: 4,
                offset: 4,
                path: "includes/base.yaml".into(),
            },
            StoreEntry {
                asset_class: None,
                kind: StoreEntryKind::Route,
                length: 4,
                offset: 8,
                path: "routes/a.yaml".into(),
            },
            StoreEntry {
                asset_class: None,
                kind: StoreEntryKind::Route,
                length: 3,
                offset: 12,
                path: "routes/b.yaml".into(),
            },
        ],
        source_plan: SourcePlan {
            references: vec!["routes/a.yaml".into(), "routes/b.yaml".into()],
        },
        store_schema: STORE_SCHEMA,
        substitutions: Vec::new(),
    };

    let bytes = index.encode_canonical().expect("index encodes");
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
    assert_eq!(String::from_utf8(bytes).expect("utf-8"), expected);
}

/// Assets embed verbatim (r2embed Task 1.1): NUL bytes, invalid UTF-8,
/// and CRLF sequences survive build and decode byte-for-byte — no BOM
/// removal, no newline conversion, no UTF-8 requirement.
#[test]
fn asset_bytes_embed_verbatim() {
    let mut asset_bytes = vec![0x00u8, 0xFF, 0xFE, b'\r', b'\n'];
    asset_bytes.extend_from_slice(b"line\r\nsecond\x00");
    let documents = [StoreDocument {
        path: "routes/main.yaml".to_string(),
        kind: StoreEntryKind::Route,
        bytes: b"routes: []\n".to_vec(),
    }];
    let assets = [StoreAsset {
        path: "static/logo.bin".to_string(),
        class: Some("static file".to_string()),
        bytes: asset_bytes.clone(),
    }];
    let store = VirtualDocumentStore::build_with_assets(
        "routes/main.yaml",
        &documents,
        &assets,
        &[],
        &["routes/main.yaml".to_string()],
        &[],
    )
    .expect("store builds");

    let index_bytes = store.index.encode_canonical().expect("index encodes");
    let decoded =
        VirtualDocumentStore::decode(store.content.clone(), &index_bytes).expect("store decodes");
    assert_eq!(
        decoded.read("assets/static/logo.bin"),
        Some(asset_bytes.as_slice()),
        "asset bytes must survive verbatim"
    );
    // The binary-hostile shapes are really still present: no BOM strip,
    // no newline conversion, NUL preserved.
    assert_eq!(&asset_bytes[..5], &[0x00, 0xFF, 0xFE, b'\r', b'\n']);
    assert!(asset_bytes.windows(2).any(|window| window == b"\r\n"));
    assert!(asset_bytes.contains(&0x00));
}

/// The schema-2 reader still accepts schema-1 store indexes (r2embed
/// Task 1.1): a legacy one-entry index decodes unchanged, with no asset
/// class and no substitution table.
#[test]
fn schema2_decoder_accepts_schema1_store() {
    let json = index_json(
        &entry_json("job", 0, 15, "jobs/nightly.job.yaml"),
        1,
        "jobs/nightly.job.yaml",
        "\"jobs/nightly.job.yaml\"",
    );
    let store = VirtualDocumentStore::decode(vec![0xA6; 15], json.as_bytes())
        .expect("schema-1 index must decode under the schema-2 reader");
    assert_eq!(store.index.store_schema, 1);
    assert_eq!(store.index.entries.len(), 1);
    assert_eq!(store.index.entries[0].kind, StoreEntryKind::Job);
    assert_eq!(store.index.entries[0].path, "jobs/nightly.job.yaml");
    assert_eq!(store.index.entries[0].asset_class, None);
    assert!(store.index.substitutions.is_empty());
    assert_eq!(
        store.read("jobs/nightly.job.yaml"),
        Some([0xA6; 15].as_slice())
    );
}
