package org.rustcamel.jms;

import static org.junit.jupiter.api.Assertions.assertDoesNotThrow;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;
import static org.mockito.ArgumentMatchers.eq;
import static org.mockito.Mockito.mock;
import static org.mockito.Mockito.when;

import java.util.List;
import java.util.Map;
import java.util.Optional;
import org.eclipse.microprofile.config.Config;
import org.eclipse.microprofile.config.spi.ConfigSource;
import org.junit.jupiter.api.Test;

/**
 * Fail-closed semantics of {@link OtlpEgressGuard}.
 *
 * <p>When the otel SDK is enabled, startup must be refused without an explicitly configured OTLP
 * traces endpoint, so the exporter's built-in {@code http://localhost:4317} fallback can never dial
 * silently. The extension injects that fallback below the source layer — it shows up in {@code
 * getOptionalValue} while no config source owns the key — so "explicit" means: present in an actual
 * source, never in {@code DefaultValuesConfigSource}.
 */
class OtlpEgressGuardTest {

  private static ConfigSource source(String name, Map<String, String> properties) {
    ConfigSource source = mock(ConfigSource.class);
    when(source.getName()).thenReturn(name);
    when(source.getProperties()).thenReturn(properties);
    return source;
  }

  private Config configWith(Boolean sdkDisabled, ConfigSource... sources) {
    Config config = mock(Config.class);
    when(config.getOptionalValue(eq(OtlpEgressGuard.SDK_DISABLED_KEY), eq(Boolean.class)))
        .thenReturn(sdkDisabled == null ? Optional.empty() : Optional.of(sdkDisabled));
    when(config.getOptionalValue(eq(OtlpEgressGuard.TRACES_ENDPOINT_KEY), eq(String.class)))
        .thenReturn(Optional.of("http://localhost:4317/")); // extension fallback, always visible
    when(config.getConfigSources()).thenReturn(List.of(sources));
    return config;
  }

  private static void enforce(Config config) {
    new OtlpEgressGuard(config).enforceOnStartup(null);
  }

  @Test
  void sdkDisabledBootsWithoutEndpoint() {
    assertDoesNotThrow(() -> enforce(configWith(true, source("EnvConfigSource", Map.of()))));
  }

  @Test
  void sdkEnabledWithoutEndpointInAnySourceFailsClosed() {
    IllegalStateException error =
        assertThrows(
            IllegalStateException.class,
            () -> enforce(configWith(false, source("EnvConfigSource", Map.of()))));
    assertTrue(error.getMessage().contains(OtlpEgressGuard.TRACES_ENDPOINT_ENV));
    assertTrue(error.getMessage().contains("fail-closed"));
  }

  @Test
  void sdkEnabledWithEndpointInEnvSourceBoots() {
    Config config =
        configWith(
            false,
            source(
                "EnvConfigSource",
                Map.of(OtlpEgressGuard.TRACES_ENDPOINT_KEY, "http://collector:4317")));
    assertDoesNotThrow(() -> enforce(config));
  }

  @Test
  void extensionDefaultAloneDoesNotCountAsExplicit() {
    // The built-in fallback shows up in getOptionalValue but lives in no real
    // source (only the skipped DefaultValuesConfigSource): must still fail closed.
    Config config =
        configWith(
            false,
            source(
                OtlpEgressGuard.DEFAULT_VALUES_SOURCE,
                Map.of(OtlpEgressGuard.TRACES_ENDPOINT_KEY, "http://localhost:4317")));
    assertThrows(IllegalStateException.class, () -> enforce(config));
  }

  @Test
  void absentSdkFlagIsTreatedAsEnabledAndFailsClosed() {
    // Flag absent = quarkus default false = SDK on: the invariant holds even if
    // someone deletes the application.yml line.
    assertThrows(
        IllegalStateException.class,
        () -> enforce(configWith(null, source("EnvConfigSource", Map.of()))));
  }

  @Test
  void explicitlyConfiguredInspectsSources() {
    Config withSource =
        configWith(
            false, source("EnvConfigSource", Map.of(OtlpEgressGuard.TRACES_ENDPOINT_KEY, "x")));
    Config withoutSource = configWith(false, source("EnvConfigSource", Map.of()));
    assertTrue(
        OtlpEgressGuard.explicitlyConfigured(withSource, OtlpEgressGuard.TRACES_ENDPOINT_KEY));
    assertFalse(
        OtlpEgressGuard.explicitlyConfigured(withoutSource, OtlpEgressGuard.TRACES_ENDPOINT_KEY));
  }
}
