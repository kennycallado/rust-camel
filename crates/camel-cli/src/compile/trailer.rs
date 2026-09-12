//! Deterministic EOF trailer codec for compiled artifacts.
//!
//! Artifact layout: `payload || manifest || fixed footer`. The exact 68-byte
//! footer is `CAMELTR1` magic (8), little-endian `u16` version 1, `u8` kind
//! (`1=route`, `2=job`), zero reserved `u8`, little-endian `u64` payload
//! length, little-endian `u64` manifest length, 32-byte BLAKE3, and terminal
//! `CAMELTR1` magic (8).
//!
//! Checksum domain: ASCII `rust-camel-trailer-v1`, one `0x00` byte, then the
//! little-endian encoded version, kind, payload length, manifest length,
//! payload, and manifest. Both magic fields and the reserved byte are
//! excluded.
//!
//! Decode distinguishes an absent trailer (no exact terminal magic — the
//! image is indistinguishable from a trailer-free executable and the caller
//! falls through to normal CLI behavior) from marked corruption (terminal
//! magic present but any field, bound, or checksum invalid — fail closed).

use std::fmt;

use super::CompileError;

/// Leading and terminal footer magic.
pub const MAGIC: [u8; 8] = *b"CAMELTR1";

/// Exact footer size in bytes.
pub const FOOTER_LEN: usize = 68;

/// Trailer format version written by `encode` and required by `decode`.
pub const FORMAT_VERSION: u16 = 1;

/// Maximum normalized document size accepted by [`normalize_document`].
pub const MAX_PAYLOAD_BYTES: usize = 16 * 1024 * 1024;

/// ASCII domain separator prefixed to every checksum input.
const CHECKSUM_DOMAIN: &[u8] = b"rust-camel-trailer-v1";

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
            Self::LengthOverflow => write!(f, "trailer length fields overflow"),
            Self::LengthOutOfBounds => write!(f, "trailer lengths exceed the artifact size"),
            Self::ChecksumMismatch => write!(f, "trailer checksum mismatch"),
            Self::KindMismatch => write!(f, "footer kind does not match the manifest kind"),
            Self::InvalidManifest => write!(f, "trailer manifest is not valid manifest JSON"),
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

    let manifest_kind = manifest_kind(manifest).ok_or(TrailerError::InvalidManifest)?;
    if manifest_kind != kind {
        return Err(TrailerError::KindMismatch);
    }

    Ok(Some(Trailer {
        kind,
        payload: payload.to_vec(),
        manifest: manifest.to_vec(),
    }))
}

/// Read the `kind` field from manifest JSON bytes.
fn manifest_kind(manifest: &[u8]) -> Option<TrailerKind> {
    let value: serde_json::Value = serde_json::from_slice(manifest).ok()?;
    TrailerKind::from_name(value.get("kind")?.as_str()?)
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
