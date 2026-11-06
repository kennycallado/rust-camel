package org.rustcamel.jms;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;
import static org.mockito.ArgumentMatchers.any;
import static org.mockito.ArgumentMatchers.anyString;
import static org.mockito.ArgumentMatchers.eq;
import static org.mockito.Mockito.doThrow;
import static org.mockito.Mockito.mock;
import static org.mockito.Mockito.never;
import static org.mockito.Mockito.verify;
import static org.mockito.Mockito.when;

import java.nio.charset.StandardCharsets;
import java.util.LinkedHashMap;
import java.util.Map;
import javax.jms.BytesMessage;
import javax.jms.Connection;
import javax.jms.JMSException;
import javax.jms.MessageProducer;
import javax.jms.Session;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;

/**
 * Outbound property-write rejection semantics of {@link JmsProducer#send}.
 *
 * <p>A broker or JMS client rejection of a property write must fail the send with a typed {@link
 * InvalidPropertyException} that names every offending property and carries no value, provider
 * text, or connection detail. Legal-name writes must still be attempted, and denied names must
 * remain skipped.
 */
class JmsProducerInvalidPropertyTest {

  private static final byte[] BODY = "b".getBytes(StandardCharsets.UTF_8);

  private JmsClientFactory factory;
  private Connection connection;
  private Session session;
  private BytesMessage msg;
  private MessageProducer messageProducer;
  private JmsProducer producer;

  @BeforeEach
  void setUp() throws Exception {
    factory = mock(JmsClientFactory.class);
    connection = mock(Connection.class);
    session = mock(Session.class);
    msg = mock(BytesMessage.class);
    messageProducer = mock(MessageProducer.class);

    when(factory.createConnection()).thenReturn(connection);
    when(connection.createSession(false, Session.AUTO_ACKNOWLEDGE)).thenReturn(session);
    when(session.createBytesMessage()).thenReturn(msg);
    when(session.createProducer(any())).thenReturn(messageProducer);
    when(msg.getJMSMessageID()).thenReturn("ID:test-1");

    producer = new JmsProducer();
    producer.factory = factory;
  }

  private static Map<String, String> headers(String... pairs) {
    Map<String, String> map = new LinkedHashMap<>();
    for (int i = 0; i < pairs.length; i += 2) {
      map.put(pairs[i], pairs[i + 1]);
    }
    return map;
  }

  @Test
  void illegal_property_write_fails_send_with_typed_error() throws Exception {
    Map<String, String> headers = headers("x-crash-proof", "proof-value");
    doThrow(new JMSException("not a valid identifier"))
        .when(msg)
        .setStringProperty(eq("x-crash-proof"), anyString());

    InvalidPropertyException thrown =
        assertThrows(
            InvalidPropertyException.class, () -> producer.send("queue:t", BODY, headers, null));

    assertTrue(
        thrown.getMessage().contains("x-crash-proof"),
        "message must name the offending property: " + thrown.getMessage());
    verify(messageProducer, never()).send(any());
  }

  @Test
  void error_text_carries_names_not_values_or_hosts() throws Exception {
    Map<String, String> headers = headers("x-crash-proof", "secret-value-123");
    doThrow(new JMSException("not a valid identifier"))
        .when(msg)
        .setStringProperty(eq("x-crash-proof"), anyString());

    InvalidPropertyException thrown =
        assertThrows(
            InvalidPropertyException.class, () -> producer.send("queue:t", BODY, headers, null));
    String message = thrown.getMessage();

    assertTrue(message.contains("x-crash-proof"), "message must name the property: " + message);
    assertFalse(
        message.contains("secret-value-123"), "message must not carry the value: " + message);
    assertFalse(
        message.contains("not a valid identifier"),
        "message must not append provider text: " + message);
    assertFalse(message.contains("localhost"), "message must not carry a host: " + message);
    assertFalse(message.contains("tcp://"), "message must not carry a URL: " + message);
  }

  @Test
  void multiple_offenders_all_named_and_legal_write_attempted() throws Exception {
    Map<String, String> headers = headers("bad-one", "v1", "traceId", "v2", "bad-two", "v3");
    doThrow(new JMSException("not a valid identifier"))
        .when(msg)
        .setStringProperty(eq("bad-one"), anyString());
    doThrow(new JMSException("not a valid identifier"))
        .when(msg)
        .setStringProperty(eq("bad-two"), anyString());

    InvalidPropertyException thrown =
        assertThrows(
            InvalidPropertyException.class, () -> producer.send("queue:t", BODY, headers, null));

    assertTrue(thrown.getMessage().contains("bad-one"), thrown.getMessage());
    assertTrue(thrown.getMessage().contains("bad-two"), thrown.getMessage());
    verify(msg).setStringProperty("traceId", "v2");
  }

  @Test
  void legal_names_forward_and_send_succeeds() throws Exception {
    Map<String, String> headers = headers("traceId", "abc");

    String messageId = producer.send("queue:t", BODY, headers, null);

    assertEquals("ID:test-1", messageId);
    verify(msg).setStringProperty("traceId", "abc");
    verify(messageProducer).send(any());
  }

  @Test
  void denied_names_still_skipped() throws Exception {
    Map<String, String> headers = headers("password", "x", "traceId", "abc");

    String messageId = producer.send("queue:t", BODY, headers, null);

    assertEquals("ID:test-1", messageId);
    verify(msg, never()).setStringProperty(eq("password"), anyString());
    verify(msg).setStringProperty("traceId", "abc");
  }
}
