package org.rustcamel.jms;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.io.IOException;
import java.net.URI;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.Map;
import org.apache.activemq.artemis.core.remoting.impl.netty.TransportConstants;
import org.junit.jupiter.api.DisplayName;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

/**
 * Unit tests for {@link JmsClientFactory#transportConfig} scheme dispatch. The explicit-values
 * overload is the testability seam: System.getenv() is immutable, so production reads the
 * BRIDGE_BROKER_* env contract while tests inject values directly.
 */
@DisplayName("JmsClientFactory broker URL scheme dispatch")
class JmsClientFactorySchemeTest {

  @TempDir Path tmp;

  private String newStore(String name) throws IOException {
    return Files.createFile(tmp.resolve(name)).toString();
  }

  @Test
  void failoverParenthesizedInnerAborts() {
    // failover:(...) carries a null host; the scheme error must win over the host check.
    IllegalStateException ex =
        assertThrows(
            IllegalStateException.class,
            () ->
                JmsClientFactory.transportConfig(
                    URI.create("failover:(ssl://broker:61617)"), null, null, null, "artemis"));
    assertTrue(ex.getMessage().contains("failover"), ex.getMessage());
    assertTrue(ex.getMessage().contains("multiple broker entries"), ex.getMessage());
  }

  @Test
  void failoverPrefixedUriAborts() {
    IllegalStateException ex =
        assertThrows(
            IllegalStateException.class,
            () ->
                JmsClientFactory.transportConfig(
                    URI.create("failover://tcp://broker:61616"), null, null, null, "artemis"));
    assertTrue(ex.getMessage().contains("failover"), ex.getMessage());
    assertTrue(ex.getMessage().contains("multiple broker entries"), ex.getMessage());
  }

  @Test
  void hostlessKnownSchemeAborts() throws IOException {
    String keystore = newStore("hostless-keystore.p12");
    String truststore = newStore("hostless-truststore.p12");

    IllegalStateException ex =
        assertThrows(
            IllegalStateException.class,
            () ->
                JmsClientFactory.transportConfig(
                    URI.create("ssl://:61617"), keystore, truststore, "secret", "artemis"));
    assertTrue(ex.getMessage().contains("no host"), ex.getMessage());
  }

  @Test
  void missingSchemeAbortsActionably() {
    IllegalStateException ex =
        assertThrows(
            IllegalStateException.class,
            () ->
                JmsClientFactory.transportConfig(
                    URI.create("//broker:61616"), null, null, null, "artemis"));
    assertTrue(ex.getMessage().contains("no scheme"), ex.getMessage());
  }

  @Test
  void nioSchemeAborts() {
    // nio:// was advertised as a plaintext scheme until this change; pin that it now
    // hits the exhaustive fail-loud dispatch so a revert cannot pass silently.
    IllegalStateException ex =
        assertThrows(
            IllegalStateException.class,
            () ->
                JmsClientFactory.transportConfig(
                    URI.create("nio://broker:61616"), null, null, null, "artemis"));
    assertTrue(ex.getMessage().contains("Unsupported broker URL scheme"), ex.getMessage());
  }

  @Test
  void tcpSchemeStaysPlaintext() {
    Map<String, Object> cfg =
        JmsClientFactory.transportConfig(
            URI.create("tcp://broker:61616"), null, null, null, "artemis");

    assertFalse(cfg.containsKey(TransportConstants.SSL_ENABLED_PROP_NAME));
  }

  @Test
  void sslSchemeEnablesSsl() throws IOException {
    String keystore = newStore("ssl-keystore.p12");
    String truststore = newStore("ssl-truststore.p12");

    Map<String, Object> cfg =
        JmsClientFactory.transportConfig(
            URI.create("ssl://broker:61617"), keystore, truststore, "secret", "artemis");

    assertEquals(Boolean.TRUE, cfg.get(TransportConstants.SSL_ENABLED_PROP_NAME));
  }

  @Test
  void wsSchemeStaysPlaintext() {
    Map<String, Object> cfg =
        JmsClientFactory.transportConfig(
            URI.create("ws://broker:61616"), null, null, null, "artemis");

    assertFalse(cfg.containsKey(TransportConstants.SSL_ENABLED_PROP_NAME));
  }

  @Test
  void wssSchemeRequiresMaterial() throws IOException {
    String keystore = newStore("wss-keystore.p12");

    IllegalStateException ex =
        assertThrows(
            IllegalStateException.class,
            () ->
                JmsClientFactory.transportConfig(
                    URI.create("wss://broker:61617"), keystore, null, "secret", "artemis"));
    assertTrue(ex.getMessage().contains("wss"), ex.getMessage());
    assertTrue(ex.getMessage().contains("TRUSTSTORE"), ex.getMessage());
  }
}
