package com.rustcamel.bench;

import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardOpenOption;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicLong;
import org.apache.camel.Processor;
import org.apache.camel.main.Main;

/**
 * Pair B entrypoint for the T2 scenario (bd rc-p9ki Task 3). Mirrors
 * the v1 `startup-minimal` AppYaml.java (at
 * {@code benchmarks/scenarios/startup-minimal/camel-standalone/camel-
 * standalone-yaml/src/main/java/com/rustcamel/bench/AppYaml.java})
 * but loads the T2 routes.yaml which implements the spec §4.1 T2
 * route: timer -> set_body -> set_header -> filter -> choice ->
 * log. Same logical route as {@code App.java} but authored in YAML
 * and parsed at runtime via {@code camel-yaml-dsl}.
 *
 * <p>Pair B is language-subsystem-equivalent to rust-camel-cli
 * (both sides use {@code ${body}} / {@code ${header.X}} Simple —
 * see {@code rust-camel-cli/routes/t2-realistic-eip.yaml}). Pair A
 * (rust-camel-lib) is NOT language-subsystem-equivalent (closure
 * predicates — see that fixture's source comment).
 *
 * <p>Tick mode (OpenSpec change {@code bench-consol-tick} task 2.4,
 * mirroring the dsl module and the lib crate's task 2.2): repeating
 * warm timer {@code timer:bench?period=10&repeatCount=10000&delay=0} (timer
 * URI lives in routes.yaml); the tick-mode steps (bracket the tick,
 * gate the marker, write the per-tick latency record) are bound as
 * named beans ({@code markStart} / {@code emitMarker} / {@code
 * writeLatency}) so the route STRUCTURE stays in the parsed YAML
 * file. The marker is latched to the FIRST completed exchange —
 * exactly one {@code BENCH_ROUTE_READY body=pong-bench} line per
 * process lifetime — and the {@code writeLatency} bean appends
 * {@code BENCH_LATENCY <id> <duration_ns>} to the {@code
 * BENCH_LATENCY_FILE} path per tick (env read once at startup;
 * canonical fallback matches the M2 protocol-B reader's path).
 */
public final class AppYaml {
    private AppYaml() {
    }

    public static void main(String[] args) throws Exception {
        // Tick-mode latency sink — truncate at startup (no
        // stale records leak across runs).
        final Path latencyFile = Path.of(latencyFilePath());
        Files.createDirectories(latencyFile.getParent());
        Files.writeString(latencyFile, "",
                StandardOpenOption.CREATE, StandardOpenOption.TRUNCATE_EXISTING);

        final AtomicBoolean markerEmitted = new AtomicBoolean(false);
        final AtomicLong tickCounter = new AtomicLong(0);

        Main main = new Main();
        main.bind("markStart", markStart());
        main.bind("emitMarker", emitMarker(markerEmitted));
        main.bind("writeLatency", writeLatency(latencyFile, tickCounter));
        main.configure()
                .withRoutesIncludePattern("classpath:routes.yaml");
        main.run(args);
    }

    /// Bracket step: records t_start at route entry, BEFORE set_body
    /// (same bracket position as the dsl module and the lib crate's
    /// t2-realistic-eip branch). Long (boxed) so it round-trips
    /// through exchange property type erasure.
    static Processor markStart() {
        return exchange ->
                exchange.setProperty("BenchStart", System.nanoTime());
    }

    /// Marker step — fires on the FIRST completed exchange only (tick
    /// mode repeats the route per tick; the marker contract is exactly
    /// one line). Prints the post-choice body, so a wrong-branch run
    /// (otherwise -> {@code pong-other}) stays observable.
    static Processor emitMarker(AtomicBoolean markerEmitted) {
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
    static Processor writeLatency(Path latencyFile, AtomicLong tickCounter) {
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
    /// `t2-realistic-eip/camel-standalone-yaml`) — the harness argv for
    /// this cell is bare, so the default is what makes the reader find
    /// the log (mirrors the lib crate, task 2.2).
    static String latencyFilePath() {
        String env = System.getenv("BENCH_LATENCY_FILE");
        if (env == null || env.isBlank()) {
            return "/tmp/v3-protocol-b-t2-realistic-eip_camel-standalone-yaml.log";
        }
        return env.trim();
    }
}
