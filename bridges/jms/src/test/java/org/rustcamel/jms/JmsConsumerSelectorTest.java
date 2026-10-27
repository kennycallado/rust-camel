package org.rustcamel.jms;

import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertSame;
import static org.junit.jupiter.api.Assertions.assertTrue;
import static org.mockito.ArgumentMatchers.any;
import static org.mockito.ArgumentMatchers.anyLong;
import static org.mockito.Mockito.doAnswer;
import static org.mockito.Mockito.doThrow;
import static org.mockito.Mockito.mock;
import static org.mockito.Mockito.never;
import static org.mockito.Mockito.times;
import static org.mockito.Mockito.verify;
import static org.mockito.Mockito.when;

import java.io.IOException;
import java.nio.channels.FileChannel;
import java.nio.file.FileVisitResult;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.SimpleFileVisitor;
import java.nio.file.StandardOpenOption;
import java.nio.file.attribute.BasicFileAttributes;
import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicReference;
import javax.jms.Connection;
import javax.jms.Destination;
import javax.jms.InvalidSelectorException;
import javax.jms.MessageConsumer;
import javax.jms.Queue;
import javax.jms.Session;
import jms_bridge.SubscribeRequest;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.DisplayName;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;
import org.mockito.Mock;
import org.mockito.MockitoAnnotations;

/**
 * Message-selector plumbing on the consumer setup path (task 2.1): the selector carried by the
 * subscribe request reaches {@code Session.createConsumer(destination, selector)} verbatim — the
 * bridge never parses or rewrites the expression — while a null or blank selector keeps the
 * one-argument overload. An invalid selector is a broker-owned failure: it surfaces on the
 * subscription failure path unchanged, with no local validation ahead of the call.
 */
@DisplayName("JmsConsumer message selector plumbing")
class JmsConsumerSelectorTest {

  private static final String DESTINATION = "queue:test";
  private static final String SUBSCRIPTION_ID = "sub-selector-test";
  private static final String SELECTOR = "priority > 5";
  private static final String INVALID_SELECTOR = "NONE SUCH >";
  private static final long AWAIT_SECONDS = 15L;
  private static final long STOP_TIMEOUT_MS = 15_000L;
  private static final String BROKER_KEY = "0123456789abcdef0123456789abcdef";

  @Mock JmsClientFactory factory;
  @Mock Connection connection;
  @Mock Session session;
  @Mock MessageConsumer messageConsumer;
  @Mock Queue queue;

  @TempDir Path tempDir;

  private AutoCloseable mocks;
  private final List<JmsConsumer> consumers = new ArrayList<>();

  @BeforeEach
  void setUp() throws Exception {
    mocks = MockitoAnnotations.openMocks(this);
    when(factory.createDedicatedConnection()).thenReturn(connection);
    doAnswer(inv -> null).when(connection).start();
    when(connection.createSession(false, Session.CLIENT_ACKNOWLEDGE)).thenReturn(session);
    when(session.createQueue("test")).thenReturn(queue);
    when(messageConsumer.receive(anyLong())).thenReturn(null);
  }

  @AfterEach
  void tearDown() {
    try {
      mocks.close();
    } catch (Exception ignored) {
    }
    for (JmsConsumer consumer : consumers) {
      consumer.stop();
      assertTrue(
          consumer.awaitStopped(STOP_TIMEOUT_MS),
          "the owner thread must release the journal before the temp-dir teardown");
    }
  }

  @Test
  void consumerUsesSelectorOverload() throws Exception {
    CountDownLatch created = new CountDownLatch(1);
    when(session.createConsumer(queue, SELECTOR))
        .thenAnswer(
            inv -> {
              created.countDown();
              return messageConsumer;
            });

    JmsConsumer consumer = newConsumer(factory);
    JmsConsumerBodyCapTest.RecordingObserver obs = new JmsConsumerBodyCapTest.RecordingObserver();
    consumer.messageSelector = subscribeRequest(SELECTOR).getMessageSelector();
    consumer.subscribe(DESTINATION, SUBSCRIPTION_ID, obs, new AtomicBoolean(false));

    assertTrue(
        created.await(AWAIT_SECONDS, TimeUnit.SECONDS), "the two-arg consumer must be created");
    verify(session, times(1)).createConsumer(queue, SELECTOR);
    verify(session, never()).createConsumer(any(Destination.class));
    consumer.stop();
  }

  @Test
  void consumerWithoutSelectorUsesPlainOverload() throws Exception {
    assertPlainOverload(null);
    assertPlainOverload("");
    assertPlainOverload("   ");
  }

  /**
   * Task 2.1: an invalid selector is a broker-owned failure (spec: "invalid selector syntax is a
   * broker-owned failure"). The bridge must not parse or rewrite the expression before the call,
   * and the provider exception must reach the subscription failure path unchanged.
   */
  @Test
  void invalidSelectorSurfacesThroughFailurePath() throws Exception {
    InvalidSelectorException invalid = new InvalidSelectorException(INVALID_SELECTOR);
    doThrow(invalid).when(session).createConsumer(queue, INVALID_SELECTOR);

    JmsConsumer consumer = newConsumer(factory);
    JmsConsumerBodyCapTest.RecordingObserver obs = new JmsConsumerBodyCapTest.RecordingObserver();
    consumer.messageSelector = subscribeRequest(INVALID_SELECTOR).getMessageSelector();
    consumer.subscribe(DESTINATION, SUBSCRIPTION_ID, obs, new AtomicBoolean(false));

    assertTrue(
        obs.errored.await(AWAIT_SECONDS, TimeUnit.SECONDS),
        "an invalid selector must fail the stream through the subscription failure path");
    assertSame(
        invalid,
        obs.error.get(),
        "the broker-owned failure must reach the observer notification unchanged");
    // The selector string arrives verbatim: no local parsing, trimming, or rewriting.
    verify(session, times(1)).createConsumer(queue, INVALID_SELECTOR);
    verify(session, never()).createConsumer(any(Destination.class));
    consumer.stop();
  }

  /**
   * A null or blank selector keeps the plain one-argument overload, invoked exactly once. Each
   * round gets its own mock set: the previous round's owner thread may still close its session
   * after {@code stop()}, and Mockito stubbing must never race concurrent invocations on the same
   * mock.
   */
  private void assertPlainOverload(String selector) throws Exception {
    JmsClientFactory roundFactory = mock(JmsClientFactory.class);
    Connection roundConnection = mock(Connection.class);
    Session roundSession = mock(Session.class);
    MessageConsumer roundMessageConsumer = mock(MessageConsumer.class);
    Queue roundQueue = mock(Queue.class);
    when(roundFactory.createDedicatedConnection()).thenReturn(roundConnection);
    doAnswer(inv -> null).when(roundConnection).start();
    when(roundConnection.createSession(false, Session.CLIENT_ACKNOWLEDGE)).thenReturn(roundSession);
    when(roundSession.createQueue("test")).thenReturn(roundQueue);
    when(roundMessageConsumer.receive(anyLong())).thenReturn(null);

    CountDownLatch created = new CountDownLatch(1);
    when(roundSession.createConsumer(roundQueue))
        .thenAnswer(
            inv -> {
              created.countDown();
              return roundMessageConsumer;
            });

    JmsConsumer consumer = newConsumer(roundFactory);
    JmsConsumerBodyCapTest.RecordingObserver obs = new JmsConsumerBodyCapTest.RecordingObserver();
    if (selector != null) {
      consumer.messageSelector = subscribeRequest(selector).getMessageSelector();
    }
    consumer.subscribe(DESTINATION, SUBSCRIPTION_ID, obs, new AtomicBoolean(false));

    assertTrue(
        created.await(AWAIT_SECONDS, TimeUnit.SECONDS),
        "the one-arg consumer must be created for selector [" + selector + "]");
    verify(roundSession, times(1)).createConsumer(roundQueue);
    verify(roundSession, never()).createConsumer(any(Destination.class), any());
    consumer.stop();
  }

  /**
   * Regression (rc-bihbp): {@code stop()} must let the owner thread close the journal channels and
   * release the OS lock before a Windows temp-dir teardown tries to delete them. A leaked lock
   * channel makes the journal directory undeletable there; the probe and delete below prove the
   * release on Linux.
   */
  @Test
  void stopReleasesJournalChannelsAndTempDirIsDeletable() throws Exception {
    CountDownLatch opened = new CountDownLatch(1);
    AtomicReference<FileChannel> captured = new AtomicReference<>();
    JournalService journal =
        new JournalService(
            tempDir,
            BridgeConfig.JOURNAL_MAX_BYTES_DEFAULT,
            BridgeConfig.JOURNAL_RETENTION_MS_DEFAULT,
            System::currentTimeMillis,
            path -> {
              FileChannel channel =
                  FileChannel.open(path, StandardOpenOption.READ, StandardOpenOption.WRITE);
              captured.set(channel);
              opened.countDown();
              return channel;
            });
    JmsConsumer consumer = new JmsConsumer();
    consumers.add(consumer);
    consumer.factory = factory;
    consumer.configureJournal(journal, BROKER_KEY, 1000L, null);

    consumer.subscribe(
        DESTINATION,
        SUBSCRIPTION_ID,
        new JmsConsumerBodyCapTest.RecordingObserver(),
        new AtomicBoolean(false));
    assertTrue(opened.await(AWAIT_SECONDS, TimeUnit.SECONDS), "the journal channel must open");
    FileChannel journalChannel = captured.get();

    consumer.stop();
    assertTrue(
        consumer.awaitStopped(STOP_TIMEOUT_MS),
        "the owner thread must terminate before the resources are probed");

    assertFalse(journalChannel.isOpen(), "stop() must close the journal channel");

    // A still-held process-wide FileLock keeps this acquire busy (the lock attempt is retried
    // until the timeout, then fails with JournalPermanentException), so success proves release.
    try (JournalService.JournalHandle probe =
        new JournalService(
                tempDir,
                BridgeConfig.JOURNAL_MAX_BYTES_DEFAULT,
                BridgeConfig.JOURNAL_RETENTION_MS_DEFAULT,
                System::currentTimeMillis)
            .acquire(BROKER_KEY, JournalService.destinationKey("test"), 1000L)) {
      // Acquired: the destination lock was released.
    }

    deleteRecursively(tempDir);
    assertFalse(Files.exists(tempDir), "the journal directory must be deletable after stop()");
    // The extension deletes the root again at teardown, so put an empty directory back.
    Files.createDirectories(tempDir);
  }

  /** Deletes {@code root} and everything below it. */
  private static void deleteRecursively(Path root) throws IOException {
    Files.walkFileTree(
        root,
        new SimpleFileVisitor<>() {
          @Override
          public FileVisitResult visitFile(Path file, BasicFileAttributes attrs)
              throws IOException {
            Files.delete(file);
            return FileVisitResult.CONTINUE;
          }

          @Override
          public FileVisitResult postVisitDirectory(Path dir, IOException exc) throws IOException {
            Files.delete(dir);
            return FileVisitResult.CONTINUE;
          }
        });
  }

  /** The subscribe request whose selector the bridge threads into the consumer. */
  private static SubscribeRequest subscribeRequest(String messageSelector) {
    return SubscribeRequest.newBuilder()
        .setDestination(DESTINATION)
        .setSubscriptionId(SUBSCRIPTION_ID)
        .setMessageSelector(messageSelector)
        .build();
  }

  /** Mirrors the stubbing style of {@link JmsConsumerCompletionTest}. */
  private JmsConsumer newConsumer(JmsClientFactory clientFactory) {
    JmsConsumer consumer = new JmsConsumer();
    consumer.factory = clientFactory;
    // A queue delivery is keyed and reserved, so install a legitimately configured journal.
    consumer.configureJournal(
        new JournalService(
            tempDir,
            BridgeConfig.JOURNAL_MAX_BYTES_DEFAULT,
            BridgeConfig.JOURNAL_RETENTION_MS_DEFAULT,
            System::currentTimeMillis),
        BROKER_KEY,
        1000L,
        null);
    consumers.add(consumer);
    return consumer;
  }
}
