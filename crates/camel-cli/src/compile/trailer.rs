//! Deterministic EOF trailer codec for compiled artifacts.
//!
//! The `CAMELTR1` family has two versions. Version 1 appends
//! `payload || manifest || fixed footer` to the executable; the exact 68-byte
//! footer is `CAMELTR1` magic (8), little-endian `u16` version 1, `u8` kind
//! (`1=route`, `2=job`), zero reserved `u8`, little-endian `u64` payload
//! length, little-endian `u64` manifest length, 32-byte BLAKE3, and terminal
//! `CAMELTR1` magic (8).
//!
//! Version 2 appends `CAMELTR1 || content || index || manifest || footer`;
//! the exact 76-byte footer is `CAMELTR1` magic (8), little-endian `u16`
//! version 2, `u8` kind (`1=route`, `2=job`), zero flags `u8`, little-endian
//! `u64` content length, `u64` index length, `u64` manifest length, 32-byte
//! BLAKE3, and terminal `CAMELTR1` magic (8). The leading magic keeps the
//! family marker so old readers recognize a marked artifact and reject the
//! unsupported version instead of treating it as a trailer-free executable.
//!
//! Checksum domains: ASCII `rust-camel-trailer-v1` (v1) or
//! `rust-camel-trailer-v2` (v2), one `0x00` byte, then the little-endian
//! encoded version, kind, and length fields, followed by the payload (v1) or
//! content/index/manifest (v2). Both magic fields and the reserved/flags
//! byte are excluded.
//!
//! Decode distinguishes an absent trailer (no exact terminal magic — the
//! image is indistinguishable from a trailer-free executable and the caller
//! falls through to normal CLI behavior) from marked corruption (terminal
//! magic present but any field, bound, or checksum invalid — fail closed).
//! A v2-capable reader decodes a v1 artifact as a one-entry virtual store;
//! a v1 reader rejects v2 as an unsupported version, and a marked
//! v2-family footer carrying any other version fails closed with that
//! unsupported version named. A v2 decode applies
//! the strict version-matched manifest rules (schema 2 with typed required
//! fields, no unknown fields, canonical embedded paths/digests/order; the
//! schema-less legacy form only in v1), validates the
//! content/index sections through the canonical
//! [`VirtualDocumentStore`] decoder, enforces the typed reference
//! invariants (entry point, configuration references, and source plan must
//! agree with the artifact kind), and enforces manifest/store agreement
//! (the manifest source name is the store entry point; every
//! `embedded_files` entry mirrors one store entry with the matching
//! content digest) before the image is accepted.

use std::fmt;
use std::io::{self, Read, Seek, SeekFrom};

use super::CompileError;
use super::manifest;
use super::store::{
    STORE_SCHEMA, SourcePlan, StoreEntry, StoreEntryKind, StoreIndex, VirtualDocumentStore,
};

/// Leading and terminal footer magic.
pub const MAGIC: [u8; 8] = *b"CAMELTR1";

/// Exact footer size in bytes.
pub const FOOTER_LEN: usize = 68;

/// Trailer format version written by `encode` and required by `decode`.
pub const FORMAT_VERSION: u16 = 1;

/// Exact v2 footer size in bytes: magic (8), version (2), kind (1), flags
/// (1), three lengths (24), BLAKE3 (32), terminal magic (8).
pub const FOOTER_LEN_V2: usize = 76;

/// Trailer format version written by `encode_v2` and required by the v2
/// branch of `decode_artifact`.
pub const FORMAT_VERSION_V2: u16 = 2;

/// ASCII domain separator prefixed to every v1 checksum input.
const CHECKSUM_DOMAIN: &[u8] = b"rust-camel-trailer-v1";

/// ASCII domain separator prefixed to every v2 checksum input.
const CHECKSUM_DOMAIN_V2: &[u8] = b"rust-camel-trailer-v2";

/// Maximum normalized document size accepted by [`normalize_document`], and
/// the aggregate cap enforced by [`normalize_documents`] across all
/// embedded documents of one artifact.
pub const MAX_PAYLOAD_BYTES: usize = 16 * 1024 * 1024;

/// The kind of document embedded in an artifact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrailerKind {
    /// A single route document (`*.yaml`/`*.json`).
    Route,
    /// A single job document (`*.job.yaml`).
    Job,
}

impl TrailerKind {
    /// Footer discriminator byte (`1=route`, `2=job`).
    pub fn disc(self) -> u8 {
        match self {
            Self::Route => 1,
            Self::Job => 2,
        }
    }

    /// Inverse of [`TrailerKind::disc`].
    pub fn from_disc(disc: u8) -> Option<Self> {
        match disc {
            1 => Some(Self::Route),
            2 => Some(Self::Job),
            _ => None,
        }
    }

    /// Canonical lowercase name used in the manifest JSON and reports.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Route => "route",
            Self::Job => "job",
        }
    }

    /// Inverse of [`TrailerKind::as_str`].
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "route" => Some(Self::Route),
            "job" => Some(Self::Job),
            _ => None,
        }
    }
}

/// Trailer decode failure. Every variant is a MARKED corruption: the terminal
/// magic was present, so the caller must fail closed instead of falling back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrailerError {
    /// Terminal magic present but fewer than [`FOOTER_LEN`] trailing bytes.
    Truncated,
    /// Leading footer magic is not [`MAGIC`].
    InvalidMagic,
    /// Footer version is not [`FORMAT_VERSION`].
    InvalidVersion(u16),
    /// Footer kind byte is neither `1` nor `2`.
    InvalidKind(u8),
    /// Reserved byte is not zero.
    InvalidReserved(u8),
    /// v2 flags byte is not zero.
    InvalidFlags(u8),
    /// Payload and manifest lengths overflow when added.
    LengthOverflow,
    /// Payload and manifest lengths exceed the bytes preceding the footer.
    LengthOutOfBounds,
    /// BLAKE3 checksum does not match the recomputed domain hash.
    ChecksumMismatch,
    /// Footer kind disagrees with the manifest JSON `kind` field.
    KindMismatch,
    /// Manifest is not JSON with a recognizable `kind` field.
    InvalidManifest,
    /// Manifest fails the strict field rules of its trailer version (for
    /// example a schema-2 manifest without `embedded_files` or a required
    /// typed string field).
    InvalidManifestFields(String),
    /// Manifest declares a `manifest_schema` the reader does not support.
    InvalidManifestSchema(u64),
    /// Embedded store content or index is invalid.
    InvalidStore(String),
}

impl fmt::Display for TrailerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated => write!(
                f,
                "trailer is truncated (terminal magic present, footer incomplete)"
            ),
            Self::InvalidMagic => write!(f, "trailer header magic is not {MAGIC:?}"),
            Self::InvalidVersion(v) => write!(f, "unsupported trailer version {v}"),
            Self::InvalidKind(k) => write!(f, "invalid trailer kind byte {k}"),
            Self::InvalidReserved(r) => write!(f, "trailer reserved byte is {r}, must be zero"),
            Self::InvalidFlags(flags) => {
                write!(f, "trailer flags byte is {flags}, must be zero")
            }
            Self::LengthOverflow => write!(f, "trailer length fields overflow"),
            Self::LengthOutOfBounds => write!(f, "trailer lengths exceed the artifact size"),
            Self::ChecksumMismatch => write!(f, "trailer checksum mismatch"),
            Self::KindMismatch => write!(f, "footer kind does not match the manifest kind"),
            Self::InvalidManifest => write!(f, "trailer manifest is not valid manifest JSON"),
            Self::InvalidManifestFields(reason) => {
                write!(f, "manifest does not satisfy its schema: {reason}")
            }
            Self::InvalidManifestSchema(schema) => {
                write!(f, "unsupported manifest schema {schema}")
            }
            Self::InvalidStore(reason) => write!(f, "embedded store is invalid: {reason}"),
        }
    }
}

impl std::error::Error for TrailerError {}

/// One embedded document: its kind, normalized payload bytes, and canonical
/// manifest bytes (in that order in the encoded image).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trailer {
    pub kind: TrailerKind,
    pub payload: Vec<u8>,
    pub manifest: Vec<u8>,
}

/// A v2 artifact's three sections in encoded order: content (the store's
/// document bytes), index (canonical store-index JSON), and manifest
/// (canonical operational-manifest JSON).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrailerV2 {
    pub kind: TrailerKind,
    pub content: Vec<u8>,
    pub index: Vec<u8>,
    pub manifest: Vec<u8>,
}

/// Result of a version-aware artifact decode: a legacy v1 single-document
/// trailer, or a v2 multi-document store artifact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodedArtifact {
    /// Legacy v1 single-document trailer; expose it as a one-entry store
    /// with [`Trailer::to_single_entry_store`].
    V1(Trailer),
    /// v2 multi-document store artifact.
    V2(TrailerV2),
}

impl Trailer {
    /// Expose a v1 single-document artifact as a one-entry virtual store:
    /// the manifest's `source_name` becomes the entry path and logical
    /// entry point, and the payload becomes the entry content.
    ///
    /// Legacy provenance rule: v1 `source_name`s predate the strict v2
    /// canonical-path rule, and existing artifacts legitimately carry
    /// absolute names (`/abs/dir/app.yaml`) or parent-relative names
    /// (`../shared/app.yaml`) — the v1 writer preserved the operator's
    /// input path. The adaptation therefore preserves the name verbatim
    /// instead of applying [`super::store::validate_path`], which stays a
    /// v2-only rule. Only a missing, empty, or NUL-carrying identity is
    /// unusable source identity and fails closed; the one-entry layout
    /// itself is canonical (offset 0, exactly covering the payload).
    pub fn to_single_entry_store(&self) -> Result<VirtualDocumentStore, TrailerError> {
        let value: serde_json::Value =
            serde_json::from_slice(&self.manifest).map_err(|_| TrailerError::InvalidManifest)?;
        let source_name = value
            .get("source_name")
            .and_then(serde_json::Value::as_str)
            .ok_or(TrailerError::InvalidManifest)?;
        if source_name.is_empty() || source_name.contains('\0') {
            return Err(TrailerError::InvalidManifest);
        }
        Ok(VirtualDocumentStore {
            content: self.payload.clone(),
            index: StoreIndex {
                config_references: Vec::new(),
                entry_point: source_name.to_string(),
                entries: vec![StoreEntry {
                    kind: StoreEntryKind::from(self.kind),
                    length: self.payload.len() as u64,
                    offset: 0,
                    path: source_name.to_string(),
                }],
                source_plan: SourcePlan {
                    references: vec![source_name.to_string()],
                },
                store_schema: STORE_SCHEMA,
            },
        })
    }
}

/// Build the checksum domain bytes: `rust-camel-trailer-v1`, `0x00`, then the
/// little-endian version/kind/length fields, payload, and manifest.
fn checksum_domain(
    version: u16,
    kind: u8,
    payload_len: u64,
    manifest_len: u64,
    payload: &[u8],
    manifest: &[u8],
) -> Vec<u8> {
    let mut buf = Vec::with_capacity(
        CHECKSUM_DOMAIN.len() + 1 + 2 + 1 + 8 + 8 + payload.len() + manifest.len(),
    );
    buf.extend_from_slice(CHECKSUM_DOMAIN);
    buf.push(0x00);
    buf.extend_from_slice(&version.to_le_bytes());
    buf.push(kind);
    buf.extend_from_slice(&payload_len.to_le_bytes());
    buf.extend_from_slice(&manifest_len.to_le_bytes());
    buf.extend_from_slice(payload);
    buf.extend_from_slice(manifest);
    buf
}

/// Encode `payload || manifest || footer`.
pub fn encode(trailer: &Trailer) -> Vec<u8> {
    let mut out = Vec::with_capacity(trailer.payload.len() + trailer.manifest.len() + FOOTER_LEN);
    out.extend_from_slice(&trailer.payload);
    out.extend_from_slice(&trailer.manifest);

    let mut footer = [0u8; FOOTER_LEN];
    footer[0..8].copy_from_slice(&MAGIC);
    footer[8..10].copy_from_slice(&FORMAT_VERSION.to_le_bytes());
    footer[10] = trailer.kind.disc();
    // footer[11]: reserved byte stays zero.
    footer[12..20].copy_from_slice(&(trailer.payload.len() as u64).to_le_bytes());
    footer[20..28].copy_from_slice(&(trailer.manifest.len() as u64).to_le_bytes());
    let checksum = blake3::hash(&checksum_domain(
        FORMAT_VERSION,
        trailer.kind.disc(),
        trailer.payload.len() as u64,
        trailer.manifest.len() as u64,
        &trailer.payload,
        &trailer.manifest,
    ));
    footer[28..60].copy_from_slice(checksum.as_bytes());
    footer[60..68].copy_from_slice(&MAGIC);
    out.extend_from_slice(&footer);
    out
}

/// Decode the trailer from an artifact image.
///
/// Returns `Ok(None)` when the image cannot contain a trailer (shorter than
/// the magic, or the final 8 bytes are not the exact terminal magic) — the
/// caller falls through to normal CLI behavior. Returns `Err` only for MARKED
/// corruption: the terminal magic is present but the footer is truncated,
/// malformed, out of bounds, checksum-invalid, or disagrees with its
/// manifest.
pub fn decode(bytes: &[u8]) -> Result<Option<Trailer>, TrailerError> {
    if bytes.len() < MAGIC.len() || bytes[bytes.len() - MAGIC.len()..] != MAGIC {
        return Ok(None);
    }
    // Terminal magic is exact: everything past this point is marked.
    if bytes.len() < FOOTER_LEN {
        return Err(TrailerError::Truncated);
    }
    let footer = &bytes[bytes.len() - FOOTER_LEN..];

    if footer[0..8] != MAGIC {
        return Err(TrailerError::InvalidMagic);
    }
    let mut field2 = [0u8; 2];
    field2.copy_from_slice(&footer[8..10]);
    let version = u16::from_le_bytes(field2);
    if version != FORMAT_VERSION {
        return Err(TrailerError::InvalidVersion(version));
    }
    let kind_disc = footer[10];
    let kind = TrailerKind::from_disc(kind_disc).ok_or(TrailerError::InvalidKind(kind_disc))?;
    let reserved = footer[11];
    if reserved != 0 {
        return Err(TrailerError::InvalidReserved(reserved));
    }

    let mut field8 = [0u8; 8];
    field8.copy_from_slice(&footer[12..20]);
    let payload_len = u64::from_le_bytes(field8);
    field8.copy_from_slice(&footer[20..28]);
    let manifest_len = u64::from_le_bytes(field8);

    let total_len = payload_len
        .checked_add(manifest_len)
        .ok_or(TrailerError::LengthOverflow)?;
    let data_end = bytes.len() - FOOTER_LEN;
    if total_len > data_end as u64 {
        return Err(TrailerError::LengthOutOfBounds);
    }

    // Bounded by `data_end`, so the usize casts cannot truncate.
    let payload_start = data_end - total_len as usize;
    let payload_end = payload_start + payload_len as usize;
    let payload = &bytes[payload_start..payload_end];
    let manifest = &bytes[payload_end..data_end];

    let checksum = blake3::hash(&checksum_domain(
        version,
        kind_disc,
        payload_len,
        manifest_len,
        payload,
        manifest,
    ));
    if checksum.as_bytes() != &footer[28..60] {
        return Err(TrailerError::ChecksumMismatch);
    }

    let manifest_kind = manifest::validate_manifest(manifest, FORMAT_VERSION)?;
    if manifest_kind != kind {
        return Err(TrailerError::KindMismatch);
    }

    Ok(Some(Trailer {
        kind,
        payload: payload.to_vec(),
        manifest: manifest.to_vec(),
    }))
}

/// Build the v2 checksum domain bytes: `rust-camel-trailer-v2`, `0x00`, then
/// the little-endian version/kind/content/index/manifest lengths, then
/// content, index, and manifest. Magic fields and the flags byte are
/// excluded. Lengths are derived from the section slices; on the decode
/// path they equal the footer fields because the sections were sliced with
/// exactly those bounds.
fn checksum_domain_v2(
    version: u16,
    kind: u8,
    content: &[u8],
    index: &[u8],
    manifest: &[u8],
) -> Vec<u8> {
    let mut buf = Vec::with_capacity(
        CHECKSUM_DOMAIN_V2.len()
            + 1
            + 2
            + 1
            + 8
            + 8
            + 8
            + content.len()
            + index.len()
            + manifest.len(),
    );
    buf.extend_from_slice(CHECKSUM_DOMAIN_V2);
    buf.push(0x00);
    buf.extend_from_slice(&version.to_le_bytes());
    buf.push(kind);
    buf.extend_from_slice(&(content.len() as u64).to_le_bytes());
    buf.extend_from_slice(&(index.len() as u64).to_le_bytes());
    buf.extend_from_slice(&(manifest.len() as u64).to_le_bytes());
    buf.extend_from_slice(content);
    buf.extend_from_slice(index);
    buf.extend_from_slice(manifest);
    buf
}

/// Encode a v2 artifact image: `CAMELTR1 || content || index || manifest ||
/// footer` with the exact 76-byte footer.
pub fn encode_v2(trailer: &TrailerV2) -> Vec<u8> {
    let mut out = Vec::with_capacity(
        MAGIC.len()
            + trailer.content.len()
            + trailer.index.len()
            + trailer.manifest.len()
            + FOOTER_LEN_V2,
    );
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&trailer.content);
    out.extend_from_slice(&trailer.index);
    out.extend_from_slice(&trailer.manifest);

    let mut footer = [0u8; FOOTER_LEN_V2];
    footer[0..8].copy_from_slice(&MAGIC);
    footer[8..10].copy_from_slice(&FORMAT_VERSION_V2.to_le_bytes());
    footer[10] = trailer.kind.disc();
    // footer[11]: flags byte stays zero.
    footer[12..20].copy_from_slice(&(trailer.content.len() as u64).to_le_bytes());
    footer[20..28].copy_from_slice(&(trailer.index.len() as u64).to_le_bytes());
    footer[28..36].copy_from_slice(&(trailer.manifest.len() as u64).to_le_bytes());
    let checksum = blake3::hash(&checksum_domain_v2(
        FORMAT_VERSION_V2,
        trailer.kind.disc(),
        &trailer.content,
        &trailer.index,
        &trailer.manifest,
    ));
    footer[36..68].copy_from_slice(checksum.as_bytes());
    footer[68..76].copy_from_slice(&MAGIC);
    out.extend_from_slice(&footer);
    out
}

/// Decode a v2 artifact image whose terminal magic is already present.
///
/// The caller has verified the image ends with the exact terminal magic, so
/// every failure here is marked corruption. Beyond framing, checksum, and
/// manifest validation, the content/index sections are decoded through the
/// canonical store decoder and the typed reference invariants are enforced
/// before the image is accepted.
fn decode_v2_marked(bytes: &[u8]) -> Result<TrailerV2, TrailerError> {
    if bytes.len() < FOOTER_LEN_V2 {
        return Err(TrailerError::Truncated);
    }
    let footer = &bytes[bytes.len() - FOOTER_LEN_V2..];

    if footer[0..8] != MAGIC {
        return Err(TrailerError::InvalidMagic);
    }
    let mut field2 = [0u8; 2];
    field2.copy_from_slice(&footer[8..10]);
    let version = u16::from_le_bytes(field2);
    if version != FORMAT_VERSION_V2 {
        return Err(TrailerError::InvalidVersion(version));
    }
    let kind_disc = footer[10];
    let kind = TrailerKind::from_disc(kind_disc).ok_or(TrailerError::InvalidKind(kind_disc))?;
    let flags = footer[11];
    if flags != 0 {
        return Err(TrailerError::InvalidFlags(flags));
    }

    let mut field8 = [0u8; 8];
    field8.copy_from_slice(&footer[12..20]);
    let content_len = u64::from_le_bytes(field8);
    field8.copy_from_slice(&footer[20..28]);
    let index_len = u64::from_le_bytes(field8);
    field8.copy_from_slice(&footer[28..36]);
    let manifest_len = u64::from_le_bytes(field8);

    let total_len = content_len
        .checked_add(index_len)
        .and_then(|len| len.checked_add(manifest_len))
        .ok_or(TrailerError::LengthOverflow)?;
    let data_end = bytes.len() - FOOTER_LEN_V2;
    if total_len > data_end as u64 {
        return Err(TrailerError::LengthOutOfBounds);
    }

    // Bounded by `data_end`, so the usize casts cannot truncate.
    let content_start = data_end - total_len as usize;
    let content_end = content_start + content_len as usize;
    let index_end = content_end + index_len as usize;
    let content = &bytes[content_start..content_end];
    let index = &bytes[content_end..index_end];
    let manifest = &bytes[index_end..data_end];

    // Image framing: the v2 region opens with the family magic right
    // before the content. The checksum excludes both magic fields, so this
    // check is what catches leading-magic tampering.
    if content_start < MAGIC.len() || bytes[content_start - MAGIC.len()..content_start] != MAGIC {
        return Err(TrailerError::InvalidMagic);
    }

    let checksum = blake3::hash(&checksum_domain_v2(
        version, kind_disc, content, index, manifest,
    ));
    if checksum.as_bytes() != &footer[36..68] {
        return Err(TrailerError::ChecksumMismatch);
    }

    let manifest_kind = manifest::validate_manifest(manifest, FORMAT_VERSION_V2)?;
    if manifest_kind != kind {
        return Err(TrailerError::KindMismatch);
    }

    // Content/index validation through the canonical store decoder, then
    // the typed reference invariants against the artifact kind: a
    // checksum-consistent image with an inconsistent store still fails
    // closed, by name.
    let decoded_index = StoreIndex::decode(index, content.len())
        .map_err(|e| TrailerError::InvalidStore(e.to_string()))?;
    super::store::validate_typed_references(&decoded_index, kind)
        .map_err(|e| TrailerError::InvalidStore(e.to_string()))?;

    // Manifest/store agreement: the schema-2 manifest must describe the
    // very store embedded next to it — same entries in the same canonical
    // order, with matching kinds, lengths, and content digests.
    validate_manifest_store_agreement(manifest, content, &decoded_index)?;

    Ok(TrailerV2 {
        kind,
        content: content.to_vec(),
        index: index.to_vec(),
        manifest: manifest.to_vec(),
    })
}

/// Enforce schema-2 manifest/store agreement: the manifest `source_name`
/// is the store `entry_point`, every `embedded_files` entry mirrors one
/// store entry (path, kind, length) in canonical order, and its digest is
/// the BLAKE3 of that entry's content range. The manifest was already
/// parsed under the strict schema-2 rules; this closes the loop to the
/// content/index sections so a checksum-consistent image whose manifest
/// describes a different store still fails closed, by name.
fn validate_manifest_store_agreement(
    manifest: &[u8],
    content: &[u8],
    index: &StoreIndex,
) -> Result<(), TrailerError> {
    let parsed = manifest::Manifest::from_canonical_json(manifest)
        .map_err(|e| TrailerError::InvalidManifestFields(e.to_string()))?;
    if parsed.source_name != index.entry_point {
        return Err(TrailerError::InvalidManifestFields(format!(
            "manifest source_name {:?} is not the store entry point {:?}",
            parsed.source_name, index.entry_point
        )));
    }
    if parsed.embedded_files.len() != index.entries.len() {
        return Err(TrailerError::InvalidManifestFields(format!(
            "embedded_files lists {} entries, the store index carries {}",
            parsed.embedded_files.len(),
            index.entries.len()
        )));
    }
    for (file, entry) in parsed.embedded_files.iter().zip(&index.entries) {
        if file.path != entry.path || file.kind != entry.kind || file.length != entry.length {
            return Err(TrailerError::InvalidManifestFields(format!(
                "embedded_files entry for {:?} disagrees with the store entry path/kind/length",
                file.path
            )));
        }
        let range = entry.offset as usize..(entry.offset + entry.length) as usize;
        let Some(bytes) = content.get(range) else {
            return Err(TrailerError::InvalidStore(
                "entry range is out of content bounds".to_string(),
            ));
        };
        if file.digest != blake3::hash(bytes).to_hex().to_string() {
            return Err(TrailerError::InvalidManifestFields(format!(
                "embedded_files digest for {:?} does not match the store content",
                file.path
            )));
        }
    }
    Ok(())
}

/// Decode an artifact image with v2-reader semantics.
///
/// Returns `Ok(None)` when the image cannot contain a trailer (shorter than
/// the magic, or the final 8 bytes are not the exact terminal magic).
/// Otherwise the version is taken from the candidate 76-byte footer window
/// and dispatch is decided by the v2-family evidence, before any v1
/// parsing: a window declaring version 2 decodes through the v2 codec (for
/// a genuine v1 image that window field aliases the first two bytes of the
/// v1 footer magic `"CA"`, so it can never read as 2); a window declaring
/// any other version falls to the v1 codec ONLY when the last
/// [`FOOTER_LEN`] bytes still open with the family magic (a genuine v1
/// footer always does), and is named [`TrailerError::InvalidVersion`]
/// otherwise — a marked v2-family footer with an unsupported version must
/// not surface as a v1 magic error. A v1 result is wrapped as
/// [`DecodedArtifact::V1`]; expose its one-entry store with
/// [`Trailer::to_single_entry_store`].
pub fn decode_artifact(bytes: &[u8]) -> Result<Option<DecodedArtifact>, TrailerError> {
    if bytes.len() < MAGIC.len() || bytes[bytes.len() - MAGIC.len()..] != MAGIC {
        return Ok(None);
    }
    if bytes.len() >= FOOTER_LEN_V2 {
        let window = &bytes[bytes.len() - FOOTER_LEN_V2..];
        let mut field2 = [0u8; 2];
        field2.copy_from_slice(&window[8..10]);
        let version = u16::from_le_bytes(field2);
        if version == FORMAT_VERSION_V2 {
            return decode_v2_marked(bytes).map(DecodedArtifact::V2).map(Some);
        }
        let footer_start = bytes.len() - FOOTER_LEN;
        if bytes[footer_start..footer_start + MAGIC.len()] != MAGIC {
            return Err(TrailerError::InvalidVersion(version));
        }
    }
    decode(bytes).map(|trailer| trailer.map(DecodedArtifact::V1))
}

/// Read the smallest tail slice of an image that carries every byte
/// [`decode_artifact`] may inspect, so a startup probe never reads the
/// whole executable (rc-j329x: the previous whole-image read cost ~80 ms
/// and one-binary-size RSS allocation on every CLI invocation of a
/// ~100 MB image, including `--help`).
///
/// Equivalence contract with `decode_artifact(&whole_image)`:
///
/// - terminal magic absent → `Ok(None)`; the probe reads at most
///   [`FOOTER_LEN_V2`] bytes, never the image body;
/// - terminal magic present → `Ok(Some(tail))` where `tail` covers the
///   footer plus every declared section (v2 also its leading magic), so
///   `decode_artifact(&tail)` returns exactly what the whole image
///   returns: valid trailers decode identically because all framing
///   arithmetic is relative to the image end, and marked corruption
///   fails closed through the same error paths;
/// - a declared span wider than the image clamps to the image length,
///   and `decode_artifact` then reports the same out-of-bounds
///   corruption it reports on the full image.
///
/// The span floor at the footer-window length keeps the tail at least
/// as wide as the dispatch window, so `decode_artifact(&tail)` walks
/// the same version-dispatch path as the whole image.
pub fn read_probe_tail<R: Read + Seek>(reader: &mut R) -> io::Result<Option<Vec<u8>>> {
    let len = reader.seek(SeekFrom::End(0))?;
    let footer_win = read_tail(reader, len, FOOTER_LEN_V2 as u64)?;
    if footer_win.len() < MAGIC.len() || footer_win[footer_win.len() - MAGIC.len()..] != MAGIC {
        return Ok(None);
    }
    let span = probe_declared_span(&footer_win)
        .min(len)
        .max(footer_win.len() as u64);
    Ok(Some(read_tail(reader, len, span)?))
}

/// One seek-plus-read of the last `want` bytes of a `len`-byte image.
fn read_tail<R: Read + Seek>(reader: &mut R, len: u64, want: u64) -> io::Result<Vec<u8>> {
    let take = want.min(len);
    reader.seek(SeekFrom::End(-(take as i64)))?;
    let mut buf = vec![0u8; take as usize];
    reader.read_exact(&mut buf)?;
    Ok(buf)
}

/// Footer-relative span arithmetic for a marked image (terminal magic
/// present): mirror the [`decode_artifact`] dispatch to derive the exact
/// byte span the declared trailer occupies, without trusting the
/// declaration. Saturating sums keep a corrupt length field from
/// panicking; the clamp in [`read_probe_tail`] turns an oversized span
/// into a full-image read that `decode_artifact` then rejects as
/// out-of-bounds, exactly as it does today.
fn probe_declared_span(footer_win: &[u8]) -> u64 {
    if footer_win.len() < FOOTER_LEN_V2 {
        // Short marked image: the whole image is the only candidate tail.
        return footer_win.len() as u64;
    }
    let version = u16::from_le_bytes([footer_win[8], footer_win[9]]);
    if version == FORMAT_VERSION_V2 {
        let sections = le_u64(&footer_win[12..20])
            .saturating_add(le_u64(&footer_win[20..28]))
            .saturating_add(le_u64(&footer_win[28..36]));
        return sections.saturating_add(FOOTER_LEN_V2 as u64 + MAGIC.len() as u64);
    }
    let v1_footer = &footer_win[footer_win.len() - FOOTER_LEN..];
    if v1_footer[0..MAGIC.len()] != MAGIC {
        // Unsupported v2-family version: decode_artifact fails on the
        // footer window alone; no section bytes are needed.
        return FOOTER_LEN_V2 as u64;
    }
    let sections = le_u64(&v1_footer[12..20]).saturating_add(le_u64(&v1_footer[20..28]));
    sections.saturating_add(FOOTER_LEN as u64)
}

/// Little-endian `u64` from the first 8 bytes of `win`.
fn le_u64(win: &[u8]) -> u64 {
    let mut field8 = [0u8; 8];
    field8.copy_from_slice(&win[..8]);
    u64::from_le_bytes(field8)
}

/// Normalize raw document bytes for embedding: valid UTF-8 only, remove one
/// leading BOM, convert CRLF and lone CR to LF, preserve terminal-newline
/// state, and enforce the [`MAX_PAYLOAD_BYTES`] encoded-byte limit.
pub fn normalize_document(bytes: &[u8]) -> Result<String, CompileError> {
    let text = std::str::from_utf8(bytes).map_err(|_| CompileError::InvalidUtf8)?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    // CRLF first, then any surviving lone CR; a terminal newline maps to a
    // terminal LF, and a document without one stays without one.
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    if normalized.len() > MAX_PAYLOAD_BYTES {
        return Err(CompileError::PayloadTooLarge);
    }
    Ok(normalized)
}

/// Normalize a set of documents with [`normalize_document`] and enforce the
/// AGGREGATE [`MAX_PAYLOAD_BYTES`] limit across all embedded bytes: the sum
/// of normalized byte lengths must not exceed 16 MiB, so no single document
/// can smuggle an artifact past the cap by splitting it. Normalization rules
/// are byte-for-byte the per-document rules: valid UTF-8 only, one BOM
/// removed, CRLF and lone CR converted to LF, terminal-newline state
/// preserved.
pub fn normalize_documents(documents: &[&[u8]]) -> Result<Vec<String>, CompileError> {
    let mut normalized = Vec::with_capacity(documents.len());
    let mut total = 0usize;
    for document in documents {
        let text = normalize_document(document)?;
        total = total
            .checked_add(text.len())
            .ok_or(CompileError::PayloadTooLarge)?;
        normalized.push(text);
    }
    if total > MAX_PAYLOAD_BYTES {
        return Err(CompileError::PayloadTooLarge);
    }
    Ok(normalized)
}

// ---------------------------------------------------------------------------
// Tests. The six blessed tests live at MODULE level (not in a nested `mod
// tests`) so the mandated filter commands
// `cargo test -p camel-cli --lib compile::trailer::<name>` match exactly.
// Extra coverage lives in `mod tests` below.
// ---------------------------------------------------------------------------

#[cfg(test)]
const PAYLOAD: &[u8] = b"routes:\n- id: r\n  from: direct:start\n";
#[cfg(test)]
const MANIFEST: &[u8] = br#"{"kind":"route","source_name":"routes/route.yaml"}"#;

#[cfg(test)]
fn sample(kind: TrailerKind) -> Trailer {
    Trailer {
        kind,
        payload: PAYLOAD.to_vec(),
        manifest: MANIFEST.to_vec(),
    }
}

/// Rewrite the footer checksum so it covers the current (mutated) footer
/// fields and data — simulating corruption that stays checksum-consistent
/// so later checks (kind, reserved) surface by name.
#[cfg(test)]
fn reseal(mut encoded: Vec<u8>) -> Vec<u8> {
    let f = encoded.len() - FOOTER_LEN;
    let mut b2 = [0u8; 2];
    b2.copy_from_slice(&encoded[f + 8..f + 10]);
    let version = u16::from_le_bytes(b2);
    let kind = encoded[f + 10];
    let mut b8 = [0u8; 8];
    b8.copy_from_slice(&encoded[f + 12..f + 20]);
    let payload_len = u64::from_le_bytes(b8);
    b8.copy_from_slice(&encoded[f + 20..f + 28]);
    let manifest_len = u64::from_le_bytes(b8);
    let data_end = f;
    let payload_start = data_end - (payload_len + manifest_len) as usize;
    let payload = &encoded[payload_start..payload_start + payload_len as usize];
    let manifest = &encoded[payload_start + payload_len as usize..data_end];
    let sum = blake3::hash(&checksum_domain(
        version,
        kind,
        payload_len,
        manifest_len,
        payload,
        manifest,
    ));
    encoded[f + 28..f + 60].copy_from_slice(sum.as_bytes());
    encoded
}

#[test]
fn trailer_round_trip_preserves_payload_and_manifest() {
    let trailer = sample(TrailerKind::Route);
    let encoded = encode(&trailer);
    let decoded = decode(&encoded)
        .expect("valid encoding must decode")
        .expect("terminal magic must mark the trailer present");

    assert_eq!(decoded.payload, PAYLOAD);
    assert_eq!(decoded.manifest, MANIFEST);
    assert_eq!(decoded.kind, TrailerKind::Route);

    // Exact checksum: footer BLAKE3 equals the recomputed domain hash.
    let footer = &encoded[encoded.len() - FOOTER_LEN..];
    let expected = blake3::hash(&checksum_domain(
        FORMAT_VERSION,
        TrailerKind::Route.disc(),
        PAYLOAD.len() as u64,
        MANIFEST.len() as u64,
        PAYLOAD,
        MANIFEST,
    ));
    assert_eq!(&footer[28..60], expected.as_bytes());
}

#[test]
fn trailer_encoding_uses_exact_68_byte_footer() {
    let payload = b"payload";
    let manifest: &[u8] = br#"{"kind":"job"}"#;
    let encoded = encode(&Trailer {
        kind: TrailerKind::Job,
        payload: payload.to_vec(),
        manifest: manifest.to_vec(),
    });

    assert_eq!(encoded.len(), payload.len() + manifest.len() + 68);
    assert_eq!(&encoded[..payload.len()], payload);
    assert_eq!(
        &encoded[payload.len()..payload.len() + manifest.len()],
        manifest
    );

    let footer = &encoded[encoded.len() - 68..];
    assert_eq!(&footer[0..8], &MAGIC, "leading magic");
    assert_eq!(&footer[8..10], &1u16.to_le_bytes(), "version u16 LE");
    assert_eq!(footer[10], 2, "kind 2=job");
    assert_eq!(footer[11], 0, "reserved byte zero");
    assert_eq!(&footer[12..20], &(payload.len() as u64).to_le_bytes());
    assert_eq!(&footer[20..28], &(manifest.len() as u64).to_le_bytes());
    assert_eq!(&footer[60..68], &MAGIC, "terminal magic");

    let expected = blake3::hash(&checksum_domain(
        FORMAT_VERSION,
        TrailerKind::Job.disc(),
        payload.len() as u64,
        manifest.len() as u64,
        payload,
        manifest,
    ));
    assert_eq!(&footer[28..60], expected.as_bytes(), "BLAKE3 checksum");
}

#[test]
fn trailer_rejects_marked_corruption_invalid_version_lengths_and_kind() {
    let encoded = encode(&sample(TrailerKind::Route));
    let f = encoded.len() - FOOTER_LEN;

    // Mutated payload -> checksum mismatch.
    let mut corrupted = encoded.clone();
    corrupted[0] ^= 0xFF;
    assert_eq!(decode(&corrupted), Err(TrailerError::ChecksumMismatch));

    // Mutated checksum -> checksum mismatch.
    let mut corrupted = encoded.clone();
    corrupted[f + 28] ^= 0xFF;
    assert_eq!(decode(&corrupted), Err(TrailerError::ChecksumMismatch));

    // Unsupported version.
    let mut corrupted = encoded.clone();
    corrupted[f + 8..f + 10].copy_from_slice(&2u16.to_le_bytes());
    assert_eq!(decode(&corrupted), Err(TrailerError::InvalidVersion(2)));

    // Overflowing length field (payload u64::MAX + manifest 1).
    let mut corrupted = encoded.clone();
    corrupted[f + 12..f + 20].copy_from_slice(&u64::MAX.to_le_bytes());
    corrupted[f + 20..f + 28].copy_from_slice(&1u64.to_le_bytes());
    assert_eq!(decode(&corrupted), Err(TrailerError::LengthOverflow));

    // Out-of-bounds length field (sum fits u64, data does not: 1 TiB).
    let mut corrupted = encoded.clone();
    corrupted[f + 12..f + 20].copy_from_slice(&(1u64 << 40).to_le_bytes());
    assert_eq!(decode(&corrupted), Err(TrailerError::LengthOutOfBounds));

    // Invalid kind byte, checksum-consistent.
    let mut corrupted = encoded.clone();
    corrupted[f + 10] = 7;
    let corrupted = reseal(corrupted);
    assert_eq!(decode(&corrupted), Err(TrailerError::InvalidKind(7)));

    // Non-zero reserved byte, checksum-consistent.
    let mut corrupted = encoded.clone();
    corrupted[f + 11] = 1;
    let corrupted = reseal(corrupted);
    assert_eq!(decode(&corrupted), Err(TrailerError::InvalidReserved(1)));

    // Footer kind disagrees with the manifest kind, checksum-consistent.
    let mut corrupted = encoded.clone();
    corrupted[f + 10] = TrailerKind::Job.disc();
    let corrupted = reseal(corrupted);
    assert_eq!(decode(&corrupted), Err(TrailerError::KindMismatch));

    // Manifest without a recognizable kind.
    let no_kind = encode(&Trailer {
        kind: TrailerKind::Route,
        payload: PAYLOAD.to_vec(),
        manifest: b"{}".to_vec(),
    });
    assert_eq!(decode(&no_kind), Err(TrailerError::InvalidManifest));

    // Corrupt header magic (checksum excludes magics).
    let mut corrupted = encoded.clone();
    corrupted[f..f + 8].copy_from_slice(b"OTHERTR1");
    assert_eq!(decode(&corrupted), Err(TrailerError::InvalidMagic));

    // Terminal magic present, footer incomplete (marked truncation).
    let mut short = Vec::new();
    short.extend_from_slice(b"ab");
    short.extend_from_slice(&MAGIC);
    short.extend_from_slice(&MAGIC);
    assert_eq!(short.len(), 18);
    assert_eq!(decode(&short), Err(TrailerError::Truncated));

    // Every case above is MARKED corruption: never an absent fallback.
    // (Each assertion above already asserted `Err`, not `Ok(None)`.)
}

#[test]
fn trailer_without_terminal_marker_is_absent() {
    // Ordinary bytes never carried a trailer.
    assert_eq!(decode(b"an ordinary executable image"), Ok(None));
    // Empty image.
    assert_eq!(decode(b""), Ok(None));
    // Shorter than the magic.
    assert_eq!(decode(b"CAMEL"), Ok(None));
    // Truncation that removes the terminal marker: bytes of a valid
    // trailer survive, but presence is undecidable -> absent.
    let encoded = encode(&sample(TrailerKind::Route));
    let truncated = &encoded[..encoded.len() - 4];
    assert_eq!(decode(truncated), Ok(None));
    // Partial magic lookalike.
    assert_eq!(decode(b"x\x00CAMELTR"), Ok(None));
}

#[test]
fn normalization_removes_bom_and_normalizes_line_endings() {
    // BOM removed, CRLF and lone CR converted, terminal state preserved
    // (no trailing newline added, none removed).
    let bom = "\u{feff}";
    let input = format!("{bom}a\r\nb\rc");
    assert_eq!(normalize_document(input.as_bytes()), Ok("a\nb\nc".into()));

    // Terminal newline preserved when present.
    assert_eq!(normalize_document(b"x\r\n"), Ok("x\n".into()));
    assert_eq!(normalize_document(b"x"), Ok("x".into()));

    // Invalid UTF-8 is named.
    assert_eq!(
        normalize_document(&[0xFF, 0xFE]),
        Err(CompileError::InvalidUtf8)
    );

    // 16 MiB limit: exactly at the cap passes, one byte over is named.
    let at_cap = vec![b'a'; MAX_PAYLOAD_BYTES];
    assert_eq!(
        normalize_document(&at_cap),
        Ok("a".repeat(MAX_PAYLOAD_BYTES))
    );
    let over_cap = vec![b'a'; MAX_PAYLOAD_BYTES + 1];
    assert_eq!(
        normalize_document(&over_cap),
        Err(CompileError::PayloadTooLarge)
    );
}

/// `Read` wrapper that counts delivered bytes, so the probe tests can pin
/// the bounded-read mechanism (rc-j329x) without any wall-clock assert.
#[cfg(test)]
struct CountingReader<R> {
    inner: R,
    read_bytes: usize,
}

#[cfg(test)]
impl<R: std::io::Read> std::io::Read for CountingReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.read_bytes += n;
        Ok(n)
    }
}

#[cfg(test)]
impl<R: std::io::Seek> std::io::Seek for CountingReader<R> {
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        self.inner.seek(pos)
    }
}

#[cfg(test)]
fn counting<R>(inner: R) -> CountingReader<R> {
    CountingReader {
        inner,
        read_bytes: 0,
    }
}

/// Debug-string comparison: whole-image decode vs tail decode must agree
/// in the `Debug` rendering of their `Result` (value or named error).
#[cfg(test)]
fn assert_tail_decode_matches_whole(tail: &[u8], whole: &[u8]) {
    assert_eq!(
        format!("{:?}", decode_artifact(tail)),
        format!("{:?}", decode_artifact(whole)),
        "bounded tail decode must equal whole-image decode"
    );
}

#[test]
fn probe_tail_plain_image_reads_only_footer_window() {
    // A plain (non-artifact) image: absence after one footer-window read.
    // The bound is the mechanism pin: a regression back to whole-image
    // reads delivers `image.len()` bytes and fails this assert.
    let image = vec![0u8; 4 * 1024 * 1024];
    let mut reader = counting(std::io::Cursor::new(image));
    let tail = read_probe_tail(&mut reader).expect("plain image must probe cleanly");
    assert!(tail.is_none(), "no terminal magic: absence, not corruption");
    assert!(
        reader.read_bytes <= FOOTER_LEN_V2,
        "probe read {} bytes; footer window is {}",
        reader.read_bytes,
        FOOTER_LEN_V2
    );
}

#[test]
fn probe_tail_short_image_is_absence() {
    for short in [0u8, 1, 7] {
        let mut reader = counting(std::io::Cursor::new(vec![0u8; short as usize]));
        let tail = read_probe_tail(&mut reader).expect("short image must probe cleanly");
        assert!(tail.is_none(), "{short}-byte image cannot carry magic");
    }
}

#[test]
fn probe_tail_v1_image_equivalence_and_bound() {
    let mut image = vec![0u8; 64 * 1024];
    image.extend_from_slice(&encode(&sample(TrailerKind::Route)));
    let mut reader = counting(std::io::Cursor::new(image.clone()));
    let tail = read_probe_tail(&mut reader)
        .expect("marked image must probe cleanly")
        .expect("terminal magic is present");
    assert_tail_decode_matches_whole(&tail, &image);
    decode_artifact(&tail)
        .expect("valid v1 trailer must decode from the tail")
        .expect("magic marks it present");
    // One footer window plus the exact v1 trailer span — never the image.
    assert!(
        reader.read_bytes <= FOOTER_LEN_V2 + FOOTER_LEN + PAYLOAD.len() + MANIFEST.len(),
        "probe read {} bytes",
        reader.read_bytes
    );
}

#[test]
fn probe_tail_v2_image_equivalence_and_bound() {
    let documents = sample_documents();
    let store = VirtualDocumentStore::build(
        "routes/main.yaml",
        &documents,
        &["cfg/camel.toml".to_string()],
        &[
            "routes/main.yaml".to_string(),
            "routes/other.yaml".to_string(),
        ],
    )
    .expect("store must build");
    let manifest_struct = sample_manifest(&documents, &store.index);
    let trailer = TrailerV2 {
        kind: TrailerKind::Route,
        content: store.content.clone(),
        index: store.index.encode_canonical().expect("index encodes"),
        manifest: manifest_struct.to_canonical_json().into_bytes(),
    };
    let mut image = vec![0u8; 64 * 1024];
    image.extend_from_slice(&encode_v2(&trailer));
    let mut reader = counting(std::io::Cursor::new(image.clone()));
    let tail = read_probe_tail(&mut reader)
        .expect("marked image must probe cleanly")
        .expect("terminal magic is present");
    assert_tail_decode_matches_whole(&tail, &image);
    // One footer window plus the exact v2 span (footer + leading magic +
    // all three sections) — never the image.
    assert!(
        reader.read_bytes
            <= FOOTER_LEN_V2
                + FOOTER_LEN_V2
                + MAGIC.len()
                + trailer.content.len()
                + trailer.index.len()
                + trailer.manifest.len(),
        "probe read {} bytes",
        reader.read_bytes
    );
}

#[test]
fn probe_tail_marked_corruption_stays_fail_closed() {
    // Checksum-corrupted v2 image: the tail probe must surface Some and
    // the whole-image decode error, never a silent absence.
    let documents = sample_documents();
    let store = VirtualDocumentStore::build(
        "routes/main.yaml",
        &documents,
        &["cfg/camel.toml".to_string()],
        &[
            "routes/main.yaml".to_string(),
            "routes/other.yaml".to_string(),
        ],
    )
    .expect("store must build");
    let manifest_struct = sample_manifest(&documents, &store.index);
    let mut image = encode_v2(&TrailerV2 {
        kind: TrailerKind::Route,
        content: store.content.clone(),
        index: store.index.encode_canonical().expect("index encodes"),
        manifest: manifest_struct.to_canonical_json().into_bytes(),
    });
    // Corrupt one checksum byte inside the footer, terminal magic intact.
    let last = image.len() - 1;
    image[last - 20] ^= 0xFF;
    let mut reader = counting(std::io::Cursor::new(image.clone()));
    let tail = read_probe_tail(&mut reader)
        .expect("marked image must probe cleanly")
        .expect("terminal magic is present");
    assert_tail_decode_matches_whole(&tail, &image);
    assert!(
        decode_artifact(&tail).is_err(),
        "checksum corruption must fail closed from the tail"
    );
}

#[test]
fn probe_tail_truncated_marked_image_fail_closed() {
    // An image that ends in the terminal magic but is shorter than any
    // footer: whole-image decode reports the named error; the probe
    // must agree.
    let mut image = vec![0u8; 40];
    image.extend_from_slice(&MAGIC);
    let mut reader = counting(std::io::Cursor::new(image.clone()));
    let tail = read_probe_tail(&mut reader)
        .expect("marked image must probe cleanly")
        .expect("terminal magic is present");
    assert_tail_decode_matches_whole(&tail, &image);
    assert!(decode_artifact(&image).is_err());
}

#[test]
fn probe_tail_unsupported_version_without_v1_magic_reads_footer_only() {
    // A marked 76-byte window declaring version 3 whose last-68 bytes
    // do not open with the family magic: decode_artifact fails on the
    // footer window alone (InvalidVersion), so the probe must return
    // exactly that window — never more, never absence.
    let mut window = vec![0u8; FOOTER_LEN_V2];
    window[8..10].copy_from_slice(&3u16.to_le_bytes());
    window[FOOTER_LEN_V2 - MAGIC.len()..].copy_from_slice(&MAGIC);
    let mut image = vec![0u8; 1000];
    image.extend_from_slice(&window);
    let mut reader = counting(std::io::Cursor::new(image.clone()));
    let tail = read_probe_tail(&mut reader)
        .expect("marked image must probe cleanly")
        .expect("terminal magic is present");
    assert_tail_decode_matches_whole(&tail, &image);
    assert!(decode_artifact(&tail).is_err());
    assert!(
        reader.read_bytes <= 2 * FOOTER_LEN_V2,
        "probe read {} bytes; two footer windows suffice",
        reader.read_bytes
    );
}

#[test]
fn probe_tail_overflowing_declared_lengths_fail_closed() {
    // A v2-marked footer whose content length is u64::MAX: the probe's
    // saturating span clamps to the image length (one whole-image read,
    // the corruption ceiling) and decode_artifact reports the same
    // length-overflow error as it does on the full image.
    let documents = sample_documents();
    let store = VirtualDocumentStore::build(
        "routes/main.yaml",
        &documents,
        &["cfg/camel.toml".to_string()],
        &[
            "routes/main.yaml".to_string(),
            "routes/other.yaml".to_string(),
        ],
    )
    .expect("store must build");
    let manifest_struct = sample_manifest(&documents, &store.index);
    let mut image = encode_v2(&TrailerV2 {
        kind: TrailerKind::Route,
        content: store.content.clone(),
        index: store.index.encode_canonical().expect("index encodes"),
        manifest: manifest_struct.to_canonical_json().into_bytes(),
    });
    let len = image.len();
    // Footer content-length field: last 76 bytes, offset 12..20.
    image[len - FOOTER_LEN_V2 + 12..len - FOOTER_LEN_V2 + 20]
        .copy_from_slice(&u64::MAX.to_le_bytes());
    let mut reader = counting(std::io::Cursor::new(image.clone()));
    let tail = read_probe_tail(&mut reader)
        .expect("marked image must probe cleanly")
        .expect("terminal magic is present");
    assert_tail_decode_matches_whole(&tail, &image);
    assert!(decode_artifact(&tail).is_err());
}

#[test]
fn probe_tail_out_of_bounds_declared_lengths_fail_closed() {
    // A v2-marked footer declaring sections wider than the image (but
    // not u64-overflowing): the clamp yields the whole image and
    // decode_artifact reports LengthOutOfBounds, as on the full image.
    let documents = sample_documents();
    let store = VirtualDocumentStore::build(
        "routes/main.yaml",
        &documents,
        &["cfg/camel.toml".to_string()],
        &[
            "routes/main.yaml".to_string(),
            "routes/other.yaml".to_string(),
        ],
    )
    .expect("store must build");
    let manifest_struct = sample_manifest(&documents, &store.index);
    let mut image = encode_v2(&TrailerV2 {
        kind: TrailerKind::Route,
        content: store.content.clone(),
        index: store.index.encode_canonical().expect("index encodes"),
        manifest: manifest_struct.to_canonical_json().into_bytes(),
    });
    let len = image.len();
    // Content length far beyond the image, no u64 overflow.
    image[len - FOOTER_LEN_V2 + 12..len - FOOTER_LEN_V2 + 20]
        .copy_from_slice(&(len as u64 * 4).to_le_bytes());
    let mut reader = counting(std::io::Cursor::new(image.clone()));
    let tail = read_probe_tail(&mut reader)
        .expect("marked image must probe cleanly")
        .expect("terminal magic is present");
    assert_tail_decode_matches_whole(&tail, &image);
    assert!(decode_artifact(&tail).is_err());
}

#[test]
fn probe_tail_short_marked_window_between_v1_footer_and_v2_footer() {
    // 68..75-byte image ending in the terminal magic: shorter than the
    // v2 dispatch window, long enough for the v1 footer check — the
    // short-image branch probes the whole image and stays fail-closed.
    let mut image = vec![0u8; 64];
    image.extend_from_slice(&MAGIC);
    let mut reader = counting(std::io::Cursor::new(image.clone()));
    let tail = read_probe_tail(&mut reader)
        .expect("marked image must probe cleanly")
        .expect("terminal magic is present");
    assert_tail_decode_matches_whole(&tail, &image);
    assert!(decode_artifact(&image).is_err());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trailer_round_trip_preserves_job_kind() {
        let mut trailer = sample(TrailerKind::Job);
        trailer.manifest = br#"{"kind":"job"}"#.to_vec();
        let decoded = decode(&encode(&trailer))
            .expect("valid encoding must decode")
            .expect("terminal magic must mark the trailer present");
        assert_eq!(decoded.kind, TrailerKind::Job);
    }
}

// ---------------------------------------------------------------------------
// v2 store trailer tests. The four blessed v2 tests live at MODULE level
// (not in a nested `mod tests`) so the mandated filter commands
// `cargo test -p camel-cli --lib compile::trailer::<name>` match exactly.
// ---------------------------------------------------------------------------

#[cfg(test)]
use super::manifest::Manifest;

#[cfg(test)]
use super::store::StoreDocument;

/// BLAKE3 hex digest used in manifest `embedded_files` entries.
#[cfg(test)]
fn digest(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

/// Sample multi-document inputs: two route entries and one config entry.
#[cfg(test)]
fn sample_documents() -> [StoreDocument; 3] {
    [
        StoreDocument {
            path: "cfg/camel.toml".to_string(),
            kind: StoreEntryKind::Config,
            bytes: b"[profiles.default]\n".to_vec(),
        },
        StoreDocument {
            path: "routes/main.yaml".to_string(),
            kind: StoreEntryKind::Route,
            bytes: b"routes:\n- id: main\n  from: direct:in\n".to_vec(),
        },
        StoreDocument {
            path: "routes/other.yaml".to_string(),
            kind: StoreEntryKind::Route,
            bytes: b"routes:\n- id: other\n  from: direct:aux\n".to_vec(),
        },
    ]
}

/// A schema-2 manifest for the entry-point document, listing every embedded
/// file in canonical index order.
#[cfg(test)]
fn sample_manifest(documents: &[StoreDocument], index: &StoreIndex) -> Manifest {
    Manifest {
        manifest_schema: manifest::MANIFEST_SCHEMA,
        source_name: "routes/main.yaml".to_string(),
        runtime_version: manifest::RUNTIME_VERSION.to_string(),
        kind: TrailerKind::Route,
        components: vec!["direct".to_string()],
        env_names: vec![],
        listeners: vec![],
        embedded_files: index
            .entries
            .iter()
            .map(|entry| {
                let bytes = &documents
                    .iter()
                    .find(|d| d.path == entry.path)
                    .expect("entry must reference a sampled document")
                    .bytes;
                manifest::EmbeddedFile {
                    digest: digest(bytes),
                    kind: entry.kind,
                    length: entry.length,
                    path: entry.path.clone(),
                }
            })
            .collect(),
    }
}

#[test]
fn v2_trailer_round_trip_preserves_store_and_manifest() {
    // Setup: two route entries, one config entry, canonical index, and a
    // schema-2 manifest.
    let documents = sample_documents();
    let store = VirtualDocumentStore::build(
        "routes/main.yaml",
        &documents,
        &["cfg/camel.toml".to_string()],
        &[
            "routes/main.yaml".to_string(),
            "routes/other.yaml".to_string(),
        ],
    )
    .expect("store must build");
    let index_bytes = store.index.encode_canonical().expect("index encodes");
    let manifest_struct = sample_manifest(&documents, &store.index);
    let manifest_bytes = manifest_struct.to_canonical_json().into_bytes();

    // Action: encode then decode v2.
    let trailer = TrailerV2 {
        kind: TrailerKind::Route,
        content: store.content.clone(),
        index: index_bytes.clone(),
        manifest: manifest_bytes.clone(),
    };
    let encoded = encode_v2(&trailer);
    let decoded = decode_artifact(&encoded)
        .expect("valid v2 encoding must decode")
        .expect("terminal magic must mark the trailer present");
    let DecodedArtifact::V2(got) = decoded else {
        panic!("v2 image must decode as DecodedArtifact::V2");
    };

    // Kind, content, index, and manifest bytes are identical.
    assert_eq!(got.kind, TrailerKind::Route);
    assert_eq!(got.content, store.content);
    assert_eq!(got.index, index_bytes);
    assert_eq!(got.manifest, manifest_bytes);

    // Entry paths/types/ranges and the source plan are identical.
    let reparsed = VirtualDocumentStore::decode(got.content.clone(), &got.index)
        .expect("decoded index must validate against the content");
    assert_eq!(reparsed.index, store.index);
    let paths: Vec<&str> = reparsed
        .index
        .entries
        .iter()
        .map(|e| e.path.as_str())
        .collect();
    assert_eq!(
        paths,
        ["cfg/camel.toml", "routes/main.yaml", "routes/other.yaml"]
    );
    let kinds: Vec<StoreEntryKind> = reparsed.index.entries.iter().map(|e| e.kind).collect();
    assert_eq!(
        kinds,
        [
            StoreEntryKind::Config,
            StoreEntryKind::Route,
            StoreEntryKind::Route
        ]
    );
    let mut expected_offset = 0u64;
    for entry in &reparsed.index.entries {
        assert_eq!(entry.offset, expected_offset, "range offsets are canonical");
        expected_offset += entry.length;
        assert_eq!(
            reparsed.read(&entry.path),
            Some(
                documents
                    .iter()
                    .find(|d| d.path == entry.path)
                    .map(|d| d.bytes.as_slice())
                    .expect("path must round-trip")
            ),
            "content range must address the original bytes"
        );
    }
    assert_eq!(
        reparsed.index.source_plan.references,
        ["routes/main.yaml", "routes/other.yaml"]
    );

    // Manifest schema is identical.
    let got_manifest =
        Manifest::from_canonical_json(&got.manifest).expect("decoded manifest must validate");
    assert_eq!(got_manifest, manifest_struct);
    assert_eq!(got_manifest.manifest_schema, manifest::MANIFEST_SCHEMA);

    // Checksum is identical: footer bytes match the v2 domain recomputed
    // over the decoded sections.
    let footer = &encoded[encoded.len() - FOOTER_LEN_V2..];
    let expected = blake3::hash(&checksum_domain_v2(
        FORMAT_VERSION_V2,
        TrailerKind::Route.disc(),
        &got.content,
        &got.index,
        &got.manifest,
    ));
    assert_eq!(&footer[36..68], expected.as_bytes());
}

#[test]
fn v2_trailer_uses_exact_footer_and_checksum_domain() {
    // Setup: a real canonical store and its schema-2 manifest — the decode
    // path validates both, so the framing and checksum-domain assertions
    // below are byte-exact for decodable inputs.
    let documents = sample_documents();
    let store = VirtualDocumentStore::build(
        "routes/main.yaml",
        &documents,
        &["cfg/camel.toml".to_string()],
        &[
            "routes/main.yaml".to_string(),
            "routes/other.yaml".to_string(),
        ],
    )
    .expect("store must build");
    let content = store.content.clone();
    let index = store.index.encode_canonical().expect("index encodes");
    let manifest = sample_manifest(&documents, &store.index)
        .to_canonical_json()
        .into_bytes();
    let encoded = encode_v2(&TrailerV2 {
        kind: TrailerKind::Route,
        content: content.clone(),
        index: index.clone(),
        manifest: manifest.clone(),
    });

    // Layout: leading CAMELTR1, then content, index, manifest, then the
    // exact 76-byte footer.
    assert_eq!(
        encoded.len(),
        MAGIC.len() + content.len() + index.len() + manifest.len() + 76
    );
    assert_eq!(&encoded[..8], &MAGIC, "leading CAMELTR1");
    assert_eq!(&encoded[8..8 + content.len()], &content[..]);
    assert_eq!(
        &encoded[8 + content.len()..8 + content.len() + index.len()],
        &index[..]
    );
    assert_eq!(
        &encoded[8 + content.len() + index.len()..encoded.len() - 76],
        &manifest[..]
    );

    // Footer fields.
    let footer = &encoded[encoded.len() - 76..];
    assert_eq!(&footer[0..8], &MAGIC, "footer leading magic");
    assert_eq!(&footer[8..10], &2u16.to_le_bytes(), "version 2");
    assert_eq!(footer[10], 1, "kind 1=route");
    assert_eq!(footer[11], 0, "zero flags");
    assert_eq!(
        &footer[12..20],
        &(content.len() as u64).to_le_bytes(),
        "content_len"
    );
    assert_eq!(
        &footer[20..28],
        &(index.len() as u64).to_le_bytes(),
        "index_len"
    );
    assert_eq!(
        &footer[28..36],
        &(manifest.len() as u64).to_le_bytes(),
        "manifest_len"
    );
    assert_eq!(&footer[68..76], &MAGIC, "terminal CAMELTR1");

    // Checksum covers ONLY the specified domain: rust-camel-trailer-v2, one
    // zero byte, LE version/kind/three lengths, content, index, manifest.
    let mut domain = Vec::new();
    domain.extend_from_slice(b"rust-camel-trailer-v2");
    domain.push(0x00);
    domain.extend_from_slice(&2u16.to_le_bytes());
    domain.push(1);
    domain.extend_from_slice(&(content.len() as u64).to_le_bytes());
    domain.extend_from_slice(&(index.len() as u64).to_le_bytes());
    domain.extend_from_slice(&(manifest.len() as u64).to_le_bytes());
    domain.extend_from_slice(&content);
    domain.extend_from_slice(&index);
    domain.extend_from_slice(&manifest);
    assert_eq!(
        &footer[36..68],
        blake3::hash(&domain).as_bytes(),
        "checksum domain"
    );

    // Mutating only excluded fields (magic, flags) keeps the checksum valid:
    // flip a leading-magic byte -> decode names InvalidMagic, not a
    // checksum mismatch, proving magic is outside the domain.
    let mut corrupted = encoded.clone();
    corrupted[0] ^= 0xFF;
    assert_eq!(decode_artifact(&corrupted), Err(TrailerError::InvalidMagic));

    // The fixed inputs still decode end-to-end.
    assert!(decode_artifact(&encoded).is_ok());
}

#[test]
fn v2_decoder_accepts_v1_as_single_entry_store() {
    // Setup: a valid v1 artifact (legacy manifest without a schema field).
    let payload = b"jobs:\n- id: nightly\n  from: cron:0 0 * * *\n".to_vec();
    let v1 = Trailer {
        kind: TrailerKind::Job,
        payload: payload.clone(),
        manifest: br#"{"kind":"job","source_name":"jobs/nightly.job.yaml"}"#.to_vec(),
    };
    let encoded = encode(&v1);

    // Action: decode with the v2 reader.
    let decoded = decode_artifact(&encoded)
        .expect("valid v1 encoding must decode")
        .expect("terminal magic must mark the trailer present");
    let DecodedArtifact::V1(trailer) = decoded else {
        panic!("v1 image must decode as DecodedArtifact::V1");
    };
    assert_eq!(trailer.payload, payload);

    // One job entry with the original source identity and payload.
    let store = trailer
        .to_single_entry_store()
        .expect("v1 artifact must expose a one-entry store");
    assert_eq!(store.index.entries.len(), 1);
    let entry = &store.index.entries[0];
    assert_eq!(
        entry.path, "jobs/nightly.job.yaml",
        "original source identity"
    );
    assert_eq!(entry.kind, StoreEntryKind::Job);
    assert_eq!(entry.offset, 0);
    assert_eq!(entry.length, payload.len() as u64);
    assert_eq!(store.index.entry_point, "jobs/nightly.job.yaml");
    assert_eq!(
        store.index.source_plan.references,
        ["jobs/nightly.job.yaml"]
    );
    assert_eq!(
        store.read("jobs/nightly.job.yaml"),
        Some(payload.as_slice())
    );

    // The one-entry store re-encodes to a canonical index that validates.
    let index_bytes = store.index.encode_canonical().expect("index encodes");
    assert!(StoreIndex::decode(&index_bytes, store.content.len()).is_ok());
}

/// Regression: v1-to-store adaptation preserves valid legacy v1
/// `source_name`s verbatim. The strict v2 canonical-path rule must not be
/// applied to legacy provenance: the v1 writer carried absolute input
/// paths and parent-relative names, and existing artifacts legitimately
/// hold them. Only a missing, empty, or NUL-carrying identity is unusable
/// and fails closed.
#[test]
fn v1_store_adaptation_preserves_legacy_source_names() {
    // Setup: a valid v1 artifact whose source identity predates the
    // canonical-path rule.
    let payload = b"routes:\n- id: a\n  from: direct:a\n".to_vec();
    for name in ["/abs/dir/app.yaml", "../shared/app.yaml"] {
        let encoded = encode(&Trailer {
            kind: TrailerKind::Route,
            payload: payload.clone(),
            manifest: format!(r#"{{"kind":"route","source_name":"{name}"}}"#).into_bytes(),
        });

        // Action: decode with the v2 reader and adapt to a store.
        let DecodedArtifact::V1(trailer) = decode_artifact(&encoded)
            .expect("valid v1 encoding must decode")
            .expect("terminal magic must mark the trailer present")
        else {
            panic!("v1 image must decode as DecodedArtifact::V1");
        };
        let store = trailer
            .to_single_entry_store()
            .expect("legacy v1 source identity must adapt");

        // Assertion: the legacy name is preserved verbatim as the entry
        // path, entry point, and plan reference.
        assert_eq!(store.index.entry_point, name);
        assert_eq!(store.index.entries.len(), 1);
        assert_eq!(store.index.entries[0].path, name);
        assert_eq!(store.index.source_plan.references, [name]);
        assert_eq!(store.read(name), Some(payload.as_slice()));
    }

    // Unusable source identity (absent, empty, NUL) fails closed, by name.
    for manifest in [
        &br#"{"kind":"route"}"#[..],
        br#"{"kind":"route","source_name":""}"#,
        // serde decodes the \u0000 escape into a real NUL byte.
        br#"{"kind":"route","source_name":"a\u0000b.yaml"}"#,
    ] {
        let trailer = Trailer {
            kind: TrailerKind::Route,
            payload: payload.clone(),
            manifest: manifest.to_vec(),
        };
        assert_eq!(
            trailer.to_single_entry_store(),
            Err(TrailerError::InvalidManifest),
            "manifest {manifest:?} carries no usable source identity"
        );
    }
}

#[test]
fn aggregate_normalization_preserves_utf8_bom_newlines_and_cap() {
    // Setup: a BOM+CRLF entry with a terminal newline and a lone-CR entry
    // without one.
    let docs: Vec<&[u8]> = vec![
        b"\xEF\xBB\xBFroutes:\n- id: a\r\n  from: direct:a\n",
        b"routes:\n- id: b\rfrom: direct:b",
    ];

    // Action + assertion: normalization is byte-for-byte the per-document
    // rules — BOM removed, CRLF and lone CR to LF, terminal-newline state
    // preserved (one keeps its trailing LF, the other stays without one).
    let normalized = normalize_documents(&docs).expect("valid set must normalize");
    assert_eq!(normalized[0], "routes:\n- id: a\n  from: direct:a\n");
    assert_eq!(normalized[1], "routes:\n- id: b\nfrom: direct:b");

    // Invalid UTF-8 in any entry is named.
    let invalid: Vec<&[u8]> = vec![b"ok", &[0xFF, 0xFE]];
    assert_eq!(
        normalize_documents(&invalid),
        Err(CompileError::InvalidUtf8)
    );

    // Aggregate cap: two documents each under the per-document cap, but
    // their sum over 16 MiB is rejected.
    let half = vec![b'a'; MAX_PAYLOAD_BYTES / 2 + 1];
    let over: Vec<&[u8]> = vec![&half, &half];
    assert_eq!(
        normalize_documents(&over),
        Err(CompileError::PayloadTooLarge)
    );

    // Exactly at the aggregate cap passes.
    let half = &half[..MAX_PAYLOAD_BYTES / 2];
    let at_cap: Vec<&[u8]> = vec![half, half];
    let normalized = normalize_documents(&at_cap).expect("aggregate at cap must pass");
    assert_eq!(normalized[0].len() + normalized[1].len(), MAX_PAYLOAD_BYTES);
}

/// Regression: a marked v2-family footer carrying an unsupported version
/// reports `InvalidVersion`, never the v1 reader's `InvalidMagic` (the
/// 68-byte footer candidate of a 76-byte-footer image opens with footer
/// bytes, not the family magic).
#[test]
fn marked_v2_family_footer_with_unsupported_version_reports_invalid_version() {
    let documents = sample_documents();
    let store = VirtualDocumentStore::build(
        "routes/main.yaml",
        &documents,
        &[],
        &["routes/main.yaml".to_string()],
    )
    .expect("store must build");
    let encoded = encode_v2(&TrailerV2 {
        kind: TrailerKind::Route,
        content: store.content,
        index: store.index.encode_canonical().expect("index encodes"),
        manifest: sample_manifest(&documents, &store.index)
            .to_canonical_json()
            .into_bytes(),
    });
    let f = encoded.len() - FOOTER_LEN_V2;

    for version in [3u16, 9, u16::MAX] {
        let mut corrupted = encoded.clone();
        corrupted[f + 8..f + 10].copy_from_slice(&version.to_le_bytes());
        assert_eq!(
            decode_artifact(&corrupted),
            Err(TrailerError::InvalidVersion(version)),
            "version {version} must be named by the v2-family dispatch"
        );
    }

    // A genuine v1 image still routes to the v1 codec: its footer opens
    // with the magic, so the window's aliased bytes fall through.
    let v1 = encode(&Trailer {
        kind: TrailerKind::Route,
        payload: PAYLOAD.to_vec(),
        manifest: br#"{"kind":"route","source_name":"routes/route.yaml"}"#.to_vec(),
    });
    assert!(matches!(
        decode_artifact(&v1),
        Ok(Some(DecodedArtifact::V1(_)))
    ));
}

/// Regression: manifest/store agreement is enforced at v2 decode — a
/// checksum-consistent image whose schema-2 manifest describes a different
/// store (entry count, path/kind/length, or content digest) fails closed
/// by name.
#[test]
fn v2_manifest_store_disagreement_fails_closed() {
    let documents = sample_documents();
    let store = VirtualDocumentStore::build(
        "routes/main.yaml",
        &documents,
        &["cfg/camel.toml".to_string()],
        &[
            "routes/main.yaml".to_string(),
            "routes/other.yaml".to_string(),
        ],
    )
    .expect("store must build");

    // Count disagreement: the manifest lists only the entry point while
    // the index carries all three entries. encode_v2 seals these exact
    // bytes, so only the agreement rule can reject the image.
    let short_manifest = Manifest {
        manifest_schema: manifest::MANIFEST_SCHEMA,
        source_name: "routes/main.yaml".to_string(),
        runtime_version: manifest::RUNTIME_VERSION.to_string(),
        kind: TrailerKind::Route,
        components: vec![],
        env_names: vec![],
        listeners: vec![],
        embedded_files: store.index.entries[..1]
            .iter()
            .map(|entry| manifest::EmbeddedFile {
                digest: digest(b"[profiles.default]\n"),
                kind: entry.kind,
                length: entry.length,
                path: entry.path.clone(),
            })
            .collect(),
    };
    let encoded = encode_v2(&TrailerV2 {
        kind: TrailerKind::Route,
        content: store.content.clone(),
        index: store.index.encode_canonical().expect("index encodes"),
        manifest: short_manifest.to_canonical_json().into_bytes(),
    });
    assert!(matches!(
        decode_artifact(&encoded),
        Err(TrailerError::InvalidManifestFields(reason))
            if reason.contains("lists 1 entries, the store index carries 3")
    ));

    // Digest disagreement: same entries, but one digest does not match the
    // embedded content bytes.
    let mut wrong_digest = sample_manifest(&documents, &store.index);
    wrong_digest.embedded_files[1].digest = digest(b"tampered");
    let encoded = encode_v2(&TrailerV2 {
        kind: TrailerKind::Route,
        content: store.content.clone(),
        index: store.index.encode_canonical().expect("index encodes"),
        manifest: wrong_digest.to_canonical_json().into_bytes(),
    });
    assert!(matches!(
        decode_artifact(&encoded),
        Err(TrailerError::InvalidManifestFields(reason))
            if reason.contains("digest for \"routes/main.yaml\" does not match")
    ));

    // Path/kind/length disagreement: one entry's length points elsewhere.
    let mut wrong_length = sample_manifest(&documents, &store.index);
    wrong_length.embedded_files[2].length += 1;
    let encoded = encode_v2(&TrailerV2 {
        kind: TrailerKind::Route,
        content: store.content,
        index: store.index.encode_canonical().expect("index encodes"),
        manifest: wrong_length.to_canonical_json().into_bytes(),
    });
    assert!(matches!(
        decode_artifact(&encoded),
        Err(TrailerError::InvalidManifestFields(reason))
            if reason.contains("disagrees with the store entry path/kind/length")
    ));
}

/// Regression: schema-2 manifest/store agreement requires
/// `manifest.source_name == index.entry_point`. A checksum-consistent
/// image whose manifest names a different existing entry (a coherent
/// manifest, just not of this store) fails closed by name.
#[test]
fn v2_manifest_source_name_entry_point_mismatch_fails_closed() {
    let documents = sample_documents();
    let store = VirtualDocumentStore::build(
        "routes/main.yaml",
        &documents,
        &[],
        &["routes/main.yaml".to_string()],
    )
    .expect("store must build");
    let mut manifest = sample_manifest(&documents, &store.index);
    manifest.source_name = "routes/other.yaml".to_string();

    // encode_v2 seals these exact bytes, so the checksum is consistent and
    // only the agreement rule can reject the image.
    let encoded = encode_v2(&TrailerV2 {
        kind: TrailerKind::Route,
        content: store.content,
        index: store.index.encode_canonical().expect("index encodes"),
        manifest: manifest.to_canonical_json().into_bytes(),
    });
    assert!(matches!(
        decode_artifact(&encoded),
        Err(TrailerError::InvalidManifestFields(reason))
            if reason.contains(
                "manifest source_name \"routes/other.yaml\" is not the store entry point \
                 \"routes/main.yaml\""
            )
    ));
}

#[cfg(test)]
mod v2_corruption_tests {
    use super::*;

    #[test]
    fn v2_decoder_rejects_marked_corruption() {
        let documents = sample_documents();
        let store = VirtualDocumentStore::build(
            "routes/main.yaml",
            &documents,
            &[],
            &["routes/main.yaml".to_string()],
        )
        .expect("store must build");
        let index_bytes = store.index.encode_canonical().expect("index encodes");
        let manifest_bytes = sample_manifest(&documents, &store.index)
            .to_canonical_json()
            .into_bytes();
        let encoded = encode_v2(&TrailerV2 {
            kind: TrailerKind::Route,
            content: store.content,
            index: index_bytes,
            manifest: manifest_bytes,
        });
        let f = encoded.len() - FOOTER_LEN_V2;

        // Mutated content -> checksum mismatch.
        let mut corrupted = encoded.clone();
        corrupted[10] ^= 0xFF;
        assert_eq!(
            decode_artifact(&corrupted),
            Err(TrailerError::ChecksumMismatch)
        );

        // Mutated checksum -> checksum mismatch.
        let mut corrupted = encoded.clone();
        corrupted[f + 36] ^= 0xFF;
        assert_eq!(
            decode_artifact(&corrupted),
            Err(TrailerError::ChecksumMismatch)
        );

        // Non-zero flags byte, checksum-consistent -> named by flags check.
        let mut corrupted = encoded.clone();
        corrupted[f + 11] = 1;
        let corrupted = reseal_v2(corrupted);
        assert_eq!(
            decode_artifact(&corrupted),
            Err(TrailerError::InvalidFlags(1))
        );

        // Footer kind disagrees with the manifest kind, checksum-consistent.
        let mut corrupted = encoded.clone();
        corrupted[f + 10] = TrailerKind::Job.disc();
        let corrupted = reseal_v2(corrupted);
        assert_eq!(decode_artifact(&corrupted), Err(TrailerError::KindMismatch));

        // Manifest with an unknown schema, checksum-consistent.
        let documents = sample_documents();
        let store = VirtualDocumentStore::build(
            "routes/main.yaml",
            &documents,
            &[],
            &["routes/main.yaml".to_string()],
        )
        .expect("store must build");
        let mut bad_schema_manifest = sample_manifest(&documents, &store.index)
            .to_canonical_json()
            .into_bytes();
        let needle = br#""manifest_schema":2"#.to_vec();
        let pos = bad_schema_manifest
            .windows(needle.len())
            .position(|w| w == needle.as_slice())
            .expect("manifest carries its schema field");
        bad_schema_manifest.splice(
            pos..pos + needle.len(),
            br#""manifest_schema":99"#.iter().copied(),
        );
        let corrupted = encode_v2(&TrailerV2 {
            kind: TrailerKind::Route,
            content: store.content.clone(),
            index: store.index.encode_canonical().expect("index encodes"),
            manifest: bad_schema_manifest,
        });
        assert_eq!(
            decode_artifact(&corrupted),
            Err(TrailerError::InvalidManifestSchema(99))
        );

        // Out-of-bounds v2 lengths (sum fits u64, data does not).
        let mut corrupted = encoded.clone();
        corrupted[f + 12..f + 20].copy_from_slice(&(1u64 << 40).to_le_bytes());
        assert_eq!(
            decode_artifact(&corrupted),
            Err(TrailerError::LengthOutOfBounds)
        );

        // Truncation that keeps the terminal magic is marked truncation.
        let mut short = Vec::new();
        short.extend_from_slice(b"ab");
        short.extend_from_slice(&MAGIC);
        short.extend_from_slice(&MAGIC);
        assert_eq!(decode_artifact(&short), Err(TrailerError::Truncated));

        // Absent trailer falls through (never an error).
        assert_eq!(decode_artifact(b"an ordinary executable image"), Ok(None));

        // A v2 image is rejected by the v1-only reader as unsupported
        // rather than misparsed or treated as absent: the garbage v1 footer
        // candidate fails the v1 checks (here the magic check).
        assert!(decode(&encoded).is_err());
        assert_ne!(decode(&encoded), Ok(None));
    }

    /// Reseal a v2 footer checksum after mutating excluded fields so later
    /// checks (flags, kind, schema) surface by name.
    #[cfg(test)]
    fn reseal_v2(mut encoded: Vec<u8>) -> Vec<u8> {
        let f = encoded.len() - FOOTER_LEN_V2;
        let mut b2 = [0u8; 2];
        b2.copy_from_slice(&encoded[f + 8..f + 10]);
        let version = u16::from_le_bytes(b2);
        let kind = encoded[f + 10];
        let mut b8 = [0u8; 8];
        b8.copy_from_slice(&encoded[f + 12..f + 20]);
        let content_len = u64::from_le_bytes(b8);
        b8.copy_from_slice(&encoded[f + 20..f + 28]);
        let index_len = u64::from_le_bytes(b8);
        b8.copy_from_slice(&encoded[f + 28..f + 36]);
        let manifest_len = u64::from_le_bytes(b8);
        let data_end = f;
        let content_start = data_end - (content_len + index_len + manifest_len) as usize;
        let content = &encoded[content_start..content_start + content_len as usize];
        let index = &encoded[content_start + content_len as usize
            ..content_start + (content_len + index_len) as usize];
        let manifest = &encoded[content_start + (content_len + index_len) as usize..data_end];
        let sum = blake3::hash(&checksum_domain_v2(version, kind, content, index, manifest));
        encoded[f + 36..f + 68].copy_from_slice(sum.as_bytes());
        encoded
    }

    /// Regression: an index that fails the canonical store decoder is
    /// rejected even when the footer checksum is recomputed over the
    /// corruption — checksum validity alone never admits a broken store.
    #[test]
    fn v2_resealed_invalid_index_fails_closed() {
        let documents = sample_documents();
        let store = VirtualDocumentStore::build(
            "routes/main.yaml",
            &documents,
            &["cfg/camel.toml".to_string()],
            &[
                "routes/main.yaml".to_string(),
                "routes/other.yaml".to_string(),
            ],
        )
        .expect("store must build");
        let content_len = store.content.len();
        let index_bytes = store.index.encode_canonical().expect("index encodes");
        let manifest_bytes = sample_manifest(&documents, &store.index)
            .to_canonical_json()
            .into_bytes();
        let encoded = encode_v2(&TrailerV2 {
            kind: TrailerKind::Route,
            content: store.content,
            index: index_bytes,
            manifest: manifest_bytes,
        });

        // The index region starts right after the leading magic + content.
        let index_start = MAGIC.len() + content_len;
        let mut corrupted = encoded.clone();
        corrupted[index_start] = b'x';
        // Without resealing, the mutation is a plain checksum mismatch.
        assert_eq!(
            decode_artifact(&corrupted),
            Err(TrailerError::ChecksumMismatch)
        );

        // Recompute the checksum over the corruption: the image is now
        // checksum-consistent, and the canonical store decoder must still
        // reject the broken index by name.
        let corrupted = reseal_v2(corrupted);
        assert!(matches!(
            decode_artifact(&corrupted),
            Err(TrailerError::InvalidStore(_))
        ));
    }

    /// Regression: typed-reference mismatches fail closed even though the
    /// image seals and decodes structurally — the entry point must match
    /// the artifact kind, configuration references must target
    /// config/include/profile entries, and the source plan must target
    /// route/job entries.
    #[test]
    fn v2_resealed_typed_reference_mismatch_fails_closed() {
        let documents = sample_documents();
        let cfg = "cfg/camel.toml".to_string();
        let main = "routes/main.yaml".to_string();
        let other = "routes/other.yaml".to_string();

        // (entry point, config references, source plan, expected reason
        // substring) — `VirtualDocumentStore::build` accepts any references
        // to existing entries; the artifact-kind invariants are enforced at
        // decode.
        let cases = [
            (
                cfg.clone(),
                vec![cfg.clone()],
                vec![main.clone()],
                "expected route",
            ),
            (
                main.clone(),
                vec![other.clone()],
                vec![main.clone()],
                "config, include, or profile",
            ),
            (main.clone(), vec![cfg.clone()], vec![cfg], "route or job"),
        ];
        for (entry_point, config_references, source_plan, reason) in cases {
            let store = VirtualDocumentStore::build(
                &entry_point,
                &documents,
                &config_references,
                &source_plan,
            )
            .expect("references name existing entries");
            let encoded = encode_v2(&TrailerV2 {
                kind: TrailerKind::Route,
                content: store.content,
                index: store.index.encode_canonical().expect("index encodes"),
                manifest: sample_manifest(&documents, &store.index)
                    .to_canonical_json()
                    .into_bytes(),
            });
            // No mutation, so the footer checksum is already valid: only
            // the typed-reference invariants can reject this image.
            let err =
                decode_artifact(&encoded).expect_err("typed-reference mismatch must fail closed");
            assert!(
                matches!(&err, TrailerError::InvalidStore(msg) if msg.contains(reason)),
                "unexpected error for entry point {entry_point:?}: {err:?}"
            );
        }
    }

    /// Regression: v2 manifest parsing is strict — schema 2 requires
    /// `embedded_files` and the required typed string fields, and the
    /// schema-less legacy form is accepted only in a v1 artifact.
    #[test]
    fn v2_manifest_strictness_rejects_incomplete_and_legacy_forms() {
        let documents = sample_documents();
        let store = VirtualDocumentStore::build(
            "routes/main.yaml",
            &documents,
            &["cfg/camel.toml".to_string()],
            &[
                "routes/main.yaml".to_string(),
                "routes/other.yaml".to_string(),
            ],
        )
        .expect("store must build");
        let encode_with = |manifest: Vec<u8>| {
            encode_v2(&TrailerV2 {
                kind: TrailerKind::Route,
                content: store.content.clone(),
                index: store.index.encode_canonical().expect("index encodes"),
                manifest,
            })
        };

        // Schema 2 without `embedded_files`: rejected by name.
        let incomplete = format!(
            r#"{{"components":["direct"],"env_names":[],"kind":"route","listeners":[],"manifest_schema":2,"runtime_version":"{rt}","source_name":"routes/main.yaml"}}"#,
            rt = manifest::RUNTIME_VERSION,
        );
        assert_eq!(
            decode_artifact(&encode_with(incomplete.into_bytes())),
            Err(TrailerError::InvalidManifestFields(
                "schema-2 manifest carries no embedded_files array".to_string()
            ))
        );

        // Schema 2 without `runtime_version`: required typed field.
        let no_runtime = br#"{"components":[],"embedded_files":[],"env_names":[],"kind":"route","listeners":[],"manifest_schema":2,"source_name":"routes/main.yaml"}"#.to_vec();
        assert_eq!(
            decode_artifact(&encode_with(no_runtime)),
            Err(TrailerError::InvalidManifestFields(
                "schema-2 manifest carries no runtime_version".to_string()
            ))
        );

        // Schema-less legacy manifest in a v2 artifact: legacy is v1-only.
        let legacy = br#"{"kind":"route","source_name":"routes/main.yaml"}"#.to_vec();
        assert_eq!(
            decode_artifact(&encode_with(legacy.clone())),
            Err(TrailerError::InvalidManifestSchema(
                manifest::MANIFEST_SCHEMA_LEGACY
            ))
        );

        // The same legacy manifest decodes fine in a v1 artifact.
        let v1 = encode(&Trailer {
            kind: TrailerKind::Route,
            payload: b"routes:\n- id: main\n  from: direct:in\n".to_vec(),
            manifest: legacy,
        });
        assert!(decode_artifact(&v1).is_ok());

        // And a v1 artifact carrying an explicit schema-2 manifest is
        // rejected too: each trailer version accepts exactly its own
        // manifest form.
        let v1_schema2_manifest = format!(
            r#"{{"components":[],"embedded_files":[],"env_names":[],"kind":"route","listeners":[],"manifest_schema":2,"runtime_version":"{rt}","source_name":"routes/main.yaml"}}"#,
            rt = manifest::RUNTIME_VERSION,
        );
        let v1_schema2 = encode(&Trailer {
            kind: TrailerKind::Route,
            payload: b"routes:\n- id: main\n  from: direct:in\n".to_vec(),
            manifest: v1_schema2_manifest.into_bytes(),
        });
        assert_eq!(
            decode_artifact(&v1_schema2),
            Err(TrailerError::InvalidManifestSchema(
                manifest::MANIFEST_SCHEMA
            ))
        );
    }
}
