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
import org.apache.activemq.ActiveMQConnectionFactory;
import org.apache.activemq.artemis.api.core.TransportConfiguration;
import org.apache.activemq.artemis.core.remoting.impl.netty.NettyConnectorFactory;
import org.apache.activemq.artemis.core.remoting.impl.netty.TransportConstants;
import org.apache.activemq.pool.PooledConnectionFactory;
import org.junit.jupiter.api.DisplayName;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;
import org.messaginghub.pooled.jms.JmsPoolConnectionFactory;

/**
 * Unit tests for {@link JmsClientFactory#transportConfig}. The explicit-values overload is the
 * testability seam: System.getenv() is immutable, so production reads the BRIDGE_BROKER_* env
 * contract while tests inject values directly.
 */
@DisplayName("JmsClientFactory TLS transport config")
class JmsClientFactoryTlsTest {

  @TempDir Path tmp;

  private String newStore(String name) throws IOException {
    return Files.createFile(tmp.resolve(name)).toString();
  }

  @Test
  void sslSchemeEnablesSslTransport() throws IOException {
    String keystore = newStore("broker-keystore.p12");
    String truststore = newStore("broker-truststore.p12");

    Map<String, Object> cfg =
        JmsClientFactory.transportConfig(
            URI.create("ssl://broker:61617"), keystore, truststore, "secret", "artemis");

    assertEquals(Boolean.TRUE, cfg.get(TransportConstants.SSL_ENABLED_PROP_NAME));
    // Artemis 2.36 names the material properties without the SSL_ prefix.
    assertEquals(keystore, cfg.get(TransportConstants.KEYSTORE_PATH_PROP_NAME));
    assertEquals(truststore, cfg.get(TransportConstants.TRUSTSTORE_PATH_PROP_NAME));
    assertEquals("secret", cfg.get(TransportConstants.KEYSTORE_PASSWORD_PROP_NAME));
  }

  @Test
  void wssSchemeEnablesSslTransport() throws IOException {
    String keystore = newStore("broker-keystore.p12");
    String truststore = newStore("broker-truststore.p12");

    Map<String, Object> cfg =
        JmsClientFactory.transportConfig(
            URI.create("wss://broker:61617"), keystore, truststore, "secret", "artemis");

    assertEquals(Boolean.TRUE, cfg.get(TransportConstants.SSL_ENABLED_PROP_NAME));
    assertEquals(keystore, cfg.get(TransportConstants.KEYSTORE_PATH_PROP_NAME));
    assertEquals(truststore, cfg.get(TransportConstants.TRUSTSTORE_PATH_PROP_NAME));
  }

  @Test
  void secureSchemeWithoutMaterialFailsStartup() throws IOException {
    // Keystore env unset; truststore present to isolate the missing-material failure.
    String truststore = newStore("trust-only.p12");

    IllegalStateException ex =
        assertThrows(
            IllegalStateException.class,
            () ->
                JmsClientFactory.transportConfig(
                    URI.create("ssl://broker:61617"), null, truststore, "secret", "artemis"));
    assertTrue(ex.getMessage().contains("ssl"), ex.getMessage());
    assertTrue(ex.getMessage().contains("BRIDGE_BROKER_KEYSTORE_PATH"), ex.getMessage());
  }

  @Test
  void secureSchemeWithPlaceholderMaterialFailsStartup() throws IOException {
    // Truststore exists on disk but its path carries the placeholder marker
    // (mirroring PortAnnouncer's placeholder fail-closed guard).
    String placeholderKeystore = Files.createFile(tmp.resolve("placeholder-broker.p12")).toString();
    String truststore = newStore("trust-only.p12");

    IllegalStateException ex =
        assertThrows(
            IllegalStateException.class,
            () ->
                JmsClientFactory.transportConfig(
                    URI.create("ssl://broker:61617"),
                    placeholderKeystore,
                    truststore,
                    "secret",
                    "artemis"));
    assertTrue(ex.getMessage().contains("ssl"), ex.getMessage());
    assertTrue(ex.getMessage().contains("placeholder-"), ex.getMessage());
  }

  @Test
  void sslSchemeWithWrongBrokerTypeFailsLoud() throws IOException {
    String keystore = newStore("wrong-type-keystore.p12");
    String truststore = newStore("wrong-type-truststore.p12");

    IllegalStateException ex =
        assertThrows(
            IllegalStateException.class,
            () ->
                JmsClientFactory.transportConfig(
                    URI.create("ssl://broker:61617"), keystore, truststore, "secret", "activemq"));
    assertTrue(ex.getMessage().contains("ssl"), ex.getMessage());
    assertTrue(ex.getMessage().contains("activemq"), ex.getMessage());
  }

  @Test
  void tcpSchemeStaysPlaintext() {
    Map<String, Object> cfg =
        JmsClientFactory.transportConfig(
            URI.create("tcp://broker:61616"), null, null, null, "artemis");

    assertFalse(cfg.containsKey(TransportConstants.SSL_ENABLED_PROP_NAME));
  }

  /**
   * Post-bump API-compat smoke: both {@code createFactory()} dispatch branches must construct
   * offline against the pinned client versions — no broker connection, constructor signatures only.
   * The Artemis branch mirrors {@code buildArtemisFactory()} through the existing {@link
   * JmsClientFactory#transportConfig} explicit-values seam ({@code BRIDGE_BROKER_TYPE=artemis} ≙
   * brokerType argument); the Classic branch mirrors {@code buildActiveMqFactory()}'s URL-only
   * construction (null credentials skipped). Pooled wrappers follow {@code createFactory()}.
   */
  @Test
  void bothBrokerFactoriesConstruct() {
    // Artemis path (BRIDGE_BROKER_TYPE=artemis, tcp://): transport config -> direct Netty
    // TransportConfiguration -> ActiveMQConnectionFactory, bypassing URI parsing/BeanSupport.
    Map<String, Object> params =
        JmsClientFactory.transportConfig(
            URI.create("tcp://broker:61616"), null, null, null, "artemis");
    assertFalse(params.containsKey(TransportConstants.SSL_ENABLED_PROP_NAME));
    TransportConfiguration tc =
        new TransportConfiguration(NettyConnectorFactory.class.getName(), params);
    org.apache.activemq.artemis.jms.client.ActiveMQConnectionFactory artemisCf =
        new org.apache.activemq.artemis.jms.client.ActiveMQConnectionFactory(false, tc);

    // Classic path: url-only constructor; pooled wrapper takes ownership like createFactory().
    ActiveMQConnectionFactory classicCf = new ActiveMQConnectionFactory("tcp://broker:61616");

    // Pooled wrappers mirror createFactory(); ActiveMQ 5.19 start() may eagerly
    // attempt one pooled connection — failure is non-fatal (log noise only).
    JmsPoolConnectionFactory artemisPool = new JmsPoolConnectionFactory();
    artemisPool.setConnectionFactory(artemisCf);
    artemisPool.setMaxConnections(5);
    artemisPool.start();
    PooledConnectionFactory classicPool = new PooledConnectionFactory(classicCf);
    classicPool.setMaxConnections(5);
    classicPool.start();

    try {
      assertEquals("tcp://broker:61616", classicCf.getBrokerURL());
      assertEquals(artemisCf, artemisPool.getConnectionFactory());
    } finally {
      artemisPool.stop();
      classicPool.stop();
    }
  }
}
