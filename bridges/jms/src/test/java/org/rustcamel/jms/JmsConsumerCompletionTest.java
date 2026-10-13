package org.rustcamel.jms;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertTrue;
import static org.mockito.ArgumentMatchers.any;
import static org.mockito.ArgumentMatchers.anyLong;
import static org.mockito.Mockito.doAnswer;
import static org.mockito.Mockito.doThrow;
import static org.mockito.Mockito.mock;
import static org.mockito.Mockito.never;
import static org.mockito.Mockito.timeout;
import static org.mockito.Mockito.times;
import static org.mockito.Mockito.verify;
import static org.mockito.Mockito.when;

import java.nio.file.Path;
import java.util.Collections;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.Future;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;
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
 * Owner-thread completion settlement: the session runs {@code CLIENT_ACKNOWLEDGE}, the owner thread
 * holds exactly one unacknowledged delivery, and it settles only at the single linearization point
 * — a validated {@code COMMIT} acknowledges once, a validated {@code ROLLBACK} recovers, and a stop
 * or a stale/duplicate completion never acknowledges.
 */
@DisplayName("JmsConsumer owner-thread completion settlement")
class JmsConsumerCompletionTest {

  private static final String DESTINATION = "queue:test";
  private static final String SUBSCRIPTION_ID = "sub-completion-test";
  private static final long AWAIT_SECONDS = 15L;

  @Mock JmsClientFactory factory;
  @Mock Connection connection;
  @Mock Session session;
  @Mock MessageConsumer messageConsumer;
  @Mock Queue queue;

  @TempDir Path tempDir;

  JmsConsumer consumer;
  TextMessage message;

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

    message = mock(TextMessage.class);
    when(message.getJMSMessageID()).thenReturn("ID:1");
    when(message.getJMSCorrelationID()).thenReturn(null);
    when(message.getJMSTimestamp()).thenReturn(0L);
    when(message.getPropertyNames()).thenReturn(Collections.emptyEnumeration());
    when(message.getText()).thenReturn("hello");
  }

  @AfterEach
  void tearDown() {
    consumer.stop();
    try {
      mocks.close();
    } catch (Exception ignored) {
    }
  }

  /** Subscribes, waits for the single forwarded delivery, and returns it with its token checked. */
  private JmsMessage deliverOne(JmsConsumerBodyCapTest.RecordingObserver obs) throws Exception {
    when(messageConsumer.receive(anyLong())).thenReturn(message).thenReturn(null);
    consumer.subscribe(DESTINATION, SUBSCRIPTION_ID, obs, new AtomicBoolean(false));
    assertTrue(obs.next.await(AWAIT_SECONDS, TimeUnit.SECONDS), "one delivery must be forwarded");
    JmsMessage delivered = obs.last.get();
    assertFalse(delivered.getDeliveryToken().isEmpty(), "a unique delivery token must be attached");
    assertEquals(
        consumer.subscriptionGeneration(),
        delivered.getSubscriptionGeneration(),
        "the subscription generation must be attached");
    return delivered;
  }

  /** Runs the blocking {@code complete} call with a bounded wait so a leaked future fails fast. */
  private boolean completeBounded(long generation, String deliveryToken, CompletionOutcome outcome)
      throws Exception {
    ExecutorService executor = Executors.newSingleThreadExecutor();
    try {
      Future<Boolean> future =
          executor.submit(() -> consumer.complete(generation, deliveryToken, outcome));
      return future.get(5, TimeUnit.SECONDS);
    } finally {
      executor.shutdownNow();
      assertTrue(executor.awaitTermination(5, TimeUnit.SECONDS));
    }
  }

  @Test
  void successful_route_commits_exactly_once() throws Exception {
    JmsMessage delivered = deliverOne(new JmsConsumerBodyCapTest.RecordingObserver());

    assertTrue(
        consumer.complete(
            consumer.subscriptionGeneration(),
            delivered.getDeliveryToken(),
            CompletionOutcome.COMMIT),
        "the first COMMIT for the current reservation must be accepted");
    assertFalse(
        consumer.complete(
            consumer.subscriptionGeneration(),
            delivered.getDeliveryToken(),
            CompletionOutcome.COMMIT),
        "a duplicate COMMIT must be rejected");

    verify(message, times(1)).acknowledge();
    verify(session, never()).recover();
  }

  @Test
  void propagated_error_rolls_back() throws Exception {
    JmsMessage delivered = deliverOne(new JmsConsumerBodyCapTest.RecordingObserver());

    assertTrue(
        consumer.complete(
            consumer.subscriptionGeneration(),
            delivered.getDeliveryToken(),
            CompletionOutcome.ROLLBACK),
        "a ROLLBACK for the current reservation must be accepted");

    verify(session, times(1)).recover();
    verify(message, never()).acknowledge();
  }

  @Test
  void cancel_before_decision_prevents_ack() throws Exception {
    deliverOne(new JmsConsumerBodyCapTest.RecordingObserver());

    consumer.stop();

    verify(message, never()).acknowledge();
    verify(session, never()).recover();
  }

  @Test
  void cancel_after_decision_does_not_undo() throws Exception {
    JmsMessage delivered = deliverOne(new JmsConsumerBodyCapTest.RecordingObserver());

    assertTrue(
        consumer.complete(
            consumer.subscriptionGeneration(),
            delivered.getDeliveryToken(),
            CompletionOutcome.COMMIT));
    consumer.stop();

    verify(message, times(1)).acknowledge();
    verify(session, never()).recover();
  }

  @Test
  void lost_commit_response_stands() throws Exception {
    JmsMessage delivered = deliverOne(new JmsConsumerBodyCapTest.RecordingObserver());

    assertTrue(
        consumer.complete(
            consumer.subscriptionGeneration(),
            delivered.getDeliveryToken(),
            CompletionOutcome.COMMIT));
    assertFalse(
        consumer.complete(
            consumer.subscriptionGeneration(),
            delivered.getDeliveryToken(),
            CompletionOutcome.COMMIT),
        "a client retry after a lost response must not acknowledge again");

    verify(message, times(1)).acknowledge();
    verify(session, never()).recover();
  }

  @Test
  void stale_generation_or_token_ignored() throws Exception {
    JmsMessage delivered = deliverOne(new JmsConsumerBodyCapTest.RecordingObserver());

    assertFalse(
        consumer.complete(
            consumer.subscriptionGeneration() + 1,
            delivered.getDeliveryToken(),
            CompletionOutcome.COMMIT),
        "a stale generation must be rejected");
    assertFalse(
        consumer.complete(
            consumer.subscriptionGeneration(), "stale-token", CompletionOutcome.COMMIT),
        "a stale token must be rejected");

    verify(message, never()).acknowledge();
    verify(session, never()).recover();
  }

  @Test
  void slow_route_stays_pending() throws Exception {
    JmsMessage delivered = deliverOne(new JmsConsumerBodyCapTest.RecordingObserver());

    // Bounded idle window: a healthy route with no completion must stay pending with no deadline.
    CountDownLatch noDeadline = new CountDownLatch(1);
    assertFalse(noDeadline.await(400, TimeUnit.MILLISECONDS), "no completion deadline may fire");
    verify(message, never()).acknowledge();
    verify(session, never()).recover();

    // A late decision still settles, proving the reservation was not torn down.
    assertTrue(
        consumer.complete(
            consumer.subscriptionGeneration(),
            delivered.getDeliveryToken(),
            CompletionOutcome.COMMIT));
    verify(message, times(1)).acknowledge();
  }

  @Test
  void commit_ack_failure_is_uncertain_and_terminal() throws Exception {
    JmsConsumerBodyCapTest.RecordingObserver obs = new JmsConsumerBodyCapTest.RecordingObserver();
    JmsMessage delivered = deliverOne(obs);
    doThrow(new JMSException("ack failed")).when(message).acknowledge();

    assertFalse(
        completeBounded(
            consumer.subscriptionGeneration(),
            delivered.getDeliveryToken(),
            CompletionOutcome.COMMIT),
        "an ack failure must not report an accepted settlement");

    assertTrue(obs.errored.await(AWAIT_SECONDS, TimeUnit.SECONDS), "the stream must fail");
    assertNotNull(obs.error.get(), "the failure must reach the observer");
    // Uncertain settlement is terminal: one attempt, no recover, no second receive, owner closes.
    verify(message, times(1)).acknowledge();
    verify(session, never()).recover();
    verify(messageConsumer, times(1)).receive(anyLong());
    verify(messageConsumer, timeout(5000)).close();
    verify(session, timeout(5000)).close();
    verify(connection, timeout(5000)).close();
  }

  @Test
  void rollback_recover_failure_is_uncertain_and_terminal() throws Exception {
    JmsConsumerBodyCapTest.RecordingObserver obs = new JmsConsumerBodyCapTest.RecordingObserver();
    JmsMessage delivered = deliverOne(obs);
    doThrow(new JMSException("recover failed")).when(session).recover();

    assertFalse(
        completeBounded(
            consumer.subscriptionGeneration(),
            delivered.getDeliveryToken(),
            CompletionOutcome.ROLLBACK),
        "a recover failure must not report an accepted settlement");

    assertTrue(obs.errored.await(AWAIT_SECONDS, TimeUnit.SECONDS), "the stream must fail");
    assertNotNull(obs.error.get(), "the failure must reach the observer");
    // Uncertain settlement is terminal: one attempt, no ack, no second receive, owner closes.
    verify(session, times(1)).recover();
    verify(message, never()).acknowledge();
    verify(messageConsumer, times(1)).receive(anyLong());
    verify(messageConsumer, timeout(5000)).close();
    verify(session, timeout(5000)).close();
    verify(connection, timeout(5000)).close();
  }

  @Test
  void unspecified_or_unknown_outcome_rejected() throws Exception {
    JmsMessage delivered = deliverOne(new JmsConsumerBodyCapTest.RecordingObserver());

    assertFalse(
        completeBounded(
            consumer.subscriptionGeneration(),
            delivered.getDeliveryToken(),
            CompletionOutcome.COMPLETION_OUTCOME_UNSPECIFIED),
        "an unspecified outcome must be rejected");
    assertFalse(
        completeBounded(
            consumer.subscriptionGeneration(),
            delivered.getDeliveryToken(),
            CompletionOutcome.UNRECOGNIZED),
        "an unknown outcome must be rejected as not a legal decision");

    verify(message, never()).acknowledge();
    verify(session, never()).recover();

    // A legal COMMIT afterwards still settles exactly once.
    assertTrue(
        consumer.complete(
            consumer.subscriptionGeneration(),
            delivered.getDeliveryToken(),
            CompletionOutcome.COMMIT));
    verify(message, times(1)).acknowledge();
    verify(session, never()).recover();
  }
}
