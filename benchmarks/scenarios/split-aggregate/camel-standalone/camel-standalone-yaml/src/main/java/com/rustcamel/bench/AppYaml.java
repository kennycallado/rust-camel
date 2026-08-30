package com.rustcamel.bench;

import java.nio.charset.StandardCharsets;
import java.security.MessageDigest;
import java.security.NoSuchAlgorithmException;
import org.apache.camel.Exchange;
import org.apache.camel.Processor;
import org.apache.camel.main.Main;

/**
 * Pair B entrypoint for the split-aggregate scenario (OpenSpec change
 * {@code bench-missing-cells} task 2.4). Mirrors the t2-json AppYaml
 * pattern but loads the split-aggregate routes.yaml (same logical
 * routes as the dsl-module {@link App}, authored in YAML DSL and
 * parsed at runtime via {@code camel-yaml-dsl}).
 *
 * <p>The route's two custom steps (build the canonical 100-item array
 * + log BENCH_INPUT_SHA256, assert the aggregated completion) are bound
 * as named beans; the route STRUCTURE stays in the parsed YAML file,
 * which is the property Pair B measures. The list-append aggregation
 * strategy is referenced from YAML via {@code #class:} (documented
 * Camel 4 XML/YAML form — stateless, no registry lookup needed). Kept
 * in this module only: Pair A's classpath carries no beans, no
 * routes.yaml.
 *
 * <p>Marker contract: one {@code BENCH_ROUTE_READY items=<n>} line,
 * emitted only from the aggregator's completion path; an assert failure
 * kills the route before the marker (cell fails). No
 * self-instrumentation — the harness owns the clock from outside.
 */
public final class AppYaml {
    private AppYaml() {
    }

    public static void main(String[] args) throws Exception {
        Main main = new Main();
        main.bind("buildArray", buildArray());
        main.bind("assertCompletion", assertCompletion());
        main.configure()
                .withRoutesIncludePattern("classpath:routes.yaml");
        main.run(args);
    }

    /// Builds the canonical 100-item JSON array and logs
    /// `BENCH_INPUT_SHA256=<digest>` before any splitting. The length
    /// assert fires here so a builder regression kills the cell before
    /// the marker.
    static Processor buildArray() {
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
    static Processor assertCompletion() {
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
