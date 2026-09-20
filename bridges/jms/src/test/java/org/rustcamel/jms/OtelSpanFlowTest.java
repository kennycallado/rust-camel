package org.rustcamel.jms;

import static org.junit.jupiter.api.Assertions.assertTrue;

import io.opentelemetry.api.OpenTelemetry;
import io.opentelemetry.sdk.testing.exporter.InMemorySpanExporter;
import io.quarkus.test.junit.QuarkusTest;
import io.quarkus.test.junit.TestProfile;
import jakarta.inject.Inject;
import java.util.Map;
import java.util.concurrent.TimeUnit;
import org.junit.jupiter.api.Test;

/**
 * SDK liveness and span flow with the operator opt-in shape, without a collector.
 *
 * <p>The profile enables the SDK and sets an explicit traces endpoint — exactly the documented
 * opt-in pair — and spans are routed to the in-memory CDI exporter ({@link OtelTestExporters}). A
 * started and ended span must show up in the exporter's finished items: the pipeline is live,
 * egress stays in-process.
 */
@QuarkusTest
@TestProfile(OtelSpanFlowTest.SpanFlowProfile.class)
class OtelSpanFlowTest {

  public static class SpanFlowProfile implements io.quarkus.test.junit.QuarkusTestProfile {

    @Override
    public Map<String, String> getConfigOverrides() {
      return Map.of(
          "quarkus.otel.sdk.disabled", "false",
          "quarkus.otel.exporter.otlp.traces.endpoint", "http://127.0.0.1:4317");
    }
  }

  @Inject OpenTelemetry openTelemetry;
  @Inject InMemorySpanExporter exporter;

  @Test
  void endedSpanReachesInMemoryExporter() throws InterruptedException {
    openTelemetry.getTracer("otel-span-flow-test").spanBuilder("bridge-span").startSpan().end();

    boolean exported = false;
    for (int i = 0; i < 100 && !exported; i++) {
      exported =
          exporter.getFinishedSpanItems().stream()
              .anyMatch(span -> "bridge-span".equals(span.getName()));
      if (!exported) {
        TimeUnit.MILLISECONDS.sleep(50L);
      }
    }
    assertTrue(exported, "span 'bridge-span' must reach the in-memory exporter");
  }
}
