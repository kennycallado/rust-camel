package com.rustcamel.bench;

import java.nio.charset.StandardCharsets;
import java.security.MessageDigest;
import java.security.NoSuchAlgorithmException;
import jakarta.enterprise.context.ApplicationScoped;
import jakarta.enterprise.inject.Produces;
import jakarta.inject.Named;
import org.apache.camel.Exchange;
import org.apache.camel.Processor;

/**
 * Named bean producers for the split-aggregate YAML routes of
 * camel-quarkus-yaml (Pair B, OpenSpec change bench-missing-cells task
 * 2.4). The route STRUCTURE lives in camel/routes.yaml (parsed at
 * runtime — the property Pair B measures); the two custom steps resolve
 * these CDI-produced beans via `process: ref:`. The list-append
 * aggregation strategy is NOT a CDI bean — the YAML route references
 * {@link ListAppendStrategy} via the documented `#class:` form.
 *
 * <p>The canonical-array builder/assert logic is identical to the dsl
 * sibling's BenchRoute (same formula, same golden digest) — the two
 * subprojects are separate build artifacts by design (pairing
 * classpath isolation), so the small per-family duplication is
 * deliberate.
 *
 * <p>Marker contract: one {@code BENCH_ROUTE_READY items=<n>} line,
 * emitted only from the aggregator's completion path; an assert failure
 * kills the route before the marker (cell fails). No
 * self-instrumentation — the harness owns the clock from outside.
 */
@ApplicationScoped
public class BenchBeans {

    @Produces
    @Named("buildArray")
    Processor buildArray() {
        return buildArrayBean();
    }

    @Produces
    @Named("assertCompletion")
    Processor assertCompletion() {
        return assertCompletionBean();
    }

    /// Builds the canonical 100-item JSON array and logs
    /// `BENCH_INPUT_SHA256=<digest>` before any splitting. The length
    /// assert fires here so a builder regression kills the cell before
    /// the marker.
    static Processor buildArrayBean() {
        return exchange -> {
            String array = canonicalArray();
            if (array.length() != CANONICAL_ARRAY_BYTES) {
                throw new IllegalStateException(
                        "split-aggregate array length " + array.length()
                                + " != expected " + CANONICAL_ARRAY_BYTES);
            }
            System.out.println("BENCH_INPUT_SHA256=" + sha256Hex(array));
            exchange.getMessage().setBody(array);
        };
    }

    /// Completion assert — the aggregated collection holds exactly 100
    /// items AND the aggregator-stamped `CamelAggregatedSize` property
    /// agrees (README completion contract). Any failure throws — route
    /// dies, no marker.
    static Processor assertCompletionBean() {
        return exchange -> {
            Integer aggregated = exchange.getProperty(AGGREGATED_SIZE_PROPERTY, Integer.class);
            java.util.List<?> items = exchange.getMessage().getBody(java.util.List.class);
            int size = items == null ? -1 : items.size();
            if (aggregated == null || aggregated != BENCH_ITEMS) {
                throw new IllegalStateException(
                        "split-aggregate CamelAggregatedSize " + aggregated
                                + " != expected " + BENCH_ITEMS);
            }
            if (size != BENCH_ITEMS) {
                throw new IllegalStateException(
                        "split-aggregate aggregated list size " + size
                                + " != expected " + BENCH_ITEMS);
            }
        };
    }

    /// Number of split fragments / aggregation bucket size (README
    /// canonical route).
    static final int BENCH_ITEMS = 100;

    /// Aggregator-stamped completion property (Apache Camel core
    /// constant `Exchange.AGGREGATED_SIZE`).
    static final String AGGREGATED_SIZE_PROPERTY = "CamelAggregatedSize";

    /// Canonical array size in bytes (["b0",...,"b99"]).
    static final int CANONICAL_ARRAY_BYTES = 591;

    /// Canonical array builder — same formula as the Rust fixtures'
    /// `split_aggregate_array_golden`: 100 items `b0`..`b99`, compact
    /// JSON (no whitespace), exactly 591 bytes.
    static String canonicalArray() {
        StringBuilder sb = new StringBuilder(CANONICAL_ARRAY_BYTES).append('[');
        for (int i = 0; i < BENCH_ITEMS; i++) {
            if (i > 0) {
                sb.append(',');
            }
            sb.append('"').append('b').append(i).append('"');
        }
        return sb.append(']').toString();
    }

    /// Lowercase hex SHA-256 of the UTF-8 bytes of `data`.
    static String sha256Hex(String data) {
        try {
            MessageDigest md = MessageDigest.getInstance("SHA-256");
            byte[] digest = md.digest(data.getBytes(StandardCharsets.UTF_8));
            StringBuilder sb = new StringBuilder(digest.length * 2);
            for (byte b : digest) {
                sb.append(Character.forDigit((b >> 4) & 0xF, 16));
                sb.append(Character.forDigit(b & 0xF, 16));
            }
            return sb.toString();
        } catch (NoSuchAlgorithmException e) {
            throw new IllegalStateException("SHA-256 MessageDigest unavailable", e);
        }
    }
}
