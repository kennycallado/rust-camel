package org.rustcamel.cxf;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

import io.quarkus.test.junit.QuarkusTest;
import jakarta.inject.Inject;
import org.eclipse.microprofile.config.Config;
import org.junit.jupiter.api.Test;

/**
 * Shipped default of the otel egress surface, read from the booted config (application.yml).
 *
 * <p>Fail-closed default: the SDK is disabled and the OTLP traces endpoint lives in no config
 * source, so a bridge started without operator opt-in dials nothing. The extension's {@code
 * http://localhost:4317} mapping-level fallback still resolves through the property API — that is
 * exactly the hazard {@link OtlpEgressGuard} neutralizes, pinned here on purpose.
 */
@QuarkusTest
class OtelEgressConfigTest {

  @Inject Config config;

  @Test
  void shippedDefaultIsSdkDisabled() {
    assertTrue(config.getValue(OtlpEgressGuard.SDK_DISABLED_KEY, Boolean.class));
  }

  @Test
  void shippedDefaultHasNoExplicitTracesEndpoint() {
    assertFalse(OtlpEgressGuard.explicitlyConfigured(config, OtlpEgressGuard.TRACES_ENDPOINT_KEY));
  }

  @Test
  void extensionFallbackIsStillMaterialized() {
    // Canary: if the extension ever stops defaulting the endpoint, this fails and
    // the guard narrative must be revisited (the hazard would be gone).
    String fallback =
        config.getOptionalValue(OtlpEgressGuard.TRACES_ENDPOINT_KEY, String.class).orElse(null);
    assertEquals("http://localhost:4317/", fallback);
  }
}
