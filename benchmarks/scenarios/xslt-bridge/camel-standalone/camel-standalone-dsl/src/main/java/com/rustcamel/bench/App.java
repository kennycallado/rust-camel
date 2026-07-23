package com.rustcamel.bench;

import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardOpenOption;
import java.time.Instant;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicLong;

import org.apache.camel.CamelContext;
import org.apache.camel.Exchange;
import org.apache.camel.builder.RouteBuilder;
import org.apache.camel.impl.DefaultCamelContext;
import org.apache.camel.impl.event.RouteStartedEvent;
import org.apache.camel.main.Main;
import org.apache.camel.spi.CamelEvent;
import org.apache.camel.support.EventNotifierSupport;

/**
 * Pair A entrypoint — hardcoded Java-DSL route for the T4a XSLT bridge
 * scenario (bd Task 4). Apache Camel runs Saxon-HE 12.5 IN-PROCESS
 * (no bridge subprocess); rust-camel's T4a fixture delegates the same
 * XSLT to bridges/xml via gRPC mTLS, paying the bridge tax that this
 * cell measures.
 *
 * Route shape (spec §4.2 / T4a brief, identical across all 4 artifacts):
 *   from("timer:bench?period=10ms&repeatCount=10000")
 *     .setBody(constant(<1KB XML payload read from shared/bench-payload.xml>))
 *     .process(set exchange property "BenchStart" = System.nanoTime())
 *     .to("xslt:<abs path to shared/identity-transform.xsl>")
 *     .process(emit "BENCH_LATENCY <id> <duration_ns>" to tmpfs file)
 *
 * # Marker mechanism
 * Per spec §4.6 + §4.10: the M1 marker is listener-bound (NOT first-tick).
 * For this timer-driven route there is no listener; the marker is
 * emitted by an EventNotifierSupport on RouteStarted (Task 3 pattern)
 * — fires after the timer consumer is registered and the route is
 * fully started. For T4a/T4b specifically, the harness also performs
 * a bridge readiness probe (first warmup tick must produce output
 * within 30s) per spec §4.6 — but that's a harness concern, not a
 * fixture concern. This fixture just emits the marker on RouteStarted.
 *
 * # No self-instrumentation
 * Per the v1/v2/T3 harness invariant, this process does not self-time
 * the M1 measurement — timing is captured externally by the harness's
 * `time -v` wrapper. The only in-band signals are the
 * BENCH_ROUTE_READY marker (1× at startup) and the BENCH_LATENCY
 * per-tick records (consumed by the M2 Protocol B harness).
 *
 * # Workload pinning
 * The 1KB XML payload and the XSLT stylesheet are byte-identical
 * across all 4 T4a artifacts. Paths are read from system properties
 * (-Dbench.payload=... -Dbench.stylesheet=...) so the smoke + harness
 * runs can inject the worktree-relative path without rebuilding.
 */
public final class App {
    private App() {
    }

    public static void main(String[] args) throws Exception {
        // Resolve shared asset paths from system properties. Defaults
        // match the smoke test's CWD-relative layout (scenario dir).
        Path payloadPath = Path.of(System.getProperty(
                "bench.payload",
                "../../shared/bench-payload.xml"));
        Path stylesheetPath = Path.of(System.getProperty(
                "bench.stylesheet",
                "../../shared/identity-transform.xsl"));
        // Per-tick latency records append to this file; the harness
        // reads it after the measurement window closes (Protocol B).
        Path latencyFile = Path.of(System.getProperty(
                "bench.latency_file",
                "/tmp/v3-protocol-b-t4a-camel-standalone-dsl.log"));
        // Pre-load the payload as a constant String so we don't pay
        // file I/O on every tick (workload pinning = byte content,
        // not file reads). The route's setBody(constant(...)) uses
        // this string verbatim.
        String payload = Files.readString(payloadPath, StandardCharsets.UTF_8);

        // Pre-create / truncate the latency file so the first append
        // is fast (file already exists, no inode allocation on hot path).
        Files.createDirectories(latencyFile.getParent());
        Files.writeString(latencyFile, "",
                StandardOpenOption.CREATE, StandardOpenOption.TRUNCATE_EXISTING);

        CamelContext context = new DefaultCamelContext();

        AtomicBoolean markerEmitted = new AtomicBoolean(false);
        AtomicLong tickCounter = new AtomicLong(0);

        // Marker: emit BENCH_ROUTE_READY on RouteStarted (1× only).
        context.getManagementStrategy().addEventNotifier(
            new EventNotifierSupport() {
                @Override
                public void notify(CamelEvent event) {
                    if (markerEmitted.get()) {
                        return;
                    }
                    if (event instanceof RouteStartedEvent) {
                        if (markerEmitted.compareAndSet(false, true)) {
                            long unixMs = Instant.now().toEpochMilli();
                            // System.out (NOT SLF4J) so the harness's
                            // stdout-grep sees the raw marker with no
                            // formatting noise.
                            System.out.println("BENCH_ROUTE_READY " + unixMs);
                        }
                    }
                }

                @Override
                public boolean isEnabled(CamelEvent event) {
                    return event.getType() == CamelEvent.Type.RouteStarted;
                }
            }
        );

        context.addRoutes(new RouteBuilder() {
            @Override
            public void configure() {
                from("timer:bench?period=10ms&repeatCount=10000")
                    .routeId("bench-xslt")
                    .setBody(constant(payload))
                    // WORKAROUND: Camel 4.8.0 + JDK 21 ships a broken
                    // TypeConverter for String -> Source that returns
                    // a java.lang.Class object instead of a Source
                    // instance (ClassCastException at
                    // XmlSourceHandlerFactoryImpl:136). Wrapping the
                    // body as a StreamSource via process() short-
                    // circuits the converter chain (body instanceof
                    // Source returns at the top of getSource()).
                    // Workload is preserved: Saxon-HE 12.5 still
                    // parses the same XML bytes; the wrapper is
                    // µs-level vs ms-scale transform.
                    .process(exchange -> {
                        String xml = exchange.getIn().getBody(String.class);
                        exchange.getIn().setBody(
                            new javax.xml.transform.stream.StreamSource(
                                new java.io.StringReader(xml)));
                    })
                    // Records t_start immediately before the .to("xslt:...")
                    // route step. Long (boxed) so it round-trips through
                    // exchange property type erasure.
                    .process(exchange -> {
                        exchange.setProperty("BenchStart", System.nanoTime());
                    })
                    .to("xslt:file://" + stylesheetPath.toAbsolutePath())
                    .process(exchange -> {
                        long id = tickCounter.incrementAndGet();
                        long tEnd = System.nanoTime();
                        Long tStart = exchange.getProperty("BenchStart", Long.class);
                        long durationNs = tEnd - tStart;
                        String line = "BENCH_LATENCY " + id + " " + durationNs + "\n";
                        // Synchronous append to tmpfs: tmpfs write is
                        // sub-microsecond, no syscall hot-path concern
                        // at 100 ticks/sec. Pre-created above so the
                        // file exists; we just append.
                        try {
                            Files.writeString(latencyFile, line,
                                    StandardCharsets.UTF_8,
                                    StandardOpenOption.APPEND);
                        } catch (Exception e) {
                            // Logging would also perturb timing;
                            // swallow and continue (the harness
                            // detects missing records on its own).
                        }
                    })
                    // The log step is here purely so SLF4J captures
                    // a per-tick breadcrumb visible in the test
                    // artifact — not load-bearing for measurement.
                    .log("BENCH_XSLT_TICK id=${header.CamelTimerCounter}");
            }
        });

        context.start();

        Runtime.getRuntime().addShutdownHook(new Thread(() -> {
            try {
                context.stop();
            } catch (Exception e) {
                // Ignore — shutting down.
            }
        }));

        try {
            Thread.currentThread().join();
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
        }
    }
}
