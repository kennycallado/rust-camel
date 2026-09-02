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

/// Item count of the split-aggregate canonical array (scenario
/// `split-aggregate`, OpenSpec change `bench-missing-cells`).
pub const SPLIT_AGGREGATE_ITEMS: usize = 100;

/// Build the split-aggregate canonical input body: a compact JSON
/// array of [`SPLIT_AGGREGATE_ITEMS`] string items, item `i` being
/// `"b<i>"`, serialized with zero whitespace — exactly what
/// `serde_json::to_string` produces for that value (591 bytes).
/// Byte-identical across every split-aggregate fixture: the rust
/// fixture pins the same construction in
/// `benchmarks/contenders/rust-camel-lib/src/scenarios/split-aggregate.rs`
/// (`canonical_split_array`), the JVM fixtures in their
/// `CanonicalArrayTest` goldens.
pub fn canonical_split_aggregate_array() -> String {
    let items: Vec<String> = (0..SPLIT_AGGREGATE_ITEMS)
        .map(|i| format!("\"b{i}\""))
        .collect();
    format!("[{}]", items.join(","))
}

/// Era-default canonical input size (bytes) for scenario `t2-json`
/// when the payload class is `shared` — the 32 KiB entry of the
/// golden canonical digest table (`GOLDEN_CANONICAL_DIGESTS`).
pub const T2_JSON_DEFAULT_PAYLOAD_SIZE: usize = 32768;

/// Canonical input digest for a `(scenario, payload-class)` pair.
///
/// The CLI-facing `payload-digest` subcommand maps `--scenario` +
/// `--payload-class` onto the same inputs the fixtures use. Class
/// `shared` — the harness's record vocabulary for every cell — maps
/// to the scenario's DEFAULT canonical input: for `t2-json` the
/// canonical JSON body at [`T2_JSON_DEFAULT_PAYLOAD_SIZE`] with the
/// self-test tick ([`CANONICAL_SELFTEST_TICK`]); for
/// `split-aggregate` the canonical 100-item array
/// ([`canonical_split_aggregate_array`]). Numeric classes are also
/// accepted for `t2-json` (payload-size axis runs) and select the
/// canonical body at that byte size. Any other scenario or class
/// yields `Err`; the CLI prints it on stderr and exits 2.
pub fn scenario_payload_digest(scenario: &str, payload_class: &str) -> Result<String, String> {
    match scenario {
        "t2-json" => {
            if payload_class == "shared" {
                return Ok(canonical_body_sha256(
                    T2_JSON_DEFAULT_PAYLOAD_SIZE,
                    CANONICAL_SELFTEST_TICK,
                ));
            }
            let size = payload_class.parse::<usize>().map_err(|_| {
                format!(
                    "unknown payload class {payload_class:?}: t2-json classes are \
                     \"shared\" or byte sizes {}",
                    VALID_PAYLOAD_SIZES
                        .iter()
                        .map(|s| s.to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })?;
            let size = validate_payload_size(size)?;
            Ok(canonical_body_sha256(size, CANONICAL_SELFTEST_TICK))
        }
        "split-aggregate" => {
            if payload_class != "shared" {
                return Err(format!(
                    "unknown payload class {payload_class:?}: split-aggregate classes are \"shared\""
                ));
            }
            Ok(sha256_hex(canonical_split_aggregate_array().as_bytes()))
        }
        other => Err(format!(
            "unknown scenario {other:?}: payload-digest supports t2-json and \
             split-aggregate (the scenarios whose fixtures build payload.rs \
             canonical bodies)"
        )),
    }
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

    /// Every tick-0 golden digest is reachable through the
    /// scenario→class mapping the `payload-digest` CLI uses, so the CLI
    /// output equals the pinned goldens (e.g. (32768,0) = `a0db69e1…`).
    #[test]
    fn scenario_payload_digest_matches_goldens() {
        for &(size, tick, golden) in &GOLDEN_CANONICAL_DIGESTS {
            if tick != CANONICAL_SELFTEST_TICK {
                continue;
            }
            let class = size.to_string();
            assert_eq!(
                scenario_payload_digest("t2-json", &class).unwrap(),
                golden,
                "scenario digest mismatch for class {class}"
            );
        }
    }

    /// `shared` maps to each scenario's DEFAULT canonical input —
    /// the mapping summarize.py relies on (it passes
    /// `--payload-class shared` for every cell): t2-json → the 32 KiB
    /// era-default golden `a0db69e1…`, which must equal the numeric
    /// `32768` class; split-aggregate → the canonical array golden
    /// `123444b4…`.
    #[test]
    fn scenario_payload_digest_shared_maps_to_scenario_default() {
        const T2_JSON_32768_GOLDEN: &str =
            "a0db69e1146a29b0b25ca22435e51f39e271ecb1ac4ec1cee0ead3212eae10e9";
        const SPLIT_AGGREGATE_GOLDEN: &str =
            "123444b475c48473309ed966eb69896c6725429021a5a5d2e0eaa0a77a159316";
        assert_eq!(
            scenario_payload_digest("t2-json", "shared").unwrap(),
            T2_JSON_32768_GOLDEN,
            "t2-json/shared must be the era-default 32768 golden"
        );
        assert_eq!(
            scenario_payload_digest("t2-json", "shared").unwrap(),
            scenario_payload_digest("t2-json", "32768").unwrap(),
            "shared must equal the era-default numeric class"
        );
        assert_eq!(
            scenario_payload_digest("split-aggregate", "shared").unwrap(),
            SPLIT_AGGREGATE_GOLDEN,
            "split-aggregate/shared must be the canonical array golden"
        );
    }

    /// Unknown scenarios and invalid classes are rejected (CLI:
    /// stderr + exit 2). The unknown-scenario error keeps its
    /// `unknown scenario` prefix — summarize.py detects it to record
    /// `input_sha256: null`. The t2-json unknown-class error names
    /// `shared` plus every valid size (self-describing usage error);
    /// numeric classes outside the axis set are rejected too.
    #[test]
    fn scenario_payload_digest_rejects_unknown() {
        for (scenario, class) in [
            ("startup-minimal", "shared"),
            ("http-server", "shared"),
            ("split-aggregate", "32768"),
        ] {
            let err = scenario_payload_digest(scenario, class)
                .err()
                .unwrap_or_else(|| panic!("({scenario}, {class}) must be rejected"));
            if matches!(scenario, "startup-minimal" | "http-server") {
                assert!(
                    err.starts_with("unknown scenario"),
                    "({scenario}, {class}): error must keep the `unknown scenario` prefix: {err}"
                );
            }
        }
        let err = scenario_payload_digest("t2-json", "garbage")
            .err()
            .unwrap_or_else(|| panic!("t2-json/garbage must be rejected"));
        assert!(
            err.contains("\"shared\""),
            "unknown-class error must name shared: {err}"
        );
        for valid in VALID_PAYLOAD_SIZES {
            assert!(
                err.contains(&valid.to_string()),
                "unknown-class error must name valid size {valid}: {err}"
            );
        }
        assert!(scenario_payload_digest("t2-json", "2048").is_err());
        let err = scenario_payload_digest("t2-json", "2048").expect_err("2048 must be rejected");
        for valid in VALID_PAYLOAD_SIZES {
            assert!(
                err.contains(&valid.to_string()),
                "error must name valid size {valid}: {err}"
            );
        }
    }

    /// Golden for the split-aggregate canonical array (change
    /// `bench-missing-cells`): exactly 591 bytes, items `b0`..`b99`,
    /// digest pinned in the scenario README + smoke evidence. Also
    /// pins the single-serialization invariant: re-serializing the
    /// parsed array with serde_json reproduces the exact same bytes.
    #[test]
    fn split_aggregate_array_digest_golden() {
        const GOLDEN: &str = "123444b475c48473309ed966eb69896c6725429021a5a5d2e0eaa0a77a159316";
        let body = canonical_split_aggregate_array();
        assert_eq!(body.len(), 591, "canonical array must be 591 bytes");
        assert!(
            body.starts_with("[\"b0\",\"b1\","),
            "canonical array must start with quoted items b0, b1"
        );
        assert!(
            body.ends_with("\"b98\",\"b99\"]"),
            "canonical array must end with quoted items b98, b99"
        );
        assert_eq!(sha256_hex(body.as_bytes()), GOLDEN, "digest drift");
        let parsed: serde_json::Value = serde_json::from_str(&body)
            .unwrap_or_else(|e| panic!("canonical array must be valid JSON: {e}"));
        let items = parsed
            .as_array()
            .unwrap_or_else(|| panic!("canonical array must parse as a JSON array"));
        assert_eq!(items.len(), SPLIT_AGGREGATE_ITEMS);
        for (i, item) in items.iter().enumerate() {
            assert_eq!(
                item,
                &serde_json::Value::String(format!("b{i}")),
                "item {i} drifted"
            );
        }
        let reserialized = serde_json::to_string(&parsed)
            .unwrap_or_else(|e| panic!("re-serialization failed: {e}"));
        assert_eq!(
            reserialized, body,
            "re-serialization must be byte-identical"
        );
        // Reachable through the scenario→class mapping the CLI uses.
        assert_eq!(
            scenario_payload_digest("split-aggregate", "shared").unwrap(),
            GOLDEN,
            "scenario digest mismatch for split-aggregate/shared"
        );
    }
}
