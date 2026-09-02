package com.rustcamel.bench;

import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardOpenOption;
import java.security.MessageDigest;
import java.security.NoSuchAlgorithmException;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicLong;
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
 * <p>The route's custom steps (set the canonical 100-item array,
 * bracket the tick, assert the aggregated completion, gate the marker,
 * write the per-tick latency record) are bound as named beans; the
 * route STRUCTURE stays in the parsed YAML file, which is the property
 * Pair B measures. The list-append aggregation strategy is referenced
 * from YAML via {@code #class:} (documented Camel 4 XML/YAML form —
 * stateless, no registry lookup needed). Kept in this module only:
 * Pair A's classpath carries no beans, no routes.yaml.
 *
 * <p>Marker contract: one {@code BENCH_ROUTE_READY items=<n>} line,
 * emitted only from the aggregator's completion path; an assert failure
 * kills the route before the marker (cell fails). No
 * self-instrumentation for cold-start — the harness owns the clock from
 * outside.
 *
 * <p>Tick mode (OpenSpec change {@code bench-consol-tick} task 2.4,
 * mirroring the dsl module and the lib crate's task 2.2): repeating
 * warm timer {@code timer:bench?period=10&repeatCount=10000&delay=0} (timer
 * URI lives in routes.yaml); the canonical array is prebuilt ONCE
 * (digest logged once at startup) and set per exchange by the {@code
 * buildArray} bean; the tick start is carried ROUTE-LOCALLY
 * (AtomicLong nanoTime slot — exchange-scoped state does not survive
 * the split+aggregate boundary, lib lesson task 2.2); the {@code
 * writeLatency} bean on the MAIN (timer) route appends {@code
 * BENCH_LATENCY <id> <duration_ns>} per tick to the {@code
 * BENCH_LATENCY_FILE} path (env read once at startup; canonical
 * fallback matches the M2 protocol-B reader's path). The aggregator
 * consumer route is NOT instrumented (blessed contract, tasks.md 2.3
 * ruling); its {@code emitMarker} bean is latched to the FIRST
 * completed bucket — exactly one marker line per process lifetime.
 */
public final class AppYaml {
    private AppYaml() {
    }

    public static void main(String[] args) throws Exception {
        final String array = canonicalArray();
        if (array.length() != CANONICAL_ARRAY_BYTES) {
            throw new IllegalStateException(
                    "split-aggregate array length " + array.length()
                            + " != expected " + CANONICAL_ARRAY_BYTES);
        }
        System.out.println("BENCH_INPUT_SHA256=" + sha256Hex(array));

        // Tick-mode latency sink — truncate at startup (no
        // stale records leak across runs).
        final Path latencyFile = Path.of(latencyFilePath());
        Files.createDirectories(latencyFile.getParent());
        Files.writeString(latencyFile, "",
                StandardOpenOption.CREATE, StandardOpenOption.TRUNCATE_EXISTING);

        final AtomicBoolean markerEmitted = new AtomicBoolean(false);
        final AtomicLong tickCounter = new AtomicLong(0);
        final AtomicLong tickStartNanos = new AtomicLong(0);

        Main main = new Main();
        main.bind("buildArray", setCanonicalArray(array));
        main.bind("markStart", markStart(tickStartNanos));
        main.bind("assertCompletion", assertCompletion());
        main.bind("emitMarker", emitMarker(markerEmitted));
        main.bind("writeLatency", writeLatency(latencyFile, tickCounter, tickStartNanos));
        main.configure()
                .withRoutesIncludePattern("classpath:routes.yaml");
        main.run(args);
    }

    /// Sets the prebuilt canonical array (built + asserted + digest
    /// logged ONCE at startup in main). Per exchange this is the
    /// {@code setBody(constant(...))} equivalent for the YAML route.
    static Processor setCanonicalArray(String array) {
        return exchange -> exchange.getMessage().setBody(array);
    }

    /// Bracket step: stores t_start in the ROUTE-LOCAL nanoTime slot —
    /// exchange-scoped state does not survive the split+aggregate
    /// boundary (lib lesson, task 2.2); ticks are sequential (10 ms
    /// period vs a sub-millisecond pipeline) so the shared slot cannot
    /// straddle ticks.
    static Processor markStart(AtomicLong tickStartNanos) {
        return exchange -> tickStartNanos.set(System.nanoTime());
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

    /// Marker step — fires ONLY from the aggregator's completion path
    /// (right after the completion assert), latched to the FIRST
    /// completed bucket: tick mode completes one bucket per tick, the
    /// marker contract is exactly one line.
    static Processor emitMarker(AtomicBoolean markerEmitted) {
        return exchange -> {
            if (markerEmitted.compareAndSet(false, true)) {
                System.out.println("BENCH_ROUTE_READY items="
                        + exchange.getProperty(AGGREGATED_SIZE_PROPERTY));
            }
        };
    }

    /// Trailing latency step of the MAIN (timer) route — appends
    /// `BENCH_LATENCY <id> <duration_ns>` to the sink file per tick
    /// (the Protocol-B warm record), reading the route-local start
    /// slot. Write failures are swallowed — the harness detects
    /// missing records.
    static Processor writeLatency(
            Path latencyFile, AtomicLong tickCounter, AtomicLong tickStartNanos) {
        return exchange -> {
            long id = tickCounter.incrementAndGet();
            long durationNs = System.nanoTime() - tickStartNanos.get();
            String line = "BENCH_LATENCY " + id + " " + durationNs + "\n";
            try {
                Files.writeString(latencyFile, line,
                        StandardCharsets.UTF_8, StandardOpenOption.APPEND);
            } catch (Exception e) {
                // Swallow — harness detects missing records.
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

    /// Tick-mode latency sink path: `BENCH_LATENCY_FILE` env when set,
    /// else the canonical harness path the M2 protocol-B reader derives
    /// for this cell (`${cell//\//_}` of
    /// `split-aggregate/camel-standalone-yaml`) — the harness argv for
    /// this cell is bare, so the default is what makes the reader find
    /// the log (mirrors the lib crate, task 2.2).
    static String latencyFilePath() {
        String env = System.getenv("BENCH_LATENCY_FILE");
        if (env == null || env.isBlank()) {
            return "/tmp/v3-protocol-b-split-aggregate_camel-standalone-yaml.log";
        }
        return env.trim();
    }

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
