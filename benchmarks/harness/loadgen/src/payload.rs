//! Payload construction for the bench harness (OpenSpec change
//! `bench-missing-cells`, tasks 1.1 + 1.2). Two payload kinds:
//! (1) the transport axis — deterministic pattern bodies for
//! Protocol A / M3 throughput, and (2) the canonical JSON document
//! builders + golden digests that Protocol-B fixtures (task 2.1+)
//! consume for byte-equivalence across contenders.
//!
//! v1–v3 pinned the wire payload at a fixed tiny literal (`"ping"` for
//! Protocol A, `"bench"` for M3 throughput). The payload-size axis lets
//! the harness re-measure selected matrix cells with 1 KiB / 32 KiB /
//! 256 KiB / 1 MiB bodies while every run without `--payload-size`
//! keeps the exact legacy behavior.
//!
//! This module is pure logic (no I/O, no async): the valid-size set,
//! the CLI-facing validation, and the body builder. Unit-testable in
//! isolation; the runtime wiring lives in `cli.rs` / `cli_runtime.rs`.

use sha2::{Digest, Sha256};

/// Payload sizes (bytes) exercised by the transport payload axis.
pub const VALID_PAYLOAD_SIZES: [usize; 4] = [1024, 32768, 262144, 1048576];

/// Validate a `--payload-size` value against [`VALID_PAYLOAD_SIZES`].
///
/// Returns the size on success; on rejection, the error message names
/// all four valid sizes so the CLI usage error is self-describing.
pub fn validate_payload_size(size: usize) -> Result<usize, String> {
    if VALID_PAYLOAD_SIZES.contains(&size) {
        Ok(size)
    } else {
        Err(format!(
            "invalid payload size {size}: must be one of {} (bytes)",
            VALID_PAYLOAD_SIZES
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ))
    }
}

/// Build a transport body of exactly `size` bytes.
///
/// The fill byte is `b'b'` — a fixed, compressible-hostile-enough
/// literal whose SHA-256 per size is pinned by the unit tests, so any
/// accidental change to the wire payload shows up in CI.
pub fn transport_body(size: usize) -> Vec<u8> {
    vec![b'b'; size]
}

/// Canonical JSON body prefix: the object opening plus `id` and the
/// start of `seq` (zero whitespace, fixed field order `id`,`seq`,`fill`).
const CANONICAL_PREFIX: &str = "{\"id\":\"bench\",\"seq\":";
/// Canonical JSON infix between the `seq` value and the fill string.
const CANONICAL_FILL_INFIX: &str = ",\"fill\":\"";
/// Canonical JSON suffix closing the fill string and the object.
const CANONICAL_SUFFIX: &str = "\"}";

/// Tick used by fixtures when emitting the canonical self-test body
/// (OpenSpec change `bench-missing-cells`, task 2.1 consumers).
pub const CANONICAL_SELFTEST_TICK: u64 = 0;

/// Build the canonical JSON benchmark body of exactly `size` bytes.
///
/// Shape: `{"id":"bench","seq":<tick>,"fill":"<K×'b'>"}` — UTF-8, zero
/// whitespace, field order `id`,`seq`,`fill`, `<tick>` as unpadded
/// decimal. `K = size - overhead` where overhead is the prefix, the
/// tick digits, the fill infix, and the suffix, so the serialized
/// document is exactly `size` bytes. This is the T2 (t2-json) input
/// contract: every runtime builds the identical bytes for a given
/// (size, tick), verified against the golden digests in the tests.
///
/// Panics if `size` is too small to hold the overhead for `tick`
/// (the payload axis starts at 1024 bytes, far above the overhead).
pub fn canonical_json_body(size: usize, tick: u64) -> String {
    let tick_str = tick.to_string();
    let overhead = CANONICAL_PREFIX.len()
        + tick_str.len()
        + CANONICAL_FILL_INFIX.len()
        + CANONICAL_SUFFIX.len();
    assert!(
        size >= overhead,
        "canonical JSON body needs at least {overhead} bytes for tick {tick}, got {size}"
    );
    let fill_len = size - overhead;
    let mut body = String::with_capacity(size);
    body.push_str(CANONICAL_PREFIX);
    body.push_str(&tick_str);
    body.push_str(CANONICAL_FILL_INFIX);
    body.push_str(&"b".repeat(fill_len));
    body.push_str(CANONICAL_SUFFIX);
    body
}

/// SHA-256 hex digest of the canonical body for `(size, tick)`.
///
/// Fixtures log `BENCH_INPUT_SHA256=<digest>` before processing so a
/// run's input bytes can be verified post-hoc against this pure
/// function.
pub fn canonical_body_sha256(size: usize, tick: u64) -> String {
    sha256_hex(canonical_json_body(size, tick).as_bytes())
}

/// Lowercase hex SHA-256 of `data`.
fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SHA-256 of `b'b' * size` for each entry of [`VALID_PAYLOAD_SIZES`],
    /// generated once via python3 hashlib and pinned here so any change
    /// to the wire payload fails CI.
    #[test]
    fn transport_body_exact_sizes_and_goldens() {
        let goldens = [
            "0c66f2c45405de575189209a768399bcaf88ccc51002407e395c0136aad2844d",
            "1234e2b34a2f7303e44a65c546423b14c2f7983aad3a0e9cd4a9a37b1f576324",
            "9e240eace59e902546b5c777cec8b8c20017915d2e0ec85580d5cc7b586da7dd",
            "e56ec8dc1862be6c09c53620cbc0f00f639de2a51c882745fbbc4e144714b3c2",
        ];
        for (size, golden) in VALID_PAYLOAD_SIZES.iter().zip(goldens) {
            let body = transport_body(*size);
            assert_eq!(body.len(), *size, "body length must equal size {size}");
            assert!(
                body.iter().all(|&b| b == b'b'),
                "body for size {size} must be all 'b' bytes"
            );
            let hex = sha256_hex(&body);
            assert_eq!(hex, golden, "sha-256 mismatch for size {size}");
        }
    }

    /// Sizes outside the axis set are rejected, and every rejection
    /// message names all four valid sizes (the CLI surfaces it verbatim
    /// as a usage error).
    #[test]
    fn validate_payload_size_rejects_others() {
        for bad in [2048usize, 0, 5_000_000] {
            let err = validate_payload_size(bad)
                .err()
                .unwrap_or_else(|| panic!("size {bad} must be rejected"));
            for valid in VALID_PAYLOAD_SIZES {
                assert!(
                    err.contains(&valid.to_string()),
                    "error for {bad} must name valid size {valid}: {err}"
                );
            }
        }
    }

    /// Golden table for the canonical JSON body (task 1.2): digests
    /// generated once via python3 hashlib over the exact
    /// `canonical_json_body` formula and pinned as literals. Test-only
    /// per the task; scenario READMEs (task 2.1) copy these values.
    const GOLDEN_CANONICAL_DIGESTS: [(usize, u64, &str); 5] = [
        (
            1024,
            CANONICAL_SELFTEST_TICK,
            "5abe5f00068356cad4e72f4d5e5e0a5d15d4a5cc9df8d0f22e22bf1448891b0f",
        ),
        (
            32768,
            CANONICAL_SELFTEST_TICK,
            "a0db69e1146a29b0b25ca22435e51f39e271ecb1ac4ec1cee0ead3212eae10e9",
        ),
        (
            262144,
            CANONICAL_SELFTEST_TICK,
            "02adf20f21dc63217c9dc2e26b82101f96dbf311af5fbbf86e818e63d7171e27",
        ),
        (
            1048576,
            CANONICAL_SELFTEST_TICK,
            "9d4da9b244b6d12bed15d624ce426099da3126422285ecc584b9d3fff93a3abd",
        ),
        (
            32768,
            7,
            "995f33e2cb370cdd8179ca80a49f921ec48af1d6558ee23f5b98d8e67624f1f8",
        ),
    ];

    /// For each golden entry the body is exactly `size` bytes, keeps the
    /// canonical prefix, fills with `'b'` only, and hashes to the pinned
    /// digest. Tick-0 entries use [`CANONICAL_SELFTEST_TICK`] so
    /// the const itself is exercised. Any mismatch names the (size, tick)
    /// pair.
    #[test]
    fn canonical_json_exact_sizes_and_digests() {
        for &(size, tick, golden) in &GOLDEN_CANONICAL_DIGESTS {
            let body = canonical_json_body(size, tick);
            let label = format!("(size={size}, tick={tick})");
            assert_eq!(body.len(), size, "{label}: length must equal size");
            assert!(
                body.starts_with("{\"id\":\"bench\",\"seq\":"),
                "{label}: wrong canonical prefix"
            );
            let fill_start =
                "{\"id\":\"bench\",\"seq\":".len() + tick.to_string().len() + ",\"fill\":\"".len();
            let fill = &body[fill_start..body.len() - 2];
            assert!(
                !fill.is_empty() && fill.bytes().all(|b| b == b'b'),
                "{label}: fill must be non-empty and all 'b'"
            );
            assert!(body.ends_with("\"}"), "{label}: wrong canonical suffix");
            assert_eq!(
                canonical_body_sha256(size, tick),
                golden,
                "{label}: sha-256 mismatch"
            );
        }
    }

    /// The K formula must hold with a multi-digit tick: the serialized
    /// document is exactly `size` bytes even when the decimal tick adds
    /// more digits than the self-test value.
    #[test]
    fn canonical_json_k_formula_exactness() {
        let size = 1_048_576;
        let tick = 123_456_789;
        let body = canonical_json_body(size, tick);
        assert_eq!(
            body.len(),
            size,
            "multi-digit tick must not break exact sizing"
        );
    }
}
