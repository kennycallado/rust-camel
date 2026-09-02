// T2 scenario route for camel-quarkus-dsl (JVM, Pair A, bd rc-p9ki Task 3).
// Mirrors the v1 BenchRoute (at
// benchmarks/scenarios/startup-minimal/camel-quarkus/camel-quarkus-dsl/
// src/main/java/com/rustcamel/bench/BenchRoute.java) but implements
// the spec §4.1 T2 route: timer -> setBody -> setHeader -> filter ->
// choice.when/otherwise -> log. The marker `BENCH_ROUTE_READY
// body=pong-bench` carries the post-choice body so a wrong-branch
// run (otherwise -> `pong-other`) is observable, not silent.
//
// Tick mode (OpenSpec change bench-consol-tick task 2.5, mirroring the
// standalone fixtures' task 2.4 and the consolidated lib crate's task
// 2.2): the timer is the repeating warm-tick form
// timer:bench?period=10&repeatCount=10000&delay=0; the marker keeps its
// code-path position (right after the choice) but is latched to the
// FIRST completed exchange — exactly one marker line per process
// lifetime. A BenchStart exchange property brackets each exchange (set
// at route entry, BEFORE set_body — same bracket position as the lib
// crate's t2-realistic-eip branch); the trailing step appends
// `BENCH_LATENCY <id> <duration_ns>` to the BENCH_LATENCY_FILE path
// (env read once at startup; the canonical fallback matches the M2
// protocol-B reader's ${cell//\//_} path).

package com.rustcamel.bench;

import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardOpenOption;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicLong;
import org.apache.camel.builder.RouteBuilder;

public class BenchRoute extends RouteBuilder {
    @Override
    public void configure() {
        // Tick-mode latency sink — truncate at startup (no
        // stale records leak across runs).
        final Path latencyFile = Path.of(latencyFilePath());
        try {
            Files.createDirectories(latencyFile.getParent());
            Files.writeString(latencyFile, "",
                    StandardOpenOption.CREATE, StandardOpenOption.TRUNCATE_EXISTING);
        } catch (IOException e) {
            throw new IllegalStateException(
                    "t2-realistic-eip latency sink truncate failed: " + latencyFile, e);
        }
        final AtomicBoolean markerEmitted = new AtomicBoolean(false);
        final AtomicLong tickCounter = new AtomicLong(0);

        from("timer:bench?period=10&repeatCount=10000&delay=0")
                // Records t_start at route entry, BEFORE set_body (same
                // bracket position as the lib crate's t2-realistic-eip
                // branch and the standalone dsl sibling). Long (boxed)
                // so it round-trips through exchange property type
                // erasure.
                .process(exchange ->
                        exchange.setProperty("BenchStart", System.nanoTime()))
                .setBody(constant("ping"))
                .setHeader("source", constant("bench"))
                .filter(simple("${body} == 'ping'"))
                .choice()
                    .when(simple("${header.source} == 'bench'"))
                        .setBody(constant("pong-bench"))
                    .otherwise()
                        .setBody(constant("pong-other"))
                .endChoice()
                .end()
                // Marker fires on the FIRST completed exchange only —
                // tick mode repeats this step per tick, the marker
                // contract is exactly one line. The printed body is the
                // post-choice body, so a wrong-branch run stays
                // observable.
                .process(exchange -> {
                    if (markerEmitted.compareAndSet(false, true)) {
                        System.out.println("BENCH_ROUTE_READY body="
                                + exchange.getMessage().getBody(String.class));
                    }
                })
                .process(exchange -> {
                    long id = tickCounter.incrementAndGet();
                    long tEnd = System.nanoTime();
                    Long tStart = exchange.getProperty("BenchStart", Long.class);
                    long durationNs = tEnd - tStart;
                    String line = "BENCH_LATENCY " + id + " " + durationNs + "\n";
                    try {
                        Files.writeString(latencyFile, line,
                                StandardCharsets.UTF_8,
                                StandardOpenOption.APPEND);
                    } catch (Exception e) {
                        // Swallow — harness detects missing records.
                    }
                });
    }

    /// Tick-mode latency sink path: `BENCH_LATENCY_FILE` env when set,
    /// else the canonical harness path the M2 protocol-B reader derives
    /// for this cell (`${cell//\//_}` of
    /// `t2-realistic-eip/camel-quarkus-dsl-native`) — the harness argv
    /// for this cell is bare, so the default is what makes a manual
    /// reader find the log (mirrors the standalone fixtures, task 2.4).
    static String latencyFilePath() {
        String env = System.getenv("BENCH_LATENCY_FILE");
        if (env == null || env.isBlank()) {
            return "/tmp/v3-protocol-b-t2-realistic-eip_camel-quarkus-dsl-native.log";
        }
        return env.trim();
    }
}
