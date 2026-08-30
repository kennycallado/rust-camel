package com.rustcamel.bench;

import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.assertEquals;

/**
 * Canonical-body parity test (OpenSpec change {@code bench-missing-cells}
 * task 2.2). Golden literals from bench-loadgen's task-1.2 table — the
 * same values the Rust fixtures assert. If this test is red, this
 * fixture would feed a different payload than every other contender in
 * the scenario and the cell is invalid.
 */
class CanonicalBodyTest {

    /// (32768, 0) golden — the harness default class (bytes=32781).
    @Test
    void goldenDigest32768Tick0() {
        String body = AppYaml.canonicalJsonBody(32768, AppYaml.CANONICAL_SELFTEST_TICK);
        assertEquals(32768, body.length());
        assertEquals(
                "a0db69e1146a29b0b25ca22435e51f39e271ecb1ac4ec1cee0ead3212eae10e9",
                AppYaml.sha256Hex(body));
    }

    /// (1024, 0) golden — the per-class smoke variant (bytes=1037).
    @Test
    void goldenDigest1024Tick0() {
        String body = AppYaml.canonicalJsonBody(1024, AppYaml.CANONICAL_SELFTEST_TICK);
        assertEquals(1024, body.length());
        assertEquals(
                "5abe5f00068356cad4e72f4d5e5e0a5d15d4a5cc9df8d0f22e22bf1448891b0f",
                AppYaml.sha256Hex(body));
    }
}
