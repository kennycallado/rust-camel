package org.rustcamel.cxf;

import static org.junit.jupiter.api.Assertions.assertDoesNotThrow;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;
import static org.mockito.ArgumentMatchers.eq;
import static org.mockito.Mockito.mock;
import static org.mockito.Mockito.when;

import io.smallrye.config.EnvConfigSource;
import io.smallrye.config.SmallRyeConfigBuilder;
import java.util.HashMap;
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
 *
 * <p>Env vars are the documented operator route and get the real {@link EnvConfigSource} (raw
 * env-var names in the property map, dotted aliases only in {@code getPropertyNames()}); file and
 * default-value sources are mocked.
 *
 * <p>Twin-copy of bridges/jms OtlpEgressGuardTest (independent gradle builds, repo convention):
 * keep the matrix in lockstep when either copy changes.
 */
class OtlpEgressGuardTest {

  private static ConfigSource fileSource(String name, Map<String, String> properties) {
    ConfigSource source = mock(ConfigSource.class);
    when(source.getName()).thenReturn(name);
    when(source.getPropertyNames()).thenReturn(properties.keySet());
    when(source.getValue(eq(OtlpEgressGuard.TRACES_ENDPOINT_KEY)))
        .thenReturn(properties.get(OtlpEgressGuard.TRACES_ENDPOINT_KEY));
    return source;
  }

  private Config configWith(Boolean sdkDisabled, ConfigSource... sources) {
    Config config = mock(Config.class);
    when(config.getOptionalValue(eq(OtlpEgressGuard.SDK_DISABLED_KEY), eq(Boolean.class)))
        .thenReturn(sdkDisabled == null ? Optional.empty() : Optional.of(sdkDisabled));
    when(config.getOptionalValue(eq(OtlpEgressGuard.TRACES_ENDPOINT_KEY), eq(String.class)))
        .thenReturn(Optional.of("http://localhost:4317/")); // extension fallback, always visible
    when(config.getConfigSources()).thenReturn(java.util.List.of(sources));
    return config;
  }

  private static void enforce(Config config) {
    new OtlpEgressGuard(config).enforceOnStartup(null);
  }

  @Test
  void sdkDisabledBootsWithoutEndpoint() {
    assertDoesNotThrow(() -> enforce(configWith(true, fileSource("EnvConfigSource", Map.of()))));
  }

  @Test
  void sdkEnabledWithoutEndpointInAnySourceFailsClosed() {
    IllegalStateException error =
        assertThrows(
            IllegalStateException.class,
            () -> enforce(configWith(false, fileSource("EnvConfigSource", Map.of()))));
    assertTrue(error.getMessage().contains(OtlpEgressGuard.TRACES_ENDPOINT_ENV));
    assertTrue(error.getMessage().contains("fail-closed"));
  }

  @Test
  void sdkEnabledWithEndpointInFileSourceBoots() {
    Map<String, String> file = new HashMap<>();
    file.put(OtlpEgressGuard.TRACES_ENDPOINT_KEY, "http://collector:4317");
    Config config = configWith(false, fileSource("PropertiesConfigSource[test]", file));
    assertDoesNotThrow(() -> enforce(config));
  }

  @Test
  void extensionDefaultAloneDoesNotCountAsExplicit() {
    // The built-in fallback shows up in getOptionalValue but lives in no real
    // source (only the skipped DefaultValuesConfigSource): must still fail closed.
    Map<String, String> defaults = new HashMap<>();
    defaults.put(OtlpEgressGuard.TRACES_ENDPOINT_KEY, "http://localhost:4317");
    Config config = configWith(false, fileSource(OtlpEgressGuard.DEFAULT_VALUES_SOURCE, defaults));
    assertThrows(IllegalStateException.class, () -> enforce(config));
  }

  @Test
  void absentSdkFlagIsTreatedAsEnabledAndFailsClosed() {
    // Flag absent = quarkus default false = SDK on: the invariant holds even if
    // someone deletes the application.yml line.
    assertThrows(
        IllegalStateException.class,
        () -> enforce(configWith(null, fileSource("EnvConfigSource", Map.of()))));
  }

  @Test
  void envVarOptInSatisfiesExplicitCheckWithRealEnvSource() {
    // Real EnvConfigSource semantics (r_glm finding): the property map is keyed by
    // the raw env-var name; the dotted alias must surface via getPropertyNames().
    Config config =
        new SmallRyeConfigBuilder()
            .withSources(
                new EnvConfigSource(
                    Map.of(
                        OtlpEgressGuard.TRACES_ENDPOINT_ENV.toUpperCase().replace(".", "_"),
                        "http://collector:4317"),
                    300))
            .build();
    assertTrue(OtlpEgressGuard.explicitlyConfigured(config, OtlpEgressGuard.TRACES_ENDPOINT_KEY));
    // SDK flag unset = enabled, endpoint explicit via env: must boot.
    assertDoesNotThrow(() -> enforce(config));
  }

  @Test
  void realEnvSourceWithoutTheVariableStaysClosed() {
    Config config =
        new SmallRyeConfigBuilder()
            .withSources(new EnvConfigSource(Map.of("UNRELATED_VAR", "x"), 300))
            .build();
    assertFalse(OtlpEgressGuard.explicitlyConfigured(config, OtlpEgressGuard.TRACES_ENDPOINT_KEY));
  }
}
