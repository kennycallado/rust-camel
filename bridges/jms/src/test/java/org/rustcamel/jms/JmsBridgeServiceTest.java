package org.rustcamel.jms;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertSame;
import static org.junit.jupiter.api.Assertions.assertTrue;
import static org.mockito.ArgumentMatchers.any;
import static org.mockito.ArgumentMatchers.anyString;
import static org.mockito.Mockito.doAnswer;
import static org.mockito.Mockito.doNothing;
import static org.mockito.Mockito.mock;
import static org.mockito.Mockito.mockStatic;
import static org.mockito.Mockito.never;
import static org.mockito.Mockito.times;
import static org.mockito.Mockito.verify;
import static org.mockito.Mockito.when;

import io.grpc.Status;
import io.grpc.stub.ServerCallStreamObserver;
import io.grpc.stub.StreamObserver;
import jakarta.enterprise.inject.Instance;
import java.lang.reflect.Field;
import java.nio.file.FileSystems;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.attribute.PosixFilePermissions;
import java.util.Map;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.CyclicBarrier;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.Future;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicReference;
import jms_bridge.JmsMessage;
import jms_bridge.SubscribeRequest;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;
import org.mockito.ArgumentCaptor;
import org.mockito.Mock;
import org.mockito.MockedStatic;
import org.mockito.MockitoAnnotations;

/**
 * Duplicate-{@code subscription_id} and teardown-ownership semantics of {@link
 * JmsBridgeService#subscribe}.
 *
 * <p>A second subscribe on a live {@code subscription_id} must be rejected with {@code
 * ALREADY_EXISTS} (destroying the freshly created consumer) instead of silently overwriting the
 * active entry, and teardown of one stream must never evict a different owner's registration.
 */
class JmsBridgeServiceTest {

  @Mock Instance<JmsConsumer> instance;
  @Mock JmsConsumer consumerA;
  @Mock JmsConsumer consumerB;
  @Mock StreamObserver<JmsMessage> obsA;
  @Mock StreamObserver<JmsMessage> obsB;

  @TempDir Path tempDir;

  JmsBridgeService service;

  private AutoCloseable mocks;

  @BeforeEach
  void setUp() throws Exception {
    mocks = MockitoAnnotations.openMocks(this);
    // The bridge resolves one shared journal runtime from BridgeConfig. Every subscribe in this
    // test uses an empty destination (treated as a queue), so the map must be a valid, trusted
    // configured journal: owner-private directory plus a 32-hex broker key. Otherwise the new
    // queue contract would fail every subscribe with FAILED_PRECONDITION before registration.
    // Windows has no POSIX mode; startup validation skips the privacy check there.
    if (FileSystems.getDefault().supportedFileAttributeViews().contains("posix")) {
      Files.setPosixFilePermissions(tempDir, PosixFilePermissions.fromString("rwx------"));
    }
    service = new JmsBridgeService();
    service.consumerFactory = instance;
    service.config =
        new BridgeConfig(
            Map.of(
                JournalService.JOURNAL_DIRECTORY_ENV,
                tempDir.toString(),
                JournalService.BROKER_KEY_ENV,
                "0123456789abcdef0123456789abcdef"));
  }

  @AfterEach
  void tearDown() throws Exception {
    mocks.close();
  }

  /** Stubs {@code consumer.subscribe} to capture its inner {@code StreamObserver} (arg 2). */
  private static AtomicReference<StreamObserver<JmsMessage>> stubbingSubscribe(
      JmsConsumer consumer) {
    AtomicReference<StreamObserver<JmsMessage>> inner = new AtomicReference<>();
    doAnswer(
            invocation -> {
              inner.set(invocation.getArgument(2));
              return null;
            })
        .when(consumer)
        .subscribe(anyString(), anyString(), any(), any());
    return inner;
  }

  @SuppressWarnings("unchecked")
  private static ConcurrentHashMap<String, JmsConsumer> activeConsumersOf(JmsBridgeService svc)
      throws Exception {
    Field field = JmsBridgeService.class.getDeclaredField("activeConsumers");
    field.setAccessible(true);
    return (ConcurrentHashMap<String, JmsConsumer>) field.get(svc);
  }

  @Test
  void duplicateSubscriptionIdRejectedAlreadyExists() throws Exception {
    when(instance.get()).thenReturn(consumerA, consumerB);
    AtomicReference<StreamObserver<JmsMessage>> innerA = stubbingSubscribe(consumerA);

    service.subscribe(req("s1"), obsA);
    service.subscribe(req("s1"), obsB);

    ArgumentCaptor<Throwable> errorCaptor = ArgumentCaptor.forClass(Throwable.class);
    verify(obsB).onError(errorCaptor.capture());
    assertEquals(
        Status.Code.ALREADY_EXISTS, Status.fromThrowable(errorCaptor.getValue()).getCode());
    verify(instance).destroy(consumerB);
    assertSame(consumerA, activeConsumersOf(service).get("s1"));

    // Rejection must not disturb stream 1: the active inner observer still delivers to obsA.
    JmsMessage msg = JmsMessage.newBuilder().setDestination("queue:test").build();
    innerA.get().onNext(msg);
    verify(obsA).onNext(msg);
  }

  @Test
  void cancelledFirstStreamDoesNotEvictSecondOwner() throws Exception {
    when(instance.get()).thenReturn(consumerA, consumerB);
    AtomicReference<StreamObserver<JmsMessage>> innerA = stubbingSubscribe(consumerA);

    service.subscribe(req("s1"), obsA);
    innerA.get().onCompleted();

    service.subscribe(req("s1"), obsB);

    // The finished stream's error path must be a no-op: the finished CAS loses the race and
    // cleanup must not evict the new owner of "s1".
    innerA.get().onError(new RuntimeException("stream 1 already finished"));

    assertSame(consumerB, activeConsumersOf(service).get("s1"));
    verify(consumerB, never()).stop();
  }

  @Test
  void differentlyKeyedCancellationLeavesOtherIntact() throws Exception {
    when(instance.get()).thenReturn(consumerA, consumerB);
    AtomicReference<StreamObserver<JmsMessage>> innerA = stubbingSubscribe(consumerA);
    AtomicReference<StreamObserver<JmsMessage>> innerB = stubbingSubscribe(consumerB);

    service.subscribe(req("s1"), obsA);
    service.subscribe(req("s2"), obsB);

    innerA.get().onCompleted();

    verify(consumerA).stop();
    verify(instance).destroy(consumerA);

    ConcurrentHashMap<String, JmsConsumer> active = activeConsumersOf(service);
    assertEquals(1, active.size());
    assertSame(consumerB, active.get("s2"));
    verify(consumerB, never()).stop();
    verify(instance, never()).destroy(consumerB);

    // s2 still delivers after s1's teardown.
    JmsMessage msg = JmsMessage.newBuilder().setDestination("queue:test").build();
    innerB.get().onNext(msg);
    verify(obsB).onNext(msg);
  }

  @Test
  @SuppressWarnings("unchecked")
  void rejectedStreamRegistersNoCleanup() throws Exception {
    when(instance.get()).thenReturn(consumerA, consumerB);
    stubbingSubscribe(consumerA);
    ServerCallStreamObserver<JmsMessage> serverObsB = mock(ServerCallStreamObserver.class);

    service.subscribe(req("s1"), obsA);
    service.subscribe(req("s1"), serverObsB);

    // No cancel-handler registration may happen for a rejected stream.
    verify(serverObsB, never()).setOnCancelHandler(any());
  }

  @Test
  void lateCleanupAfterDrainDoesNotDoubleDestroy() throws Exception {
    when(instance.get()).thenReturn(consumerA);
    AtomicReference<StreamObserver<JmsMessage>> innerA = stubbingSubscribe(consumerA);

    service.subscribe(req("s1"), obsA);
    service.shutdown();
    // Late CAS winner: the consumer's error callback fires after the drain already tore it down.
    innerA.get().onError(new RuntimeException("late teardown"));

    verify(instance, times(1)).destroy(consumerA);
    verify(consumerA, times(1)).stop();
    assertTrue(activeConsumersOf(service).isEmpty());
  }

  @Test
  void drainCatchesSubscriberRegisteredBeforeFlag() throws Exception {
    when(instance.get()).thenReturn(consumerA);
    stubbingSubscribe(consumerA);

    service.subscribe(req("s1"), obsA);
    service.shutdown();

    verify(instance, times(1)).destroy(consumerA);
    assertTrue(activeConsumersOf(service).isEmpty());
  }

  @Test
  void subscribeRefusedAfterFlagDestroysOwnConsumer() throws Exception {
    when(instance.get()).thenReturn(consumerB);
    service.shutdown();

    service.subscribe(req("s1"), obsB);

    ArgumentCaptor<Throwable> errorCaptor = ArgumentCaptor.forClass(Throwable.class);
    verify(obsB).onError(errorCaptor.capture());
    assertEquals(Status.Code.UNAVAILABLE, Status.fromThrowable(errorCaptor.getValue()).getCode());
    verify(instance, times(1)).destroy(consumerB);
    verify(consumerB, never()).subscribe(anyString(), anyString(), any(), any());
    assertTrue(activeConsumersOf(service).isEmpty());
  }

  @Test
  void racingRegistrationObservedAfterFlagRefuses() throws Exception {
    CountDownLatch enteredGet = new CountDownLatch(1);
    CountDownLatch releaseGet = new CountDownLatch(1);
    doAnswer(
            invocation -> {
              enteredGet.countDown();
              releaseGet.await(5, TimeUnit.SECONDS);
              return consumerB;
            })
        .when(instance)
        .get();

    ExecutorService executor = Executors.newSingleThreadExecutor();
    try {
      Future<?> subscribeTask = executor.submit(() -> service.subscribe(req("s1"), obsB));

      assertTrue(enteredGet.await(5, TimeUnit.SECONDS));
      // Flag rises while the subscribe thread is still inside consumerFactory.get().
      service.shutdown();
      releaseGet.countDown();
      subscribeTask.get(5, TimeUnit.SECONDS);

      ArgumentCaptor<Throwable> errorCaptor = ArgumentCaptor.forClass(Throwable.class);
      verify(obsB).onError(errorCaptor.capture());
      assertEquals(Status.Code.UNAVAILABLE, Status.fromThrowable(errorCaptor.getValue()).getCode());
      verify(instance, times(1)).destroy(consumerB);
      verify(consumerB, never()).subscribe(anyString(), anyString(), any(), any());
      assertTrue(activeConsumersOf(service).isEmpty());
    } finally {
      executor.shutdownNow();
      assertTrue(executor.awaitTermination(5, TimeUnit.SECONDS));
    }
  }

  @Test
  void shutdownAndCleanupRaceDestroysExactlyOnce() throws Exception {
    when(instance.get()).thenReturn(consumerA);
    AtomicReference<StreamObserver<JmsMessage>> innerA = stubbingSubscribe(consumerA);

    service.subscribe(req("s1"), obsA);

    CyclicBarrier barrier = new CyclicBarrier(2);
    ExecutorService executor = Executors.newFixedThreadPool(2);
    try {
      Future<?> shutdownTask =
          executor.submit(
              () -> {
                barrier.await(5, TimeUnit.SECONDS);
                service.shutdown();
                return null;
              });
      Future<?> errorTask =
          executor.submit(
              () -> {
                barrier.await(5, TimeUnit.SECONDS);
                innerA.get().onError(new RuntimeException("cancel races shutdown"));
                return null;
              });

      shutdownTask.get(5, TimeUnit.SECONDS);
      errorTask.get(5, TimeUnit.SECONDS);

      verify(consumerA, times(1)).stop();
      verify(instance, times(1)).destroy(consumerA);
      assertTrue(activeConsumersOf(service).isEmpty());
    } finally {
      executor.shutdownNow();
      assertTrue(executor.awaitTermination(5, TimeUnit.SECONDS));
    }
  }

  /**
   * A pending transport-close future completes first; the later cancel must reuse the same {@code
   * finished} CAS, so stop/destroy run exactly once.
   */
  @Test
  void transportCloseRunsCleanupOnce() throws Exception {
    when(instance.get()).thenReturn(consumerA);
    stubbingSubscribe(consumerA);
    ServerCallStreamObserver<JmsMessage> serverObs = mock(ServerCallStreamObserver.class);
    ArgumentCaptor<Runnable> cancelHandler = ArgumentCaptor.forClass(Runnable.class);
    doNothing().when(serverObs).setOnCancelHandler(cancelHandler.capture());

    CompletableFuture<Void> transportClose = new CompletableFuture<>();
    try (MockedStatic<TransportCloseFilter> mocked = mockStatic(TransportCloseFilter.class)) {
      mocked.when(TransportCloseFilter::current).thenReturn(transportClose);

      service.subscribe(req("s1"), serverObs);

      transportClose.complete(null);
      verify(consumerA, times(1)).stop();
      verify(instance, times(1)).destroy(consumerA);
      assertTrue(activeConsumersOf(service).isEmpty());

      // The cancel path loses the finished CAS and must not release a second time.
      cancelHandler.getValue().run();
      verify(consumerA, times(1)).stop();
      verify(instance, times(1)).destroy(consumerA);
    }
  }

  /**
   * A healthy stream emits and completes normally, with no cancel and no socket cut. Cleanup runs
   * exactly once through the stream-end path, and the transport future stays pending so it cannot
   * trigger early cleanup.
   */
  @Test
  void normalStreamDoesNotCleanupEarly() throws Exception {
    when(instance.get()).thenReturn(consumerA);
    AtomicReference<StreamObserver<JmsMessage>> inner = stubbingSubscribe(consumerA);

    CompletableFuture<Void> transportClose = new CompletableFuture<>();
    try (MockedStatic<TransportCloseFilter> mocked = mockStatic(TransportCloseFilter.class)) {
      mocked.when(TransportCloseFilter::current).thenReturn(transportClose);

      service.subscribe(req("s1"), obsA);

      assertSame(consumerA, activeConsumersOf(service).get("s1"));
      verify(consumerA, never()).stop();

      JmsMessage msg = JmsMessage.newBuilder().setDestination("queue:test").build();
      inner.get().onNext(msg);
      inner.get().onCompleted();
      verify(obsA).onNext(msg);
      verify(obsA).onCompleted();
    }

    assertFalse(
        transportClose.isDone(), "transport future must stay pending on a normal stream end");
    verify(consumerA, times(1)).stop();
    verify(instance, times(1)).destroy(consumerA);
    assertTrue(activeConsumersOf(service).isEmpty());
  }

  private static SubscribeRequest req(String subId) {
    return SubscribeRequest.newBuilder().setSubscriptionId(subId).build();
  }
}
