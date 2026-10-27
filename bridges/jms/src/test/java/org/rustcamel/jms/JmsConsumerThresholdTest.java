package org.rustcamel.jms;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertTrue;
import static org.mockito.ArgumentMatchers.any;
import static org.mockito.ArgumentMatchers.anyLong;
import static org.mockito.ArgumentMatchers.anyString;
import static org.mockito.Mockito.doAnswer;
import static org.mockito.Mockito.doThrow;
import static org.mockito.Mockito.mock;
import static org.mockito.Mockito.never;
import static org.mockito.Mockito.timeout;
import static org.mockito.Mockito.times;
import static org.mockito.Mockito.verify;
import static org.mockito.Mockito.when;

import io.grpc.Status;
import io.grpc.stub.StreamObserver;
import java.nio.file.Path;
import java.util.Collections;
import java.util.List;
import java.util.Map;
import java.util.concurrent.CopyOnWriteArrayList;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicReference;
import javax.jms.Connection;
import javax.jms.JMSException;
import javax.jms.MessageConsumer;
import javax.jms.Queue;
import javax.jms.Session;
import javax.jms.TextMessage;
import jms_bridge.CompletionOutcome;
import jms_bridge.JmsMessage;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.DisplayName;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;
import org.mockito.Mock;
import org.mockito.MockitoAnnotations;

/**
 * Reservation threshold and reject-then-log poison policy (jms-bridge-080 task 3.8).
 *
 * <p>The cap is {@code JMS_MAX_DELIVERY_ATTEMPTS} (default 6, validated by {@link BridgeConfig}),
 * read from the durable journal handle and applied BEFORE conversion. Exactly the configured number
 * of reservations are recorded; on the receipt whose recorded count has reached the cap the message
 * is rejected without a handoff: the body and header accessors never run, the message is
 * acknowledged once, and the discard is logged with only the message ID and count. The reservation
 * is cleared only after the confirmed ACK. No dead-letter destination is used.
 */
@DisplayName("JmsConsumer reservation threshold and poison policy")
class JmsConsumerThresholdTest {

  private static final String KEY = "0123456789abcdef0123456789abcdef";
  private static final String DESTINATION = "queue:test";
  private static final String QUEUE_NAME = "test";
  private static final String SUBSCRIPTION_ID = "sub-threshold-test";
  private static final String MESSAGE_ID = "ID:1";
  private static final long AWAIT_SECONDS = 15L;
  private static final long STOP_TIMEOUT_MS = 15_000L;

  @Mock JmsClientFactory factory;
  @Mock Connection connection;
  @Mock Session session;
  @Mock MessageConsumer messageConsumer;
  @Mock Queue queue;

  @TempDir Path tempDir;

  JournalService journal;
  JmsConsumer consumer;
  JmsConsumer capped;
  TextMessage message;

  private AutoCloseable mocks;

  @BeforeEach
  void setUp() throws Exception {
    mocks = MockitoAnnotations.openMocks(this);

    journal =
        new JournalService(
            tempDir,
            BridgeConfig.JOURNAL_MAX_BYTES_DEFAULT,
            BridgeConfig.JOURNAL_RETENTION_MS_DEFAULT,
            System::currentTimeMillis);

    consumer = new JmsConsumer();
    consumer.factory = factory;
    consumer.configureJournal(journal, KEY, 1000L, null);

    when(factory.createDedicatedConnection()).thenReturn(connection);
    doAnswer(inv -> null).when(connection).start();
    when(connection.createSession(false, Session.CLIENT_ACKNOWLEDGE)).thenReturn(session);
    when(session.createQueue(QUEUE_NAME)).thenReturn(queue);
    when(session.createConsumer(any(Queue.class))).thenReturn(messageConsumer);

    // Full conversion attributes so the boundary test can hand the message off; the terminal tests
    // assert none of them (except getJMSMessageID) is ever read.
    message = mock(TextMessage.class);
    when(message.getJMSMessageID()).thenReturn(MESSAGE_ID);
    when(message.getJMSCorrelationID()).thenReturn(null);
    when(message.getJMSTimestamp()).thenReturn(0L);
    when(message.getStringProperty("ContentType")).thenReturn(null);
    when(message.getPropertyNames()).thenReturn(Collections.emptyEnumeration());
    when(message.getText()).thenReturn("hello");
  }

  @AfterEach
  void tearDown() {
    if (capped != null) {
      capped.stop();
    }
    if (consumer != null) {
      consumer.stop();
    }
    try {
      if (mocks != null) {
        mocks.close();
      }
    } catch (Exception ignored) {
    }
    if (capped != null) {
      assertTrue(
          capped.awaitStopped(STOP_TIMEOUT_MS),
          "the capped owner thread must release the journal before the temp-dir teardown");
    }
    if (consumer != null) {
      assertTrue(
          consumer.awaitStopped(STOP_TIMEOUT_MS),
          "the owner thread must release the journal before the temp-dir teardown");
    }
  }

  /** Pre-populates the shared destination journal with {@code reservations} durable RESERVEs. */
  private void preReserve(String messageId, int reservations) throws Exception {
    JournalService.JournalHandle handle =
        journal.acquire(KEY, JournalService.destinationKey(QUEUE_NAME), 1000L);
    try {
      for (int i = 0; i < reservations; i++) {
        handle.reserve(messageId);
      }
    } finally {
      handle.close();
    }
  }

  /** Subscribes one mocked delivery then idles so the owner thread never spins. */
  private RecordingObserver subscribeOnce(JmsConsumer target, javax.jms.Message msg)
      throws JMSException {
    when(messageConsumer.receive(anyLong()))
        .thenReturn(msg)
        .thenAnswer(
            invocation -> {
              Thread.sleep(200);
              return null;
            });
    RecordingObserver obs = new RecordingObserver();
    target.subscribe(DESTINATION, SUBSCRIPTION_ID, obs, new AtomicBoolean(false));
    return obs;
  }

  /** Polls the durable count through a fresh handle until it matches or the deadline passes. */
  private void awaitJournalCount(String messageId, long expected) throws Exception {
    long deadline = System.nanoTime() + TimeUnit.SECONDS.toNanos(5);
    long seen = -1;
    while (System.nanoTime() < deadline) {
      try (JournalService.JournalHandle handle =
          journal.acquire(KEY, JournalService.destinationKey(QUEUE_NAME), 1000L)) {
        seen = handle.count(messageId);
      }
      if (seen == expected) {
        return;
      }
      Thread.sleep(20);
    }
    assertEquals(expected, seen, "journal count for " + messageId);
  }

  /** Waits for the first terminal line a capturing consumer records, or null on timeout. */
  private static String awaitTerminalLine(CapturingConsumer consumer, long timeoutMillis)
      throws InterruptedException {
    long deadline = System.nanoTime() + TimeUnit.MILLISECONDS.toNanos(timeoutMillis);
    while (System.nanoTime() < deadline) {
      List<String> lines = consumer.terminalLines();
      if (!lines.isEmpty()) {
        return lines.get(0);
      }
      Thread.sleep(10);
    }
    return consumer.terminalLines().isEmpty() ? null : consumer.terminalLines().get(0);
  }

  @Test
  void six_reservations_then_terminal() throws Exception {
    preReserve(MESSAGE_ID, 6);
    RecordingObserver obs = subscribeOnce(consumer, message);

    // The seventh receipt is terminal: reject-then-log, never handed off.
    assertFalse(
        obs.next.await(750, TimeUnit.MILLISECONDS), "the seventh receipt must not be handed off");
    verify(message, timeout(5000)).acknowledge();
    assertEquals(0, obs.nextCount, "no poison receipt reaches the route");
    assertNull(obs.error.get(), "a terminal discard is not a stream failure");

    // The reservation is cleared only after the confirmed ACK.
    awaitJournalCount(MESSAGE_ID, 0L);
    verify(message, never()).getText();
  }

  @Test
  void terminal_logs_id_and_count_only() throws Exception {
    preReserve(MESSAGE_ID, 6);
    CapturingConsumer capturing = new CapturingConsumer();
    capturing.factory = factory;
    capturing.configureJournal(journal, KEY, 1000L, null);
    RecordingObserver obs = subscribeOnce(capturing, message);
    verify(message, timeout(5000)).acknowledge();

    String line = awaitTerminalLine(capturing, 5000);
    assertNotNull(line, "the terminal discard must be logged");
    assertTrue(line.contains(MESSAGE_ID), "the line must carry the message ID: " + line);
    assertTrue(line.contains("6"), "the line must carry the reservation count: " + line);
    assertFalse(line.contains("hello"), "the line must not carry body bytes: " + line);
    assertFalse(line.contains("ContentType"), "the line must not carry a header name: " + line);

    // No body or header accessor ever runs on a terminal receipt; only the message ID is read.
    verify(message, never()).getText();
    verify(message, never()).getPropertyNames();
    verify(message, never()).getStringProperty(anyString());
    verify(message, never()).getJMSCorrelationID();
    verify(message, never()).getJMSTimestamp();
    capturing.stop();
  }

  @Test
  void configured_cap_two_terminal_on_third_receipt() throws Exception {
    // The cap comes from the validated BridgeConfig, which is the sole authority.
    assertEquals(
        6L,
        new BridgeConfig(Map.of()).maxDeliveryAttempts(),
        "the default cap is six from the validated config");
    long cap =
        new BridgeConfig(Map.of(BridgeConfig.MAX_DELIVERY_ATTEMPTS_ENV, "2")).maxDeliveryAttempts();
    assertEquals(2L, cap, "JMS_MAX_DELIVERY_ATTEMPTS=2 must be accepted as a valid cap");

    capped = new JmsConsumer();
    capped.factory = factory;
    capped.configureJournal(journal, KEY, 1000L, null, cap);
    preReserve(MESSAGE_ID, 2);

    RecordingObserver obs = subscribeOnce(capped, message);
    assertFalse(
        obs.next.await(750, TimeUnit.MILLISECONDS), "the third receipt must be terminal at cap 2");
    verify(message, timeout(5000)).acknowledge();
    assertEquals(0, obs.nextCount, "no receipt reaches the route at cap 2");
    capped.stop();
  }

  @Test
  void sixth_receipt_handed_off_at_five_reservations() throws Exception {
    // Boundary: exactly six reservations are permitted, so the sixth receipt (count five) is still
    // handed off with reservation count six.
    preReserve(MESSAGE_ID, 5);
    RecordingObserver obs = subscribeOnce(consumer, message);
    assertTrue(obs.next.await(AWAIT_SECONDS, TimeUnit.SECONDS), "the sixth reservation is allowed");
    JmsMessage delivered = obs.last.get();
    assertEquals(6, delivered.getReservationCount(), "the delivery records the sixth reservation");
    assertEquals(1, obs.nextCount);

    assertTrue(
        consumer.complete(
            consumer.subscriptionGeneration(),
            delivered.getDeliveryToken(),
            CompletionOutcome.COMMIT),
        "the sixth reservation is a normal route delivery");
    verify(message, times(1)).acknowledge();
    awaitJournalCount(MESSAGE_ID, 0L);
  }

  /**
   * Task 4.4: an empty {@code JMSMessageID} is unkeyable. The bridge must terminate the
   * subscription without acknowledging, preserve the message for broker redelivery, and never
   * manufacture an ID.
   */
  @Test
  void unkeyable_id_preserves_message() throws Exception {
    TextMessage unkeyable = mock(TextMessage.class);
    when(unkeyable.getJMSMessageID()).thenReturn("");
    RecordingObserver obs = subscribeOnce(consumer, unkeyable);

    assertTrue(obs.errored.await(5, TimeUnit.SECONDS), "an unkeyable id must fail the stream");
    assertEquals(
        Status.Code.FAILED_PRECONDITION,
        Status.fromThrowable(obs.error.get()).getCode(),
        "an unkeyable id is a permanent precondition failure");

    // No handoff means no manufactured ID: the bridge never emits a message it cannot key.
    assertEquals(0, obs.nextCount, "an unkeyable delivery must never be handed off");

    // The unacknowledged broker message is preserved for redelivery: exactly one receive, no ack.
    verify(unkeyable, never()).acknowledge();
    verify(messageConsumer, times(1)).receive(anyLong());
    assertFalse(
        obs.next.await(200, TimeUnit.MILLISECONDS),
        "the subscription must terminate after the unkeyable delivery");
  }

  @Test
  void terminal_ack_failure_is_uncertain_and_preserves_count() throws Exception {
    preReserve(MESSAGE_ID, 6);
    doThrow(new JMSException("ack failed")).when(message).acknowledge();

    CapturingConsumer capturing = new CapturingConsumer();
    capturing.factory = factory;
    capturing.configureJournal(journal, KEY, 1000L, null);
    RecordingObserver obs = subscribeOnce(capturing, message);

    assertTrue(
        obs.errored.await(5, TimeUnit.SECONDS), "an uncertain terminal ACK must fail the stream");
    assertEquals(0, obs.nextCount, "the poison receipt must never be handed off");
    verify(message, times(1)).acknowledge();
    verify(messageConsumer, times(1)).receive(anyLong());
    awaitJournalCount(MESSAGE_ID, 6L);
    assertTrue(
        capturing.terminalLines().isEmpty(), "an uncertain ACK must not be logged as a discard");
  }

  @Test
  void terminal_journal_clear_failure_still_logs_and_fails_stream() throws Exception {
    preReserve(MESSAGE_ID, 6);
    CapturingConsumer failing =
        new CapturingConsumer() {
          @Override
          void clearTerminalReservation(String messageId) throws JournalPermanentException {
            throw new JournalPermanentException("simulated journal clear failure");
          }
        };
    failing.factory = factory;
    failing.configureJournal(journal, KEY, 1000L, null);

    RecordingObserver obs = subscribeOnce(failing, message);
    assertTrue(
        obs.errored.await(5, TimeUnit.SECONDS), "a failed terminal clear must fail the stream");
    assertEquals(0, obs.nextCount, "the poison receipt must never be handed off");
    verify(message, times(1)).acknowledge();

    String line = awaitTerminalLine(failing, 5000);
    assertNotNull(line, "the discard must be logged even when the journal clear fails");
    assertTrue(line.contains(MESSAGE_ID), "the line must carry the message ID: " + line);
    // The count lingers conservatively; the ACK is never retried.
    awaitJournalCount(MESSAGE_ID, 6L);
  }

  /** A consumer that records the exact terminal lines it logs. */
  private static class CapturingConsumer extends JmsConsumer {
    private final List<String> terminalLines = new CopyOnWriteArrayList<>();

    @Override
    void logTerminalDiscard(String line) {
      terminalLines.add(line);
      super.logTerminalDiscard(line);
    }

    List<String> terminalLines() {
      return terminalLines;
    }
  }

  /** Records stream outcomes with latches the tests can block on. */
  static final class RecordingObserver implements StreamObserver<JmsMessage> {
    final java.util.concurrent.CountDownLatch next = new java.util.concurrent.CountDownLatch(1);
    final java.util.concurrent.CountDownLatch errored = new java.util.concurrent.CountDownLatch(1);
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
