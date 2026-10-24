package org.rustcamel.jms;

import static org.junit.jupiter.api.Assertions.assertDoesNotThrow;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertInstanceOf;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.net.URI;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.Arrays;
import java.util.List;
import java.util.stream.Collectors;
import javax.net.ssl.KeyManager;
import javax.net.ssl.TrustManager;
import javax.net.ssl.X509KeyManager;
import javax.net.ssl.X509TrustManager;
import org.apache.activemq.ActiveMQConnectionFactory;
import org.apache.activemq.management.JMSStatsImpl;
import org.apache.activemq.transport.Transport;
import org.apache.activemq.transport.TransportFactory;
import org.apache.activemq.transport.failover.FailoverTransport;
import org.junit.jupiter.api.DisplayName;
import org.junit.jupiter.api.Test;

/**
 * Unit tests for {@link BrokerTlsMaterial} and the TLS factory seam of {@link
 * JmsClientFactory#buildActiveMqFactory} (task 5.1). Fixtures under {@code
 * src/test/resources/tls-fixtures} are generated once by the {@link BrokerTlsMaterial} javadoc
 * commands and shared with later tasks; TLS env values are injected (System.getenv is immutable).
 */
@DisplayName("Broker TLS PEM material and factory seam")
class BrokerTlsMaterialTest {

  private static final JmsClientFactory.BrokerTlsEnv NO_TLS_ENV =
      new JmsClientFactory.BrokerTlsEnv(null, null, null, null);

  private static String fixture(String name) throws Exception {
    return Path.of(BrokerTlsMaterialTest.class.getResource("/tls-fixtures/" + name).toURI())
        .toString();
  }

  @Test
  void trustManagersFromPemBundle() throws Exception {
    TrustManager[] tm = BrokerTlsMaterial.buildTrustManagers(fixture("test-ca.pem"));

    assertTrue(tm.length > 0, "trust manager array non-empty");
    X509TrustManager x509 = assertInstanceOf(X509TrustManager.class, tm[0]);
    assertNotNull(x509.getAcceptedIssuers(), "accepted issuers present");
    assertTrue(x509.getAcceptedIssuers().length > 0, "accepted issuers non-empty");
  }

  @Test
  void keyManagersFromPkcs8Pair() throws Exception {
    KeyManager[] km =
        BrokerTlsMaterial.buildKeyManagers(
            fixture("client-cert.pem"), fixture("client-key-pkcs8.pem"));

    List<X509KeyManager> x509 =
        Arrays.stream(km)
            .filter(X509KeyManager.class::isInstance)
            .map(X509KeyManager.class::cast)
            .collect(Collectors.toList());
    assertEquals(1, x509.size(), "exactly one X509KeyManager");
    String[] aliases = x509.get(0).getClientAliases("RSA", null);
    assertNotNull(aliases, "RSA client aliases present");
    assertTrue(aliases.length > 0, "RSA client aliases non-empty");
    assertNotNull(x509.get(0).getPrivateKey(aliases[0]), "non-null private key");
  }

  @Test
  void encryptedKeyRejected() throws Exception {
    String encrypted = fixture("client-key-encrypted.pem");

    IllegalStateException ex =
        assertThrows(
            IllegalStateException.class,
            () -> BrokerTlsMaterial.buildKeyManagers(fixture("client-cert.pem"), encrypted));

    assertTrue(ex.getMessage().contains("PKCS#8"), ex.getMessage());
    // No key material may leak into the message: no base64 payload line of the encrypted key.
    List<String> payload =
        Files.readAllLines(Path.of(encrypted)).stream()
            .map(String::trim)
            .filter(l -> !l.isEmpty() && !l.startsWith("-----"))
            .collect(Collectors.toList());
    for (String line : payload) {
      assertFalse(
          ex.getMessage().contains(line.substring(0, Math.min(16, line.length()))),
          "message must not contain key material");
    }
  }

  @Test
  void factorySelectionByTransportSecurity() throws Exception {
    // ssl member + no TLS env -> BridgeActiveMqConnectionFactory in platform-trust mode (null
    // KM/TM; the branch logs at info).
    ActiveMQConnectionFactory platform =
        JmsClientFactory.buildActiveMqFactory(
            "failover:(ssl://a:1,ssl://b:2)", null, null, NO_TLS_ENV);
    BridgeActiveMqConnectionFactory platformBridge =
        assertInstanceOf(BridgeActiveMqConnectionFactory.class, platform);
    assertNull(platformBridge.configuredKeyManagers(), "platform-trust: no client identity");
    assertNull(platformBridge.configuredTrustManagers(), "platform-trust: no pinned trust");

    // TLS env set (injected) -> the same class in pinned-material mode.
    JmsClientFactory.BrokerTlsEnv tlsEnv =
        new JmsClientFactory.BrokerTlsEnv(
            fixture("test-ca.pem"),
            fixture("client-cert.pem"),
            fixture("client-key-pkcs8.pem"),
            "mq.internal.example");
    ActiveMQConnectionFactory pinned =
        JmsClientFactory.buildActiveMqFactory(
            "ssl://mq.internal.example:61617", null, null, tlsEnv);
    BridgeActiveMqConnectionFactory pinnedBridge =
        assertInstanceOf(BridgeActiveMqConnectionFactory.class, pinned);
    assertNotNull(pinnedBridge.configuredKeyManagers(), "pinned-material: client identity");
    assertNotNull(pinnedBridge.configuredTrustManagers(), "pinned-material: pinned trust");

    // tcp URL -> the plain factory exactly as today (regression).
    ActiveMQConnectionFactory plain =
        JmsClientFactory.buildActiveMqFactory("tcp://a:1", null, null, NO_TLS_ENV);
    assertEquals(ActiveMQConnectionFactory.class, plain.getClass());
  }

  @Test
  void platformTrustCompositeStillGuarded() throws Exception {
    ActiveMQConnectionFactory cf =
        JmsClientFactory.buildActiveMqFactory(
            "failover:(ssl://a:1,ssl://b:2)", null, null, NO_TLS_ENV);
    BridgeActiveMqConnectionFactory bridge =
        assertInstanceOf(BridgeActiveMqConnectionFactory.class, cf);

    Transport transport =
        TransportFactory.compositeConnect(URI.create("failover:(ssl://a:1,ssl://b:2)"));
    var connection = bridge.createActiveMQConnection(transport, new JMSStatsImpl());
    try {
      FailoverTransport failover = transport.narrow(FailoverTransport.class);
      assertNotNull(failover, "composite yields a FailoverTransport");
      // The guard is class-wide: it fires even in platform-trust mode (no TLS env).
      assertFalse(failover.isUpdateURIsSupported(), "updateURIs disabled");
      assertFalse(failover.isReconnectSupported(), "broker-directed reconnect disabled");
    } finally {
      assertDoesNotThrow(connection::close);
    }
  }
}
