package org.rustcamel.jms;

import com.google.protobuf.ByteString;
import io.grpc.Status;
import io.grpc.StatusRuntimeException;
import io.grpc.stub.StreamObserver;
import jakarta.enterprise.context.Dependent;
import jakarta.inject.Inject;
import java.util.Map;
import java.util.UUID;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.ExecutionException;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicLong;
import javax.jms.BytesMessage;
import javax.jms.Connection;
import javax.jms.Destination;
import javax.jms.JMSException;
import javax.jms.Message;
import javax.jms.MessageConsumer;
import javax.jms.Session;
import javax.jms.TextMessage;
import jms_bridge.CompletionOutcome;
import jms_bridge.JmsMessage;
import org.jboss.logging.Logger;

@Dependent
public class JmsConsumer {
  private static final Logger LOG = Logger.getLogger(JmsConsumer.class);
  private static final String MAX_BODY_BYTES_ENV = "JMS_MAX_BODY_BYTES";
  private static final long DEFAULT_MAX_BODY_BYTES = 16L * 1024 * 1024;

  /**
   * Hard ceiling for {@code JMS_MAX_BODY_BYTES}: the Rust IPC decode limit is 20 MiB, so a
   * configured cap above 19 MiB would let bodies pass this gate only to fail decode on the Rust
   * side. The headroom absorbs IPC framing overhead.
   */
  static final long MAX_BODY_BYTES_CEILING = 19L * 1024 * 1024;

  /**
   * One generation per {@code JmsConsumer} instance; a consumer serves exactly one subscription.
   */
  private static final AtomicLong GENERATION_COUNTER = new AtomicLong();

  @Inject JmsClientFactory factory;

  private volatile Connection connection;
  private volatile Session session;
  private volatile MessageConsumer consumer;
  private volatile boolean running = false;

  /** Dedicated owner thread running the subscribe loop; null until the first subscribe. */
  private volatile Thread ownerThread;

  private final AtomicBoolean resourcesClosed = new AtomicBoolean(false);
  private final long subscriptionGeneration = GENERATION_COUNTER.incrementAndGet();

  /**
   * JMS message selector for this subscription, or {@code null} when none was requested. Assigned
   * from the subscribe request on the injected instance before the consume loop starts and read
   * only at consumer creation: the bridge never parses or rewrites the expression, so an invalid
   * selector stays a broker-owned failure.
   */
  volatile String messageSelector;

  /**
   * Owner-thread settlement state. The session is touched only by the owner thread; a gRPC {@code
   * Complete} handler never calls into JMS — it publishes one terminal decision and waits for the
   * owner thread to settle. All fields are guarded by {@link #settleLock}.
   */
  private final Object settleLock = new Object();

  /** Token of the delivery currently awaiting a terminal decision; null when none is in flight. */
  private String activeToken;

  /** Accepted terminal decision for {@link #activeToken}; null until a valid Complete arrives. */
  private CompletionOutcome pendingDecision;

  /** Completed by the owner thread after it settles {@link #pendingDecision}; null when none. */
  private CompletableFuture<Boolean> pendingDecisionFuture;

  /** Set by {@link #stop()} before the terminal decision; the owner thread then must not ack. */
  private boolean stopping;

  /**
   * Test seam: pins the body cap deterministically. {@code -1} (production) resolves the cap from
   * {@link #maxBodyBytes()} per message.
   */
  long pinnedMaxBodyBytes = -1;

  /**
   * Process-local conversion-failure diagnostic for this consumer. It is NOT the durable
   * reservation count: since task 3.4 the durable authority is the per-destination {@code
   * JournalService}, which records one RESERVE per receipt keyed by {@code JMSMessageID} before
   * conversion and settles it with COMMIT or ROLLBACK. This counter is kept only as a test-visible
   * metric of conversion failures.
   */
  private final AtomicLong reservationFailures = new AtomicLong();

  /**
   * Records one conversion failure. The durable reservation is already recorded by the journal
   * before conversion; this method never adds a second reservation.
   */
  void countReservationFailure() {
    reservationFailures.incrementAndGet();
  }

  /**
   * @return the process-local reservation failures this consumer has recorded.
   */
  long reservationFailureCount() {
    return reservationFailures.get();
  }

  /**
   * Returns the subscription-scoped volatile topic reservation count for {@code messageId}; zero
   * when absent. Test visibility only: topics never reach the journal.
   *
   * @param messageId the provider {@code JMSMessageID}
   * @return the volatile attempt count for this subscription
   */
  long topicReservationCount(String messageId) {
    Long count = topicReservationCounts.get(messageId);
    return count == null ? 0L : count;
  }

  /**
   * Shared, per-bridge journal service. Every queue consumer in one bridge process reuses the same
   * instance, so two subscriptions on one queue domain share one budget through the service's
   * per-domain refcount. A sequential owner inherits the retained counts through lock handoff.
   * Cross-process duplicates are bounded at N times the configured maximum, where N is the number
   * of distinct (directory, broker key, destination key) journal domains consuming the queue within
   * one retention window. {@code null} when the journal environment is unset; {@link
   * #journalMissingEnv} then names the missing variable.
   */
  JournalService journal;

  /** Broker key that scopes the shared journal namespace; {@code null} when unconfigured. */
  String journalBrokerKey;

  /**
   * The missing environment variable when the journal is unconfigured; {@code null} when the bridge
   * is configured. A queue subscribe then fails {@code FAILED_PRECONDITION} naming it.
   */
  String journalMissingEnv;

  /** Bounded wait for a busy destination lock, from {@code JMS_ATTEMPT_JOURNAL_LOCK_WAIT_MS}. */
  long journalLockWaitMs = BridgeConfig.JOURNAL_LOCK_WAIT_MS_DEFAULT;

  /**
   * Validated terminal delivery-attempt cap, from {@code JMS_MAX_DELIVERY_ATTEMPTS}. This field is
   * the sole authority for the poison policy: the consumer never derives a cap from {@code
   * JMSXDeliveryCount}, a message header, a message property, or any other user data. {@link
   * BridgeConfig#maxDeliveryAttempts()} validates the {@code [1, 100]} range before the value
   * reaches here, so the consumer trusts it.
   */
  long maxDeliveryAttempts = BridgeConfig.MAX_DELIVERY_ATTEMPTS_DEFAULT;

  /** The destination journal handle held while this subscription is active; null for topics. */
  private volatile JournalService.JournalHandle journalHandle;

  /**
   * Subscription-scoped volatile redelivery counter for topic destinations (task 3.7). Topics are
   * non-durable: the count lives only for this consumer's subscription, never in a journal file and
   * never under the journal lock. It is keyed by the provider {@code JMSMessageID}, never by an
   * exchange header or an ephemeral subscription identifier, and it resets on disconnect and close.
   * Only the owner thread mutates it; {@link #topicReservationCount(String)} exposes it to tests.
   */
  private final Map<String, Long> topicReservationCounts = new ConcurrentHashMap<>();

  /**
   * Installs the shared journal runtime resolved once by the bridge. A configured runtime carries
   * the process-wide service and broker key; an unconfigured one carries the missing variable so a
   * queue subscribe can fail {@code FAILED_PRECONDITION} naming it. Package-private seam so tests
   * represent a legitimately configured journal without bypassing production configuration.
   *
   * @param journal the shared journal service, or {@code null} when unset
   * @param brokerKey the broker key, or {@code null} when unset
   * @param lockWaitMs the bounded lock wait
   * @param missingEnv the missing variable name, or {@code null} when configured
   */
  void configureJournal(
      JournalService journal, String brokerKey, long lockWaitMs, String missingEnv) {
    configureJournal(
        journal, brokerKey, lockWaitMs, missingEnv, BridgeConfig.MAX_DELIVERY_ATTEMPTS_DEFAULT);
  }

  /**
   * Installs the shared journal runtime and the validated terminal cap. Overload seam: callers that
   * do not pin a cap keep the {@link BridgeConfig} default.
   *
   * @param journal the shared journal service, or {@code null} when unset
   * @param brokerKey the broker key, or {@code null} when unset
   * @param lockWaitMs the bounded lock wait
   * @param missingEnv the missing variable name, or {@code null} when configured
   * @param maxDeliveryAttempts the validated terminal cap from {@code JMS_MAX_DELIVERY_ATTEMPTS}
   */
  void configureJournal(
      JournalService journal,
      String brokerKey,
      long lockWaitMs,
      String missingEnv,
      long maxDeliveryAttempts) {
    this.journal = journal;
    this.journalBrokerKey = brokerKey;
    this.journalLockWaitMs = lockWaitMs;
    this.journalMissingEnv = missingEnv;
    this.maxDeliveryAttempts = maxDeliveryAttempts;
  }

  /** The generation assigned to this subscription, emitted on every forwarded delivery. */
  long subscriptionGeneration() {
    return subscriptionGeneration;
  }

  private long resolveMaxBodyBytes() {
    return pinnedMaxBodyBytes > 0 ? pinnedMaxBodyBytes : maxBodyBytes();
  }

  /**
   * Reads the consumer body cap from {@code JMS_MAX_BODY_BYTES} in bytes. Fails loud on a malformed
   * or non-positive value so a typo never silently disables the cap.
   */
  static long maxBodyBytes() {
    String raw = System.getenv(MAX_BODY_BYTES_ENV);
    if (raw == null || raw.isBlank()) {
      return DEFAULT_MAX_BODY_BYTES;
    }
    return parseCap(raw);
  }

  /**
   * Parses one {@code JMS_MAX_BODY_BYTES} value, failing loud with the env name on malformed,
   * non-positive, or above-ceiling input.
   */
  static long parseCap(String raw) {
    try {
      long parsed = Long.parseLong(raw.trim());
      if (parsed <= 0) {
        throw new IllegalStateException(
            MAX_BODY_BYTES_ENV + " must be a positive byte count: " + raw);
      }
      if (parsed > MAX_BODY_BYTES_CEILING) {
        throw new IllegalStateException(
            MAX_BODY_BYTES_ENV
                + " exceeds its "
                + MAX_BODY_BYTES_CEILING
                + "-byte ceiling: "
                + parsed
                + "; caps above 19 MiB invert the decode-limit ordering, bodies pass this"
                + " Java cap only to fail at the 20 MiB Rust IPC limit");
      }
      return parsed;
    } catch (NumberFormatException e) {
      throw new IllegalStateException(MAX_BODY_BYTES_ENV + " invalid: " + raw, e);
    }
  }

  /**
   * Subscribe to a JMS destination and forward messages to the gRPC stream.
   *
   * <p>The owner thread (this call's dedicated thread) runs {@code CLIENT_ACKNOWLEDGE} with exactly
   * one unacknowledged delivery at a time. It polls with {@code receive(timeout)} instead of an
   * async listener so it owns both the JMS receive and the gRPC write, then blocks after each
   * handoff until a valid {@code Complete} decision arrives. The thread is the only thread that
   * touches the session.
   *
   * <p>The message selector arrives through the {@link #messageSelector} field, assigned from the
   * subscribe request before this call — the same pre-loop wiring as the journal runtime. The
   * expression goes to the provider verbatim: an invalid selector is a broker-owned failure
   * surfaced on the observer, never validated or rewritten locally.
   *
   * @param finished shared flag indicating the stream has terminated. The gRPC cancel handler or
   *     this consumer's terminal callback sets it; the consumer only reads it, so the service's
   *     compared-and-set teardown stays the single owner of observer termination.
   */
  public void subscribe(
      String destination,
      String subscriptionId,
      StreamObserver<JmsMessage> observer,
      AtomicBoolean finished) {
    // Startup validation (ADR-0033): a malformed JMS_MAX_BODY_BYTES must fail loud before any
    // broker connection is created.
    maxBodyBytes();

    synchronized (settleLock) {
      running = true;
      stopping = false;
      activeToken = null;
      pendingDecision = null;
      pendingDecisionFuture = null;
    }
    resourcesClosed.set(false);
    journalHandle = null;
    topicReservationCounts.clear();

    Thread t =
        new Thread(
            () -> runOwner(destination, observer, finished), "jms-consumer-" + subscriptionId);
    t.setDaemon(true);
    ownerThread = t;
    t.start();
  }

  /**
   * Owner thread body: connect, acquire the destination journal for a queue, then for each delivery
   * reserve, hand it off, and settle at one linearization point. All session operations (receive,
   * acknowledge, recover, close) stay on this thread.
   */
  private void runOwner(
      String destination, StreamObserver<JmsMessage> observer, AtomicBoolean finished) {
    boolean topic = isTopic(destination);
    try {
      // A queue receipt is keyed and reserved, so an unconfigured journal is a permanent
      // precondition failure for the route. A topic uses a subscription-scoped volatile counter
      // and never touches the journal.
      if (!topic) {
        requireJournalConfigured();
      }

      try {
        connection = factory.createDedicatedConnection();
      } catch (Exception e) {
        if (running && !finished.get()) {
          safeOnError(observer, e);
        }
        return;
      }

      connection.start();
      // Bridge-managed settlement: the session is ALWAYS CLIENT_ACKNOWLEDGE. Endpoint
      // acknowledgementMode and transactionMode options never reach Java over the proto and, by
      // design, do not change this (spec: "Bridge-managed settlement ignores endpoint options").
      // A requested transactionMode=Session only warns on the Rust side.
      session = connection.createSession(false, Session.CLIENT_ACKNOWLEDGE);
      Destination dest = JmsProducer.parseDestination(session, destination);
      if (messageSelector != null && !messageSelector.isBlank()) {
        consumer = session.createConsumer(dest, messageSelector);
      } else {
        consumer = session.createConsumer(dest);
      }

      if (!topic) {
        // Lazy ownership: acquire the destination journal before the first receive, on this owner
        // thread. The shared service refcounts the domain, so the OS lock is released when the
        // last subscription ends (cancel, disconnect, stop, shutdown).
        acquireJournal(destination);
      }

      while (running && !finished.get()) {
        Message msg = consumer.receive(1000);
        if (msg == null) {
          continue;
        }
        LOG.debug("Received JMS message on " + destination);

        String messageId = readMessageId(msg);

        // Terminal reject-then-log BEFORE conversion (task 3.8): when the recorded reservation
        // count has reached the configured cap, the receipt is discarded without a handoff, so no
        // body or header accessor ever runs on a poison message. The count is read and never
        // incremented: exactly maxDeliveryAttempts reservations are recorded, and the receipt that
        // would exceed the cap adds no RESERVE. A first receipt with no entry reads zero and is
        // reserved normally below.
        long recordedCount =
            topic ? topicReservationCount(messageId) : journalHandle.count(messageId);
        if (recordedCount >= maxDeliveryAttempts) {
          terminalDiscard(msg, messageId, recordedCount, destination, topic);
          continue;
        }

        if (topic) {
          // Topics never touch the journal: the attempt is recorded in the subscription-scoped
          // volatile counter before conversion, keyed by the provider JMSMessageID, and a
          // redelivery of the same id increments it.
          reserveTopic(messageId);
        } else {
          reserve(messageId);
        }
        long reservationCount =
            topic ? topicReservationCount(messageId) : journalHandle.count(messageId);

        JmsMessage grpcMsg;
        try {
          grpcMsg = convertMessage(msg, destination);
        } catch (Exception e) {
          // Conversion failure: the receipt already counted one attempt; roll the queue journal
          // back (the count stays) and recover the session before the next receive. This is the
          // SAME attempt, never a second reservation. A topic keeps its volatile count, so the
          // redelivery increments it. The message never reached the route, so NEVER acknowledge it;
          // a recoverable failure keeps the stream healthy and the broker redelivers. If recover
          // fails the session is unusable, so close without acknowledging.
          countReservationFailure();
          LOG.error("Error forwarding message: " + e.getMessage(), e);
          if (!topic) {
            rollbackAttempt(messageId);
          }
          try {
            session.recover();
          } catch (Exception recoverFailure) {
            if (running && !finished.get()) {
              safeOnError(observer, recoverFailure);
            }
            return;
          }
          continue;
        }

        String token = UUID.randomUUID().toString();
        grpcMsg =
            grpcMsg.toBuilder()
                .setDeliveryToken(token)
                .setSubscriptionGeneration(subscriptionGeneration)
                .setReservationCount((int) Math.min(reservationCount, Integer.MAX_VALUE))
                .build();
        synchronized (settleLock) {
          activeToken = token;
          pendingDecision = null;
        }

        if (!running || finished.get()) {
          // Cancel/disconnect observed before the handoff: close without acknowledging. No journal
          // settlement runs, so the durable count is retained (conservative retention).
          break;
        }
        try {
          observer.onNext(grpcMsg);
        } catch (Exception e) {
          // Stream broke before the route saw the delivery: close without acknowledging.
          break;
        }

        CompletionOutcome settled = awaitDecision();
        if (settled == null) {
          // Stop/cancel/disconnect before the decision: no ack, no recover, no journal settlement.
          // The count is retained.
          settleCurrent(false);
          break;
        }
        try {
          if (settled == CompletionOutcome.COMMIT) {
            // Ack first: the broker is the delivery authority. The journal commit runs only after
            // the ack is confirmed; if it then fails the count lingers conservatively for
            // retention, and the ack is never retried. A committed topic delivery clears its
            // volatile entry, so a later redelivery of the same id starts a fresh count.
            msg.acknowledge();
            if (topic) {
              topicReservationCounts.remove(messageId);
            } else {
              journalHandle.commit(messageId);
            }
          } else {
            // Recover first; the queue journal rollback runs only after a confirmed recover. Both
            // keep the reservation count. A topic rollback keeps its volatile count so the
            // redelivery increments it; there is no journal to touch.
            session.recover();
            if (!topic) {
              journalHandle.rollback(messageId);
            }
          }
        } catch (Exception e) {
          // Uncertain settlement: the broker may or may not have applied it, and a later
          // cumulative CLIENT_ACKNOWLEDGE would cover this delivery. Never receive again and
          // never retry speculatively: release the caller as not accepted and fail the stream so
          // the owner thread closes every resource.
          settleCurrent(false);
          JMSException uncertain =
              new JMSException("settlement uncertain for " + destination + ": " + e);
          uncertain.initCause(e);
          throw uncertain;
        }
        settleCurrent(true);
      }

      if (!finished.get()) {
        try {
          observer.onCompleted();
        } catch (Exception ignored) {
        }
      }
    } catch (Exception e) {
      if (running && !finished.get()) {
        safeOnError(observer, e);
      }
    } finally {
      failPendingDecision();
      closeResources();
    }
  }

  /** Whether the destination addresses a topic rather than a queue. */
  private static boolean isTopic(String destination) {
    return destination.startsWith("topic:");
  }

  /** Fails a queue subscribe when the bridge has no journal directory or broker key. */
  private void requireJournalConfigured() {
    if (journalMissingEnv != null) {
      throw failedPrecondition(
          new JournalPermanentException(
              journalMissingEnv
                  + " is not set; queue consumption requires a configured attempt journal"));
    }
  }

  /**
   * Acquires the destination journal on the owner thread before the first receive. The shared
   * service refcounts the domain, so the OS lock is held until the last subscription ends.
   */
  private void acquireJournal(String destination) {
    if (journal == null || journalBrokerKey == null || journalBrokerKey.isBlank()) {
      throw failedPrecondition(
          new JournalPermanentException(
              JournalService.JOURNAL_DIRECTORY_ENV + " is not configured"));
    }
    String queueName =
        destination.startsWith("queue:") ? destination.substring("queue:".length()) : destination;
    try {
      journalHandle =
          journal.acquire(
              journalBrokerKey, JournalService.destinationKey(queueName), journalLockWaitMs);
    } catch (JournalPermanentException e) {
      throw failedPrecondition(e);
    }
  }

  /**
   * Reads the delivery key, failing the route as a permanent precondition when it is unreadable.
   */
  private static String readMessageId(Message msg) {
    try {
      return msg.getJMSMessageID();
    } catch (JMSException e) {
      throw failedPrecondition(
          new JournalPermanentException("cannot read JMSMessageID for reservation", e));
    }
  }

  /** Records one durable RESERVE before conversion; an unkeyable id fails the route closed. */
  private void reserve(String messageId) {
    if (messageId == null || messageId.isBlank()) {
      throw failedPrecondition(
          new JournalPermanentException(
              "message has no usable JMSMessageID; it is preserved and never acknowledged"));
    }
    try {
      journalHandle.reserve(messageId);
    } catch (JournalPermanentException e) {
      throw failedPrecondition(e);
    }
  }

  /**
   * Records one volatile topic attempt before conversion, keyed by the provider {@code
   * JMSMessageID}. Topics are non-durable, so the count never reaches a file or the journal lock; a
   * redelivery of the same id increments it. An unkeyable id fails the route closed exactly like a
   * queue receipt, and a header is never trusted as the key.
   */
  private void reserveTopic(String messageId) {
    if (messageId == null || messageId.isBlank()) {
      throw failedPrecondition(
          new JournalPermanentException(
              "message has no usable JMSMessageID; it is preserved and never acknowledged"));
    }
    topicReservationCounts.merge(messageId, 1L, Long::sum);
  }

  /** Rolls back the current attempt after a conversion failure; the count is kept. */
  private void rollbackAttempt(String messageId) {
    try {
      journalHandle.rollback(messageId);
    } catch (JournalPermanentException e) {
      throw failedPrecondition(e);
    }
  }

  /**
   * Terminal reject-then-log for a receipt whose recorded reservation count has already reached
   * {@link #maxDeliveryAttempts}. The delivery is never handed off and no body or header accessor
   * runs: the provider {@code JMSMessageID} is the only message field read. The broker ACK is the
   * delivery authority, so the reservation is cleared only after the ACK is confirmed; the discard
   * is logged after the ACK but before the clear, so a journal-clear I/O error can never suppress
   * the terminal observability. No dead-letter or database forwarding happens.
   *
   * @param msg the poison delivery; only {@code acknowledge()} is invoked
   * @param messageId the provider message ID used as the reservation key
   * @param count the recorded reservation count (equal to the cap)
   * @param destination the destination, for diagnostics only
   * @param topic whether this is a non-durable topic delivery
   */
  private void terminalDiscard(
      Message msg, String messageId, long count, String destination, boolean topic)
      throws JMSException {
    try {
      msg.acknowledge();
    } catch (Exception e) {
      // Uncertain ACK: the broker may or may not have discarded the delivery. Never receive again,
      // never retry speculatively, and never clear the reservation. Release any waiter as not
      // accepted and fail the stream so the owner thread closes every resource.
      settleCurrent(false);
      JMSException uncertain =
          new JMSException("terminal acknowledgement uncertain for " + destination + ": " + e);
      uncertain.initCause(e);
      throw uncertain;
    }
    // The ACK stands: the discard is real, so log it now. The line carries only the message ID and
    // the recorded count — never the body, headers, or credential values.
    logTerminalDiscard(terminalDiscardLogLine(messageId, count));
    if (topic) {
      // Topics are non-durable: the subscription-scoped volatile entry is the only state to clear.
      topicReservationCounts.remove(messageId);
      return;
    }
    try {
      clearTerminalReservation(messageId);
    } catch (Exception e) {
      // The broker ACK already stands; the reservation lingers conservatively and the ACK is never
      // retried. Fail the stream so the owner thread closes every resource.
      JMSException clearFailure =
          new JMSException("terminal reservation clear failed for " + destination + ": " + e);
      clearFailure.initCause(e);
      throw clearFailure;
    }
  }

  /**
   * The terminal discard line: the provider {@code JMSMessageID} and the recorded reservation count
   * only. No body, header name, header value, or credential value ever enters it. Package-private
   * so a test can assert its exact content.
   *
   * @param messageId the provider message ID
   * @param count the recorded reservation count
   * @return the log line
   */
  static String terminalDiscardLogLine(String messageId, long count) {
    return "Poison message discarded after " + count + " delivery attempts: " + messageId;
  }

  /**
   * Emits the terminal discard at error. A test-instrumentation seam so a test can observe the
   * exact line the owner thread logs.
   *
   * @param line the pre-built discard line
   */
  void logTerminalDiscard(String line) {
    LOG.error(line);
  }

  /**
   * Clears the terminal reservation after a confirmed broker ACK. A test-instrumentation seam so a
   * test can simulate a post-ACK journal clear I/O error.
   *
   * @param messageId the provider message ID used as the reservation key
   * @throws JournalPermanentException when the durable clear fails
   */
  void clearTerminalReservation(String messageId) throws JournalPermanentException {
    journalHandle.commit(messageId);
  }

  /** Builds the permanent-failure status the bridge maps to a route-only precondition failure. */
  private static StatusRuntimeException failedPrecondition(JournalPermanentException cause) {
    return Status.FAILED_PRECONDITION.withDescription(cause.getMessage()).asRuntimeException();
  }

  /**
   * Wait without a deadline for the terminal decision on the current reservation. A stop wakes the
   * wait and yields no decision, so the owner thread closes without acknowledging.
   */
  private CompletionOutcome awaitDecision() {
    synchronized (settleLock) {
      while (pendingDecision == null && !stopping) {
        try {
          settleLock.wait();
        } catch (InterruptedException e) {
          Thread.currentThread().interrupt();
          return null;
        }
      }
      return pendingDecision;
    }
  }

  /**
   * Validate and publish a terminal decision. Returns only after the owner thread has settled it
   * (acknowledged or recovered), so the caller reports the actual settlement. A stale generation or
   * token, an outcome other than {@code COMMIT} or {@code ROLLBACK}, a stop, or a duplicate for the
   * same reservation is rejected without touching the session.
   */
  public boolean complete(long generation, String deliveryToken, CompletionOutcome outcome) {
    CompletableFuture<Boolean> settled;
    synchronized (settleLock) {
      if (stopping
          || !running
          || activeToken == null
          || deliveryToken == null
          || !activeToken.equals(deliveryToken)
          || generation != subscriptionGeneration
          || (outcome != CompletionOutcome.COMMIT && outcome != CompletionOutcome.ROLLBACK)
          || pendingDecision != null) {
        return false;
      }
      pendingDecision = outcome;
      settled = new CompletableFuture<>();
      pendingDecisionFuture = settled;
      settleLock.notifyAll();
    }
    try {
      return Boolean.TRUE.equals(settled.get());
    } catch (InterruptedException e) {
      Thread.currentThread().interrupt();
      return false;
    } catch (ExecutionException e) {
      return false;
    }
  }

  /** Clear the current reservation and release any waiting Complete caller. */
  private void settleCurrent(boolean accepted) {
    synchronized (settleLock) {
      CompletableFuture<Boolean> f = pendingDecisionFuture;
      activeToken = null;
      pendingDecision = null;
      pendingDecisionFuture = null;
      if (f != null) {
        f.complete(accepted);
      }
    }
  }

  /** Release any waiting Complete caller at teardown so no RPC future leaks. */
  private void failPendingDecision() {
    synchronized (settleLock) {
      CompletableFuture<Boolean> f = pendingDecisionFuture;
      activeToken = null;
      pendingDecision = null;
      pendingDecisionFuture = null;
      if (f != null && !f.isDone()) {
        f.complete(false);
      }
      settleLock.notifyAll();
    }
  }

  private static void safeOnError(StreamObserver<JmsMessage> observer, Throwable t) {
    try {
      observer.onError(t);
    } catch (Exception ignored) {
    }
  }

  /**
   * Signal teardown without touching JMS. The owner thread wakes, closes the session on its own
   * thread, and never acknowledges a reservation stopped before its decision.
   */
  public void stop() {
    synchronized (settleLock) {
      running = false;
      stopping = true;
      settleLock.notifyAll();
    }
  }

  /**
   * Waits, bounded, for the owner thread to close the JMS resources and release the journal.
   *
   * <p>Test seam: {@link #stop()} is deliberately non-blocking, so a caller that must observe the
   * resources released (for example a Windows temp-dir teardown) can join the owner thread here.
   * Returns immediately when no owner thread ever started.
   *
   * @param timeoutMs the maximum wait in milliseconds; non-positive values never block
   * @return true when no owner thread is running or it terminated within the timeout
   */
  boolean awaitStopped(long timeoutMs) {
    Thread owner = ownerThread;
    if (owner == null) {
      return true;
    }
    if (timeoutMs > 0L) {
      try {
        owner.join(timeoutMs);
      } catch (InterruptedException e) {
        Thread.currentThread().interrupt();
      }
    }
    return !owner.isAlive();
  }

  private void closeResources() {
    if (!resourcesClosed.compareAndSet(false, true)) return;
    try {
      if (consumer != null) {
        consumer.close();
        consumer = null;
      }
    } catch (Exception ignored) {
    }
    try {
      if (session != null) {
        session.close();
        session = null;
      }
    } catch (Exception ignored) {
    }
    try {
      if (connection != null) {
        connection.close();
        connection = null;
      }
    } catch (Exception ignored) {
    }
    // Release the destination journal only after the JMS resources are closed, so the shared OS
    // lock is never dropped while the session still holds a delivery. The shared service releases
    // the kernel lock on the last handle for the domain. Topic counts are non-durable and are
    // discarded with the subscription: a later disconnect always starts a fresh budget.
    JournalService.JournalHandle handle = journalHandle;
    journalHandle = null;
    if (handle != null) {
      handle.close();
    }
    topicReservationCounts.clear();
  }

  private JmsMessage convertMessage(Message msg, String destination) throws JMSException {
    JmsMessage.Builder b = JmsMessage.newBuilder();
    b.setMessageId(msg.getJMSMessageID() != null ? msg.getJMSMessageID() : "");
    b.setCorrelationId(msg.getJMSCorrelationID() != null ? msg.getJMSCorrelationID() : "");
    b.setTimestamp(msg.getJMSTimestamp());
    b.setDestination(destination);

    // Content-type fidelity (rc-kzti): an explicit "ContentType" property wins over the
    // TextMessage default. Defensive read: well-behaved providers return null for an absent
    // property; a misbehaving provider that throws (checked or unchecked) must not kill
    // forwarding — same posture as the header-enumeration loop below.
    String contentTypeProp = null;
    try {
      contentTypeProp = msg.getStringProperty("ContentType");
    } catch (Exception ignored) {
    }

    if (msg instanceof BytesMessage bm) {
      long len = bm.getBodyLength();
      long cap = resolveMaxBodyBytes();
      if (len < 0 || len > cap) {
        String diagnostic =
            MAX_BODY_BYTES_ENV
                + ": rejecting message body of "
                + len
                + " bytes (cap "
                + cap
                + " bytes); message not forwarded";
        // Handler-contract boundary (ADR-0012): the consumer logs at warn and forwards the error
        // outcome; the route owns the operational signal.
        LOG.warn(diagnostic);
        throw new JMSException(diagnostic);
      }
      byte[] buf = new byte[(int) len];
      bm.readBytes(buf);
      b.setBody(ByteString.copyFrom(buf));
    } else if (msg instanceof TextMessage tm) {
      // TextMessage exposes no pre-read length, so the cap lands after getText(): the string is
      // materialized by the JMS client either way, but oversized text never reaches the protobuf
      // body or the stream.
      String text = tm.getText();
      ByteString body = ByteString.copyFromUtf8(text != null ? text : "");
      long size = body.size();
      long cap = resolveMaxBodyBytes();
      if (size > cap) {
        String diagnostic =
            MAX_BODY_BYTES_ENV
                + ": rejecting message body of "
                + size
                + " bytes (cap "
                + cap
                + " bytes); message not forwarded";
        // Handler-contract boundary (ADR-0012): the consumer logs at warn and forwards the error
        // outcome; the route owns the operational signal.
        LOG.warn(diagnostic);
        throw new JMSException(diagnostic);
      }
      b.setBody(body);
      // Empty means "no real content type"; whitespace-only values are preserved as-is.
      b.setContentType(
          contentTypeProp != null && !contentTypeProp.isEmpty() ? contentTypeProp : "text/plain");
    }

    java.util.Enumeration<?> names = msg.getPropertyNames();
    while (names.hasMoreElements()) {
      String name = names.nextElement().toString();
      try {
        b.putHeaders(name, String.valueOf(msg.getObjectProperty(name)));
      } catch (Exception ignored) {
      }
    }
    return b.build();
  }
}
