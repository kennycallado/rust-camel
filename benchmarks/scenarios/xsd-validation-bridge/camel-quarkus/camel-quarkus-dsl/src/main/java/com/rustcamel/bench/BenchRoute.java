package com.rustcamel.bench;

import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardOpenOption;
import java.time.Instant;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicLong;

import jakarta.enterprise.context.ApplicationScoped;
import jakarta.inject.Inject;

import org.apache.camel.CamelContext;
import org.apache.camel.builder.RouteBuilder;
import org.apache.camel.impl.event.RouteStartedEvent;
import org.apache.camel.spi.CamelEvent;
import org.apache.camel.support.EventNotifierSupport;

/**
 * T4b XSD validation bridge route + marker emitter (Pair A — hardcoded
 * Java DSL) for camel-quarkus-dsl (bd Task 4). Same route shape as the
 * standalone T4b fixture: timer → setBody(1KB XML) →
 * validator(schema.xsd) → process(emit BENCH_LATENCY).
 *
 * Marker + per-tick patterns mirror the standalone App.java; see that
 * file's docstring for the full spec rationale (§4.6, §4.8, §4.10).
 */
@ApplicationScoped
public class BenchRoute extends RouteBuilder {

    @Inject
    CamelContext context;

    private static final AtomicBoolean MARKER_EMITTED = new AtomicBoolean(false);
    private static final AtomicLong TICK_COUNTER = new AtomicLong(0);

    @Override
    public void configure() {
        Path payloadPath = Path.of(System.getProperty(
                "bench.payload",
                "../../../shared/bench-payload.xml"));
        Path schemaPath = Path.of(System.getProperty(
                "bench.schema",
                "../../../shared/schema.xsd"));
        Path latencyFile = Path.of(System.getProperty(
                "bench.latency_file",
                "/tmp/v3-protocol-b-t4b-camel-quarkus-dsl-native.log"));

        final String payload;
        try {
            payload = Files.readString(payloadPath, StandardCharsets.UTF_8);
            Files.createDirectories(latencyFile.getParent());
            Files.writeString(latencyFile, "",
                    StandardOpenOption.CREATE, StandardOpenOption.TRUNCATE_EXISTING);
        } catch (Exception e) {
            throw new RuntimeException("failed to initialize bench assets", e);
        }

        context.getManagementStrategy().addEventNotifier(
            new EventNotifierSupport() {
                @Override
                public void notify(CamelEvent event) {
                    if (MARKER_EMITTED.get()) {
                        return;
                    }
                    if (event instanceof RouteStartedEvent) {
                        if (MARKER_EMITTED.compareAndSet(false, true)) {
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

        from("timer:bench?period=10&repeatCount=10000")
            .routeId("bench-xsd")
            .setBody(constant(payload))
            // WORKAROUND: same Camel 4.8.0 + JDK 21 body-conversion
            // bug as T4a (see xslt-bridge BenchRoute for full note).
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
                long id = TICK_COUNTER.incrementAndGet();
                long tEnd = System.nanoTime();
                Long tStart = exchange.getProperty("BenchStart", Long.class);
                long durationNs = tEnd - tStart;
                String line = "BENCH_LATENCY " + id + " " + durationNs + "\n";
                try {
                    Files.writeString(latencyFile, line,
                            StandardCharsets.UTF_8,
                            StandardOpenOption.APPEND);
                } catch (Exception e) {
                    // Swallow.
                }
            })
            .log("BENCH_XSD_TICK id=${header.CamelTimerCounter}");
    }
}
