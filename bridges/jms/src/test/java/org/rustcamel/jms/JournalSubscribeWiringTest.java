package org.rustcamel.jms;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;
import static org.mockito.ArgumentMatchers.any;
import static org.mockito.ArgumentMatchers.anyLong;
import static org.mockito.ArgumentMatchers.anyString;
import static org.mockito.Mockito.doAnswer;
import static org.mockito.Mockito.mock;
import static org.mockito.Mockito.never;
import static org.mockito.Mockito.verify;
import static org.mockito.Mockito.when;

import io.grpc.Status;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.attribute.PosixFilePermissions;
import java.util.Map;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.concurrent.atomic.AtomicReference;
import javax.jms.Connection;
import javax.jms.MessageConsumer;
import javax.jms.MessageProducer;
import javax.jms.Queue;
import javax.jms.Session;
import javax.jms.TextMessage;
import javax.jms.Topic;
import jms_bridge.CompletionOutcome;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;
import org.mockito.stubbing.Answer;

/**
 * Lazy journal ownership wired into a queue subscribe (jms-bridge-080 task 3.4).
 *
 * <p>A queue subscribe acquires the destination journal on the owner thread before the first
 * receive and releases it when the last subscription ends; producers and topics never lock.
 * Readiness validates the directory without opening a journal, unset env defers the failure to a
 * queue subscribe, and a set-but-invalid directory or broker key is a startup permanent failure.
 * All nine tests use a real {@link JournalService} on a trusted temp directory, never a production
 * bypass.
 */
class JournalSubscribeWiringTest {

  private static final String KEY = "0123456789abcdef0123456789abcdef";
  private static final String DEST = "queue:test";
  private static final String TOPIC = "topic:test";
  private static final long AWAIT_SECONDS = 15L;

  @TempDir Path tempDir;

  /** A consumer wired to fresh mocked JMS and a real journal runtime. */
  private record Wired(
      JmsConsumer consumer,
      JmsClientFactory factory,
      Session session,
      MessageConsumer receive,
      Connection connection) {}

  private JournalService newJournal() {
    return new JournalService(
        tempDir,
        BridgeConfig.JOURNAL_MAX_BYTES_DEFAULT,
        BridgeConfig.JOURNAL_RETENTION_MS_DEFAULT,
        System::currentTimeMillis);
  }

  private Path brokerDir() {
    return tempDir.resolve(KEY);
  }

  private Path journalFile() {
    return brokerDir().resolve(JournalService.destinationKey("test") + ".journal");
  }

  private Wired wired(JournalService journal, String brokerKey, long lockWaitMs, String missingEnv)
      throws Exception {
    Connection connection = mock(Connection.class);
    Session session = mock(Session.class);
    MessageConsumer messageConsumer = mock(MessageConsumer.class);
    Queue queue = mock(Queue.class);
    JmsClientFactory factory = mock(JmsClientFactory.class);
    when(factory.createDedicatedConnection()).thenReturn(connection);
    doAnswer(inv -> null).when(connection).start();
    when(connection.createSession(false, Session.CLIENT_ACKNOWLEDGE)).thenReturn(session);
    when(session.createQueue("test")).thenReturn(queue);
    when(session.createConsumer(any())).thenReturn(messageConsumer);
    JmsConsumer consumer = new JmsConsumer();
    consumer.factory = factory;
    consumer.configureJournal(journal, brokerKey, lockWaitMs, missingEnv);
    return new Wired(consumer, factory, session, messageConsumer, connection);
  }

  /** Idle between polls so a mocked receive never spins the owner thread. */
  private static Answer<Object> idleNull() {
    return invocation -> {
      Thread.sleep(200);
      return null;
    };
  }

  private static TextMessage textMessage(String messageId) throws Exception {
    TextMessage message = mock(TextMessage.class);
    when(message.getJMSMessageID()).thenReturn(messageId);
    when(message.getJMSCorrelationID()).thenReturn(null);
    when(message.getJMSTimestamp()).thenReturn(0L);
    when(message.getPropertyNames()).thenReturn(java.util.Collections.emptyEnumeration());
    when(message.getText()).thenReturn("hello");
    return message;
  }

  private static void await(BooleanSupplier condition, String what) throws Exception {
    long deadline = System.nanoTime() + TimeUnit.SECONDS.toNanos(AWAIT_SECONDS);
    while (!condition.getAsBoolean() && System.nanoTime() < deadline) {
      Thread.sleep(10);
    }
    assertTrue(condition.getAsBoolean(), what);
  }

  /** Minimal boolean supplier so the polling helper reads clearly. */
  @FunctionalInterface
  private interface BooleanSupplier {
    boolean getAsBoolean();
  }

  @Test
  void lock_acquired_at_first_subscribe() throws Exception {
    JournalService journal = newJournal();
    Wired w = wired(journal, KEY, 2000L, null);
    AtomicReference<Boolean> existedAtFirstReceive = new AtomicReference<>();
    AtomicInteger receives = new AtomicInteger();
    when(w.receive().receive(anyLong()))
        .thenAnswer(
            invocation -> {
              if (receives.incrementAndGet() == 1) {
                existedAtFirstReceive.set(Files.exists(journalFile()));
                return textMessage("ID:1");
              }
              Thread.sleep(200);
              return null;
            });
    JmsConsumerBodyCapTest.RecordingObserver obs = new JmsConsumerBodyCapTest.RecordingObserver();

    w.consumer().subscribe(DEST, "sub", obs, new AtomicBoolean(false));
    assertTrue(obs.next.await(AWAIT_SECONDS, TimeUnit.SECONDS), "one delivery must be forwarded");

    assertEquals(
        Boolean.TRUE,
        existedAtFirstReceive.get(),
        "the destination journal must be acquired before the first receive");
    w.consumer().stop();
  }

  @Test
  void lock_released_on_stream_end() throws Exception {
    JournalService journal = newJournal();
    Wired w = wired(journal, KEY, 2000L, null);
    // One keyable delivery on the first poll; later polls idle.
    AtomicInteger receives = new AtomicInteger();
    when(w.receive().receive(anyLong()))
        .thenAnswer(
            invocation -> {
              if (receives.incrementAndGet() == 1) {
                return textMessage("ID:1");
              }
              Thread.sleep(200);
              return null;
            });

    JmsConsumerBodyCapTest.RecordingObserver obs = new JmsConsumerBodyCapTest.RecordingObserver();
    w.consumer().subscribe(DEST, "sub", obs, new AtomicBoolean(false));
    assertTrue(obs.next.await(AWAIT_SECONDS, TimeUnit.SECONDS), "one delivery must be forwarded");

    // While the subscription is live the domain is locked: a second service cannot acquire it.
    JournalService other = newJournal();
    assertThrows(
        JournalPermanentException.class,
        () -> other.acquire(KEY, JournalService.destinationKey("test"), 150L),
        "the live subscription must hold the destination lock");

    // Stream end releases the lock and a later owner observes the retained count.
    w.consumer().stop();
    JournalService.JournalHandle reopened = null;
    long deadline = System.nanoTime() + TimeUnit.SECONDS.toNanos(AWAIT_SECONDS);
    while (reopened == null && System.nanoTime() < deadline) {
      try {
        reopened = other.acquire(KEY, JournalService.destinationKey("test"), 200L);
      } catch (JournalPermanentException e) {
        Thread.sleep(20);
      }
    }
    assertTrue(reopened != null, "the lock must be released when the stream ends");
    try (JournalService.JournalHandle handle = reopened) {
      assertEquals(
          1L, handle.count("ID:1"), "the later owner must observe the retained reservation count");
    }
  }

  @Test
  void producer_and_topic_paths_never_lock() throws Exception {
    JournalService journal = newJournal();
    Wired topicWired = wired(journal, KEY, 2000L, null);
    when(topicWired.session().createTopic("test")).thenReturn(mock(Topic.class));
    when(topicWired.receive().receive(anyLong())).thenAnswer(idleNull());
    JmsConsumerBodyCapTest.RecordingObserver topicObs =
        new JmsConsumerBodyCapTest.RecordingObserver();
    topicWired.consumer().subscribe(TOPIC, "sub-topic", topicObs, new AtomicBoolean(false));
    topicWired.consumer().stop();

    // Producer path: a queue send through the same bridge never opens the journal.
    JmsProducer producer = new JmsProducer();
    JmsClientFactory producerFactory = mock(JmsClientFactory.class);
    Connection connection = mock(Connection.class);
    Session session = mock(Session.class);
    MessageProducer messageProducer = mock(MessageProducer.class);
    TextMessage sent = mock(TextMessage.class);
    when(sent.getJMSMessageID()).thenReturn("ID:sent");
    when(producerFactory.createConnection()).thenReturn(connection);
    when(connection.createSession(false, Session.AUTO_ACKNOWLEDGE)).thenReturn(session);
    when(session.createQueue("test")).thenReturn(mock(Queue.class));
    when(session.createProducer(any())).thenReturn(messageProducer);
    when(session.createTextMessage(anyString())).thenReturn(sent);
    producer.factory = producerFactory;
    producer.send(DEST, "hi".getBytes(StandardCharsets.UTF_8), Map.of(), "text/plain");

    assertFalse(
        Files.exists(brokerDir()),
        "producer and topic paths must never create, lock, or replay a queue journal");
  }

  @Test
  void busy_lock_fails_precondition() throws Exception {
    JournalService owner = newJournal();
    JournalService other = newJournal();
    try (JournalService.JournalHandle held =
        owner.acquire(KEY, JournalService.destinationKey("test"), 1000L)) {
      Wired w = wired(other, KEY, 150L, null);
      when(w.receive().receive(anyLong())).thenAnswer(idleNull());
      JmsConsumerBodyCapTest.RecordingObserver obs = new JmsConsumerBodyCapTest.RecordingObserver();

      w.consumer().subscribe(DEST, "sub", obs, new AtomicBoolean(false));

      assertTrue(obs.errored.await(AWAIT_SECONDS, TimeUnit.SECONDS), "a busy lock must fail");
      Status status = Status.fromThrowable(obs.error.get());
      assertEquals(Status.Code.FAILED_PRECONDITION, status.getCode());
      assertTrue(
          status.getDescription().contains(JournalService.JOURNAL_DIRECTORY_ENV),
          "the failure must name JMS_ATTEMPT_JOURNAL_DIR: " + status.getDescription());
    }
  }

  @Test
  void unset_journal_env_readiness_producer_ok_subscribe_precondition() throws Exception {
    // Readiness accepts unset values: startup validation must not fail.
    JournalService.validateStartup(null, null);
    PortAnnouncer.validateJournalStartup(Map.of());

    // Producer works without a journal.
    JmsProducer producer = new JmsProducer();
    JmsClientFactory producerFactory = mock(JmsClientFactory.class);
    Connection connection = mock(Connection.class);
    Session session = mock(Session.class);
    MessageProducer messageProducer = mock(MessageProducer.class);
    TextMessage sent = mock(TextMessage.class);
    when(sent.getJMSMessageID()).thenReturn("ID:sent");
    when(producerFactory.createConnection()).thenReturn(connection);
    when(connection.createSession(false, Session.AUTO_ACKNOWLEDGE)).thenReturn(session);
    when(session.createQueue("test")).thenReturn(mock(Queue.class));
    when(session.createProducer(any())).thenReturn(messageProducer);
    when(session.createTextMessage(anyString())).thenReturn(sent);
    producer.factory = producerFactory;
    producer.send(DEST, "hi".getBytes(StandardCharsets.UTF_8), Map.of(), "text/plain");
    verify(producerFactory).createConnection();

    // Queue subscribe fails FAILED_PRECONDITION naming the missing variable, before connecting.
    Wired queueWired = wired(null, null, 1000L, JournalService.BROKER_KEY_ENV);
    JmsConsumerBodyCapTest.RecordingObserver queueObs =
        new JmsConsumerBodyCapTest.RecordingObserver();
    queueWired.consumer().subscribe(DEST, "sub-queue", queueObs, new AtomicBoolean(false));
    assertTrue(queueObs.errored.await(AWAIT_SECONDS, TimeUnit.SECONDS), "queue must fail closed");
    Status status = Status.fromThrowable(queueObs.error.get());
    assertEquals(Status.Code.FAILED_PRECONDITION, status.getCode());
    assertTrue(
        status.getDescription().contains(JournalService.BROKER_KEY_ENV),
        "the failure must name JMS_BRIDGE_BROKER_KEY: " + status.getDescription());
    verify(queueWired.factory(), never()).createDedicatedConnection();

    // Topic consumption still works.
    Wired topicWired = wired(null, null, 1000L, JournalService.BROKER_KEY_ENV);
    when(topicWired.session().createTopic("test")).thenReturn(mock(Topic.class));
    TextMessage topicMessage = textMessage("ID:t");
    when(topicWired.receive().receive(anyLong())).thenReturn(topicMessage).thenAnswer(idleNull());
    JmsConsumerBodyCapTest.RecordingObserver topicObs =
        new JmsConsumerBodyCapTest.RecordingObserver();
    topicWired.consumer().subscribe(TOPIC, "sub-topic", topicObs, new AtomicBoolean(false));
    assertTrue(topicObs.next.await(AWAIT_SECONDS, TimeUnit.SECONDS), "topics must keep working");
    topicWired.consumer().stop();
  }

  @Test
  void invalid_dir_is_startup_permanent_before_readiness() throws Exception {
    int announcementsBefore = PortAnnouncer.ANNOUNCEMENTS.size();
    Path target = Files.createDirectory(tempDir.resolve("target"));
    Path symlink = tempDir.resolve("escape");
    Files.createSymbolicLink(symlink, target);

    assertThrows(
        JournalPermanentException.class,
        () ->
            PortAnnouncer.validateJournalStartup(
                Map.of(
                    JournalService.JOURNAL_DIRECTORY_ENV,
                    symlink.toString(),
                    JournalService.BROKER_KEY_ENV,
                    KEY)),
        "a symlink escape must fail before readiness");
    assertThrows(
        JournalPermanentException.class,
        () ->
            PortAnnouncer.validateJournalStartup(
                Map.of(
                    JournalService.JOURNAL_DIRECTORY_ENV,
                    tempDir.resolve("missing").toString(),
                    JournalService.BROKER_KEY_ENV,
                    KEY)),
        "an unresolved directory must fail before readiness");

    Path lax =
        Files.createDirectory(
            tempDir.resolve("lax"),
            PosixFilePermissions.asFileAttribute(PosixFilePermissions.fromString("rwxr-xr-x")));
    assertThrows(
        JournalPermanentException.class,
        () ->
            PortAnnouncer.validateJournalStartup(
                Map.of(
                    JournalService.JOURNAL_DIRECTORY_ENV,
                    lax.toString(),
                    JournalService.BROKER_KEY_ENV,
                    KEY)),
        "a group-accessible directory is not owner-private");

    assertEquals(
        announcementsBefore,
        PortAnnouncer.ANNOUNCEMENTS.size(),
        "a startup permanent failure must not report readiness");
  }

  @Test
  void invalid_broker_key_is_startup_permanent() {
    JournalPermanentException tooShort =
        assertThrows(
            JournalPermanentException.class,
            () -> JournalService.validateStartup(null, "0123456789abcdef"));
    assertTrue(
        tooShort.getMessage().contains(JournalService.BROKER_KEY_ENV), tooShort.getMessage());

    assertThrows(
        JournalPermanentException.class,
        () -> JournalService.validateStartup(null, "NOTHEX"),
        "a non-hex key must fail");
    assertThrows(
        JournalPermanentException.class,
        () -> JournalService.validateStartup(null, "0123456789ABCDEF0123456789ABCDEF"),
        "an uppercase key must fail");
  }

  @Test
  void readiness_opens_no_journal() throws Exception {
    Path root =
        Files.createDirectory(
            tempDir.resolve("root"),
            PosixFilePermissions.asFileAttribute(PosixFilePermissions.fromString("rwx------")));

    JournalService.validateStartup(root.toString(), KEY);
    PortAnnouncer.validateJournalStartup(
        Map.of(
            JournalService.JOURNAL_DIRECTORY_ENV,
            root.toString(),
            JournalService.BROKER_KEY_ENV,
            KEY));

    try (var entries = Files.list(root)) {
      assertEquals(0L, entries.count(), "readiness must not open, lock, or create a journal");
    }
  }

  @Test
  void two_subscriptions_same_domain_share_cap() throws Exception {
    JournalService journal = newJournal();
    Wired first = wired(journal, KEY, 2000L, null);
    Wired second = wired(journal, KEY, 2000L, null);
    TextMessage firstMessage = textMessage("ID:shared");
    TextMessage secondMessage = textMessage("ID:shared");
    when(first.receive().receive(anyLong())).thenReturn(firstMessage).thenAnswer(idleNull());
    when(second.receive().receive(anyLong())).thenReturn(secondMessage).thenAnswer(idleNull());

    JmsConsumerBodyCapTest.RecordingObserver firstObs =
        new JmsConsumerBodyCapTest.RecordingObserver();
    first.consumer().subscribe(DEST, "sub-a", firstObs, new AtomicBoolean(false));
    assertTrue(firstObs.next.await(AWAIT_SECONDS, TimeUnit.SECONDS), "first delivery must arrive");
    assertEquals(
        1, firstObs.last.get().getReservationCount(), "the first receipt is reservation one");

    // Roll back the first attempt: the count stays, so the second subscription sees it.
    assertTrue(
        first
            .consumer()
            .complete(
                first.consumer().subscriptionGeneration(),
                firstObs.last.get().getDeliveryToken(),
                CompletionOutcome.ROLLBACK),
        "the rollback must settle on the owner thread");

    JmsConsumerBodyCapTest.RecordingObserver secondObs =
        new JmsConsumerBodyCapTest.RecordingObserver();
    second.consumer().subscribe(DEST, "sub-b", secondObs, new AtomicBoolean(false));
    assertTrue(
        secondObs.next.await(AWAIT_SECONDS, TimeUnit.SECONDS), "second delivery must arrive");

    assertEquals(
        2,
        secondObs.last.get().getReservationCount(),
        "both subscriptions on one domain must share one reservation budget");
    assertNull(secondObs.error.get(), "the shared-budget path must stay healthy");

    first.consumer().stop();
    second.consumer().stop();
  }

  @Test
  void topic_uses_volatile_counter() throws Exception {
    JournalService journal = newJournal();
    Wired w = wired(journal, KEY, 2000L, null);
    when(w.session().createTopic("test")).thenReturn(mock(Topic.class));
    TextMessage message = textMessage("ID:topic");
    // Three receipts of the same provider id, then idle. The owner thread settles one delivery at a
    // time, so the test drives each redelivery with a completion.
    when(w.receive().receive(anyLong()))
        .thenReturn(message)
        .thenReturn(message)
        .thenReturn(message)
        .thenAnswer(idleNull());
    JmsConsumerBodyCapTest.RecordingObserver obs = new JmsConsumerBodyCapTest.RecordingObserver();

    w.consumer().subscribe(TOPIC, "sub-topic", obs, new AtomicBoolean(false));

    // First receipt: a fresh topic delivery is attempt one.
    awaitNextCount(obs, 1, "the first topic delivery must arrive");
    assertEquals(1, obs.last.get().getReservationCount(), "the first topic receipt is attempt one");
    assertTrue(
        w.consumer()
            .complete(
                w.consumer().subscriptionGeneration(),
                obs.last.get().getDeliveryToken(),
                CompletionOutcome.ROLLBACK),
        "the rollback must settle on the owner thread");

    // ROLLBACK keeps the volatile count: the redelivery of the same id is attempt two.
    awaitNextCount(obs, 2, "the redelivery after a rollback must arrive");
    assertEquals(2, obs.last.get().getReservationCount(), "a rollback keeps the volatile count");
    assertTrue(
        w.consumer()
            .complete(
                w.consumer().subscriptionGeneration(),
                obs.last.get().getDeliveryToken(),
                CompletionOutcome.COMMIT),
        "the commit must settle on the owner thread");

    // COMMIT clears the volatile entry: a later redelivery restarts at one.
    awaitNextCount(obs, 3, "the redelivery after a commit must arrive");
    assertEquals(1, obs.last.get().getReservationCount(), "a commit clears the volatile count");

    // Topics are non-durable: no journal file, lock, or replay ever happens.
    assertFalse(Files.exists(brokerDir()), "a topic subscription must never touch the journal");
    assertNull(obs.error.get(), "the volatile-topic path must stay healthy");

    // Close discards every volatile topic count: a later disconnect starts a fresh budget.
    assertEquals(1L, w.consumer().topicReservationCount("ID:topic"));
    w.consumer().stop();
    awaitTopicCleared(w.consumer(), "ID:topic");
  }

  /** Bounded wait until the subscription's volatile topic counts are discarded at close. */
  private static void awaitTopicCleared(JmsConsumer consumer, String messageId) throws Exception {
    long deadline = System.nanoTime() + TimeUnit.SECONDS.toNanos(AWAIT_SECONDS);
    while (consumer.topicReservationCount(messageId) != 0L && System.nanoTime() < deadline) {
      Thread.sleep(10);
    }
    assertEquals(0L, consumer.topicReservationCount(messageId), "close discards all topic counts");
  }

  /** Bounded wait until {@code obs} has seen at least {@code expected} deliveries. */
  private static void awaitNextCount(
      JmsConsumerBodyCapTest.RecordingObserver obs, int expected, String what) throws Exception {
    long deadline = System.nanoTime() + TimeUnit.SECONDS.toNanos(AWAIT_SECONDS);
    while (obs.nextCount < expected && System.nanoTime() < deadline) {
      Thread.sleep(10);
    }
    assertTrue(obs.nextCount >= expected, what);
  }
}
