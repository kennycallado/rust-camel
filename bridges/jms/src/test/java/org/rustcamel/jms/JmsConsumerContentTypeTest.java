package org.rustcamel.jms;

import static org.junit.jupiter.api.Assertions.assertArrayEquals;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertTrue;
import static org.mockito.ArgumentMatchers.any;
import static org.mockito.ArgumentMatchers.anyLong;
import static org.mockito.Mockito.doAnswer;
import static org.mockito.Mockito.mock;
import static org.mockito.Mockito.when;

import java.util.Collections;
import java.util.List;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;
import javax.jms.BytesMessage;
import javax.jms.Connection;
import javax.jms.MessageConsumer;
import javax.jms.Queue;
import javax.jms.Session;
import javax.jms.TextMessage;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.DisplayName;
import org.junit.jupiter.api.Test;
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

  /** Subscribes, waits for the single forwarded message, then stops the consumer. */
  private void forwardOne(javax.jms.Message msg, JmsConsumerBodyCapTest.RecordingObserver obs)
      throws Exception {
    when(messageConsumer.receive(anyLong())).thenReturn(msg).thenReturn(null);
    consumer.subscribe(DESTINATION, SUBSCRIPTION_ID, obs, new AtomicBoolean(false));
    assertTrue(obs.next.await(15, TimeUnit.SECONDS), "message must be forwarded");
    consumer.stop();
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
