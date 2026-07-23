package com.rustcamel.bench;

import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardOpenOption;
import java.time.Instant;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicLong;

import org.apache.camel.CamelContext;
import org.apache.camel.builder.RouteBuilder;
import org.apache.camel.impl.DefaultCamelContext;
import org.apache.camel.impl.event.RouteStartedEvent;
import org.apache.camel.main.Main;
import org.apache.camel.spi.CamelEvent;
import org.apache.camel.support.EventNotifierSupport;

/**
 * Pair A entrypoint — hardcoded Java-DSL route for the T4b XSD
 * validation bridge scenario (bd Task 4). Apache Camel runs
 * Xerces-J 2.12.2 IN-PROCESS (no bridge subprocess); rust-camel's
 * T4b fixture delegates the same validation to bridges/xml via gRPC
 * mTLS, paying the bridge tax that this cell measures.
 *
 * Route shape (spec §4.2 / T4b brief, identical across all 4 artifacts):
 *   from("timer:bench?period=10&repeatCount=10000")
 *     .setBody(constant(<1KB XML payload>))
 *     .process(set exchange property "BenchStart" = System.nanoTime())
 *     .to("validator:<abs path to shared/schema.xsd>")
 *     .process(emit "BENCH_LATENCY <id> <duration_ns>" to tmpfs file)
 *
 * Marker + per-tick patterns mirror the T4a App.java; see that file's
 * docstring for the full spec rationale (§4.6, §4.8, §4.10).
 */
public final class App {
    private App() {
    }

    public static void main(String[] args) throws Exception {
        Path payloadPath = Path.of(System.getProperty(
                "bench.payload",
                "../../shared/bench-payload.xml"));
        Path schemaPath = Path.of(System.getProperty(
                "bench.schema",
                "../../shared/schema.xsd"));
        Path latencyFile = Path.of(System.getProperty(
                "bench.latency_file",
                "/tmp/v3-protocol-b-t4b-camel-standalone-dsl.log"));
        String payload = Files.readString(payloadPath, StandardCharsets.UTF_8);

        Files.createDirectories(latencyFile.getParent());
        Files.writeString(latencyFile, "",
                StandardOpenOption.CREATE, StandardOpenOption.TRUNCATE_EXISTING);

        CamelContext context = new DefaultCamelContext();

        AtomicBoolean markerEmitted = new AtomicBoolean(false);
        AtomicLong tickCounter = new AtomicLong(0);

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
                from("timer:bench?period=10&repeatCount=10000")
                    .routeId("bench-xsd")
                    .setBody(constant(payload))
                    // WORKAROUND: Camel 4.8.0 + JDK 21 ships a broken
                    // TypeConverter for String -> Source that returns
                    // a java.lang.Class object instead of a Source
                    // instance (ClassCastException / ExpectedBodyType-
                    // Exception at validator's body-conversion step).
                    // Wrapping the body as a StreamSource via process()
                    // short-circuits the converter chain. Workload is
                    // preserved: Xerces-J 2.12.2 still parses the same
                    // XML bytes.
                    .process(exchange -> {
                        String xml = exchange.getIn().getBody(String.class);
                        exchange.getIn().setBody(
                            new javax.xml.transform.stream.StreamSource(
                                new java.io.StringReader(xml)));
                    })
                    // Records t_start immediately before the .to("validator:...")
                    // route step. Long (boxed) so it round-trips through
                    // exchange property type erasure.
                    .process(exchange -> {
                        exchange.setProperty("BenchStart", System.nanoTime());
                    })
                    .to("validator:file://" + schemaPath.toAbsolutePath())
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
                    })
                    .log("BENCH_XSD_TICK id=${header.CamelTimerCounter}");
            }
        });

        context.start();

        Runtime.getRuntime().addShutdownHook(new Thread(() -> {
            try {
                context.stop();
            } catch (Exception e) {
                // Ignore.
            }
        }));

        try {
            Thread.currentThread().join();
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
        }
    }
}
