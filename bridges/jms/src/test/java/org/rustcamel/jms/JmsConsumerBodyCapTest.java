package org.rustcamel.jms;

import static org.junit.jupiter.api.Assertions.assertArrayEquals;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;
import static org.mockito.ArgumentMatchers.any;
import static org.mockito.Mockito.mock;
import static org.mockito.Mockito.when;

import io.grpc.stub.StreamObserver;
import java.util.Arrays;
import java.util.Collections;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicReference;
import javax.jms.BytesMessage;
import javax.jms.Connection;
import javax.jms.JMSException;
import javax.jms.MessageConsumer;
import javax.jms.Queue;
import javax.jms.Session;
import javax.jms.TextMessage;
import jms_bridge.JmsMessage;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;
import org.mockito.Mock;
import org.mockito.MockitoAnnotations;

/**
 * JMS consumer body cap ({@code JMS_MAX_BODY_BYTES}).
 *
 * <p>Gates run against {@value #TEST_CAP_BYTES} bytes: {@code setUp} pins {@code
 * JmsConsumer.pinnedMaxBodyBytes}, making the cap deterministic whether or not {@code
 * JMS_MAX_BODY_BYTES} is exported.
 */
class JmsConsumerBodyCapTest {

  private static final String CAP_ENV_VAR = "JMS_MAX_BODY_BYTES";
  private static final long TEST_CAP_BYTES = 1024;
  private static final String DESTINATION = "queue:test";
  private static final String SUBSCRIPTION_ID = "sub-body-cap-test";

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
    // Deterministic cap: the pin bypasses JMS_MAX_BODY_BYTES resolution entirely.
    consumer.pinnedMaxBodyBytes = TEST_CAP_BYTES;

    when(factory.createDedicatedConnection()).thenReturn(connection);
    org.mockito.Mockito.doAnswer(inv -> null).when(connection).start();
    when(connection.createSession(false, Session.AUTO_ACKNOWLEDGE)).thenReturn(session);
    when(session.createQueue("test")).thenReturn(queue);
    when(session.createConsumer(any(Queue.class))).thenReturn(messageConsumer);
  }

  @AfterEach
  void tearDown() {
    consumer.stop();
  }

  /**
   * Subscribe targeting one mocked delivery; subsequent polls return null until {@link #consumer}
   * stops.
   */
  private RecordingObserver subscribeOnce(javax.jms.Message msg) throws JMSException {
    when(messageConsumer.receive(org.mockito.ArgumentMatchers.anyLong()))
        .thenReturn(msg)
        .thenReturn(null);
    RecordingObserver obs = new RecordingObserver();
    consumer.subscribe(DESTINATION, SUBSCRIPTION_ID, obs, new AtomicBoolean(false));
    return obs;
  }

  /** Common header/property stubbing; convertMessage touches these before the type branch. */
  private static void stubCommonAttributes(javax.jms.Message msg) throws JMSException {
    when(msg.getJMSMessageID()).thenReturn(null);
    when(msg.getJMSCorrelationID()).thenReturn(null);
    when(msg.getJMSTimestamp()).thenReturn(0L);
    when(msg.getPropertyNames()).thenReturn(Collections.emptyEnumeration());
  }

  @Test
  void oversizedBytesMessageRejectedWithoutFullAllocation() throws Exception {
    // Fake a *huge* body without backing memory: the mock reports 4096 bytes but flags any full
    // read attempt; an implementation honoring the cap must reject before allocating or reading.
    AtomicBoolean bodyRead = new AtomicBoolean(false);
    BytesMessage big = mock(BytesMessage.class);
    stubCommonAttributes(big);
    when(big.getBodyLength()).thenReturn(4096L);
    when(big.readBytes(any(byte[].class)))
        .thenAnswer(
            invocation -> {
              bodyRead.set(true);
              return -1;
            });

    RecordingObserver obs = subscribeOnce(big);
    assertTrue(obs.errored.await(15, TimeUnit.SECONDS), "oversized body must yield an error");

    assertFalse(bodyRead.get(), "readBytes must NEVER be called for an oversized body");
    assertEquals(0, obs.nextCount, "oversized body must not be delivered");
    Throwable t = obs.error.get();
    assertNotNull(t, "error outcome must be carried");
    assertTrue(
        String.valueOf(t.getMessage()).contains(CAP_ENV_VAR),
        "error must name " + CAP_ENV_VAR + ": " + t.getMessage());
    assertTrue(
        String.valueOf(t.getMessage()).contains("4096"),
        "error must report the actual length: " + t.getMessage());
  }

  @Test
  void negativeBodyLengthRejectedWithoutAllocation() throws Exception {
    // A broker reporting a negative body length must hit the same clean rejection as an oversized
    // body — never new byte[(int) len] (NegativeArraySizeException).
    AtomicBoolean bodyRead = new AtomicBoolean(false);
    BytesMessage bogus = mock(BytesMessage.class);
    stubCommonAttributes(bogus);
    when(bogus.getBodyLength()).thenReturn(-1L);
    when(bogus.readBytes(any(byte[].class)))
        .thenAnswer(
            invocation -> {
              bodyRead.set(true);
              return -1;
            });

    RecordingObserver obs = subscribeOnce(bogus);
    assertTrue(obs.errored.await(15, TimeUnit.SECONDS), "negative length must yield an error");

    assertFalse(bodyRead.get(), "readBytes must NEVER be called for a negative body length");
    assertEquals(0, obs.nextCount, "negative-length body must not be delivered");
    Throwable t = obs.error.get();
    assertNotNull(t, "error outcome must be carried");
    assertTrue(
        String.valueOf(t.getMessage()).contains("-1"),
        "error must report the actual length: " + t.getMessage());
  }

  @Test
  void underCapBytesMessageDelivered() throws Exception {
    byte[] payload = new byte[512];
    Arrays.fill(payload, (byte) 'x');
    BytesMessage small = mock(BytesMessage.class);
    stubCommonAttributes(small);
    when(small.getBodyLength()).thenReturn((long) payload.length);
    when(small.readBytes(any(byte[].class)))
        .thenAnswer(
            invocation -> {
              byte[] buf = invocation.getArgument(0);
              System.arraycopy(payload, 0, buf, 0, payload.length);
              return buf.length;
            });

    RecordingObserver obs = subscribeOnce(small);
    assertTrue(obs.next.await(15, TimeUnit.SECONDS), "under-cap body must be delivered");
    consumer.stop();

    assertEquals(1, obs.nextCount, "exactly one message delivered");
    assertArrayEquals(payload, obs.last.get().getBody().toByteArray(), "body must arrive intact");
  }

  @Test
  void oversizedTextMessageRejected() throws Exception {
    // TextMessage has no pre-read length, so the cap lands on the UTF-8 encoded size of the
    // materialized string: a body
    // above the cap must fail with the same diagnostic shape as the BytesMessage gate.
    TextMessage big = mock(TextMessage.class);
    stubCommonAttributes(big);
    char[] chars = new char[2048];
    Arrays.fill(chars, 'x');
    when(big.getText()).thenReturn(new String(chars));

    RecordingObserver obs = subscribeOnce(big);
    assertTrue(obs.errored.await(15, TimeUnit.SECONDS), "oversized text must yield an error");

    assertEquals(0, obs.nextCount, "oversized text must not be delivered");
    Throwable t = obs.error.get();
    assertNotNull(t, "error outcome must be carried");
    assertTrue(
        String.valueOf(t.getMessage()).contains(CAP_ENV_VAR),
        "error must name " + CAP_ENV_VAR + ": " + t.getMessage());
    assertTrue(
        String.valueOf(t.getMessage()).contains("2048"),
        "error must report the actual length: " + t.getMessage());
    assertTrue(
        String.valueOf(t.getMessage()).contains("bytes"),
        "error must report the length in bytes: " + t.getMessage());
  }

  @Test
  void textMessageUtf8OverCapRejected() throws Exception {
    // 1024 CJK chars: UTF-16 length 1024 == cap, so the old char-length gate forwards; UTF-8
    // size 3072 > cap, so the byte-accurate gate must reject. Honest RED against the old gate.
    TextMessage cjk = mock(TextMessage.class);
    stubCommonAttributes(cjk);
    when(cjk.getText()).thenReturn("\u4e2d".repeat(Math.toIntExact(TEST_CAP_BYTES)));

    RecordingObserver obs = subscribeOnce(cjk);
    assertTrue(obs.errored.await(15, TimeUnit.SECONDS), "UTF-8-oversized text must yield an error");

    assertEquals(0, obs.nextCount, "UTF-8-oversized text must not be delivered");
    Throwable t = obs.error.get();
    assertNotNull(t, "error outcome must be carried");
    assertTrue(
        String.valueOf(t.getMessage()).contains("3072"),
        "error must report the UTF-8 size in bytes: " + t.getMessage());
    assertTrue(
        String.valueOf(t.getMessage()).contains("bytes"),
        "error must use byte units: " + t.getMessage());
  }

  @Test
  void textMessageAsciiAtExactlyCapPasses() throws Exception {
    // Boundary pin: ASCII text whose UTF-8 size equals the cap exactly must be forwarded.
    TextMessage atCap = mock(TextMessage.class);
    stubCommonAttributes(atCap);
    when(atCap.getText()).thenReturn("x".repeat(Math.toIntExact(TEST_CAP_BYTES)));

    RecordingObserver obs = subscribeOnce(atCap);
    assertTrue(obs.next.await(15, TimeUnit.SECONDS), "at-cap text must be delivered");
    consumer.stop();

    assertEquals(1, obs.nextCount, "exactly one message delivered");
    JmsMessage delivered = obs.last.get();
    assertEquals(TEST_CAP_BYTES, delivered.getBody().size(), "body size must equal the cap");
  }

  @Test
  void underCapTextMessageDelivered() throws Exception {
    String payload = "caf\u00e9 ".repeat(80).trim(); // 400 chars, UTF-8 multi-byte on purpose
    TextMessage small = mock(TextMessage.class);
    stubCommonAttributes(small);
    when(small.getText()).thenReturn(payload);

    RecordingObserver obs = subscribeOnce(small);
    assertTrue(obs.next.await(15, TimeUnit.SECONDS), "under-cap text must be delivered");
    consumer.stop();

    assertEquals(1, obs.nextCount, "exactly one message delivered");
    JmsMessage delivered = obs.last.get();
    assertEquals(payload, delivered.getBody().toStringUtf8(), "text must arrive intact");
    assertEquals("text/plain", delivered.getContentType(), "text content type must be set");
  }

  @Test
  void parseCapMalformedFailsLoud() {
    IllegalStateException ex =
        assertThrows(IllegalStateException.class, () -> JmsConsumer.parseCap("abc"));
    assertTrue(
        ex.getMessage().contains(CAP_ENV_VAR),
        "failure must name " + CAP_ENV_VAR + ": " + ex.getMessage());
    assertTrue(ex.getMessage().contains("abc"), "failure must echo the raw value");
  }

  @Test
  void parseCapValidValue() {
    assertEquals(2048L, JmsConsumer.parseCap("2048"));
  }

  @Test
  void parseCapAboveCeilingFailsLoud() {
    // 20 MiB would invert the decode-limit ordering against the 20 MiB Rust IPC limit.
    IllegalStateException ex =
        assertThrows(
            IllegalStateException.class,
            () -> JmsConsumer.parseCap(String.valueOf(20L * 1024 * 1024)));
    assertTrue(
        ex.getMessage().contains(CAP_ENV_VAR),
        "failure must name " + CAP_ENV_VAR + ": " + ex.getMessage());
    assertTrue(
        ex.getMessage().contains("19") && ex.getMessage().contains("decode"),
        "failure must explain the 19 MiB decode-ordering ceiling: " + ex.getMessage());
  }

  /** Records stream outcomes with latches the tests can block on. */
  static final class RecordingObserver implements StreamObserver<JmsMessage> {
    final CountDownLatch next = new CountDownLatch(1);
    final CountDownLatch errored = new CountDownLatch(1);
    final AtomicReference<JmsMessage> last = new AtomicReference<>();
    final AtomicReference<Throwable> error = new AtomicReference<>();
    volatile int nextCount;

    @Override
    public void onNext(JmsMessage value) {
      last.set(value);
      nextCount++;
      next.countDown();
    }

    @Override
    public void onError(Throwable t) {
      error.set(t);
      errored.countDown();
    }

    @Override
    public void onCompleted() {}
  }
}
