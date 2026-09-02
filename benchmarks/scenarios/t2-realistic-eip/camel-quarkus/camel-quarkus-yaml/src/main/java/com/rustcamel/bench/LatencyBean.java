package com.rustcamel.bench;

import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardOpenOption;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicLong;
import jakarta.enterprise.context.ApplicationScoped;
import jakarta.enterprise.inject.Produces;
import jakarta.inject.Named;
import org.apache.camel.Processor;

/**
 * Named bean producers for the t2-realistic-eip YAML route of
 * camel-quarkus-yaml (Pair B, bd rc-p9ki Task 3). The route STRUCTURE
 * lives in camel/routes.yaml (parsed at runtime — the property Pair B
 * measures); the three custom steps resolve these CDI-produced beans
 * via `process: ref:`. This module carried no Java sources before
 * bench-consol-tick — the latency-writer Processor bean lands here
 * mirroring the standalone fixture's AppYaml beans (task 2.4) and the
 * BenchBeans pattern of the t2-json/split-aggregate yaml siblings.
 *
 * <p>Marker contract: one {@code BENCH_ROUTE_READY body=pong-bench}
 * line; the printed body is the post-choice body, so a wrong-branch
 * run (otherwise -> {@code pong-other}) stays observable. No
 * self-instrumentation for cold-start — the harness owns the clock
 * from outside.
 *
 * <p>Tick mode (OpenSpec change {@code bench-consol-tick} task 2.5,
 * mirroring the standalone fixtures' task 2.4): the repeating warm
 * timer URI lives in camel/routes.yaml; the {@code markStart} bean
 * records t_start at route entry, BEFORE set_body (same bracket
 * position as the lib crate's t2-realistic-eip branch), the {@code
 * emitMarker} bean is latched to the FIRST completed exchange (exactly
 * one marker line per process lifetime), and the {@code writeLatency}
 * bean appends {@code BENCH_LATENCY <id> <duration_ns>} per exchange
 * to the {@code BENCH_LATENCY_FILE} path (env read once at startup;
 * the canonical fallback matches the M2 protocol-B reader's path — the
 * sink is truncated at producer time, i.e. before any tick, so no
 * stale records leak across runs).
 */
@ApplicationScoped
public class LatencyBean {

    /// Tick-mode state — the marker latch (exactly one marker line per
    /// process lifetime) and the per-tick record id.
    private final AtomicBoolean markerEmitted = new AtomicBoolean(false);
    private final AtomicLong tickCounter = new AtomicLong(0);

    @Produces
    @Named("markStart")
    Processor markStart() {
        return markStartBean();
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
                    "t2-realistic-eip latency sink truncate failed: " + latencyFile, e);
        }
        return writeLatencyBean(latencyFile, tickCounter);
    }

    /// Bracket step: records t_start at route entry, BEFORE set_body
    /// (same position as the dsl module and the lib crate's
    /// t2-realistic-eip branch). Long (boxed) so it round-trips through
    /// exchange property type erasure.
    static Processor markStartBean() {
        return exchange ->
                exchange.setProperty("BenchStart", System.nanoTime());
    }

    /// Marker step — fires on the FIRST completed exchange only (tick
    /// mode repeats the route per tick; the marker contract is exactly
    /// one line). Prints the post-choice body, so a wrong-branch run
    /// (otherwise -> {@code pong-other}) stays observable.
    static Processor emitMarkerBean(AtomicBoolean markerEmitted) {
        return exchange -> {
            if (markerEmitted.compareAndSet(false, true)) {
                System.out.println("BENCH_ROUTE_READY body="
                        + exchange.getMessage().getBody(String.class));
            }
        };
    }

    /// Trailing latency step — appends `BENCH_LATENCY <id> <duration_ns>`
    /// to the sink file per exchange (the Protocol-B warm record).
    /// Write failures are swallowed — the harness detects missing
    /// records.
    static Processor writeLatencyBean(Path latencyFile, AtomicLong tickCounter) {
        return exchange -> {
            long id = tickCounter.incrementAndGet();
            long tEnd = System.nanoTime();
            Long tStart = exchange.getProperty("BenchStart", Long.class);
            long durationNs = tEnd - tStart;
            String line = "BENCH_LATENCY " + id + " " + durationNs + "\n";
            try {
                Files.writeString(latencyFile, line,
                        StandardCharsets.UTF_8, StandardOpenOption.APPEND);
            } catch (Exception e) {
                // Swallow — harness detects missing records.
            }
        };
    }

    /// Tick-mode latency sink path: `BENCH_LATENCY_FILE` env when set,
    /// else the canonical harness path the M2 protocol-B reader derives
    /// for this cell (`${cell//\//_}` of
    /// `t2-realistic-eip/camel-quarkus-yaml-native`) — the harness argv
    /// for this cell is bare, so the default is what makes a manual
    /// reader find the log (mirrors the standalone fixtures, task 2.4).
    static String latencyFilePath() {
        String env = System.getenv("BENCH_LATENCY_FILE");
        if (env == null || env.isBlank()) {
            return "/tmp/v3-protocol-b-t2-realistic-eip_camel-quarkus-yaml-native.log";
        }
        return env.trim();
    }
}
