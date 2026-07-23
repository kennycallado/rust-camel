package com.rustcamel.bench;

import jakarta.enterprise.context.ApplicationScoped;
import jakarta.inject.Inject;
import org.apache.camel.CamelContext;
import org.apache.camel.builder.RouteBuilder;
import org.apache.camel.impl.event.RouteStartedEvent;
import org.apache.camel.spi.CamelEvent;
import org.apache.camel.support.EventNotifierSupport;

import java.util.concurrent.atomic.AtomicBoolean;

/**
 * T3 HTTP server route + marker emitter (Pair A — hardcoded
 * Java DSL) for camel-quarkus-dsl (bd Task 3). Combines the
 * route definition AND the marker-emitting notifier
 * registration in one place — the notifier must be
 * registered on the `CamelContext` BEFORE the context
 * starts (so it sees the `RouteStarted` event during
 * `context.start()`). Registering from inside the
 * `RouteBuilder.configure()` method is the canonical
 * place: the route configuration runs during CDI init
 * (well before `context.start()`), and the `CamelContext`
 * is injectable as a CDI bean.
 *
 * NOTE: The native variant (camel-quarkus-dsl-native) uses its own
 * copy of this file with `platform-http:/bench` instead of
 * `jetty:http://0.0.0.0:8080/bench`. See that subproject's
 * src/main/java/. (platform-http backs onto Quarkus' managed
 * Vert.x/Netty layer and is +18.8% faster in native mode.)
 *
 * The marker `BENCH_ROUTE_READY <unix_ms>` is emitted on
 * `CamelEvent.RouteStarted` — the event that fires AFTER
 * the `jetty:` consumer has bound the listener and the
 * route is fully started, satisfying the spec §4.10
 * listener-bound (NOT first-request) criterion.
 *
 * # Per-request work
 * The route is the canonical minimal `setBody("pong")` only —
 * no per-request counter, no process step, no log emission.
 * The harness measures cold-start + RSS, not request latency;
 * per-request observability belongs to the loadgen, not the
 * fixture.
 */
@ApplicationScoped
public class BenchRoute extends RouteBuilder {

    @Inject
    CamelContext context;

    private static final AtomicBoolean MARKER_EMITTED = new AtomicBoolean(false);

    @Override
    public void configure() {
        // Register the marker-emitting notifier FIRST so it's
        // in place before context.start() fires the
        // RouteStarted event. (configure() runs during CDI
        // init; context.start() runs later in the Quarkus
        // boot sequence — there's a clean ordering here.)
        context.getManagementStrategy().addEventNotifier(
            new EventNotifierSupport() {
                @Override
                public void notify(CamelEvent event) {
                    if (MARKER_EMITTED.get()) {
                        return;
                    }
                    if (event instanceof RouteStartedEvent) {
                        if (MARKER_EMITTED.compareAndSet(false, true)) {
                            long unixMs = System.currentTimeMillis();
                            // System.out (NOT SLF4J) so the
                            // line lands on the harness's
                            // captured stdout with no
                            // formatting noise. The
                            // trailing unix_ms is a
                            // parseable suffix.
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

        // The T3 route — same logical shape as the
        // standalone fixtures (POST `/bench` body `ping`
        // → 200 `pong`), using the `jetty:` server-side
        // HTTP scheme. No method restriction on the consumer:
        // the loadgen drives only POST `/bench`, so restricting
        // it is unnecessary and would diverge from the rust-camel
        // fixtures (which accept any method). All fixtures are
        // identical on this axis.
        from("jetty:http://0.0.0.0:8080/bench")
            .setBody(constant("pong"));
    }
}
