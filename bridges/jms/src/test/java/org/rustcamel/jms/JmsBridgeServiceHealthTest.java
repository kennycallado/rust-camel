package org.rustcamel.jms;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertInstanceOf;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertSame;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;
import static org.junit.jupiter.api.Assertions.fail;
import static org.mockito.ArgumentMatchers.any;
import static org.mockito.ArgumentMatchers.anyMap;
import static org.mockito.ArgumentMatchers.anyString;
import static org.mockito.Mockito.mock;
import static org.mockito.Mockito.never;
import static org.mockito.Mockito.times;
import static org.mockito.Mockito.verify;
import static org.mockito.Mockito.when;

import io.grpc.stub.StreamObserver;
import jakarta.enterprise.inject.Instance;
import java.lang.reflect.Field;
import java.util.List;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.CopyOnWriteArrayList;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.CyclicBarrier;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.Future;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.concurrent.atomic.AtomicReference;
import java.util.function.Supplier;
import jms_bridge.CompletionRequest;
import jms_bridge.CompletionResponse;
import jms_bridge.HealthRequest;
import jms_bridge.HealthResponse;
import jms_bridge.SendRequest;
import jms_bridge.SendResponse;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;
import org.mockito.MockitoAnnotations;

/** Cached health and send wiring of {@link JmsBridgeService}. */
class JmsBridgeServiceHealthTest {

  private AutoCloseable mocks;
  private JmsBridgeService service;
  private JmsClientFactory clientFactory;
  private int refreshWorkersAtStart;
  private int closeWorkersAtStart;
  private int probeCleanupWorkersAtStart;

  @BeforeEach
  void setUp() throws Exception {
    mocks = MockitoAnnotations.openMocks(this);
    refreshWorkersAtStart = threadCount("jms-health-refresh");
    closeWorkersAtStart = threadCount("jms-health-close");
    probeCleanupWorkersAtStart = threadCount("jms-probe-cleanup");
    service = newService();
  }

  @AfterEach
  void tearDown() throws Exception {
    try {
      service.shutdown();
    } finally {
      awaitOwnedWorkersTerminated();
      mocks.close();
    }
  }

  @Test
  void healthReturnsCachedSnapshotWithoutWaitingForProbe() throws Exception {
    CountDownLatch entered = new CountDownLatch(1);
    CountDownLatch release = new CountDownLatch(1);
    FakeProbe probe =
        new FakeProbe(
            (call, generation) -> {
              entered.countDown();
              release.await();
              return BrokerProbeResult.up();
            });
    BridgeHealthCache cache =
        install(new BridgeHealthCache(probe, false, System::currentTimeMillis));
    try {
      long firstStarted = System.nanoTime();
      HealthResponse first = healthOf(service);
      long firstElapsed = TimeUnit.NANOSECONDS.toMillis(System.nanoTime() - firstStarted);

      long secondStarted = System.nanoTime();
      HealthResponse second = healthOf(service);
      long secondElapsed = TimeUnit.NANOSECONDS.toMillis(System.nanoTime() - secondStarted);

      assertTrue(firstElapsed < 100, "first health call took " + firstElapsed + " ms");
      assertTrue(secondElapsed < 100, "second health call took " + secondElapsed + " ms");
      assertUnknown(first);
      assertUnknown(second);
      awaitCondition("probe invocation", () -> probe.probeCalls.get() == 1);
      assertEquals(1, probe.probeCalls.get());
      assertSame(cache, service.healthCacheForTests());
    } finally {
      release.countDown();
    }
  }

  @Test
  void skipModeNeverCallsTheProbe() {
    FakeProbe probe =
        new FakeProbe(
            (call, generation) -> {
              throw new AssertionError("skip mode must not call the probe");
            });
    install(new BridgeHealthCache(probe, true, System::currentTimeMillis));

    HealthResponse response = healthOf(service);

    assertTrue(response.getHealthy());
    assertFalse(response.getBrokerConnected());
    assertEquals("broker probe skipped; connectivity unverified", response.getMessage());
    assertEquals(0, probe.probeCalls.get());
  }

  @Test
  void providerFailureDoesNotResetTheProducerPool() {
    FakeProbe probe =
        new FakeProbe((call, generation) -> BrokerProbeResult.down("broker unreachable"));
    BridgeHealthCache cache =
        install(new BridgeHealthCache(probe, false, System::currentTimeMillis));

    healthOf(service);
    awaitStatus(cache, HealthSnapshot.Status.DOWN);
    HealthResponse response = healthOf(service);

    assertFalse(response.getHealthy());
    assertFalse(response.getBrokerConnected());
    assertEquals("broker unreachable", response.getMessage());
    verify(clientFactory, never()).reset();
  }

  @Test
  void successfulSendPublishesVerifiedUp() throws Exception {
    FakeProbe probe =
        new FakeProbe(
            (call, generation) -> {
              throw new AssertionError("a send success must not need a probe");
            });
    install(new BridgeHealthCache(probe, false, System::currentTimeMillis));
    SendRequest request =
        SendRequest.newBuilder().setDestination("queue:test").setContentType("text/plain").build();
    when(service.producer.send(anyString(), any(byte[].class), anyMap(), any())).thenReturn("id-1");
    RecordingObserver<SendResponse> observer = new RecordingObserver<>();

    service.send(request, observer);
    HealthResponse response = healthOf(service);

    assertTrue(response.getHealthy());
    assertTrue(response.getBrokerConnected());
    assertEquals("ok", response.getMessage());
    assertTrue(observer.completed.get(), "send did not complete");
    assertEquals(1, observer.values.size());
    assertEquals("id-1", observer.values.get(0).getMessageId());
  }

  @Test
  void lateSendCannotResurrectInvalidatedGeneration() throws Exception {
    FakeProbe probe = new FakeProbe((call, generation) -> BrokerProbeResult.up());
    BridgeHealthCache cache =
        install(new BridgeHealthCache(probe, false, System::currentTimeMillis));
    CountDownLatch entered = new CountDownLatch(1);
    CountDownLatch release = new CountDownLatch(1);
    when(service.producer.send(anyString(), any(byte[].class), anyMap(), any()))
        .thenAnswer(
            invocation -> {
              entered.countDown();
              release.await();
              return "id-1";
            });
    RecordingObserver<SendResponse> observer = new RecordingObserver<>();
    SendRequest request = SendRequest.newBuilder().setDestination("queue:test").build();
    ExecutorService sender = Executors.newSingleThreadExecutor();
    try {
      Future<?> send = sender.submit(() -> service.send(request, observer));
      assertTrue(entered.await(5, TimeUnit.SECONDS), "send did not enter the producer");
      cache.invalidate();
      release.countDown();
      send.get(5, TimeUnit.SECONDS);
    } finally {
      release.countDown();
      sender.shutdownNow();
    }

    HealthResponse response = healthOf(service);

    assertFalse(response.getBrokerConnected());
    assertEquals("broker status not yet checked", response.getMessage());
    assertTrue(observer.completed.get(), "send did not complete");
  }

  @Test
  void providerFailureDoesNotEvictActiveConsumers() throws Exception {
    FakeProbe probe =
        new FakeProbe((call, generation) -> BrokerProbeResult.down("broker unreachable"));
    BridgeHealthCache cache =
        install(new BridgeHealthCache(probe, false, System::currentTimeMillis));
    JmsConsumer consumer = mock(JmsConsumer.class);
    activeConsumersOf(service).put("s1", consumer);

    healthOf(service);
    awaitStatus(cache, HealthSnapshot.Status.DOWN);
    HealthResponse response = healthOf(service);

    assertFalse(response.getHealthy());
    assertFalse(response.getBrokerConnected());
    assertSame(consumer, activeConsumersOf(service).get("s1"));
    verify(consumer, never()).stop();
  }

  @Test
  void concurrentFirstHealthAccessCreatesOneCacheAndWorker() throws Exception {
    CountDownLatch entered = new CountDownLatch(1);
    CountDownLatch release = new CountDownLatch(1);
    AtomicInteger opens = new AtomicInteger();
    JmsBrokerProbe probe = jmsProbe("activemq", blockingDiagnosticFactory(opens, entered, release));
    when(clientFactory.healthProbe()).thenReturn(probe);
    int refreshWorkersBefore = threadCount("jms-health-refresh");
    CountDownLatch start = new CountDownLatch(1);
    int callers = 8;
    ExecutorService callerPool = Executors.newFixedThreadPool(callers);
    List<Future<HealthResponse>> futures = new CopyOnWriteArrayList<>();
    try {
      for (int caller = 0; caller < callers; caller++) {
        futures.add(
            callerPool.submit(
                () -> {
                  start.await();
                  return healthOf(service);
                }));
      }
      start.countDown();

      List<HealthResponse> responses = new CopyOnWriteArrayList<>();
      for (Future<HealthResponse> future : futures) {
        responses.add(future.get(5, TimeUnit.SECONDS));
      }

      BridgeHealthCache cache = service.healthCacheForTests();
      assertNotNull(cache, "concurrent health access did not create one cache");
      assertEquals(callers, responses.size());
      for (HealthResponse response : responses) {
        assertUnknown(response);
      }
      awaitCondition("probe invocation", () -> opens.get() == 1);
      assertEquals(1, opens.get());
      assertEquals(refreshWorkersBefore + 1, threadCount("jms-health-refresh"));
      verify(clientFactory, times(1)).healthProbe();
    } finally {
      release.countDown();
      callerPool.shutdownNow();
    }
  }

  /**
   * T6 Java-side paired-contract coverage: a blocked fake broker must not turn health into a
   * broker-latency wait across at least three 5-second probe periods. The fake clock advances to 0,
   * 5000, 10000, and 15000 ms while the real handler keeps answering inside the 100 ms deadline and
   * the one admitted diagnostic open stays flat.
   *
   * <p>This is paired-contract coverage only, not a live-broker or native-image soak. The Rust real
   * monitor supplies the zero-restart half, and CI owns native/live-broker evidence.
   */
  @Test
  void t6BlockedBrokerThreeProbePeriodsKeepsHealthPromptAndOpensFlat() throws Exception {
    long deadlineMillis = 100;
    long periodMillis = 5_000L;
    CountDownLatch entered = new CountDownLatch(1);
    CountDownLatch release = new CountDownLatch(1);
    BridgeHealthCacheTest.BlockedBrokerProbe probe =
        BridgeHealthCacheTest.gatedBrokerProbe(entered, release, BrokerProbeResult.up());
    BridgeHealthCacheTest.MutableClock clock = new BridgeHealthCacheTest.MutableClock();
    install(new BridgeHealthCache(probe, false, clock.supplier()));
    long simulatedStart = clock.get();
    try {
      HealthResponse atZero = healthWithin(deadlineMillis);
      assertTrue(entered.await(5, TimeUnit.SECONDS), "blocked refresh did not start");

      clock.set(periodMillis);
      HealthResponse atOnePeriod = healthWithin(deadlineMillis);
      clock.set(2 * periodMillis);
      HealthResponse atTwoPeriods = healthWithin(deadlineMillis);
      clock.set(3 * periodMillis);
      HealthResponse atThreePeriods = healthWithin(deadlineMillis);

      for (HealthResponse response : List.of(atZero, atOnePeriod, atTwoPeriods, atThreePeriods)) {
        assertTrue(response.getHealthy());
        assertFalse(response.getBrokerConnected());
      }
      assertUnknown(atZero);
      assertUnknown(atOnePeriod);
      assertTrue(
          clock.get() - simulatedStart >= 3 * periodMillis,
          "simulated elapsed time was only " + clock.get() + " ms");
      assertEquals(1, probe.opens());
      assertEquals(1, probe.probeCalls());
    } finally {
      release.countDown();
    }
  }

  @Test
  void t5BlockedBrokerDoesNotDelayHealthOrUnrelatedRpc() throws Exception {
    long deadlineMillis = 100;
    CountDownLatch entered = new CountDownLatch(1);
    CountDownLatch release = new CountDownLatch(1);
    BridgeHealthCacheTest.BlockedBrokerProbe probe =
        BridgeHealthCacheTest.gatedBrokerProbe(entered, release, BrokerProbeResult.up());
    BridgeHealthCacheTest.MutableClock clock = new BridgeHealthCacheTest.MutableClock();
    install(new BridgeHealthCache(probe, false, clock.supplier()));
    try {
      HealthResponse first = healthWithin(deadlineMillis);
      assertTrue(entered.await(5, TimeUnit.SECONDS), "blocked refresh did not start");
      assertEquals(refreshWorkersAtStart + 1, threadCount("jms-health-refresh"));

      RecordingObserver<CompletionResponse> observer = new RecordingObserver<>();
      long completionStarted = System.nanoTime();
      service.complete(CompletionRequest.getDefaultInstance(), observer);
      long completionElapsed = TimeUnit.NANOSECONDS.toMillis(System.nanoTime() - completionStarted);
      assertTrue(
          completionElapsed < deadlineMillis,
          "completion call took " + completionElapsed + " ms while the probe was blocked");
      HealthResponse second = healthWithin(deadlineMillis);

      assertTrue(observer.completed.get(), "completion did not finish");
      assertEquals(1, observer.values.size());
      assertFalse(observer.values.get(0).getAccepted());
      assertUnknown(first);
      assertUnknown(second);
      assertEquals(1, probe.probeCalls());
      assertEquals(1, probe.opens());
      assertEquals(refreshWorkersAtStart + 1, threadCount("jms-health-refresh"));
    } finally {
      release.countDown();
    }
  }

  @Test
  void repeatedHealthCallsDoNotQueueRefreshWork() throws Exception {
    long deadlineMillis = 100;
    CountDownLatch entered = new CountDownLatch(1);
    CountDownLatch release = new CountDownLatch(1);
    BridgeHealthCacheTest.BlockedBrokerProbe probe =
        BridgeHealthCacheTest.gatedBrokerProbe(entered, release, BrokerProbeResult.up());
    BridgeHealthCacheTest.MutableClock clock = new BridgeHealthCacheTest.MutableClock();
    AtomicInteger executorCreations = new AtomicInteger();
    Supplier<ExecutorService> trackingFactory =
        () -> {
          executorCreations.incrementAndGet();
          return Executors.newSingleThreadExecutor(
              runnable -> {
                Thread thread = new Thread(runnable, "jms-health-refresh");
                thread.setDaemon(true);
                return thread;
              });
        };
    BridgeHealthCache cache =
        install(new BridgeHealthCache(probe, false, clock.supplier(), trackingFactory));
    try {
      for (int call = 0; call < 20; call++) {
        assertUnknown(healthWithin(deadlineMillis));
      }
      assertTrue(entered.await(5, TimeUnit.SECONDS), "blocked refresh did not start");
      assertEquals(1, probe.probeCalls());
      assertEquals(1, probe.opens());
      assertEquals(1, executorCreations.get());
    } finally {
      release.countDown();
      awaitCacheWorkerIdle(cache);
      assertEquals(1, probe.probeCalls());
    }
  }

  @Test
  void shutdownNeverCreatesACache() throws Exception {
    int refreshWorkersBefore = threadCount("jms-health-refresh");
    service.shutdown();
    service.shutdown();

    assertNull(service.healthCacheForTests());
    assertEquals(refreshWorkersBefore, threadCount("jms-health-refresh"));
    assertThrows(IllegalStateException.class, () -> healthOf(service));

    JmsBridgeService racing = newService();
    AtomicInteger opens = new AtomicInteger();
    JmsBrokerProbe probe = jmsProbe("activemq", diagnosticFactory(opens));
    when(racing.clientFactory.healthProbe()).thenReturn(probe);
    AtomicReference<HealthResponse> response = new AtomicReference<>();
    AtomicReference<Throwable> failure = new AtomicReference<>();
    CyclicBarrier barrier = new CyclicBarrier(2);
    Thread healthCaller =
        new Thread(
            () -> {
              try {
                barrier.await();
                response.set(healthOf(racing));
              } catch (Throwable error) {
                failure.set(error);
              }
            },
            "health-shutdown-racer");
    Thread shutdownCaller =
        new Thread(
            () -> {
              try {
                barrier.await();
                racing.shutdown();
              } catch (Throwable error) {
                failure.set(error);
              }
            },
            "shutdown-racer");
    healthCaller.start();
    shutdownCaller.start();
    healthCaller.join(TimeUnit.SECONDS.toMillis(5));
    shutdownCaller.join(TimeUnit.SECONDS.toMillis(5));

    assertFalse(healthCaller.isAlive(), "health caller did not return");
    assertFalse(shutdownCaller.isAlive(), "shutdown caller did not return");
    if (failure.get() != null) {
      IllegalStateException stopped = assertInstanceOf(IllegalStateException.class, failure.get());
      assertTrue(stopped.getMessage().contains("shutting down"));
      assertNull(racing.healthCacheForTests());
    } else {
      assertNotNull(response.get());
      assertFalse(response.get().getBrokerConnected());
      assertNotNull(racing.healthCacheForTests());
      assertUnknown(healthOf(racing));
    }
    awaitCondition(
        "racing cache workers terminated",
        () ->
            threadCount("jms-health-refresh") <= refreshWorkersBefore
                && threadCount("jms-health-close") <= closeWorkersAtStart);
    assertEquals(refreshWorkersBefore, threadCount("jms-health-refresh"));
  }

  @Test
  void shutdownCapturesCacheBeingConstructed() throws Exception {
    CountDownLatch factoryEntered = new CountDownLatch(1);
    CountDownLatch releaseFactory = new CountDownLatch(1);
    JmsBrokerProbe probe = jmsProbe("activemq", diagnosticFactory(new AtomicInteger()));
    when(clientFactory.healthProbe())
        .thenAnswer(
            invocation -> {
              factoryEntered.countDown();
              releaseFactory.await();
              return probe;
            });
    ExecutorService pool = Executors.newFixedThreadPool(2);
    try {
      Future<HealthResponse> health = pool.submit(() -> healthOf(service));
      assertTrue(factoryEntered.await(5, TimeUnit.SECONDS), "cache construction did not gate");
      Future<?> shutdown = pool.submit(() -> service.shutdown());
      releaseFactory.countDown();

      HealthResponse response = health.get(5, TimeUnit.SECONDS);
      shutdown.get(5, TimeUnit.SECONDS);

      BridgeHealthCache cache = service.healthCacheForTests();
      assertNotNull(cache, "shutdown did not capture the constructed cache");
      assertUnknown(response);
      verify(clientFactory, times(1)).healthProbe();
      assertUnknown(healthOf(service));
      assertSame(cache, service.healthCacheForTests());
      long generationAfterShutdown = cache.generation();
      cache.invalidate();
      assertEquals(
          generationAfterShutdown,
          cache.generation(),
          "shutdown did not terminally close the captured cache");
      verify(clientFactory, times(1)).healthProbe();
    } finally {
      releaseFactory.countDown();
      pool.shutdownNow();
    }
  }

  @Test
  void factoryResetInvalidatesActiveRefreshGeneration() throws Exception {
    CountDownLatch entered = new CountDownLatch(1);
    CountDownLatch release = new CountDownLatch(1);
    CountDownLatch returned = new CountDownLatch(1);
    FakeProbe probe =
        new FakeProbe(
            (call, generation) -> {
              entered.countDown();
              try {
                release.await();
              } finally {
                returned.countDown();
              }
              return BrokerProbeResult.up();
            });
    JmsClientFactory realFactory = new JmsClientFactory();
    service.clientFactory = realFactory;
    BridgeHealthCache cache =
        install(new BridgeHealthCache(probe, false, System::currentTimeMillis));
    try {
      healthOf(service);
      assertTrue(entered.await(5, TimeUnit.SECONDS), "refresh did not start");

      realFactory.reset();

      assertEquals(List.of(2L), probe.invalidatedGenerations);
      assertEquals(2L, cache.generation());
      assertEquals(0, probe.closeCalls.get(), "provider close ran on the reset caller");

      release.countDown();
      assertTrue(returned.await(5, TimeUnit.SECONDS), "probe did not return");
      awaitCacheWorkerIdle(cache);
      assertEquals(HealthSnapshot.Status.UNKNOWN, cache.snapshotValue().status());

      HealthResponse response = healthOf(service);
      assertFalse(response.getBrokerConnected());
      assertEquals("broker status not yet checked", response.getMessage());
    } finally {
      release.countDown();
    }
  }

  @Test
  void factoryResetIsNotCalledOnProbeFailure() {
    FakeProbe probe =
        new FakeProbe((call, generation) -> BrokerProbeResult.down("broker unreachable"));
    BridgeHealthCache cache =
        install(new BridgeHealthCache(probe, false, System::currentTimeMillis));

    healthOf(service);
    awaitStatus(cache, HealthSnapshot.Status.DOWN);
    HealthResponse response = healthOf(service);

    assertFalse(response.getBrokerConnected());
    verify(clientFactory, never()).reset();
    assertEquals(1L, cache.generation());
  }

  private JmsBridgeService newService() {
    JmsBridgeService created = new JmsBridgeService();
    clientFactory = mock(JmsClientFactory.class);
    created.clientFactory = clientFactory;
    created.config = new BridgeConfig(Map.of());
    created.producer = mock(JmsProducer.class);
    created.consumerFactory = mock(Instance.class);
    return created;
  }

  private BridgeHealthCache install(BridgeHealthCache cache) {
    service.installHealthCacheForTests(cache);
    return cache;
  }

  private static HealthResponse healthOf(JmsBridgeService target) {
    RecordingObserver<HealthResponse> observer = new RecordingObserver<>();
    target.health(HealthRequest.getDefaultInstance(), observer);
    assertTrue(observer.completed.get(), "health did not complete");
    assertEquals(1, observer.values.size());
    return observer.values.get(0);
  }

  private HealthResponse healthWithin(long deadlineMillis) {
    long started = System.nanoTime();
    HealthResponse response = healthOf(service);
    long elapsedMillis = TimeUnit.NANOSECONDS.toMillis(System.nanoTime() - started);
    assertTrue(
        elapsedMillis < deadlineMillis,
        "health call took " + elapsedMillis + " ms while the probe was blocked");
    return response;
  }

  private static void assertUnknown(HealthResponse response) {
    assertTrue(response.getHealthy());
    assertFalse(response.getBrokerConnected());
    assertEquals("broker status not yet checked", response.getMessage());
  }

  @SuppressWarnings("unchecked")
  private static ConcurrentHashMap<String, JmsConsumer> activeConsumersOf(JmsBridgeService target)
      throws Exception {
    Field field = JmsBridgeService.class.getDeclaredField("activeConsumers");
    field.setAccessible(true);
    return (ConcurrentHashMap<String, JmsConsumer>) field.get(target);
  }

  private static void awaitCacheWorkerIdle(BridgeHealthCache cache) throws Exception {
    Field field = BridgeHealthCache.class.getDeclaredField("worker");
    field.setAccessible(true);
    ExecutorService worker = (ExecutorService) field.get(cache);
    Future<?> marker = worker.submit(() -> {});
    marker.get(5, TimeUnit.SECONDS);
  }

  private static void awaitStatus(BridgeHealthCache cache, HealthSnapshot.Status expected) {
    awaitCondition("snapshot status " + expected, () -> cache.snapshotValue().status() == expected);
  }

  private static JmsBrokerProbe jmsProbe(
      String brokerType, JmsBrokerProbe.DiagnosticFactory factory) {
    Map<String, String> env =
        Map.of("BRIDGE_BROKER_TYPE", brokerType, "BRIDGE_BROKER_URL", "tcp://broker:61616");
    return new JmsBrokerProbe(
        new BridgeConfig(env), factory, () -> {}, () -> {}, () -> {}, () -> {});
  }

  private static JmsBrokerProbe.DiagnosticFactory diagnosticFactory(AtomicInteger opens) {
    return (type, url, username, password) -> {
      opens.incrementAndGet();
      return new JmsBrokerProbe.Diagnostic() {
        @Override
        public void verify() {}

        @Override
        public void close() {}
      };
    };
  }

  private static JmsBrokerProbe.DiagnosticFactory blockingDiagnosticFactory(
      AtomicInteger opens, CountDownLatch entered, CountDownLatch release) {
    return (type, url, username, password) -> {
      opens.incrementAndGet();
      entered.countDown();
      release.await();
      return new JmsBrokerProbe.Diagnostic() {
        @Override
        public void verify() {}

        @Override
        public void close() {}
      };
    };
  }

  private static int cacheThreadCount() {
    return threadCount("jms-health-refresh") + threadCount("jms-health-close");
  }

  private static int threadCount(String name) {
    int count = 0;
    for (Thread thread : Thread.getAllStackTraces().keySet()) {
      if (name.equals(thread.getName()) && thread.isAlive()) {
        count++;
      }
    }
    return count;
  }

  private void awaitOwnedWorkersTerminated() {
    long deadline = System.nanoTime() + TimeUnit.SECONDS.toNanos(5);
    while (System.nanoTime() < deadline) {
      if (threadCount("jms-health-refresh") <= refreshWorkersAtStart
          && threadCount("jms-health-close") <= closeWorkersAtStart
          && threadCount("jms-probe-cleanup") <= probeCleanupWorkersAtStart) {
        return;
      }
      try {
        Thread.sleep(2);
      } catch (InterruptedException error) {
        Thread.currentThread().interrupt();
        return;
      }
    }
  }

  private static void awaitCondition(String description, Condition condition) {
    long deadline = System.nanoTime() + TimeUnit.SECONDS.toNanos(5);
    while (System.nanoTime() < deadline) {
      if (condition.satisfied()) {
        return;
      }
      try {
        Thread.sleep(2);
      } catch (InterruptedException error) {
        Thread.currentThread().interrupt();
        fail("interrupted while waiting for " + description);
      }
    }
    fail("timed out waiting for " + description);
  }

  @FunctionalInterface
  private interface Condition {
    boolean satisfied();
  }

  @FunctionalInterface
  private interface ProbeAction {
    BrokerProbeResult run(int call, long generation) throws Exception;
  }

  private static final class FakeProbe implements BrokerProbe {
    private final ProbeAction action;
    private final AtomicInteger probeCalls = new AtomicInteger();
    private final AtomicInteger closeCalls = new AtomicInteger();
    private final List<Long> invalidatedGenerations = new CopyOnWriteArrayList<>();

    FakeProbe(ProbeAction action) {
      this.action = action;
    }

    @Override
    public BrokerProbeResult probe(long generation) throws Exception {
      return action.run(probeCalls.incrementAndGet(), generation);
    }

    @Override
    public void invalidate(long generation) {
      invalidatedGenerations.add(generation);
    }

    @Override
    public void close() {
      closeCalls.incrementAndGet();
    }
  }

  private static final class RecordingObserver<T> implements StreamObserver<T> {
    private final List<T> values = new CopyOnWriteArrayList<>();
    private final AtomicReference<Throwable> error = new AtomicReference<>();
    private final AtomicBoolean completed = new AtomicBoolean();

    @Override
    public void onNext(T value) {
      values.add(value);
    }

    @Override
    public void onError(Throwable throwable) {
      error.set(throwable);
    }

    @Override
    public void onCompleted() {
      completed.set(true);
    }
  }
}
