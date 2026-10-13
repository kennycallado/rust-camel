package org.rustcamel.jms;

import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.lang.reflect.Field;
import org.apache.activemq.ActiveMQConnectionFactory;
import org.junit.jupiter.api.DisplayName;
import org.junit.jupiter.api.Test;

/**
 * Pins {@link JmsClientFactory}'s ActiveMQ Classic prefix precedence: when the broker URL carries
 * an explicit {@code jms.connectionIDPrefix} (or {@code jms.clientIDPrefix}) the parsed value
 * survives as the base and a per-process token is appended; otherwise the per-process token stands
 * alone.
 *
 * <p>Cross-process uniqueness (two distinct effective IDs on a live broker) is proven by the native
 * live-broker task 6.5, not here: within one JVM the token is intentionally stable.
 */
@DisplayName("JmsClientFactory connection/client ID prefix precedence")
class JmsClientFactoryPrefixTest {

  private static final String SENTINEL = "explicit-sentinel-prefix";
  private static final String RANDOM_TOKEN_PATTERN =
      ".*[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}.*";

  private static ActiveMQConnectionFactory build(String url) {
    return new JmsClientFactory().buildActiveMqFactory(url, null, null);
  }

  /**
   * Reads the effective connection-ID prefix. ActiveMQ 5.19.x exposes {@code setConnectionIDPrefix}
   * but no getter, so the field is read reflectively (the same seam the JVM-only integration proof
   * cannot exercise).
   */
  private static String connectionPrefix(ActiveMQConnectionFactory cf) throws Exception {
    Field field = ActiveMQConnectionFactory.class.getDeclaredField("connectionIDPrefix");
    field.setAccessible(true);
    return (String) field.get(cf);
  }

  @Test
  void explicit_prefix_survives_with_process_token() throws Exception {
    ActiveMQConnectionFactory cf =
        build(
            "tcp://localhost:61616?jms.connectionIDPrefix="
                + SENTINEL
                + "&jms.clientIDPrefix="
                + SENTINEL);

    String connection = connectionPrefix(cf);
    assertNotNull(connection, "effective connection ID prefix must be non-null");
    assertTrue(connection.contains(SENTINEL), "explicit base must survive: " + connection);
    assertTrue(
        connection.matches(RANDOM_TOKEN_PATTERN),
        "per-process token must be appended: " + connection);

    String client = cf.getClientIDPrefix();
    assertNotNull(client, "effective client ID prefix must be non-null");
    assertTrue(client.contains(SENTINEL), "explicit base must survive: " + client);
    assertTrue(
        client.matches(RANDOM_TOKEN_PATTERN), "per-process token must be appended: " + client);
  }

  @Test
  void no_explicit_prefix_uses_random_token() throws Exception {
    ActiveMQConnectionFactory cf = build("tcp://localhost:61616");

    String connection = connectionPrefix(cf);
    assertNotNull(connection, "effective connection ID prefix must be non-null");
    assertTrue(
        connection.matches(RANDOM_TOKEN_PATTERN), "a random token must stand alone: " + connection);

    String client = cf.getClientIDPrefix();
    assertNotNull(client, "effective client ID prefix must be non-null");
    assertTrue(client.matches(RANDOM_TOKEN_PATTERN), "a random token must stand alone: " + client);
  }

  @Test
  void plain_ip_literal_url_without_query_is_accepted() throws Exception {
    // A plain URL whose scheme-specific part starts with a digit (an IP-literal host, the shape
    // every real fixture uses) is not composite. URISupport.parseComposite would strip "//" and
    // reparse "127.0.0.1:61616" as a URI ("Illegal character in scheme name"); the plain no-query
    // path must not attempt composite parsing.
    ActiveMQConnectionFactory cf = build("tcp://127.0.0.1:61616");

    String connection = connectionPrefix(cf);
    assertNotNull(connection, "effective connection ID prefix must be non-null");
    assertTrue(
        connection.matches(RANDOM_TOKEN_PATTERN), "a random token must stand alone: " + connection);

    String client = cf.getClientIDPrefix();
    assertNotNull(client, "effective client ID prefix must be non-null");
    assertTrue(client.matches(RANDOM_TOKEN_PATTERN), "a random token must stand alone: " + client);
  }

  @Test
  void composite_url_outer_option_is_read() throws Exception {
    // A composite URI (failover:) is opaque, so it has no plain query; its outer parameters must
    // still be read. The client base is ActiveMQ-owned here — this pins the connection option that
    // JmsClientFactory itself parses.
    ActiveMQConnectionFactory cf =
        build("failover:(tcp://127.0.0.1:61616)?jms.connectionIDPrefix=" + SENTINEL);

    String connection = connectionPrefix(cf);
    assertNotNull(connection, "effective connection ID prefix must be non-null");
    assertTrue(connection.contains(SENTINEL), "composite outer base must survive: " + connection);
    assertTrue(
        connection.matches(RANDOM_TOKEN_PATTERN),
        "per-process token must be appended: " + connection);
  }

  @Test
  void whitespace_prefix_is_preserved_with_process_token() throws Exception {
    // A whitespace-only base (%20) is non-empty, not absent: it must survive as a real base with
    // the separator and per-process token appended (isEmpty, not isBlank).
    ActiveMQConnectionFactory cf =
        build("tcp://localhost:61616?jms.connectionIDPrefix=%20&jms.clientIDPrefix=%20");

    String connection = connectionPrefix(cf);
    assertNotNull(connection, "effective connection ID prefix must be non-null");
    assertTrue(connection.startsWith(" -"), "whitespace base must survive: " + connection);
    assertTrue(
        connection.matches(RANDOM_TOKEN_PATTERN),
        "per-process token must be appended: " + connection);

    String client = cf.getClientIDPrefix();
    assertNotNull(client, "effective client ID prefix must be non-null");
    assertTrue(client.startsWith(" -"), "whitespace base must survive: " + client);
    assertTrue(
        client.matches(RANDOM_TOKEN_PATTERN), "per-process token must be appended: " + client);
  }
}
