package com.rustcamel.bench;

import org.apache.camel.CamelContext;
import org.apache.camel.impl.event.RouteStartedEvent;
import org.apache.camel.main.Main;
import org.apache.camel.spi.CamelEvent;

import java.util.concurrent.atomic.AtomicBoolean;

/**
 * Pair B entrypoint — loads the same T3 route as
 * `camel-standalone-dsl/App.java` but authored in
 * `resources/routes.yaml` and parsed at runtime via
 * camel-yaml-dsl. Mirrors the v1 / T2 AppYaml.java shape
 * (YAML route file + explicit routesIncludePattern).
 *
 * # Marker mechanism
 * Same as `camel-standalone-dsl/App.java` — the `EventNotifier`
 * fires on `RouteStartedEvent` and prints the marker line on
 * stdout. The route file is parsed at startup; the
 * `RouteStarted` event fires AFTER the `jetty:` consumer has
 * bound the listener and the route is fully started.
 *
 * # Per-request work
 * The route is the canonical minimal `setBody("pong")` only —
 * no per-request counter, no process step, no log emission,
 * no bean invocation. The harness measures cold-start + RSS,
 * not request latency; per-request observability belongs to
 * the loadgen, not the fixture.
 */
public final class AppYaml {
    private AppYaml() {
    }

    public static void main(String[] args) throws Exception {
        // We extend `Main` to install the marker-emitting
        // event notifier as part of `initCamelContext()`
        // (the camel-main hook that runs AFTER the context is
        // created but BEFORE the routes are started). This
        // is the canonical camel-main extension point for
        // "I need to register a notifier before the routes
        // start but after the context is initialized" — the
        // bare `Main` class doesn't expose the notifier
        // registry, and `main.getCamelContext()` returns
        // null until `run()` is called, so a pre-`run()`
        // registration from a non-`Main` subclass is
        // awkward.
        final AtomicBoolean markerEmitted = new AtomicBoolean(false);

        Main main = new Main() {
            @Override
            protected void initCamelContext() throws Exception {
                super.initCamelContext();
                CamelContext context = getCamelContext();
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
            }
        };
        main.configure()
            .withRoutesIncludePattern("classpath:routes.yaml");
        main.run(args);
    }
}
