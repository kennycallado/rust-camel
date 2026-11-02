//! Cross-version compatibility for the detached Ed25519ph envelope
//! (ADR-0083): a frozen envelope produced with ed25519-dalek 2.2.0 must
//! verify and re-sign byte-identically under 3.x.
//!
//! The fixture under `tests/fixtures/ed25519ph_envelope_dalek_2_2.hex`
//! carries its own provenance header and regeneration recipe. The seed
//! and prehash below are the public synthetic inputs that recipe uses;
//! the fixture file is the committed cross-version contract.

use camel_cli::compile::signature::{self, EnvelopeError};
use ed25519_dalek::SigningKey;

/// Public synthetic seed (never key material), documented in the fixture.
const FIXTURE_SEED: [u8; 32] = *b"r4sign-compat-fixture-seed-00000";

/// Fixed 64-byte SHA-512 prehash fed to the Ed25519ph signing API.
const FIXTURE_PREHASH: [u8; 64] =
    *b"r4sign-mission-490-family-3-prehash-fixture-0123456789abcdefghij";

const FIXTURE: &str = include_str!("fixtures/ed25519ph_envelope_dalek_2_2.hex");

// Envelope field offsets from the documented 148-byte layout.
const VERIFYING_KEY_OFFSET: usize = 12;
const SIGNATURE_OFFSET: usize = 44;
const CHECKSUM_OFFSET: usize = 108;
const TERMINAL_MAGIC_OFFSET: usize = 140;

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(out, "{byte:02x}").expect("writing to a String cannot fail");
    }
    out
}

fn unhex(text: &str) -> Vec<u8> {
    assert!(text.len().is_multiple_of(2), "hex text has an even length");
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).expect("fixture is valid hex"))
        .collect()
}

/// Read one `name = hex` field from the fixture, skipping `#` comments.
fn fixture_field(name: &str) -> Vec<u8> {
    let value = FIXTURE
        .lines()
        .filter_map(|line| line.split_once('='))
        .find(|(key, _)| key.trim() == name)
        .map(|(_, value)| value.trim())
        .unwrap_or_else(|| panic!("fixture carries the {name} field"));
    unhex(value)
}

fn fixture_envelope() -> [u8; signature::ENVELOPE_LEN] {
    fixture_field("envelope")
        .try_into()
        .expect("fixture envelope is 148 bytes")
}

fn fixture_prehash() -> [u8; 64] {
    fixture_field("prehash")
        .try_into()
        .expect("fixture prehash is 64 bytes")
}

fn fixture_public_key() -> [u8; 32] {
    fixture_field("public_key")
        .try_into()
        .expect("fixture public key is 32 bytes")
}

fn fixture_signature() -> [u8; 64] {
    fixture_field("signature")
        .try_into()
        .expect("fixture signature is 64 bytes")
}

/// Recompute the envelope BLAKE3 trailer so a tampered signature reaches
/// the Ed25519 check instead of dying at the integrity step.
fn envelope_checksum(envelope: &[u8; signature::ENVELOPE_LEN]) -> [u8; 32] {
    let mut input = Vec::with_capacity(32 + 1 + 2 + 1 + 1 + 32 + 64);
    input.extend_from_slice(b"rust-camel-signature-v1");
    input.push(0);
    input.extend_from_slice(&envelope[8..10]);
    input.push(envelope[10]);
    input.push(envelope[11]);
    input.extend_from_slice(&envelope[VERIFYING_KEY_OFFSET..SIGNATURE_OFFSET]);
    input.extend_from_slice(&envelope[SIGNATURE_OFFSET..CHECKSUM_OFFSET]);
    *blake3::hash(&input).as_bytes()
}

/// Regenerate the fixture from the current ed25519-dalek version. The
/// result is version-independent by contract: run it on a 2.2.0 checkout
/// to reproduce the frozen fixture, or on 3.x to prove the vector is
/// unchanged. Writes `tests/fixtures/ed25519ph_envelope_dalek_2_2.hex`.
#[test]
#[ignore = "slow test: fixture regeneration helper, overwrites the fixture file; run explicitly with --ignored"]
fn regenerate_fixture() {
    let key = SigningKey::from_bytes(&FIXTURE_SEED);
    let envelope = signature::encode_envelope(&key, &FIXTURE_PREHASH);
    let verifying_key = key.verifying_key().to_bytes();
    let signed = envelope[SIGNATURE_OFFSET..CHECKSUM_OFFSET].to_vec();
    let contents = format!(
        "# Provenance: generated with ed25519-dalek 2.2.0 via encode_envelope at\n\
         # mission-490 family-3, generator steps: seed -> SigningKey::from_bytes ->\n\
         # sign_prehashed(RawPrehash(prehash), None) -> encode_envelope.\n\
         #\n\
         # Regeneration: on any ed25519-dalek version (2.2.0 or 3.x), from the\n\
         # workspace root run:\n\
         #   cargo test -p camel-cli --test signing_compat_test \\\n\
         #     regenerate_fixture -- --ignored --exact\n\
         # The 148-byte envelope is byte-identical across versions by contract;\n\
         # a diff means the prehashed plumbing changed and ADR-0083 is at risk.\n\
         #\n\
         # Public synthetic inputs, never key material.\n\
         seed = {seed}\n\
         prehash = {prehash}\n\
         public_key = {public_key}\n\
         signature = {signature}\n\
         envelope = {envelope}\n",
        seed = hex(&FIXTURE_SEED),
        prehash = hex(&FIXTURE_PREHASH),
        public_key = hex(&verifying_key),
        signature = hex(&signed),
        envelope = hex(&envelope),
    );
    std::fs::write(
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/ed25519ph_envelope_dalek_2_2.hex"
        ),
        contents,
    )
    .expect("write fixture");
}

#[test]
fn fixture_inputs_are_stable() {
    assert_eq!(FIXTURE_SEED.len(), 32);
    assert_eq!(FIXTURE_PREHASH.len(), 64);
    assert_eq!(fixture_prehash(), FIXTURE_PREHASH);
    assert_eq!(fixture_field("seed"), FIXTURE_SEED);
    assert_eq!(fixture_envelope().len(), signature::ENVELOPE_LEN);
}

#[test]
fn fixture_envelope_from_dalek_2_verifies_under_3() {
    let prehash = fixture_prehash();
    let public_key = fixture_public_key();
    let envelope = fixture_envelope();
    let expected_fingerprint = signature::fingerprint(&public_key);

    // (a) The frozen 2.2.0 envelope verifies under the current version.
    let verified = signature::verify_envelope(
        &envelope,
        &prehash,
        &expected_fingerprint,
        signature::ALGORITHM_NAME_ED25519PH,
    )
    .expect("the frozen 2.2.0 envelope verifies under 3.x");
    assert_eq!(verified.fingerprint, expected_fingerprint);

    // (b) Re-signing the fixture prehash under the current version is
    // byte-identical to the frozen envelope, including the BLAKE3 trailer.
    let key = SigningKey::from_bytes(&FIXTURE_SEED);
    assert_eq!(key.verifying_key().to_bytes(), public_key);
    let resigned = signature::encode_envelope(&key, &prehash);
    assert_eq!(
        resigned, envelope,
        "the 148-byte envelope must be byte-identical across ed25519-dalek versions"
    );

    // (c) Tamper rejection. Flip one signature byte and repair the BLAKE3
    // trailer so the failure is the Ed25519 check itself, not integrity.
    let mut tampered_signature = envelope;
    tampered_signature[SIGNATURE_OFFSET] ^= 1;
    let repaired = envelope_checksum(&tampered_signature);
    tampered_signature[CHECKSUM_OFFSET..TERMINAL_MAGIC_OFFSET].copy_from_slice(&repaired);
    assert_eq!(
        signature::verify_envelope(
            &tampered_signature,
            &prehash,
            &expected_fingerprint,
            signature::ALGORITHM_NAME_ED25519PH,
        )
        .unwrap_err(),
        EnvelopeError::SignatureInvalid
    );

    // Flip one prehash byte: the envelope stays intact and the Ed25519
    // verification rejects it.
    let mut tampered_prehash = prehash;
    tampered_prehash[0] ^= 1;
    assert_eq!(
        signature::verify_envelope(
            &envelope,
            &tampered_prehash,
            &expected_fingerprint,
            signature::ALGORITHM_NAME_ED25519PH,
        )
        .unwrap_err(),
        EnvelopeError::SignatureInvalid
    );

    // The committed fixture's signature field matches the envelope bytes.
    assert_eq!(
        fixture_signature(),
        envelope[SIGNATURE_OFFSET..CHECKSUM_OFFSET]
    );
}
