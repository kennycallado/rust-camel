package org.rustcamel.jms;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertTrue;
import static org.mockito.ArgumentMatchers.any;
import static org.mockito.ArgumentMatchers.anyLong;
import static org.mockito.ArgumentMatchers.anyString;
import static org.mockito.Mockito.doAnswer;
import static org.mockito.Mockito.mock;
import static org.mockito.Mockito.never;
import static org.mockito.Mockito.verify;
import static org.mockito.Mockito.when;

import java.util.Collections;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;
import javax.jms.Connection;
import javax.jms.MapMessage;
import javax.jms.MessageConsumer;
import javax.jms.ObjectMessage;
import javax.jms.Queue;
import javax.jms.Session;
import javax.jms.StreamMessage;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.DisplayName;
import org.junit.jupiter.api.Test;
import org.mockito.Mock;
import org.mockito.MockitoAnnotations;

/**
 * Consumer handling of non-Bytes/Text message types (ADR-0067): {@code ObjectMessage} bodies are
 * NEVER deserialized — {@code getObject()} would run Java serialization gadget code — and {@code
 * MapMessage}/{@code StreamMessage} bodies are never read either. All three types forward with an
 * EMPTY body under an AUTO_ACKNOWLEDGE session, message properties preserved as headers; these
 * tests document exactly that behavior.
 */
@DisplayName("JmsConsumer unsupported message type policy")
class JmsConsumerMessageTypesTest {

  private static final String DESTINATION = "queue:test";
  private static final String SUBSCRIPTION_ID = "sub-message-types-test";

  @Mock JmsClientFactory factory;
  @Mock Connection connection;
  @Mock Session session;
  @Mock MessageConsumer messageConsumer;
  @Mock Queue queue;

  JmsConsumer consumer;

  private AutoCloseable mocks;

  @BeforeEach
  void setUp() throws Exception {
    mocks = MockitoAnnotations.openMocks(this);

    consumer = new JmsConsumer();
    consumer.factory = factory;

    when(factory.createDedicatedConnection()).thenReturn(connection);
    doAnswer(inv -> null).when(connection).start();
    when(connection.createSession(false, Session.AUTO_ACKNOWLEDGE)).thenReturn(session);
    when(session.createQueue("test")).thenReturn(queue);
    when(session.createConsumer(any(Queue.class))).thenReturn(messageConsumer);
  }

  @AfterEach
  void tearDown() {
    consumer.stop();
    try {
      mocks.close();
    } catch (Exception ignored) {
    }
  }

  /** Common header/property stubbing; convertMessage touches these before the type branch. */
  private static void stubCommonAttributes(javax.jms.Message msg) throws Exception {
    when(msg.getJMSMessageID()).thenReturn(null);
    when(msg.getJMSCorrelationID()).thenReturn(null);
    when(msg.getJMSTimestamp()).thenReturn(0L);
    when(msg.getPropertyNames()).thenReturn(Collections.emptyEnumeration());
  }

  @Test
  void unsupportedMessageTypeForwardedEmptyAndAcked() throws Exception {
    // ObjectMessage whose body is deliberately NOT stubbed: any getObject() call would return
    // null rather than throw, so 'never' verification below is what proves deserialization
    // stays absent.
    ObjectMessage objectMessage = mock(ObjectMessage.class);
    stubCommonAttributes(objectMessage);
    // stubCommonAttributes stubs an EMPTY property enumeration; headers-preserved needs its own.
    when(objectMessage.getPropertyNames())
        .thenReturn(Collections.enumeration(Collections.singletonList("X-Trace")));
    when(objectMessage.getObjectProperty("X-Trace")).thenReturn("m1");

    when(messageConsumer.receive(anyLong())).thenReturn(objectMessage).thenReturn(null);

    JmsConsumerBodyCapTest.RecordingObserver obs = new JmsConsumerBodyCapTest.RecordingObserver();
    consumer.subscribe(DESTINATION, SUBSCRIPTION_ID, obs, new AtomicBoolean(false));
    assertTrue(obs.next.await(15, TimeUnit.SECONDS), "unsupported type must still be forwarded");
    consumer.stop();

    assertEquals(1, obs.nextCount, "exactly one message delivered");
    assertEquals(0, obs.last.get().getBody().size(), "body must be forwarded EMPTY");
    assertEquals("", obs.last.get().getContentType(), "no content-type for unknown payloads");
    assertNull(obs.error.get(), "forwarding must not surface an error outcome");
    assertEquals(
        "m1", obs.last.get().getHeadersMap().get("X-Trace"), "properties must survive as headers");

    // Ack policy: the session runs AUTO_ACKNOWLEDGE, so receipt itself acknowledges the
    // message — convertMessage must not need any explicit acknowledge() call.
    verify(connection).createSession(false, Session.AUTO_ACKNOWLEDGE);

    // ADR-0067: deserialization stays absent — getObject() must never be invoked.
    verify(objectMessage, never()).getObject();
    verify(objectMessage, never()).acknowledge();
  }

  @Test
  void mapMessageForwardedEmptyAndAcked() throws Exception {
    MapMessage mapMessage = mock(MapMessage.class);
    stubCommonAttributes(mapMessage);
    // stubCommonAttributes stubs an EMPTY property enumeration; headers-preserved needs its own.
    when(mapMessage.getPropertyNames())
        .thenReturn(Collections.enumeration(Collections.singletonList("X-Trace")));
    when(mapMessage.getObjectProperty("X-Trace")).thenReturn("m1");

    when(messageConsumer.receive(anyLong())).thenReturn(mapMessage).thenReturn(null);

    JmsConsumerBodyCapTest.RecordingObserver obs = new JmsConsumerBodyCapTest.RecordingObserver();
    consumer.subscribe(DESTINATION, SUBSCRIPTION_ID, obs, new AtomicBoolean(false));
    assertTrue(obs.next.await(15, TimeUnit.SECONDS), "MapMessage must still be forwarded");
    consumer.stop();

    assertEquals(1, obs.nextCount, "exactly one message delivered");
    assertEquals(0, obs.last.get().getBody().size(), "body must be forwarded EMPTY");
    assertNull(obs.error.get(), "forwarding must not surface an error outcome");
    assertEquals(
        "m1", obs.last.get().getHeadersMap().get("X-Trace"), "properties must survive as headers");

    // Ack policy: the session runs AUTO_ACKNOWLEDGE, so receipt itself acknowledges the
    // message — convertMessage must not need any explicit acknowledge() call.
    verify(connection).createSession(false, Session.AUTO_ACKNOWLEDGE);

    // ADR-0067: map entries stay unread — forwarding carries properties, never the map body.
    verify(mapMessage, never()).getMapNames();
    verify(mapMessage, never()).getObject(anyString());
    verify(mapMessage, never()).acknowledge();
  }

  @Test
  void streamMessageForwardedEmptyAndAcked() throws Exception {
    StreamMessage streamMessage = mock(StreamMessage.class);
    stubCommonAttributes(streamMessage);
    // stubCommonAttributes stubs an EMPTY property enumeration; headers-preserved needs its own.
    when(streamMessage.getPropertyNames())
        .thenReturn(Collections.enumeration(Collections.singletonList("X-Trace")));
    when(streamMessage.getObjectProperty("X-Trace")).thenReturn("m1");

    when(messageConsumer.receive(anyLong())).thenReturn(streamMessage).thenReturn(null);

    JmsConsumerBodyCapTest.RecordingObserver obs = new JmsConsumerBodyCapTest.RecordingObserver();
    consumer.subscribe(DESTINATION, SUBSCRIPTION_ID, obs, new AtomicBoolean(false));
    assertTrue(obs.next.await(15, TimeUnit.SECONDS), "StreamMessage must still be forwarded");
    consumer.stop();

    assertEquals(1, obs.nextCount, "exactly one message delivered");
    assertEquals(0, obs.last.get().getBody().size(), "body must be forwarded EMPTY");
    assertNull(obs.error.get(), "forwarding must not surface an error outcome");
    assertEquals(
        "m1", obs.last.get().getHeadersMap().get("X-Trace"), "properties must survive as headers");

    // Ack policy: the session runs AUTO_ACKNOWLEDGE, so receipt itself acknowledges the
    // message — convertMessage must not need any explicit acknowledge() call.
    verify(connection).createSession(false, Session.AUTO_ACKNOWLEDGE);

    // ADR-0067: the stream stays unread — forwarding carries properties, never stream fields.
    verify(streamMessage, never()).readInt();
    verify(streamMessage, never()).readString();
    verify(streamMessage, never()).readBytes(any());
    verify(streamMessage, never()).acknowledge();
  }
}
