package org.rustcamel.jms;

import io.opentelemetry.sdk.testing.exporter.InMemorySpanExporter;
import jakarta.enterprise.context.ApplicationScoped;
import jakarta.enterprise.inject.Produces;
import jakarta.inject.Singleton;

/**
 * Test-only CDI exporter for the otel wiring tests.
 *
 * <p>Quarkus wires trace exporters through CDI (the {@code quarkus.otel.traces.exporter=cdi}
 * default), so providing this bean routes spans to memory instead of OTLP: no collector, no egress.
 * The SDK is disabled by default in application.yml, so this bean is only exercised by tests whose
 * profile enables the SDK.
 *
 * <p>Twin copies of this file live in bridges/xml and bridges/cxf (independent gradle builds, repo
 * convention): keep the bean in lockstep when any copy changes.
 */
@ApplicationScoped
public class OtelTestExporters {

  @Produces
  @Singleton
  InMemorySpanExporter inMemorySpanExporter() {
    return InMemorySpanExporter.create();
  }
}
