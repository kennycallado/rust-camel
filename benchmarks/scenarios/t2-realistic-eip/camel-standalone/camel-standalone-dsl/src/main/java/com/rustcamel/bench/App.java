package com.rustcamel.bench;

import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardOpenOption;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicLong;
import org.apache.camel.builder.RouteBuilder;
import org.apache.camel.main.Main;

/**
 * Pair A entrypoint (hardcoded Java-DSL route) for the T2 scenario
 * (bd rc-p9ki Task 3). Mirrors the v1 `startup-minimal` App.java
 * (at {@code benchmarks/scenarios/startup-minimal/camel-standalone/
 * camel-standalone-dsl/src/main/java/com/rustcamel/bench/App.java})
 * but implements the spec §4.1 T2 route: timer -> set_body ->
 * set_header -> filter(simple) -> choice.when(simple)/otherwise ->
 * log with body interpolation.
 *
 * <p>The T2 marker {@code BENCH_ROUTE_READY body=pong-bench} carries
 * the post-choice body so a wrong-branch run (otherwise -> {@code
 * pong-other}) is observable, not silent. The harness greps stdout
 * for this exact string.
 *
 * <p>Tick mode (OpenSpec change {@code bench-consol-tick} task 2.4,
 * mirroring the consolidated lib crate's task 2.2 and the
 * xsd-validation-bridge DSL reference): repeating warm timer {@code
 * timer:bench?period=10&repeatCount=10000&delay=0}; the marker keeps its
 * code-path position (after the choice, carrying the post-choice body)
 * but is latched to the FIRST completed exchange — exactly one marker
 * line per process lifetime. A {@code BenchStart} exchange property
 * brackets each exchange BEFORE set_body (the same bracket position
 * the lib crate's t2-realistic-eip branch uses); the trailing step
 * appends {@code BENCH_LATENCY <id> <duration_ns>} to the {@code
 * BENCH_LATENCY_FILE} path (env read once at startup; the canonical
 * fallback matches the M2 protocol-B reader's {@code ${cell//\//_}}
 * path, so the bare harness argv needs no env wiring).
 *
 * <p>No self-instrumentation for cold-start -- per the final arbitrated
 * measurement design (v1 plan, e_gpt), timing and RSS are captured by
 * the harness from OUTSIDE this process; the per-tick latency writer is
 * the Protocol-B warm record, not self-timing.
 */
public final class App {
    private App() {
    }

    public static void main(String[] args) throws Exception {
        // Tick-mode latency sink — truncate at startup (no
        // stale records leak across runs).
        final Path latencyFile = Path.of(latencyFilePath());
        Files.createDirectories(latencyFile.getParent());
        Files.writeString(latencyFile, "",
                StandardOpenOption.CREATE, StandardOpenOption.TRUNCATE_EXISTING);

        Main main = new Main();
        final AtomicBoolean markerEmitted = new AtomicBoolean(false);
        final AtomicLong tickCounter = new AtomicLong(0);
        main.configure().addRoutesBuilder(new RouteBuilder() {
            @Override
            public void configure() {
                from("timer:bench?period=10&repeatCount=10000&delay=0")
                        // Records t_start at route entry, BEFORE set_body
                        // (same bracket position as the lib crate's
                        // t2-realistic-eip branch). Long (boxed) so it
                        // round-trips through exchange property type
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
                        // Marker fires on the FIRST completed exchange
                        // only — tick mode repeats this step per tick,
                        // the marker contract is exactly one line. The
                        // printed body is the post-choice body, so a
                        // wrong-branch run stays observable.
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
        });
        main.run(args);
    }

    /// Tick-mode latency sink path: `BENCH_LATENCY_FILE` env when set,
    /// else the canonical harness path the M2 protocol-B reader derives
    /// for this cell (`${cell//\//_}` of
    /// `t2-realistic-eip/camel-standalone-dsl`) — the harness argv for
    /// this cell is bare, so the default is what makes the reader find
    /// the log (mirrors the lib crate, task 2.2).
    static String latencyFilePath() {
        String env = System.getenv("BENCH_LATENCY_FILE");
        if (env == null || env.isBlank()) {
            return "/tmp/v3-protocol-b-t2-realistic-eip_camel-standalone-dsl.log";
        }
        return env.trim();
    }
}
