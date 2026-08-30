package com.rustcamel.bench;

import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.assertEquals;

/**
 * Canonical-array parity test (OpenSpec change {@code
 * bench-missing-cells} task 2.4). Golden literal recomputed with
 * python3 (json.dumps separators=(",",":")) and cross-checked against
 * the scenario README's golden table and the Rust fixtures'
 * {@code split_aggregate_array_golden}. If this test is red, this
 * fixture would feed a different payload than every other contender in
 * the scenario and the cell is invalid.
 */
class CanonicalArrayTest {

    /// The canonical 100-item array is exactly 591 bytes.
    @Test
    void canonicalArrayLength() {
        assertEquals(591, BenchRoute.canonicalArray().length());
        assertEquals(BenchRoute.CANONICAL_ARRAY_BYTES, BenchRoute.canonicalArray().length());
    }

    /// Golden digest — SHA-256 of the UTF-8 bytes of
    /// ["b0","b1",...,"b99"] (compact, no whitespace).
    @Test
    void goldenDigest() {
        assertEquals(
                "123444b475c48473309ed966eb69896c6725429021a5a5d2e0eaa0a77a159316",
                BenchRoute.sha256Hex(BenchRoute.canonicalArray()));
    }
}
