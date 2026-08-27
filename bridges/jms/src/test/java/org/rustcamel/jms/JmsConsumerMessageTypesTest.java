package org.rustcamel.jms;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertTrue;
import static org.mockito.ArgumentMatchers.any;
import static org.mockito.ArgumentMatchers.anyLong;
import static org.mockito.Mockito.doAnswer;
import static org.mockito.Mockito.mock;
import static org.mockito.Mockito.never;
import static org.mockito.Mockito.verify;
import static org.mockito.Mockito.when;

import java.util.Collections;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;
import javax.jms.Connection;
import javax.jms.MessageConsumer;
import javax.jms.ObjectMessage;
import javax.jms.Queue;
import javax.jms.Session;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.DisplayName;
import org.junit.jupiter.api.Test;
import org.mockito.Mock;
import org.mockito.MockitoAnnotations;

/**
 * Consumer handling of non-Bytes/Text message types (policy rc-41h3): {@code ObjectMessage} bodies
 * are NEVER deserialized — {@code getObject()} would run Java serialization gadget code. The
 * current policy forwards such messages with an EMPTY body under an AUTO_ACKNOWLEDGE session, so
 * this test documents exactly that behavior.
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

    when(messageConsumer.receive(anyLong())).thenReturn(objectMessage).thenReturn(null);

    JmsConsumerBodyCapTest.RecordingObserver obs = new JmsConsumerBodyCapTest.RecordingObserver();
    consumer.subscribe(DESTINATION, SUBSCRIPTION_ID, obs, new AtomicBoolean(false));
    assertTrue(obs.next.await(15, TimeUnit.SECONDS), "unsupported type must still be forwarded");
    consumer.stop();

    assertEquals(1, obs.nextCount, "exactly one message delivered");
    assertEquals(0, obs.last.get().getBody().size(), "body must be forwarded EMPTY");
    assertEquals("", obs.last.get().getContentType(), "no content-type for unknown payloads");
    assertNull(obs.error.get(), "forwarding must not surface an error outcome");

    // Ack policy: the session runs AUTO_ACKNOWLEDGE, so receipt itself acknowledges the
    // message — convertMessage must not need any explicit acknowledge() call.
    verify(connection).createSession(false, Session.AUTO_ACKNOWLEDGE);

    // rc-41h3: deserialization stays absent — getObject() must never be invoked.
    verify(objectMessage, never()).getObject();
  }
}
