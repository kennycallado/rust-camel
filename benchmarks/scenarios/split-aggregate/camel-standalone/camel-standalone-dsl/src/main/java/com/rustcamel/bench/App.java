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
import org.apache.camel.main.Main;

/**
 * Pair A entrypoint (hardcoded Java-DSL route) for the split-aggregate
 * scenario (OpenSpec change {@code bench-missing-cells} task 2.4).
 * Mirrors the t2-json App.java pairing shape (Pair A, no {@code
 * camel-yaml-dsl} on the classpath — classpath isolation is a pairing
 * fairness invariant) but implements the split-aggregate Protocol-B
 * route (design D3): timer -> process(build canonical 100-item array +
 * log BENCH_INPUT_SHA256) -> split(jsonpath "$", sequential — the
 * Splitter is sequential unless an executorService is configured) ->
 * to(direct:agg-in) per fragment; direct:agg-in -> setHeader(constant
 * correlation) -> aggregate(completionSize=100, list-append strategy,
 * forceCompletionOnStop=false) -> process(assert completion) -> log
 * marker.
 *
 * <p>The canonical input is the fixed JSON array {@code
 * ["b0","b1",...,"b99"]} — exactly 591 bytes, byte-identical across
 * every split-aggregate contender (scenario README golden table); its
 * digest is logged before the marker so payload skew is observable.
 *
 * <p>Completion contract (spec): the marker fires ONLY from the
 * aggregator's completion path, and only when the aggregated collection
 * holds exactly {@value #BENCH_ITEMS} items AND the aggregator-stamped
 * {@code CamelAggregatedSize} property agrees. {@code
 * forceCompletionOnStop} stays false — an incomplete bucket emits NO
 * marker and the cell fails by the harness marker deadline. An assert
 * failure throws inside the processor — the route dies BEFORE the
 * marker, so the cell fails.
 *
 * <p>No self-instrumentation — timing and RSS are captured by the
 * harness from OUTSIDE this process. {@code delay=0} on the timer (v1
 * Fix 3) keeps idle wait out of the measurement.
 */
public final class App {
    private App() {
    }

    public static void main(String[] args) throws Exception {
        Main main = new Main();
        main.configure().addRoutesBuilder(new RouteBuilder() {
            @Override
            public void configure() {
                // Outer route (design D3): one tick builds the canonical
                // array and fans it out. The split fragments are the bare
                // strings "b0".."b99"; each fragment exchange is sent to
                // direct:agg-in.
                from("timer:bench?repeatCount=1&delay=0")
                        .process(buildArray())
                        .split(jsonpath("$"))
                        .to("direct:agg-in");

                // Aggregation route: constant correlation key, complete
                // at exactly 100 items. forceCompletionOnStop stays at
                // its DEFAULT false (Camel 4.8's DSL method is no-arg
                // and would SET it true) — an incomplete bucket emits
                // NO marker and the cell fails by the harness marker
                // deadline, spec F2.
                from("direct:agg-in")
                        .setHeader(BENCH_CORRELATION_HEADER, constant(BENCH_CORRELATION))
                        .aggregate(header(BENCH_CORRELATION_HEADER), appendToList())
                        .completionSize(BENCH_ITEMS)
                        .process(assertCompletion())
                        .log("BENCH_ROUTE_READY items=${exchangeProperty.CamelAggregatedSize}");
            }
        });
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
