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
import static org.mockito.Mockito.timeout;
import static org.mockito.Mockito.times;
import static org.mockito.Mockito.verify;
import static org.mockito.Mockito.when;

import java.nio.file.Path;
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
import jms_bridge.CompletionOutcome;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.DisplayName;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;
import org.mockito.Mock;
import org.mockito.MockitoAnnotations;

/**
 * Consumer handling of non-Bytes/Text message types (ADR-0067): {@code ObjectMessage} bodies are
 * NEVER deserialized — {@code getObject()} would run Java serialization gadget code — and {@code
 * MapMessage}/{@code StreamMessage} bodies are never read either. All three types forward with an
 * EMPTY body under the bridge-managed CLIENT_ACKNOWLEDGE session, message properties preserved as
 * headers; these tests document exactly that behavior. No acknowledgement happens before a route
 * completion decision, so a stopped consumer leaves the delivery unacknowledged.
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

  @TempDir Path tempDir;

  JmsConsumer consumer;

  private AutoCloseable mocks;

  @BeforeEach
  void setUp() throws Exception {
    mocks = MockitoAnnotations.openMocks(this);

    consumer = new JmsConsumer();
    consumer.factory = factory;
    // A queue delivery is keyed and reserved, so install a legitimately configured journal.
    consumer.configureJournal(
        new JournalService(
            tempDir,
            BridgeConfig.JOURNAL_MAX_BYTES_DEFAULT,
            BridgeConfig.JOURNAL_RETENTION_MS_DEFAULT,
            System::currentTimeMillis),
        "0123456789abcdef0123456789abcdef",
        1000L,
        null);

    when(factory.createDedicatedConnection()).thenReturn(connection);
    doAnswer(inv -> null).when(connection).start();
    when(connection.createSession(false, Session.CLIENT_ACKNOWLEDGE)).thenReturn(session);
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
    when(msg.getJMSMessageID()).thenReturn("ID:test");
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

    // Ack policy: bridge-managed CLIENT_ACKNOWLEDGE settlement — no acknowledgement happens
    // until the route completion decision, so a stopped consumer leaves the delivery pending.
    verify(connection).createSession(false, Session.CLIENT_ACKNOWLEDGE);

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

    // Ack policy: bridge-managed CLIENT_ACKNOWLEDGE settlement — no acknowledgement happens
    // until the route completion decision, so a stopped consumer leaves the delivery pending.
    verify(connection).createSession(false, Session.CLIENT_ACKNOWLEDGE);

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

    // Ack policy: bridge-managed CLIENT_ACKNOWLEDGE settlement — no acknowledgement happens
    // until the route completion decision, so a stopped consumer leaves the delivery pending.
    verify(connection).createSession(false, Session.CLIENT_ACKNOWLEDGE);

    // ADR-0067: the stream stays unread — forwarding carries properties, never stream fields.
    verify(streamMessage, never()).readInt();
    verify(streamMessage, never()).readString();
    verify(streamMessage, never()).readBytes(any());
    verify(streamMessage, never()).acknowledge();
  }

  /**
   * One subscription forwards Object, Map, and Stream messages under a bridge-managed {@code
   * CLIENT_ACKNOWLEDGE} session. No body accessor runs, and no message is acknowledged before its
   * route reports success.
   */
  @Test
  void message_types_use_client_acknowledge() throws Exception {
    ObjectMessage objectMessage = mock(ObjectMessage.class);
    stubCommonAttributes(objectMessage);
    MapMessage mapMessage = mock(MapMessage.class);
    stubCommonAttributes(mapMessage);
    StreamMessage streamMessage = mock(StreamMessage.class);
    stubCommonAttributes(streamMessage);

    when(messageConsumer.receive(anyLong()))
        .thenReturn(objectMessage)
        .thenReturn(mapMessage)
        .thenReturn(streamMessage)
        .thenAnswer(
            invocation -> {
              Thread.sleep(200);
              return null;
            });

    JmsConsumerBodyCapTest.RecordingObserver obs = new JmsConsumerBodyCapTest.RecordingObserver();
    consumer.subscribe(DESTINATION, SUBSCRIPTION_ID, obs, new AtomicBoolean(false));

    // Bridge-managed settlement: the session is always CLIENT_ACKNOWLEDGE. No
    // acknowledgementMode/transactionMode option reaches Java over the proto, so none can change
    // it.
    // The owner thread opens the session asynchronously, so wait for it.
    verify(connection, timeout(15000)).createSession(false, Session.CLIENT_ACKNOWLEDGE);

    // ObjectMessage: empty body, never deserialized, ack only after the route commits.
    awaitDelivery(obs, 1);
    assertEquals(0, obs.last.get().getBody().size(), "ObjectMessage body must be forwarded empty");
    verify(objectMessage, never()).getObject();
    verify(objectMessage, never()).acknowledge();
    assertTrue(
        consumer.complete(
            consumer.subscriptionGeneration(),
            obs.last.get().getDeliveryToken(),
            CompletionOutcome.COMMIT),
        "COMMIT after route success must be accepted");
    verify(objectMessage, times(1)).acknowledge();

    // MapMessage: entries stay unread.
    awaitDelivery(obs, 2);
    assertEquals(0, obs.last.get().getBody().size(), "MapMessage body must be forwarded empty");
    verify(mapMessage, never()).getMapNames();
    verify(mapMessage, never()).getObject(anyString());
    verify(mapMessage, never()).acknowledge();
    assertTrue(
        consumer.complete(
            consumer.subscriptionGeneration(),
            obs.last.get().getDeliveryToken(),
            CompletionOutcome.COMMIT));
    verify(mapMessage, times(1)).acknowledge();

    // StreamMessage: fields stay unread.
    awaitDelivery(obs, 3);
    assertEquals(0, obs.last.get().getBody().size(), "StreamMessage body must be forwarded empty");
    verify(streamMessage, never()).readInt();
    verify(streamMessage, never()).readString();
    verify(streamMessage, never()).readBytes(any());
    verify(streamMessage, never()).acknowledge();
    assertTrue(
        consumer.complete(
            consumer.subscriptionGeneration(),
            obs.last.get().getDeliveryToken(),
            CompletionOutcome.COMMIT));
    verify(streamMessage, times(1)).acknowledge();

    consumer.stop();
    // The only ack before route success is the terminal reject-then-log discard at
    // JMS_MAX_DELIVERY_ATTEMPTS (task 3.8); it is not wired in this task, so every ack above
    // follows a COMMIT.
    assertNull(obs.error.get(), "no conversion failure means the stream stays healthy");
  }

  /**
   * Waits until the owner thread has forwarded {@code count} deliveries. The observer's one-shot
   * latch only reports the first, so poll its counter for the later ones.
   */
  private static void awaitDelivery(JmsConsumerBodyCapTest.RecordingObserver obs, int count)
      throws InterruptedException {
    long deadline = System.nanoTime() + TimeUnit.SECONDS.toNanos(15);
    while (obs.nextCount < count && System.nanoTime() < deadline) {
      Thread.sleep(10);
    }
    assertEquals(count, obs.nextCount, "delivery " + count + " must be forwarded");
  }
}
