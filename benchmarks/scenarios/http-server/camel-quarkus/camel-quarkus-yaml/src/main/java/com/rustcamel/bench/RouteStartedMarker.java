package com.rustcamel.bench;

import jakarta.annotation.PostConstruct;
import jakarta.enterprise.context.ApplicationScoped;
import jakarta.inject.Inject;
import org.apache.camel.CamelContext;
import org.apache.camel.builder.RouteBuilder;
import org.apache.camel.impl.event.RouteStartedEvent;
import org.apache.camel.spi.CamelEvent;
import org.apache.camel.support.EventNotifierSupport;

import java.util.concurrent.atomic.AtomicBoolean;

/**
 * Marker emitter for the T3 YAML Quarkus fixture. The yaml
 * subproject's primary route is in `camel/routes.yaml`
 * (loaded by camel-quarkus-yaml-dsl at startup), but the
 * marker-emitting notifier MUST be registered on the
 * `CamelContext` BEFORE the context starts — so it sees
 * the `RouteStarted` event during `context.start()`.
 *
 * Why a dummy `RouteBuilder` class as well:
 * `camel-quarkus-core-deployment` auto-discovers any
 * CDI bean that's a `RouteBuilder` subclass and calls
 * its `configure()` method during route-registration
 * phase, BEFORE the context starts. This is the same
 * hook the dsl subproject's `BenchRoute` uses, and it
 * reliably fires before the context starts.
 *
 * The dummy `configure()` body is empty — the actual
 * route is in `camel/routes.yaml`. We just need the
 * `RouteBuilder.configure()` lifecycle hook to register
 * the notifier.
 *
 * The marker `BENCH_ROUTE_READY <unix_ms>` is emitted on
 * the first `CamelEvent.RouteStarted` — the event that
 * fires AFTER the `platform-http:` consumer has bound the
 * listener and the route is fully started (spec §4.10
 * listener-bound, not first-request).
 *
 * One-shot flag: the harness's `marker_count != 1` check
 * is fatal-on-mismatch, so we MUST emit exactly one
 * marker. The `AtomicBoolean` ensures that.
 *
 * # Per-request work
 * The route is the canonical minimal `setBody("pong")` only —
 * no per-request counter, no process step, no log emission,
 * no bean invocation. The harness measures cold-start + RSS,
 * not request latency; per-request observability belongs to
 * the loadgen, not the fixture.
 */
@ApplicationScoped
public class RouteStartedMarker extends RouteBuilder {

    @Inject
    CamelContext context;

    private static final AtomicBoolean MARKER_EMITTED = new AtomicBoolean(false);
    private static final AtomicBoolean NOTIFIER_REGISTERED = new AtomicBoolean(false);

    @PostConstruct
    void registerNotifierPostConstruct() {
        // Best-effort: try to register now in case the
        // context is already available (it might be —
        // the dsl subproject's BenchRoute uses the same
        // bean-injection pattern). The configure() call
        // below also registers; whichever fires first
        // wins (NOTIFIER_REGISTERED is a one-shot guard).
        register();
    }

    @Override
    public void configure() {
        // The actual T3 route is in camel/routes.yaml
        // (loaded by camel-quarkus-yaml-dsl at startup).
        // This `configure()` body is empty — we only
        // need this `RouteBuilder` subclass so the
        // Quarkus core deployer calls `configure()` on
        // it during route-registration phase, which is
        // before the context starts.
        register();
    }

    private void register() {
        if (NOTIFIER_REGISTERED.compareAndSet(false, true)) {
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
    }
}
