package org.rustcamel.jms;

import static org.junit.jupiter.api.Assertions.assertArrayEquals;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertTrue;
import static org.mockito.ArgumentMatchers.any;
import static org.mockito.ArgumentMatchers.anyLong;
import static org.mockito.Mockito.doAnswer;
import static org.mockito.Mockito.doThrow;
import static org.mockito.Mockito.inOrder;
import static org.mockito.Mockito.mock;
import static org.mockito.Mockito.never;
import static org.mockito.Mockito.timeout;
import static org.mockito.Mockito.times;
import static org.mockito.Mockito.verify;
import static org.mockito.Mockito.when;

import java.nio.file.Path;
import java.util.Collections;
import java.util.List;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;
import javax.jms.BytesMessage;
import javax.jms.Connection;
import javax.jms.JMSException;
import javax.jms.MessageConsumer;
import javax.jms.Queue;
import javax.jms.Session;
import javax.jms.TextMessage;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.DisplayName;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;
import org.mockito.InOrder;
import org.mockito.Mock;
import org.mockito.MockitoAnnotations;

/**
 * Content-type fidelity for forwarded messages (rc-kzti): a {@code ContentType} string property on
 * the JMS message overrides the default {@code text/plain} content type of TextMessage bodies,
 * while BytesMessage stays content-type-less regardless of the property. The property itself keeps
 * flowing through the generic headers map.
 */
@DisplayName("JmsConsumer content type handling")
class JmsConsumerContentTypeTest {

  private static final String DESTINATION = "queue:test";
  private static final String SUBSCRIPTION_ID = "sub-content-type-test";
  private static final long STOP_TIMEOUT_MS = 15_000L;

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
    assertTrue(
        consumer.awaitStopped(STOP_TIMEOUT_MS),
        "the owner thread must release the journal before the temp-dir teardown");
  }

  /** Common header/property stubbing; convertMessage touches these before the type branch. */
  private static void stubCommonAttributes(javax.jms.Message msg) throws Exception {
    when(msg.getJMSMessageID()).thenReturn("ID:test");
    when(msg.getJMSCorrelationID()).thenReturn(null);
    when(msg.getJMSTimestamp()).thenReturn(0L);
    when(msg.getPropertyNames()).thenReturn(Collections.emptyEnumeration());
  }

  /** Subscribes, waits for the single forwarded message, then stops the consumer. */
  private void forwardOne(javax.jms.Message msg, JmsConsumerBodyCapTest.RecordingObserver obs)
      throws Exception {
    when(messageConsumer.receive(anyLong())).thenReturn(msg).thenReturn(null);
    consumer.subscribe(DESTINATION, SUBSCRIPTION_ID, obs, new AtomicBoolean(false));
    assertTrue(obs.next.await(15, TimeUnit.SECONDS), "message must be forwarded");
    consumer.stop();
    // A successful conversion is not a reservation failure: recover only runs on conversion
    // failure, so a forwarded message never recovers the session.
    verify(session, never()).recover();
  }

  /**
   * A conversion failure is a reservation failure, not a route error. The owner thread must recover
   * the session before it receives another message and must never acknowledge a message the route
   * never saw. A recover failure is the only terminal outcome.
   */
  @Test
  void conversion_failure_recovers_before_next_receive() throws Exception {
    javax.jms.Message poison = mock(javax.jms.Message.class);
    // A keyable message id lets the durable reservation run first; the conversion then fails on a
    // later accessor, so the same attempt is rolled back and the session recovered.
    when(poison.getJMSMessageID()).thenReturn("ID:poison");
    when(poison.getJMSCorrelationID()).thenThrow(new JMSException("bad conversion"));
    when(messageConsumer.receive(anyLong()))
        .thenReturn(poison)
        .thenAnswer(
            invocation -> {
              Thread.sleep(200);
              return null;
            });

    JmsConsumerBodyCapTest.RecordingObserver obs = new JmsConsumerBodyCapTest.RecordingObserver();
    consumer.subscribe(DESTINATION, SUBSCRIPTION_ID, obs, new AtomicBoolean(false));

    // Wait until the owner thread has received once, recovered, and received again, then assert
    // the ordering receive -> recover -> next receive without a polling in-order verification.
    verify(session, timeout(15000)).recover();
    verify(messageConsumer, timeout(15000).times(2)).receive(anyLong());
    InOrder inOrder = inOrder(messageConsumer, session);
    inOrder.verify(messageConsumer).receive(anyLong());
    inOrder.verify(session).recover();
    inOrder.verify(messageConsumer).receive(anyLong());

    assertEquals(
        1, consumer.reservationFailureCount(), "one conversion failure is one reservation failure");
    verify(poison, never()).acknowledge();
    assertEquals(0, obs.nextCount, "the poison message must not be forwarded");
    assertNull(obs.error.get(), "a recoverable conversion failure must not terminate the stream");
    consumer.stop();
  }

  @Test
  void conversion_failure_recover_failure_closes_without_ack() throws Exception {
    javax.jms.Message poison = mock(javax.jms.Message.class);
    when(poison.getJMSMessageID()).thenReturn("ID:poison");
    when(poison.getJMSCorrelationID()).thenThrow(new JMSException("bad conversion"));
    doThrow(new JMSException("recover failed")).when(session).recover();
    when(messageConsumer.receive(anyLong()))
        .thenReturn(poison)
        .thenAnswer(
            invocation -> {
              Thread.sleep(200);
              return null;
            });

    JmsConsumerBodyCapTest.RecordingObserver obs = new JmsConsumerBodyCapTest.RecordingObserver();
    consumer.subscribe(DESTINATION, SUBSCRIPTION_ID, obs, new AtomicBoolean(false));

    assertTrue(obs.errored.await(15, TimeUnit.SECONDS), "a recover failure must fail the stream");
    assertNotNull(obs.error.get(), "the recover failure must reach the observer");
    assertEquals(
        1, consumer.reservationFailureCount(), "one conversion failure is one reservation failure");
    verify(session, times(1)).recover();
    verify(poison, never()).acknowledge();
    // Recover failed: no second receive, and the owner closes every resource on its own thread.
    verify(messageConsumer, times(1)).receive(anyLong());
    verify(messageConsumer, timeout(5000)).close();
    verify(session, timeout(5000)).close();
    verify(connection, timeout(5000)).close();
  }

  @Test
  void contentTypePropertyPreservedOnTextMessage() throws Exception {
    TextMessage tm = mock(TextMessage.class);
    stubCommonAttributes(tm);
    when(tm.getStringProperty("ContentType")).thenReturn("application/xml");
    when(tm.getText()).thenReturn("<a/>");
    when(tm.getPropertyNames()).thenReturn(Collections.enumeration(List.of("ContentType")));
    when(tm.getObjectProperty("ContentType")).thenReturn("application/xml");

    JmsConsumerBodyCapTest.RecordingObserver obs = new JmsConsumerBodyCapTest.RecordingObserver();
    forwardOne(tm, obs);

    assertEquals(1, obs.nextCount, "exactly one message delivered");
    assertEquals("application/xml", obs.last.get().getContentType());
    assertEquals("<a/>", obs.last.get().getBody().toStringUtf8());
    assertEquals("application/xml", obs.last.get().getHeadersMap().get("ContentType"));
    assertNull(obs.error.get(), "forwarding must not surface an error outcome");
  }

  @Test
  void absentContentTypeFallsBackToTextPlain() throws Exception {
    TextMessage tm = mock(TextMessage.class);
    stubCommonAttributes(tm);
    when(tm.getStringProperty("ContentType")).thenReturn(null);
    when(tm.getText()).thenReturn("hello");

    JmsConsumerBodyCapTest.RecordingObserver obs = new JmsConsumerBodyCapTest.RecordingObserver();
    forwardOne(tm, obs);

    assertEquals("text/plain", obs.last.get().getContentType());
    assertEquals("hello", obs.last.get().getBody().toStringUtf8());
    assertNull(obs.error.get(), "forwarding must not surface an error outcome");
  }

  @Test
  void emptyContentTypeFallsBackToTextPlain() throws Exception {
    TextMessage tm = mock(TextMessage.class);
    stubCommonAttributes(tm);
    when(tm.getStringProperty("ContentType")).thenReturn("");
    when(tm.getText()).thenReturn("hello");

    JmsConsumerBodyCapTest.RecordingObserver obs = new JmsConsumerBodyCapTest.RecordingObserver();
    forwardOne(tm, obs);

    assertEquals("text/plain", obs.last.get().getContentType());
    assertEquals("hello", obs.last.get().getBody().toStringUtf8());
    assertNull(obs.error.get(), "forwarding must not surface an error outcome");
  }

  @Test
  void bytesMessageContentTypeStaysEmpty() throws Exception {
    BytesMessage bm = mock(BytesMessage.class);
    stubCommonAttributes(bm);
    when(bm.getPropertyNames()).thenReturn(Collections.enumeration(List.of("ContentType")));
    when(bm.getObjectProperty("ContentType")).thenReturn("application/xml");
    when(bm.getStringProperty("ContentType")).thenReturn("application/xml");
    when(bm.getBodyLength()).thenReturn(3L);
    doAnswer(
            inv -> {
              byte[] buf = inv.getArgument(0);
              buf[0] = 0x01;
              buf[1] = 0x02;
              buf[2] = 0x03;
              return 3;
            })
        .when(bm)
        .readBytes(any(byte[].class));

    JmsConsumerBodyCapTest.RecordingObserver obs = new JmsConsumerBodyCapTest.RecordingObserver();
    forwardOne(bm, obs);

    assertEquals("", obs.last.get().getContentType(), "BytesMessage must carry no content type");
    assertArrayEquals(new byte[] {0x01, 0x02, 0x03}, obs.last.get().getBody().toByteArray());
    assertEquals("application/xml", obs.last.get().getHeadersMap().get("ContentType"));
    assertNull(obs.error.get(), "forwarding must not surface an error outcome");
  }
}
