//! Detached Ed25519ph signature envelope for compiled artifacts.
//!
//! A signed artifact carries a `CAMELSG1` sidecar file (`<artifact>.sig`)
//! of exactly [`ENVELOPE_LEN`] bytes (house trailer style):
//!
//! ```text
//! off  len  field
//! 0    8    magic "CAMELSG1"
//! 8    2    u16 LE envelope version = 1
//! 10   1    u8 algorithm = 1 (Ed25519ph, RFC 8032 §5.1, null context)
//! 11   1    u8 flags = 0 (reserved, must be zero)
//! 12   32   ed25519 verifying key (compressed)
//! 44   64   signature
//! 108  32   BLAKE3 over b"rust-camel-signature-v1" || 0u8 || version ||
//!           algorithm || flags || pubkey || signature
//! 140  8    terminal magic "CAMELSG1"
//! ```
//!
//! The signature covers the FINAL artifact bytes. Artifacts are large, so
//! the SHA-512 prehash is computed by the caller while the artifact
//! streams: callers feed the byte stream through `ed25519_dalek::Sha512`
//! (the [`ed25519_dalek::Digest`] trait) and hand this module only the
//! finalized 64-byte digest. No function here buffers a message.
//!
//! [`fingerprint`] (`blake3:` + lowercase hex over the 32-byte verifying
//! key) is the identity recorded in the manifest signing block and pinned
//! by operators. The envelope BLAKE3 checksum is integrity of the envelope
//! itself and stays outside the trailer checksum domain.

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use ed25519_dalek::ed25519::signature::digest::{
    FixedOutput, HashMarker, Output, OutputSizeUser, Update, typenum::U64,
};
use ed25519_dalek::{Signature, SigningKey, VerifyingKey};

/// Leading and terminal envelope magic. Eight bytes per the `CAMELTR1`
/// family framing (`CAMEL` + two-letter domain code + format version): the
/// spec prose says `CAMELSIG1`, which is nine bytes and cannot fit the
/// eight-byte magic field of the 148-byte layout.
pub const SIGNATURE_MAGIC: [u8; 8] = *b"CAMELSG1";

/// Envelope format version written by [`encode_envelope`] and required by
/// [`verify_envelope`].
pub const ENVELOPE_VERSION: u16 = 1;

/// Algorithm byte for Ed25519ph (RFC 8032 §5.1, null context).
pub const ALGORITHM_ED25519PH: u8 = 1;

/// Canonical algorithm name for [`ALGORITHM_ED25519PH`], used in the
/// manifest signing block and as the `expected_algorithm` vocabulary of
/// [`verify_envelope`].
pub const ALGORITHM_NAME_ED25519PH: &str = "ed25519ph";

/// Exact envelope size in bytes: magic (8), version (2), algorithm (1),
/// flags (1), verifying key (32), signature (64), BLAKE3 checksum (32),
/// terminal magic (8).
pub const ENVELOPE_LEN: usize = 148;

/// ASCII domain separator prefixed to every envelope checksum input.
const CHECKSUM_DOMAIN: &[u8] = b"rust-camel-signature-v1";

const VERSION_OFFSET: usize = 8;
const ALGORITHM_OFFSET: usize = 10;
const FLAGS_OFFSET: usize = 11;
const VERIFYING_KEY_OFFSET: usize = 12;
const SIGNATURE_OFFSET: usize = 44;
const CHECKSUM_OFFSET: usize = 108;
const TERMINAL_MAGIC_OFFSET: usize = 140;

/// Signature envelope failure. Every variant names the failing step so the
/// CLI can surface a precise exit-2 diagnostic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnvelopeError {
    /// Input is not exactly [`ENVELOPE_LEN`] bytes.
    Truncated,
    /// Leading or terminal magic is not [`SIGNATURE_MAGIC`].
    BadMagic,
    /// Envelope version is not [`ENVELOPE_VERSION`].
    UnsupportedVersion(u16),
    /// Algorithm byte is not [`ALGORITHM_ED25519PH`].
    UnsupportedAlgorithm(u8),
    /// Flags byte is not zero.
    BadFlags(u8),
    /// BLAKE3 checksum does not match the recomputed domain hash.
    ChecksumMismatch,
    /// Envelope algorithm does not match the caller's expected algorithm.
    AlgorithmMismatch,
    /// Verifying key fingerprint does not match the expected fingerprint.
    FingerprintMismatch,
    /// Ed25519ph verification rejected the signature over the digest.
    SignatureInvalid,
    /// Signing key file is not exactly 32 bytes (an ed25519 seed).
    BadKeyFile {
        /// The rejected key file path.
        path: PathBuf,
        /// The size the file actually has.
        size: usize,
    },
    /// Key file could not be read. The `io::Error` is stringified so the
    /// error stays cloneable and comparable.
    Io(String),
}

impl fmt::Display for EnvelopeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated => write!(
                f,
                "signature envelope is truncated (must be exactly {ENVELOPE_LEN} bytes)"
            ),
            Self::BadMagic => write!(
                f,
                "signature envelope leading or terminal magic is not {}",
                core::str::from_utf8(&SIGNATURE_MAGIC).unwrap_or("CAMELSG1")
            ),
            Self::UnsupportedVersion(v) => write!(f, "unsupported signature envelope version {v}"),
            Self::UnsupportedAlgorithm(a) => write!(f, "unsupported signature algorithm byte {a}"),
            Self::BadFlags(flags) => {
                write!(f, "signature envelope flags byte is {flags}, must be zero")
            }
            Self::ChecksumMismatch => write!(f, "signature envelope checksum mismatch"),
            Self::AlgorithmMismatch => write!(
                f,
                "signature envelope algorithm does not match the expected algorithm"
            ),
            Self::FingerprintMismatch => write!(
                f,
                "verifying key fingerprint does not match the expected fingerprint"
            ),
            Self::SignatureInvalid => write!(
                f,
                "signature is not valid for the message digest and verifying key"
            ),
            Self::BadKeyFile { path, size } => write!(
                f,
                "signing key file {path:?} is {size} bytes, must be exactly 32"
            ),
            Self::Io(reason) => write!(f, "signing key file could not be read: {reason}"),
        }
    }
}

impl std::error::Error for EnvelopeError {}

/// Successful [`verify_envelope`] result: the canonical algorithm name and
/// the verifying-key fingerprint the caller can log or compare against the
/// manifest signing block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedEnvelope {
    /// Canonical algorithm name (currently always `ed25519ph`).
    pub algorithm_name: &'static str,
    /// `blake3:` fingerprint of the envelope's verifying key.
    pub fingerprint: String,
}

/// Identity of a verifying key: `blake3:` + lowercase hex BLAKE3 over the
/// 32-byte compressed key.
pub fn fingerprint(verifying_key_bytes: &[u8; 32]) -> String {
    format!("blake3:{}", blake3::hash(verifying_key_bytes).to_hex())
}

/// Load a 32-byte ed25519 seed file. The file must be exactly 32 bytes —
/// any other size fails with [`EnvelopeError::BadKeyFile`] naming the path
/// and the actual size.
pub fn load_signing_key(path: &Path) -> Result<SigningKey, EnvelopeError> {
    let bytes = fs::read(path).map_err(|e| EnvelopeError::Io(e.to_string()))?;
    let seed: [u8; 32] = bytes
        .as_slice()
        .try_into()
        .map_err(|_| EnvelopeError::BadKeyFile {
            path: path.to_path_buf(),
            size: bytes.len(),
        })?;
    Ok(SigningKey::from_bytes(&seed))
}

/// Adapter presenting a finalized 64-byte SHA-512 prehash as the
/// `Digest<OutputSize = U64>` that ed25519-dalek's prehashed API consumes.
/// ed25519-dalek finalizes the digest once to obtain the RFC 8032 prehash,
/// so the adapter stores the caller's streamed digest verbatim and returns
/// it on finalize; `update` is a no-op by construction.
#[derive(Clone)]
struct RawPrehash([u8; 64]);

impl Default for RawPrehash {
    fn default() -> Self {
        Self([0u8; 64])
    }
}

impl HashMarker for RawPrehash {}

impl OutputSizeUser for RawPrehash {
    type OutputSize = U64;
}

impl Update for RawPrehash {
    fn update(&mut self, _data: &[u8]) {}
}

impl FixedOutput for RawPrehash {
    fn finalize_into(self, out: &mut Output<Self>) {
        let dst: &mut [u8] = out.as_mut();
        dst.copy_from_slice(&self.0);
    }
}

/// Algorithm id for a canonical algorithm name, if the name is known.
fn algorithm_id(name: &str) -> Option<u8> {
    match name {
        ALGORITHM_NAME_ED25519PH => Some(ALGORITHM_ED25519PH),
        _ => None,
    }
}

/// BLAKE3 over `domain || 0u8 || version || algorithm || flags || pubkey ||
/// signature` — the envelope's own integrity checksum.
fn envelope_checksum(
    version: u16,
    algorithm: u8,
    flags: u8,
    verifying_key: &[u8; 32],
    signature: &[u8; 64],
) -> [u8; 32] {
    let mut input = Vec::with_capacity(CHECKSUM_DOMAIN.len() + 1 + 2 + 1 + 1 + 32 + 64);
    input.extend_from_slice(CHECKSUM_DOMAIN);
    input.push(0);
    input.extend_from_slice(&version.to_le_bytes());
    input.push(algorithm);
    input.push(flags);
    input.extend_from_slice(verifying_key);
    input.extend_from_slice(signature);
    *blake3::hash(&input).as_bytes()
}

/// Encode the [`ENVELOPE_LEN`]-byte envelope for `key` over the
/// caller-streamed SHA-512 digest of the artifact bytes. Deterministic:
/// the same key and digest always produce byte-identical output.
pub fn encode_envelope(key: &SigningKey, message_sha512: &[u8; 64]) -> [u8; ENVELOPE_LEN] {
    let signature = key
        .sign_prehashed(RawPrehash(*message_sha512), None)
        .expect("Ed25519ph signing with a null context cannot fail"); // allow-unwrap: null-context Ed25519ph signing is infallible
    let verifying_key = key.verifying_key().to_bytes();
    let signature = signature.to_bytes();
    let mut envelope = [0u8; ENVELOPE_LEN];
    envelope[..8].copy_from_slice(&SIGNATURE_MAGIC);
    envelope[VERSION_OFFSET..ALGORITHM_OFFSET].copy_from_slice(&ENVELOPE_VERSION.to_le_bytes());
    envelope[ALGORITHM_OFFSET] = ALGORITHM_ED25519PH;
    // flags stays zero (reserved).
    envelope[VERIFYING_KEY_OFFSET..SIGNATURE_OFFSET].copy_from_slice(&verifying_key);
    envelope[SIGNATURE_OFFSET..CHECKSUM_OFFSET].copy_from_slice(&signature);
    envelope[CHECKSUM_OFFSET..TERMINAL_MAGIC_OFFSET].copy_from_slice(&envelope_checksum(
        ENVELOPE_VERSION,
        ALGORITHM_ED25519PH,
        0,
        &verifying_key,
        &signature,
    ));
    envelope[TERMINAL_MAGIC_OFFSET..].copy_from_slice(&SIGNATURE_MAGIC);
    envelope
}

/// Parse and fully verify a signature envelope against the caller-streamed
/// SHA-512 digest of the artifact bytes. Checks, in order, each step named
/// by its error: length, magics, version, algorithm byte, flags, BLAKE3
/// checksum, expected algorithm, fingerprint, Ed25519ph signature.
pub fn verify_envelope(
    bytes: &[u8],
    message_sha512: &[u8; 64],
    expected_fingerprint: &str,
    expected_algorithm: &str,
) -> Result<VerifiedEnvelope, EnvelopeError> {
    if bytes.len() != ENVELOPE_LEN {
        return Err(EnvelopeError::Truncated);
    }
    if bytes[..8] != SIGNATURE_MAGIC || bytes[TERMINAL_MAGIC_OFFSET..] != SIGNATURE_MAGIC {
        return Err(EnvelopeError::BadMagic);
    }
    let version = u16::from_le_bytes([bytes[VERSION_OFFSET], bytes[VERSION_OFFSET + 1]]);
    if version != ENVELOPE_VERSION {
        return Err(EnvelopeError::UnsupportedVersion(version));
    }
    let algorithm = bytes[ALGORITHM_OFFSET];
    if algorithm != ALGORITHM_ED25519PH {
        return Err(EnvelopeError::UnsupportedAlgorithm(algorithm));
    }
    let flags = bytes[FLAGS_OFFSET];
    if flags != 0 {
        return Err(EnvelopeError::BadFlags(flags));
    }
    let mut key_bytes = [0u8; 32];
    key_bytes.copy_from_slice(&bytes[VERIFYING_KEY_OFFSET..SIGNATURE_OFFSET]);
    let mut signature_bytes = [0u8; 64];
    signature_bytes.copy_from_slice(&bytes[SIGNATURE_OFFSET..CHECKSUM_OFFSET]);
    let mut checksum = [0u8; 32];
    checksum.copy_from_slice(&bytes[CHECKSUM_OFFSET..TERMINAL_MAGIC_OFFSET]);
    if envelope_checksum(version, algorithm, flags, &key_bytes, &signature_bytes) != checksum {
        return Err(EnvelopeError::ChecksumMismatch);
    }
    if algorithm_id(expected_algorithm) != Some(algorithm) {
        return Err(EnvelopeError::AlgorithmMismatch);
    }
    let fingerprint = fingerprint(&key_bytes);
    if fingerprint != expected_fingerprint {
        return Err(EnvelopeError::FingerprintMismatch);
    }
    // A malformed key maps to SignatureInvalid, not a dedicated variant: with a single algorithm the distinction changes no operator decision — split into a BadVerifyingKey variant when a second algorithm joins (bd rc-5u0jx).
    let verifying_key =
        VerifyingKey::from_bytes(&key_bytes).map_err(|_| EnvelopeError::SignatureInvalid)?;
    let signature =
        Signature::from_slice(&signature_bytes).map_err(|_| EnvelopeError::SignatureInvalid)?;
    verifying_key
        .verify_prehashed(RawPrehash(*message_sha512), None, &signature)
        .map_err(|_| EnvelopeError::SignatureInvalid)?;
    Ok(VerifiedEnvelope {
        algorithm_name: ALGORITHM_NAME_ED25519PH,
        fingerprint,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Obvious synthetic-pattern seeds — never real key material.
    const TEST_SEED_A: [u8; 32] = *b"r4sign-test-seed-00000000000000\0";
    const TEST_SEED_B: [u8; 32] = *b"r4sign-test-seed-00000000000001\0";

    fn test_key(seed: &[u8; 32]) -> SigningKey {
        SigningKey::from_bytes(seed)
    }

    /// Fixed synthetic digest pattern, parameterized by a tag byte so
    /// tests can derive distinct digests without real message bytes.
    fn test_digest(tag: u8) -> [u8; 64] {
        let mut digest = [tag; 64];
        for (i, byte) in digest.iter_mut().enumerate() {
            *byte ^= i as u8;
        }
        digest
    }

    #[test]
    fn envelope_roundtrip_and_determinism() {
        let key = test_key(&TEST_SEED_A);
        let digest = test_digest(0xA5);
        let fingerprint = fingerprint(&key.verifying_key().to_bytes());

        let first = encode_envelope(&key, &digest);
        let second = encode_envelope(&key, &digest);
        assert_eq!(first.len(), ENVELOPE_LEN);
        assert_eq!(first, second, "Ed25519ph must be deterministic");
        assert_eq!(&first[..8], &SIGNATURE_MAGIC);
        assert_eq!(&first[TERMINAL_MAGIC_OFFSET..], &SIGNATURE_MAGIC);
        assert_eq!(first[FLAGS_OFFSET], 0);

        let verified = verify_envelope(&first, &digest, &fingerprint, ALGORITHM_NAME_ED25519PH)
            .expect("valid envelope verifies");
        assert_eq!(verified.algorithm_name, ALGORITHM_NAME_ED25519PH);
        assert_eq!(verified.fingerprint, fingerprint);
    }

    #[test]
    fn envelope_parse_rejects_corruption() {
        let key = test_key(&TEST_SEED_A);
        let digest = test_digest(0xA5);
        let expected_fp = fingerprint(&key.verifying_key().to_bytes());
        let envelope = encode_envelope(&key, &digest);

        assert_eq!(
            verify_envelope(
                &envelope[..ENVELOPE_LEN - 1],
                &digest,
                &expected_fp,
                ALGORITHM_NAME_ED25519PH
            )
            .unwrap_err(),
            EnvelopeError::Truncated
        );

        let mut leading_magic = envelope;
        leading_magic[0] ^= 0xff;
        assert_eq!(
            verify_envelope(
                &leading_magic,
                &digest,
                &expected_fp,
                ALGORITHM_NAME_ED25519PH
            )
            .unwrap_err(),
            EnvelopeError::BadMagic
        );

        let mut version = envelope;
        version[VERSION_OFFSET] = 2;
        assert_eq!(
            verify_envelope(&version, &digest, &expected_fp, ALGORITHM_NAME_ED25519PH).unwrap_err(),
            EnvelopeError::UnsupportedVersion(2)
        );

        let mut algorithm = envelope;
        algorithm[ALGORITHM_OFFSET] = 9;
        assert_eq!(
            verify_envelope(&algorithm, &digest, &expected_fp, ALGORITHM_NAME_ED25519PH)
                .unwrap_err(),
            EnvelopeError::UnsupportedAlgorithm(9)
        );

        let mut flags = envelope;
        flags[FLAGS_OFFSET] = 1;
        assert_eq!(
            verify_envelope(&flags, &digest, &expected_fp, ALGORITHM_NAME_ED25519PH).unwrap_err(),
            EnvelopeError::BadFlags(1)
        );

        let mut checksum = envelope;
        checksum[CHECKSUM_OFFSET] ^= 1;
        assert_eq!(
            verify_envelope(&checksum, &digest, &expected_fp, ALGORITHM_NAME_ED25519PH)
                .unwrap_err(),
            EnvelopeError::ChecksumMismatch
        );

        // The checksum covers the verifying key, so a flipped key byte is
        // caught by the checksum step before the fingerprint step.
        let mut pubkey = envelope;
        pubkey[VERIFYING_KEY_OFFSET] ^= 1;
        assert_eq!(
            verify_envelope(&pubkey, &digest, &expected_fp, ALGORITHM_NAME_ED25519PH).unwrap_err(),
            EnvelopeError::ChecksumMismatch
        );
    }

    #[test]
    fn envelope_wrong_fingerprint_and_signature_fail() {
        let key = test_key(&TEST_SEED_A);
        let other = test_key(&TEST_SEED_B);
        let digest = test_digest(0xA5);
        let envelope = encode_envelope(&key, &digest);

        let wrong_fp = fingerprint(&other.verifying_key().to_bytes());
        assert_eq!(
            verify_envelope(&envelope, &digest, &wrong_fp, ALGORITHM_NAME_ED25519PH).unwrap_err(),
            EnvelopeError::FingerprintMismatch
        );

        let mut tampered = digest;
        tampered[0] ^= 1;
        assert_eq!(
            verify_envelope(
                &envelope,
                &tampered,
                &fingerprint(&key.verifying_key().to_bytes()),
                ALGORITHM_NAME_ED25519PH
            )
            .unwrap_err(),
            EnvelopeError::SignatureInvalid
        );

        // A mismatched expected-algorithm name fails closed before any
        // cryptographic check (r_glm finding: cover the branch).
        assert_eq!(
            verify_envelope(
                &envelope,
                &digest,
                &fingerprint(&key.verifying_key().to_bytes()),
                "ecdsa"
            )
            .unwrap_err(),
            EnvelopeError::AlgorithmMismatch
        );
    }

    #[test]
    fn malformed_verifying_key_maps_to_signature_invalid() {
        // The key-parse arm of the verify chain is reachable only with a
        // checksum-consistent envelope whose key bytes are not a valid
        // compressed Edwards point. This is the recomputed-checksum
        // hand-craft that reaches it; mapping a malformed key to
        // `SignatureInvalid` (no dedicated variant) is intentional until
        // a second algorithm joins, bd rc-5u0jx item (b).
        let key = test_key(&TEST_SEED_A);
        let digest = test_digest(0xA5);
        let envelope = encode_envelope(&key, &digest);

        // Precondition: the chosen key bytes are a rejected encoding, so
        // `VerifyingKey::from_bytes` fails inside `verify_envelope`.
        // y=2: x² = (y²−1)/(d·y²+1) is a non-square, so decompression
        // fails (unlike [0xff; 32], which is a valid curve point).
        let mut bad_key = [0u8; 32];
        bad_key[0] = 2;
        assert!(
            VerifyingKey::from_bytes(&bad_key).is_err(),
            "the chosen key bytes must be rejected by from_bytes"
        );

        // Overwrite the key field and RECOMPUTE the envelope checksum
        // over the new key: without the recompute the envelope dies at
        // the checksum step, never reaching the key parse.
        let signature_bytes: [u8; 64] = envelope[SIGNATURE_OFFSET..CHECKSUM_OFFSET]
            .try_into()
            .expect("signature field is 64 bytes");
        let mut crafted = envelope;
        crafted[VERIFYING_KEY_OFFSET..SIGNATURE_OFFSET].copy_from_slice(&bad_key);
        crafted[CHECKSUM_OFFSET..TERMINAL_MAGIC_OFFSET].copy_from_slice(&envelope_checksum(
            ENVELOPE_VERSION,
            ALGORITHM_ED25519PH,
            0,
            &bad_key,
            &signature_bytes,
        ));

        // The expected fingerprint is derived from the bad key itself, so
        // the fingerprint step passes and the first failing step is
        // exactly the key parse: SignatureInvalid.
        assert_eq!(
            verify_envelope(
                &crafted,
                &digest,
                &fingerprint(&bad_key),
                ALGORITHM_NAME_ED25519PH
            )
            .unwrap_err(),
            EnvelopeError::SignatureInvalid
        );
    }

    #[test]
    fn fingerprint_format_is_stable() {
        let key = test_key(&TEST_SEED_A);
        // Committed value: BLAKE3 over the 32-byte verifying key derived
        // from the synthetic seed above. Public material, not a secret.
        assert_eq!(
            fingerprint(&key.verifying_key().to_bytes()),
            "blake3:2297f4e80497ea78c81426d568ec0ec917a925a54ab3beaeabafa9ac7e7deba9"
        );
    }

    #[test]
    fn load_signing_key_rejects_wrong_size() {
        let dir = tempfile::tempdir().expect("tempdir");
        for size in [31usize, 33] {
            let path = dir.path().join(format!("seed-{size}.key"));
            std::fs::write(&path, vec![0x42; size]).expect("write seed file");
            let err = load_signing_key(&path).unwrap_err();
            assert_eq!(
                err,
                EnvelopeError::BadKeyFile {
                    path: path.clone(),
                    size
                }
            );
            let message = err.to_string();
            assert!(
                message.contains(&format!("is {size} bytes")),
                "error must name the actual size: {message}"
            );
            assert!(
                message.contains(&format!("{path:?}")),
                "error must name the path: {message}"
            );
        }
    }
}
