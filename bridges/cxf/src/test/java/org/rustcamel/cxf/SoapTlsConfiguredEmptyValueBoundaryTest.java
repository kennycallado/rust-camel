package org.rustcamel.cxf;

import static org.junit.jupiter.api.Assertions.assertDoesNotThrow;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

import io.smallrye.config.EnvConfigSource;
import io.smallrye.config.SmallRyeConfig;
import io.smallrye.config.SmallRyeConfigBuilder;
import java.util.HashMap;
import java.util.Map;
import org.junit.jupiter.api.Test;

/**
 * N4 parity at the REAL Java config boundary, not just a helper handed a blank string.
 *
 * <p>Measured boundary semantics (the exact {@code getOptionalValue} lookup {@link BridgeConfig}'s
 * {@code @ConfigProperty Optional<String>} fields perform, against a real {@link EnvConfigSource}
 * carrying the {@code CXF_SOAP_TLS_*} env vars): a whitespace-only value (including NBSP) survives
 * verbatim, but a configured-empty ("") env value is erased to {@code Optional.empty()} —
 * indistinguishable from genuinely omitted. Production therefore runs TWO checks: {@code
 * SoapEndpointPublisher.rejectConfiguredBlankTlsEnv} reads the raw env vars for presence (the same
 * existing-config-API route {@code BridgeConfig.parseMaxBodyBytes} uses), and {@code
 * validateListenerTls} rejects blanks among surviving values. These tests drive both through the
 * real config source, mirroring {@code validateConfiguredListenerTls}.
 */
class SoapTlsConfiguredEmptyValueBoundaryTest {

  /** The exact Optional lookups BridgeConfig's {@code @ConfigProperty} fields perform. */
  private static String bridgeConfigValue(SmallRyeConfig config, String dottedKey) {
    return config.getOptionalValue(dottedKey, String.class).orElse(null);
  }

  private static SmallRyeConfig configWithEnv(Map<String, String> env) {
    return new SmallRyeConfigBuilder().withSources(new EnvConfigSource(env, 300)).build();
  }

  /**
   * Mirrors {@code SoapEndpointPublisher.validateConfiguredListenerTls}: raw configured-env
   * presence first, then shape validation over the Optional-injected values.
   */
  private static void validateStartupThroughBoundary(
      SmallRyeConfig config, Map<String, String> env) {
    SoapEndpointPublisher.rejectConfiguredBlankTlsEnv(env::get);
    SoapEndpointPublisher.validateListenerTls(
        bridgeConfigValue(config, "cxf.address"),
        bridgeConfigValue(config, "cxf.soap.tls.cert.path"),
        bridgeConfigValue(config, "cxf.soap.tls.key.path"),
        bridgeConfigValue(config, "cxf.soap.tls.client.ca.path"),
        bridgeConfigValue(config, "cxf.soap.tls.pkcs12.path"),
        bridgeConfigValue(config, "cxf.soap.tls.pkcs12.password"));
  }

  @Test
  void configuredBlankListenerTlsValueIsRejectedThroughRealConfigPath() {
    // Valid PEM form plus an NBSP-only PKCS12 password (Unicode White_Space:
    // blank for the policy, non-breaking so no layer trims it away). The value
    // SURVIVES the Optional lookup, so validateListenerTls itself must reject it.
    Map<String, String> env = new HashMap<>();
    env.put("CXF_ADDRESS", "https://0.0.0.0:9443/soap");
    env.put("CXF_SOAP_TLS_CERT_PATH", "/etc/cxf/server.pem");
    env.put("CXF_SOAP_TLS_KEY_PATH", "/etc/cxf/server.key");
    env.put("CXF_SOAP_TLS_CLIENT_CA_PATH", "/etc/cxf/ca.pem");
    env.put("CXF_SOAP_TLS_PKCS12_PASSWORD", "\u00A0");
    SmallRyeConfig config = configWithEnv(env);

    IllegalStateException ex =
        assertThrows(
            IllegalStateException.class, () -> validateStartupThroughBoundary(config, env));
    // Either rejection layer is correct: the raw-env check names the env var,
    // validateListenerTls the pool key.
    assertTrue(
        ex.getMessage().contains("CXF_SOAP_TLS_PKCS12_PASSWORD")
            || ex.getMessage().contains("soap_tls_pkcs12_password"),
        ex.getMessage());
    assertTrue(ex.getMessage().contains("whitespace"), ex.getMessage());
    assertFalse(ex.getMessage().contains("\u00A0"), "error must not echo the blank value");
  }

  @Test
  void configuredEmptyListenerTlsValueIsRejectedThroughRealConfigPath() {
    // A genuinely empty configured password. The Optional lookup erases "" to
    // Optional.empty() (verified: without the raw-env presence check nothing is
    // rejected here), so the raw configured-env boundary must catch it — the
    // configured-empty property path can never fall back to "unset" semantics.
    Map<String, String> env = new HashMap<>();
    env.put("CXF_ADDRESS", "https://0.0.0.0:9443/soap");
    env.put("CXF_SOAP_TLS_CERT_PATH", "/etc/cxf/server.pem");
    env.put("CXF_SOAP_TLS_KEY_PATH", "/etc/cxf/server.key");
    env.put("CXF_SOAP_TLS_CLIENT_CA_PATH", "/etc/cxf/ca.pem");
    env.put("CXF_SOAP_TLS_PKCS12_PASSWORD", "");
    SmallRyeConfig config = configWithEnv(env);

    IllegalStateException ex =
        assertThrows(
            IllegalStateException.class, () -> validateStartupThroughBoundary(config, env));
    assertTrue(ex.getMessage().contains("CXF_SOAP_TLS_PKCS12_PASSWORD"), ex.getMessage());
    assertTrue(ex.getMessage().contains("whitespace"), ex.getMessage());
  }

  @Test
  void configuredWhitespaceOnlyCertPathIsRejectedThroughRealConfigPath() {
    Map<String, String> env = new HashMap<>();
    env.put("CXF_ADDRESS", "https://0.0.0.0:9443/soap");
    env.put("CXF_SOAP_TLS_CERT_PATH", " \t\r\n");
    env.put("CXF_SOAP_TLS_KEY_PATH", "/etc/cxf/server.key");
    env.put("CXF_SOAP_TLS_CLIENT_CA_PATH", "/etc/cxf/ca.pem");
    SmallRyeConfig config = configWithEnv(env);

    IllegalStateException ex =
        assertThrows(
            IllegalStateException.class, () -> validateStartupThroughBoundary(config, env));
    // Either rejection layer is correct: the raw-env check names the env var,
    // validateListenerTls the pool key.
    assertTrue(
        ex.getMessage().contains("CXF_SOAP_TLS_CERT_PATH")
            || ex.getMessage().contains("soap_tls_cert_path"),
        ex.getMessage());
    assertTrue(ex.getMessage().contains("whitespace"), ex.getMessage());
  }

  @Test
  void genuinelyUnsetListenerTlsStaysValidThroughRealConfigPath() {
    // Producer-only: no CXF_ADDRESS, no CXF_SOAP_TLS_* — nothing is rejected.
    Map<String, String> env = Map.of("UNRELATED_VAR", "x");
    SmallRyeConfig config = configWithEnv(env);
    assertDoesNotThrow(() -> validateStartupThroughBoundary(config, env));
  }

  @Test
  void configuredNonblankListenerTlsMaterialStaysValidThroughRealConfigPath() {
    // Sanity of the boundary plumbing: nonblank material must flow through
    // unchanged (accepted here; the shape rules then apply as before).
    Map<String, String> env = new HashMap<>();
    env.put("CXF_ADDRESS", "https://0.0.0.0:9443/soap");
    env.put("CXF_SOAP_TLS_CERT_PATH", "/etc/cxf/server.pem");
    env.put("CXF_SOAP_TLS_KEY_PATH", "/etc/cxf/server.key");
    env.put("CXF_SOAP_TLS_CLIENT_CA_PATH", "/etc/cxf/ca.pem");
    SmallRyeConfig config = configWithEnv(env);
    assertNotNull(bridgeConfigValue(config, "cxf.soap.tls.cert.path"));
    assertDoesNotThrow(() -> validateStartupThroughBoundary(config, env));
  }
}
