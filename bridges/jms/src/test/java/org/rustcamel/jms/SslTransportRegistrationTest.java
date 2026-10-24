package org.rustcamel.jms;

import static org.junit.jupiter.api.Assertions.assertDoesNotThrow;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.net.URI;
import java.nio.file.Path;
import java.util.Map;
import org.apache.activemq.transport.TransportFactory;
import org.junit.jupiter.api.DisplayName;
import org.junit.jupiter.api.Test;

/**
 * Unit tests for the bridge-owned {@code ssl} transport registration and the startup scheme
 * self-check (task 4.1), plus the TLS-store URL-option rejection seam of {@link
 * JmsClientFactory#buildActiveMqFactory} (task 6.1). {@link
 * JmsClientFactory#registerTransportFactories()} is the testability seam: it registers the
 * transport factories without the full native workaround init.
 */
@DisplayName("JmsClientFactory ssl transport registration")
class SslTransportRegistrationTest {

  private static final String SSL_SERVICE = "META-INF/services/org/apache/activemq/transport/ssl";

  private static final JmsClientFactory.BrokerTlsEnv NO_TLS_ENV =
      new JmsClientFactory.BrokerTlsEnv(null, null, null, null);

  private static String fixture(String name) throws Exception {
    return Path.of(SslTransportRegistrationTest.class.getResource("/tls-fixtures/" + name).toURI())
        .toString();
  }

  @Test
  void sslSchemeResolvesAfterRegistration() throws Exception {
    JmsClientFactory.registerTransportFactories();

    TransportFactory factory =
        TransportFactory.findTransportFactory(new URI("ssl://localhost:61617"));

    assertTrue(
        factory instanceof CamelSslTransportFactory,
        () -> "expected CamelSslTransportFactory, got " + factory.getClass().getName());
  }

  @Test
  void knownServicesContainsSsl() {
    JmsClientFactory.registerTransportFactories();

    Map<String, ?> services = JmsClientFactory.knownServices();

    assertTrue(services.containsKey(SSL_SERVICE), () -> "knownServices missing " + SSL_SERVICE);
  }

  @Test
  void verifySchemesRejectsUnknown() {
    JmsClientFactory.registerTransportFactories();

    IllegalStateException error =
        assertThrows(
            IllegalStateException.class,
            () -> JmsClientFactory.verifySchemesResolvable("qls://h:1"));

    String message = error.getMessage();
    assertTrue(message.contains("UNSUPPORTED_TRANSPORT"), message);
    assertTrue(message.contains("qls"), message);
    // Redaction contract: the message carries the redacted URL, never host credentials.
    assertFalse(message.contains("@"), message);
    assertFalse(message.contains("password"), message);
    assertFalse(message.contains("secret"), message);
  }

  @Test
  void verifySchemesRedactsCompositeCredentials() {
    JmsClientFactory.registerTransportFactories();

    IllegalStateException error =
        assertThrows(
            IllegalStateException.class,
            () ->
                JmsClientFactory.verifySchemesResolvable(
                    "failover:(ssl://a:1?x=1,qls://alice:secret@b:2)"));

    String message = error.getMessage();
    assertFalse(message.contains("alice"), message);
    assertFalse(message.contains("secret"), message);
    assertTrue(message.contains("UNSUPPORTED_TRANSPORT"), message);
    assertTrue(message.contains("qls"), message);
    // Each member is redacted independently; the offending member keeps its masked userinfo shape.
    assertTrue(message.contains("qls://***@b:2"), message);
  }

  @Test
  void verifySchemesAcceptsComposite() {
    JmsClientFactory.registerTransportFactories();

    assertDoesNotThrow(
        () ->
            JmsClientFactory.verifySchemesResolvable(
                "failover:(ssl://a:1,ssl://b:2)?maxReconnectAttempts=3"));
  }

  @Test
  void registrationIsIdempotent() {
    JmsClientFactory.registerTransportFactories();
    JmsClientFactory.registerTransportFactories();
  }

  @Test
  void tlsStoreUrlOptionRejectedInBridge() {
    IllegalStateException error =
        assertThrows(
            IllegalStateException.class,
            () ->
                JmsClientFactory.buildActiveMqFactory(
                    "ssl://h:1?jms.trustStore=store-value-marker-42", null, null, NO_TLS_ENV));

    String message = error.getMessage();
    assertTrue(message.contains("UNSUPPORTED_TRANSPORT"), message);
    assertTrue(message.contains("jms.trustStore"), message);
    // The message names the rejected option; it never echoes the option value.
    assertFalse(message.contains("store-value-marker-42"), message);
  }

  @Test
  void pinnedTrustSurvivesUrlStoreSettings() throws Exception {
    // Pin-preservation regression: the expert's issuer-count scenario (pinned issuer count 1 -> 111
    // with JDK cacerts once ActiveMQ's bean-setting + createTransport() rebuilt the managers from
    // the URL-carried store) is UNREACHABLE once the rejection is in place — the rejected URL never
    // reaches the factory, so the tls-table pinning can never be overridden. Asserting the
    // rejection IS the regression; the Rust validation layer rejects the same URL even earlier.
    JmsClientFactory.BrokerTlsEnv pinned =
        new JmsClientFactory.BrokerTlsEnv(fixture("test-ca.pem"), null, null, null);
    IllegalStateException error =
        assertThrows(
            IllegalStateException.class,
            () ->
                JmsClientFactory.buildActiveMqFactory(
                    "ssl://h:1?jms.trustStore=store-value-marker-42", null, null, pinned));
    assertTrue(error.getMessage().contains("jms.trustStore"), error.getMessage());
  }
}
