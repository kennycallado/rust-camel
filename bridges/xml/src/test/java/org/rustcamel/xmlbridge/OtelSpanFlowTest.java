package org.rustcamel.xmlbridge;

import static org.junit.jupiter.api.Assertions.assertTrue;

import io.grpc.ManagedChannel;
import io.grpc.ManagedChannelBuilder;
import io.grpc.Metadata;
import io.grpc.stub.MetadataUtils;
import io.opentelemetry.api.OpenTelemetry;
import io.opentelemetry.sdk.testing.exporter.InMemorySpanExporter;
import io.quarkus.test.junit.QuarkusTest;
import io.quarkus.test.junit.TestProfile;
import jakarta.inject.Inject;
import java.util.Map;
import java.util.concurrent.TimeUnit;
import org.junit.jupiter.api.Test;
import xml_bridge.HealthCheckRequest;
import xml_bridge.HealthGrpc;

/**
 * SDK liveness and trace continuation with the operator opt-in shape, without a collector.
 *
 * <p>The profile enables the SDK and sets an explicit traces endpoint — exactly the documented
 * opt-in pair — and spans are routed to the in-memory CDI exporter ({@link OtelTestExporters}). A
 * started and ended span must show up in the exporter's finished items (pipeline live, egress
 * in-process), and a bridge Health RPC carrying a W3C {@code traceparent} header (the control-plane
 * propagation convention) must produce a server span parented by the propagated context.
 *
 * <p>Twin-copy of bridges/jms OtelSpanFlowTest (independent gradle builds, repo convention): keep
 * the assertions in lockstep when either copy changes.
 */
@QuarkusTest
@TestProfile(OtelSpanFlowTest.SpanFlowProfile.class)
class OtelSpanFlowTest {

  private static final String TRACE_ID = "4bf92f3577b34da6a3ce929d0e0e4736";
  private static final String PARENT_SPAN_ID = "00f067aa0ba902b7";

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

    assertTrue(awaitSpan(span -> "bridge-span".equals(span.getName())), "span must be exported");
  }

  @Test
  void grpcServerSpanContinuesPropagatedTraceparent() throws Exception {
    ManagedChannel channel =
        ManagedChannelBuilder.forAddress("localhost", 9001).usePlaintext().build();
    try {
      Metadata headers = new Metadata();
      headers.put(
          Metadata.Key.of("traceparent", Metadata.ASCII_STRING_MARSHALLER),
          "00-" + TRACE_ID + "-" + PARENT_SPAN_ID + "-01");
      HealthGrpc.newBlockingStub(channel)
          .withInterceptors(MetadataUtils.newAttachHeadersInterceptor(headers))
          .check(HealthCheckRequest.getDefaultInstance());

      assertTrue(
          awaitSpan(
              span ->
                  TRACE_ID.equals(span.getTraceId())
                      && PARENT_SPAN_ID.equals(span.getParentSpanId())),
          "grpc server span must continue the propagated traceparent");
    } finally {
      channel.shutdownNow().awaitTermination(5, TimeUnit.SECONDS);
    }
  }

  private boolean awaitSpan(
      java.util.function.Predicate<io.opentelemetry.sdk.trace.data.SpanData> predicate)
      throws InterruptedException {
    for (int i = 0; i < 200; i++) {
      boolean matched = exporter.getFinishedSpanItems().stream().anyMatch(predicate);
      if (matched) {
        return true;
      }
      TimeUnit.MILLISECONDS.sleep(50L);
    }
    return false;
  }
}
