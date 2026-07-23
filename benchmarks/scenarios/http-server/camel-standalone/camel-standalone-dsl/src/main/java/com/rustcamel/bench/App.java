package com.rustcamel.bench;

import org.apache.camel.CamelContext;
import org.apache.camel.builder.RouteBuilder;
import org.apache.camel.impl.DefaultCamelContext;
import org.apache.camel.impl.event.RouteStartedEvent;
import org.apache.camel.spi.CamelEvent;

import java.util.concurrent.atomic.AtomicBoolean;

/**
 * Pair A entrypoint — hardcoded Java-DSL route for the T3 HTTP
 * server scenario (bd Task 3). Mirrors the v1 / T2 App.java
 * shape (hardcoded route, no YAML/DSL parsing on the classpath)
 * but implements the canonical minimal T3 route:
 *   from("jetty:http://0.0.0.0:8080/bench")
 *   .setBody(constant("pong"))
 *
 * The `jetty:` component is Apache Camel's server-side HTTP
 * component in 4.x. `setBody(constant("pong"))` returns 200 +
 * body=pong + Content-Type=text/plain per the spec contract.
 *
 * # Marker mechanism (HARD T3 entrance criterion)
 * Per spec §4.10: "M1 marker is listener-bound AND accepting
 * connections (NOT on first request)". The Task 1 review
 * explicitly upgraded this to a hard T3 entrance criterion
 * (Task 1 report "Review fixes" → "Important #3"). The marker
 * is emitted by a `RouteStarted` event notifier installed via
 * the Camel context's management strategy — this fires AFTER
 * the `jetty:` consumer has bound the listener and started
 * the server's accept loop (the `camel-jetty` consumer's
 * `start()` awaits `JettyServer.start()` which returns when
 * the listener is bound and ready to accept connections). The
 * notifier is 1:1 coupled to "listener accepting", NOT to
 * first-request. A one-shot `markerEmitted` flag prevents
 * re-emission on supervision restarts.
 *
 * # Per-request work
 * The route is the canonical minimal `setBody("pong")` only —
 * no per-request counter, no process step, no log emission.
 * The harness measures cold-start + RSS, not request latency;
 * per-request observability belongs to the loadgen, not the
 * fixture.
 *
 * # No self-instrumentation
 * Per the v1 / T2 harness invariant, this process does not
 * self-time — timing is captured externally by the harness's
 * `time -v` GNU wrapper. The only in-band signal the harness
 * watches for is the `BENCH_ROUTE_READY` marker line.
 */
public final class App {
    private App() {
    }

    public static void main(String[] args) throws Exception {
        // We build the CamelContext directly (rather than using
        // `new Main()` + `main.run()`) so we can install the
        // marker-emitting event notifier BEFORE the route starts.
        // `Main.getCamelContext()` returns null until `run()` is
        // called (per the camel 4.x main.Main javadoc / source),
        // which makes the notifier-registration-before-run
        // pattern awkward. The direct-context pattern is the
        // canonical camel idiom and is the same shape the
        // v1 / T2 fixtures use internally for `Main` — the
        // difference is that we skip the `Main` helper and
        // own the lifecycle ourselves.
        final AtomicBoolean markerEmitted = new AtomicBoolean(false);

        CamelContext context = new DefaultCamelContext();
        context.getManagementStrategy().addEventNotifier(
            new org.apache.camel.support.EventNotifierSupport() {
                @Override
                public void notify(CamelEvent event) {
                    if (markerEmitted.get()) {
                        return;
                    }
                    if (event instanceof RouteStartedEvent) {
                        if (markerEmitted.compareAndSet(false, true)) {
                            long unixMs = System.currentTimeMillis();
                            // System.out (NOT the SLF4J logger) so
                            // the line lands on the harness's
                            // captured stdout with no formatting
                            // noise. The trailing unix_ms is a
                            // parseable suffix; the harness's
                            // `grep -cF "BENCH_ROUTE_READY"` counts
                            // this line as a match.
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

        // The T3 route — hardcoded Java DSL (Pair A: no YAML
        // route-file parsing). Canonical minimal shape:
        // `from(jetty).setBody(pong)`.
        //
        // `jetty:` is the server-side HTTP component in Apache
        // Camel 4.x. The URI includes the scheme that
        // auto-selects the consumer/producer mode.
        //
        // No method restriction on the consumer: the loadgen
        // drives only POST `/bench`, so restricting it is
        // unnecessary and would diverge from the rust-camel
        // fixtures (which accept any method). All fixtures are
        // identical on this axis.
        //
        // `setBody(constant("pong"))` sets the response body.
        // The default status (200 OK) is implied by normal
        // pipeline completion (per ADR-0024); Content-Type
        // is derived from the Body variant (constant string →
        // text/plain; charset=utf-8 per the camel-jetty
        // contract).
        context.addRoutes(new RouteBuilder() {
            @Override
            public void configure() {
                from("jetty:http://0.0.0.0:8080/bench")
                    .setBody(constant("pong"));
            }
        });

        // Start the context. The jetty consumer binds the
        // listener inside its start() method (camel-jetty
        // delegates to JettyServer.start() which returns when
        // the listener is bound), so when start() returns the
        // listener is accepting. The RouteStarted event then
        // fires, the notifier prints the marker, and the
        // harness's M1 clock stops on the marker line.
        context.start();

        // Block until killed (the harness KILLs the process
        // after the marker is captured and the M1 RSS is
        // recorded).
        Runtime.getRuntime().addShutdownHook(new Thread(() -> {
            try {
                context.stop();
            } catch (Exception e) {
                // Ignore — we're shutting down.
            }
        }));

        // Sleep in a loop; main.run() would do this via
        // MainSupport.waitUntilCompleted(), but we're not
        // using Main so we do it manually.
        try {
            Thread.currentThread().join();
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
        }
    }
}
