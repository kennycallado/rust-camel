package com.rustcamel.bench;

import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardOpenOption;
import java.security.MessageDigest;
import java.security.NoSuchAlgorithmException;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicLong;
import jakarta.enterprise.context.ApplicationScoped;
import jakarta.enterprise.inject.Produces;
import jakarta.inject.Named;
import org.apache.camel.Exchange;
import org.apache.camel.Processor;

/**
 * Named bean producers for the split-aggregate YAML routes of
 * camel-quarkus-yaml (Pair B, OpenSpec change bench-missing-cells task
 * 2.4). The route STRUCTURE lives in camel/routes.yaml (parsed at
 * runtime — the property Pair B measures); the custom steps resolve
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
 * self-instrumentation for cold-start — the harness owns the clock from
 * outside.
 *
 * <p>Tick mode (OpenSpec change {@code bench-consol-tick} task 2.5,
 * mirroring the standalone fixtures' task 2.4): the repeating warm
 * timer URI lives in camel/routes.yaml; the {@code markStart} bean
 * stores t_start in the ROUTE-LOCAL nanoTime slot (exchange-scoped
 * state does not survive the split+aggregate boundary — lib lesson,
 * task 2.2; ticks are sequential — 10 ms period vs a sub-millisecond
 * pipeline — so the shared slot cannot straddle ticks), the {@code
 * writeLatency} bean ends the MAIN (timer) route and appends {@code
 * BENCH_LATENCY <id> <duration_ns>} per tick to the {@code
 * BENCH_LATENCY_FILE} path (env read once at startup; the canonical
 * fallback matches the M2 protocol-B reader's path — the sink is
 * truncated at producer time, i.e. before any tick), and the {@code
 * emitMarker} bean is latched to the FIRST completed bucket, fired
 * only from the aggregator's completion path (exactly one marker line
 * per process lifetime).
 */
@ApplicationScoped
public class BenchBeans {

    /// Tick-mode state — the marker latch (exactly one marker line per
    /// process lifetime), the per-tick record id, and the route-local
    /// t_start slot the markStart/writeLatency beans share.
    private final AtomicBoolean markerEmitted = new AtomicBoolean(false);
    private final AtomicLong tickCounter = new AtomicLong(0);
    private final AtomicLong tickStartNanos = new AtomicLong(0);

    @Produces
    @Named("buildArray")
    Processor buildArray() {
        // Canonical array prebuilt ONCE at producer time (= startup,
        // before the first tick; digest logged once — per-exchange SHA
        // printing would spam 10000 lines), then set per exchange by a
        // quiet bean — the standalone yaml sibling's AppYaml idiom
        // (task 2.4).
        final String array = canonicalArray();
        if (array.length() != CANONICAL_ARRAY_BYTES) {
            throw new IllegalStateException(
                    "split-aggregate array length " + array.length()
                            + " != expected " + CANONICAL_ARRAY_BYTES);
        }
        System.out.println("BENCH_INPUT_SHA256=" + sha256Hex(array));
        return setCanonicalArray(array);
    }

    @Produces
    @Named("markStart")
    Processor markStart() {
        return markStartBean(tickStartNanos);
    }

    @Produces
    @Named("assertCompletion")
    Processor assertCompletion() {
        return assertCompletionBean();
    }

    @Produces
    @Named("emitMarker")
    Processor emitMarker() {
        return emitMarkerBean(markerEmitted);
    }

    @Produces
    @Named("writeLatency")
    Processor writeLatency() {
        // Tick-mode latency sink — truncate at producer time (startup,
        // before any tick; no stale records leak across runs).
        final Path latencyFile = Path.of(latencyFilePath());
        try {
            Files.createDirectories(latencyFile.getParent());
            Files.writeString(latencyFile, "",
                    StandardOpenOption.CREATE, StandardOpenOption.TRUNCATE_EXISTING);
        } catch (IOException e) {
            throw new IllegalStateException(
                    "split-aggregate latency sink truncate failed: " + latencyFile, e);
        }
        return writeLatencyBean(latencyFile, tickCounter, tickStartNanos);
    }

    /// Sets the prebuilt canonical array (built + asserted + digest
    /// logged ONCE at producer time). Per exchange this is the
    /// {@code setBody(constant(...))} equivalent for the YAML route.
    static Processor setCanonicalArray(String array) {
        return exchange -> exchange.getMessage().setBody(array);
    }

    /// Bracket step: stores t_start in the ROUTE-LOCAL nanoTime slot —
    /// exchange-scoped state does not survive the split+aggregate
    /// boundary (lib lesson, task 2.2); ticks are sequential (10 ms
    /// period vs a sub-millisecond pipeline) so the shared slot cannot
    /// straddle ticks.
    static Processor markStartBean(AtomicLong tickStartNanos) {
        return exchange -> tickStartNanos.set(System.nanoTime());
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

    /// Marker step — fires ONLY from the aggregator's completion path
    /// (right after the completion assert), latched to the FIRST
    /// completed bucket: tick mode completes one bucket per tick, the
    /// marker contract is exactly one line.
    static Processor emitMarkerBean(AtomicBoolean markerEmitted) {
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
    static Processor writeLatencyBean(
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
    /// `split-aggregate/camel-quarkus-yaml-native`) — the harness argv
    /// passes the path as a -D system property, so the env fallback is
    /// what makes the reader find the log (mirrors the standalone
    /// fixtures, task 2.4).
    static String latencyFilePath() {
        String env = System.getenv("BENCH_LATENCY_FILE");
        if (env == null || env.isBlank()) {
            return "/tmp/v3-protocol-b-split-aggregate_camel-quarkus-yaml-native.log";
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
