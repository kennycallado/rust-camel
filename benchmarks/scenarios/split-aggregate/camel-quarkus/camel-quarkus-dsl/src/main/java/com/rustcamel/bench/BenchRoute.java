// split-aggregate scenario route for camel-quarkus-dsl (JVM, Pair A,
// OpenSpec change bench-missing-cells task 2.4). Same pairing shape as
// the t2-json BenchRoute (Pair A, no YAML on the classpath) but
// implements the split-aggregate Protocol-B route (design D3):
// outer route: timer -> process(build canonical 100-item array +
// BENCH_INPUT_SHA256) -> split(jsonpath "$", sequential by default) ->
// to(direct:agg-in) per fragment; agg route: direct:agg-in ->
// setHeader(constant correlation) -> aggregate(completionSize=100,
// list-append strategy, forceCompletionOnStop=false) ->
// process(assert completion) -> log marker. The marker
// `BENCH_ROUTE_READY items=<n>` is the harness grep target and fires
// ONLY from the aggregator's completion path; any assert failure kills
// the route BEFORE the marker so the cell fails loudly.
//
// The canonical input is the fixed JSON array ["b0","b1",...,"b99"] —
// exactly 591 bytes, byte-identical across every split-aggregate
// contender (scenario README golden table).

package com.rustcamel.bench;

import java.nio.charset.StandardCharsets;
import java.security.MessageDigest;
import java.security.NoSuchAlgorithmException;
import java.util.ArrayList;
import java.util.List;
import org.apache.camel.AggregationStrategy;
import org.apache.camel.Exchange;
import org.apache.camel.Processor;
import org.apache.camel.builder.RouteBuilder;

public class BenchRoute extends RouteBuilder {

    @Override
    public void configure() {
        // Outer route (design D3): one tick builds the canonical array
        // and fans it out; fragments are the strings "b0".."b99".
        from("timer:bench?repeatCount=1&delay=0")
                .process(buildArray())
                .split(jsonpath("$"))
                .to("direct:agg-in");

        // Aggregation route: constant correlation key, complete at
        // exactly 100 items. forceCompletionOnStop stays at its DEFAULT
        // false (Camel 4.8's DSL method is no-arg and would SET it
        // true) — an incomplete bucket emits NO marker and the cell
        // fails by the harness marker deadline, spec F2.
        from("direct:agg-in")
                .setHeader(BENCH_CORRELATION_HEADER, constant(BENCH_CORRELATION))
                .aggregate(header(BENCH_CORRELATION_HEADER), appendToList())
                .completionSize(BENCH_ITEMS)
                .process(assertCompletion())
                .log("BENCH_ROUTE_READY items=${exchangeProperty.CamelAggregatedSize}");
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

    /// List-append aggregation strategy (README `collect_all`): the
    /// first fragment seeds a fresh list; every later fragment appends
    /// to the accumulating exchange's body. Defensive copy so exchange
    /// reuse can never alias the accumulated list.
    static AggregationStrategy appendToList() {
        return (oldExchange, newExchange) -> {
            List<Object> items = oldExchange == null
                    ? new ArrayList<>()
                    : new ArrayList<>(oldExchange.getMessage().getBody(List.class));
            items.add(newExchange.getMessage().getBody(String.class));
            Exchange acc = oldExchange == null ? newExchange : oldExchange;
            acc.getMessage().setBody(items);
            return acc;
        };
    }

    /// Completion assert — the aggregated collection holds exactly 100
    /// items AND the aggregator-stamped `CamelAggregatedSize` property
    /// agrees (README completion contract). Any failure throws — route
    /// dies, no marker.
    static Processor assertCompletion() {
        return exchange -> {
            Integer aggregated = exchange.getProperty(AGGREGATED_SIZE_PROPERTY, Integer.class);
            List<?> items = exchange.getMessage().getBody(List.class);
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

    /// Correlation header + constant value (README canonical route).
    static final String BENCH_CORRELATION_HEADER = "bench.correlation";
    static final String BENCH_CORRELATION = "bench";

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
