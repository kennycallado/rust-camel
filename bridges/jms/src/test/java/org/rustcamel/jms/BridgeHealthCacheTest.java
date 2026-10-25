package org.rustcamel.jms;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;
import static org.junit.jupiter.api.Assertions.fail;
import static org.mockito.ArgumentMatchers.any;
import static org.mockito.Mockito.doAnswer;
import static org.mockito.Mockito.mock;

import java.nio.file.Files;
import java.nio.file.Path;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.concurrent.AbstractExecutorService;
import java.util.concurrent.CopyOnWriteArrayList;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.Future;
import java.util.concurrent.RejectedExecutionException;
import java.util.concurrent.ThreadFactory;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.concurrent.atomic.AtomicLong;
import java.util.concurrent.atomic.AtomicReference;
import java.util.function.LongConsumer;
import java.util.function.LongSupplier;
import java.util.function.Supplier;
import jms_bridge.HealthResponse;
import jms_bridge.HealthStatus;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.Test;

/** Immutable snapshot and single-flight refresh contract of {@link BridgeHealthCache}. */
class BridgeHealthCacheTest {

  private final List<ExecutorService> workers = new CopyOnWriteArrayList<>();
  private final List<BrokerProbe> probes = new CopyOnWriteArrayList<>();
  private final AtomicInteger executorCreations = new AtomicInteger();
  private final AtomicReference<ExecutorService> createdWorker = new AtomicReference<>();
  private final AtomicReference<FakeScheduler> createdScheduler = new AtomicReference<>();

  @AfterEach
  void stopOwnedWorkers() {
    for (ExecutorService worker : workers) {
      worker.shutdownNow();
    }
    for (BrokerProbe probe : probes) {
      try {
        probe.close();
      } catch (RuntimeException ignored) {
        // Best-effort teardown of test-owned probe executors.
      }
    }
  }

  @Test
  void snapshotResponseCarriesExplicitStatusAndAge() {
    HealthResponse unknown = HealthSnapshot.unknown(1L, 100L).toResponse();
    assertEquals(HealthStatus.UNKNOWN, unknown.getStatus());
    assertTrue(unknown.getHealthy());
    assertFalse(unknown.getBrokerConnected());
    assertEquals(0L, unknown.getAgeMs());

    HealthResponse skipped = HealthSnapshot.skipped(1L, 100L).toResponse();
    assertEquals(HealthStatus.SKIPPED, skipped.getStatus());
    assertTrue(skipped.getHealthy());
    assertFalse(skipped.getBrokerConnected());
    assertEquals(0L, skipped.getAgeMs());

    HealthResponse stale = HealthSnapshot.stale(1L, 60_000L, 100L).toResponse();
    assertEquals(HealthStatus.STALE, stale.getStatus());
    assertTrue(stale.getHealthy());
    assertFalse(stale.getBrokerConnected());
    assertEquals(60_000L, stale.getAgeMs());

    HealthResponse up = HealthSnapshot.up(1L, 100L).toResponse();
    assertEquals(HealthStatus.UP, up.getStatus());
    assertTrue(up.getHealthy());
    assertTrue(up.getBrokerConnected());
    assertEquals(0L, up.getAgeMs());

    HealthResponse down = HealthSnapshot.down(1L, "broker unreachable", 100L).toResponse();
    assertEquals(HealthStatus.DOWN, down.getStatus());
    assertFalse(down.getHealthy());
    assertFalse(down.getBrokerConnected());
    assertEquals(0L, down.getAgeMs());
  }

  @Test
  void unknownSchedulesExactlyOneRefresh() throws Exception {
    MutableClock clock = new MutableClock();
    CountDownLatch gate = new CountDownLatch(1);
    FakeProbe probe =
        new FakeProbe(
            (call, generation) -> {
              gate.await();
              return BrokerProbeResult.up();
            });
    BridgeHealthCache cache = cache(probe, false, clock);
    try {
      for (int call = 0; call < 5; call++) {
        HealthSnapshot snapshot = cache.current();
        assertEquals(HealthSnapshot.Status.UNKNOWN, snapshot.status());
        assertTrue(snapshot.healthy());
        assertFalse(snapshot.brokerConnected());
        assertEquals("broker status not yet checked", snapshot.message());
      }
      tick(cache);
      awaitCondition("probe invocation", () -> probe.probeCalls.get() == 1);
      for (int call = 0; call < 4; call++) {
        clock.set(clock.get() + BridgeHealthCache.REFRESH_AFTER_MS);
        tick(cache);
      }
      assertEquals(1, probe.probeCalls.get());
      assertEquals(1, executorCreations.get());
    } finally {
      gate.countDown();
    }
  }

  @Test
  void scheduledRefreshKeepsLongPollFreshWithoutTraffic() throws Exception {
    MutableClock clock = new MutableClock();
    clock.set(0L);
    FakeProbe probe = new FakeProbe((call, generation) -> BrokerProbeResult.up());
    FakeScheduler scheduler = new FakeScheduler();
    BridgeHealthCache cache = cache(probe, false, clock, scheduler);

    scheduler.runTick();
    awaitStatus(cache, HealthSnapshot.Status.UP);
    awaitWorkerIdle(createdWorker.get());
    HealthSnapshot initial = cache.current();
    assertEquals(HealthSnapshot.Status.UP, initial.status());

    for (long tickAt = BridgeHealthCache.REFRESH_AFTER_MS;
        tickAt <= 60_000L;
        tickAt += BridgeHealthCache.REFRESH_AFTER_MS) {
      clock.set(tickAt);
      scheduler.runTick();
      awaitWorkerIdle(createdWorker.get());
    }

    HealthSnapshot afterLongPoll = cache.current();
    assertEquals(HealthSnapshot.Status.UP, afterLongPoll.status());
    assertTrue(afterLongPoll.healthy());
    assertTrue(afterLongPoll.brokerConnected());
    assertTrue(probe.probeCalls.get() > 1, "scheduler did not refresh without reads or sends");
  }

  @Test
  void scheduledProbeFailureStillReportsDown() throws Exception {
    MutableClock clock = new MutableClock();
    FakeProbe probe =
        new FakeProbe(
            (call, generation) -> {
              if (call == 1) {
                return BrokerProbeResult.up();
              }
              throw new IllegalStateException("broker connection lost");
            });
    FakeScheduler scheduler = new FakeScheduler();
    BridgeHealthCache cache = cache(probe, false, clock, scheduler);

    clock.set(0L);
    scheduler.runTick();
    awaitStatus(cache, HealthSnapshot.Status.UP);
    awaitWorkerIdle(createdWorker.get());

    clock.set(BridgeHealthCache.REFRESH_AFTER_MS);
    scheduler.runTick();
    awaitStatus(cache, HealthSnapshot.Status.DOWN);
    awaitWorkerIdle(createdWorker.get());

    HealthSnapshot snapshot = cache.current();
    assertEquals(HealthSnapshot.Status.DOWN, snapshot.status());
    assertFalse(snapshot.healthy());
    assertFalse(snapshot.brokerConnected());
  }

  @Test
  void scheduledTicksRemainSingleFlightAndStopOnClose() throws Exception {
    MutableClock clock = new MutableClock();
    AtomicInteger inFlight = new AtomicInteger();
    AtomicInteger maxInFlight = new AtomicInteger();
    CountDownLatch probeEntered = new CountDownLatch(1);
    CountDownLatch releaseProbe = new CountDownLatch(1);
    AtomicLong blockedGeneration = new AtomicLong(-1L);
    FakeProbe probe =
        new FakeProbe(
            (call, generation) -> {
              int concurrent = inFlight.incrementAndGet();
              maxInFlight.accumulateAndGet(concurrent, Math::max);
              try {
                blockedGeneration.set(generation);
                probeEntered.countDown();
                releaseProbe.await();
                return BrokerProbeResult.up();
              } finally {
                inFlight.decrementAndGet();
              }
            });
    FakeScheduler scheduler = new FakeScheduler();
    BridgeHealthCache cache = cache(probe, false, clock, scheduler);

    scheduler.runTick();
    assertTrue(probeEntered.await(5, TimeUnit.SECONDS), "initial scheduled probe did not start");
    long invalidatedGeneration = blockedGeneration.get() + 1;
    for (int extraTick = 0; extraTick < 5; extraTick++) {
      clock.set(clock.get() + BridgeHealthCache.REFRESH_AFTER_MS);
      scheduler.runTick();
    }

    cache.invalidate();
    assertEquals(invalidatedGeneration, cache.generation());

    releaseProbe.countDown();
    awaitWorkerIdle(createdWorker.get());

    assertEquals(1, maxInFlight.get(), "more than one probe ran concurrently");
    assertEquals(1, probe.probeCalls.get(), "scheduled ticks queued refresh work");
    assertEquals(
        HealthSnapshot.Status.UNKNOWN,
        cache.snapshotValue().status(),
        "late old-generation probe published");
    assertEquals(
        invalidatedGeneration,
        cache.snapshotValue().generation(),
        "late old-generation probe published");

    cache.close();
    assertTrue(scheduler.isShutdown(), "scheduler was not shut down on close");
    int probeCallsAtClose = probe.probeCalls.get();
    scheduler.runTick();
    assertEquals(probeCallsAtClose, probe.probeCalls.get(), "probe admitted after close");
    assertEquals(HealthSnapshot.Status.UNKNOWN, cache.snapshotValue().status());
    assertFalse(cache.snapshotValue().brokerConnected());
    assertEquals(invalidatedGeneration + 1, cache.snapshotValue().generation());
  }

  @Test
  void verifiedUpAndDownMapToTheContract() throws Exception {
    MutableClock clock = new MutableClock();
    FakeProbe probe =
        new FakeProbe(
            (call, generation) ->
                call == 1 ? BrokerProbeResult.up() : BrokerProbeResult.down("broker unreachable"));
    BridgeHealthCache cache = cache(probe, false, clock);

    tick(cache);
    awaitStatus(cache, HealthSnapshot.Status.UP);
    awaitWorkerIdle(createdWorker.get());
    HealthSnapshot up = cache.current();
    assertEquals(HealthSnapshot.Status.UP, up.status());
    assertTrue(up.healthy());
    assertTrue(up.brokerConnected());
    assertEquals("ok", up.message());

    clock.set(5_001L);
    tick(cache);
    awaitStatus(cache, HealthSnapshot.Status.DOWN);
    awaitWorkerIdle(createdWorker.get());
    HealthSnapshot down = cache.current();
    assertEquals(HealthSnapshot.Status.DOWN, down.status());
    assertFalse(down.healthy());
    assertFalse(down.brokerConnected());
    assertEquals("broker unreachable", down.message());
  }

  @Test
  void skipModeNeverCallsTheProbe() throws Exception {
    MutableClock clock = new MutableClock();
    FakeProbe probe =
        new FakeProbe(
            (call, generation) -> {
              throw new AssertionError("skip mode must not call the probe");
            });
    BridgeHealthCache cache = cache(probe, true, clock);

    for (int call = 0; call < 3; call++) {
      HealthSnapshot snapshot = cache.current();
      assertEquals(HealthSnapshot.Status.SKIPPED, snapshot.status());
      assertTrue(snapshot.healthy());
      assertFalse(snapshot.brokerConnected());
      assertEquals("broker probe skipped; connectivity unverified", snapshot.message());
    }
    assertEquals(0, probe.probeCalls.get());
  }

  @Test
  void verifiedSnapshotAtClockZeroIsNotUnknown() throws Exception {
    MutableClock clock = new MutableClock();
    FakeProbe probe = new FakeProbe((call, generation) -> BrokerProbeResult.up());
    BridgeHealthCache cache = cache(probe, false, clock);

    tick(cache);
    awaitStatus(cache, HealthSnapshot.Status.UP);
    awaitWorkerIdle(createdWorker.get());
    HealthSnapshot snapshot = cache.current();
    assertEquals(HealthSnapshot.Status.UP, snapshot.status());
    assertTrue(snapshot.healthy());
    assertTrue(snapshot.brokerConnected());
    assertEquals(0L, snapshot.checkedAtMillis());
    assertEquals("ok", snapshot.message());
  }

  @Test
  void staleAfterTtlWhileRefreshIsBlocked() throws Exception {
    MutableClock clock = new MutableClock();
    CountDownLatch gate = new CountDownLatch(1);
    FakeProbe probe =
        new FakeProbe(
            (call, generation) -> {
              if (call == 1) {
                return BrokerProbeResult.up();
              }
              gate.await();
              return BrokerProbeResult.up();
            });
    BridgeHealthCache cache = cache(probe, false, clock);
    try {
      tick(cache);
      awaitStatus(cache, HealthSnapshot.Status.UP);
      awaitWorkerIdle(createdWorker.get());
      clock.set(5_001L);
      HealthSnapshot admitted = cache.current();
      assertTrue(admitted.brokerConnected());
      tick(cache);
      awaitCondition("second probe invocation", () -> probe.probeCalls.get() == 2);

      clock.set(10_001L);
      HealthSnapshot stale = cache.current();
      assertEquals(HealthSnapshot.Status.STALE, stale.status());
      assertTrue(stale.healthy());
      assertFalse(stale.brokerConnected());
      assertTrue(stale.message().contains("stale"));
      assertTrue(stale.message().contains("10001"));

      cache.current();
      assertEquals(2, probe.probeCalls.get());
      assertEquals(1, executorCreations.get());
    } finally {
      gate.countDown();
    }
  }

  @Test
  void refreshStartsAtHalfTtlWithoutFlapping() throws Exception {
    MutableClock clock = new MutableClock();
    FakeProbe probe = new FakeProbe((call, generation) -> BrokerProbeResult.up());
    BridgeHealthCache cache = cache(probe, false, clock);

    tick(cache);
    awaitStatus(cache, HealthSnapshot.Status.UP);
    awaitWorkerIdle(createdWorker.get());
    clock.set(5_001L);
    HealthSnapshot halfTtl = cache.current();
    assertEquals(HealthSnapshot.Status.UP, halfTtl.status());
    assertTrue(halfTtl.healthy());
    assertTrue(halfTtl.brokerConnected());

    tick(cache);
    awaitCondition("second probe invocation", () -> probe.probeCalls.get() == 2);
    awaitCondition("second publish", () -> cache.snapshotValue().checkedAtMillis() == 5_001L);
    awaitWorkerIdle(createdWorker.get());
    HealthSnapshot refreshed = cache.current();
    assertEquals(HealthSnapshot.Status.UP, refreshed.status());
    assertTrue(refreshed.healthy());
    assertTrue(refreshed.brokerConnected());
  }

  @Test
  void hungRefreshKeepsCurrentStaleAfterSendSuccess() throws Exception {
    MutableClock clock = new MutableClock();
    CountDownLatch gate = new CountDownLatch(1);
    FakeProbe probe =
        new FakeProbe(
            (call, generation) -> {
              if (call == 1) {
                return BrokerProbeResult.up();
              }
              gate.await();
              return BrokerProbeResult.up();
            });
    BridgeHealthCache cache = cache(probe, false, clock);
    try {
      tick(cache);
      awaitStatus(cache, HealthSnapshot.Status.UP);
      awaitWorkerIdle(createdWorker.get());

      clock.set(5_001L);
      tick(cache);
      awaitCondition("hung refresh started", () -> probe.probeCalls.get() == 2);

      clock.set(9_000L);
      cache.recordSendSuccess(cache.generation());
      assertEquals(HealthSnapshot.Status.UP, cache.snapshotValue().status());
      assertTrue(cache.snapshotValue().brokerConnected());

      clock.set(15_002L);
      HealthSnapshot stale = cache.current();
      assertEquals(HealthSnapshot.Status.STALE, stale.status());
      assertTrue(stale.healthy());
      assertFalse(stale.brokerConnected());
    } finally {
      gate.countDown();
    }
  }

  @Test
  void sendSuccessPublishesVerifiedUp() throws Exception {
    MutableClock clock = new MutableClock();
    CountDownLatch gate = new CountDownLatch(1);
    FakeProbe probe =
        new FakeProbe(
            (call, generation) -> {
              gate.await();
              return BrokerProbeResult.up();
            });
    BridgeHealthCache cache = cache(probe, false, clock);
    try {
      long generation = cache.generation();
      cache.recordSendSuccess(generation);

      HealthSnapshot snapshot = cache.current();
      assertEquals(HealthSnapshot.Status.UP, snapshot.status());
      assertTrue(snapshot.healthy());
      assertTrue(snapshot.brokerConnected());
      assertEquals("ok", snapshot.message());
      assertEquals(0, probe.probeCalls.get());
    } finally {
      gate.countDown();
    }
  }

  @Test
  void staleGenerationRefreshAndSendDoNotWrite() throws Exception {
    MutableClock clock = new MutableClock();
    AtomicReference<BridgeHealthCache> cacheReference = new AtomicReference<>();
    FakeProbe probe =
        new FakeProbe(
            (call, generation) -> {
              cacheReference.get().invalidate();
              return BrokerProbeResult.up();
            });
    BridgeHealthCache cache = cache(probe, false, clock);
    cacheReference.set(cache);

    tick(cache);
    awaitWorkerIdle(createdWorker.get());

    assertTrue(probe.invalidatedGenerations.contains(2L));
    assertEquals(2L, cache.generation());
    HealthSnapshot invalidated = cache.snapshotValue();
    assertEquals(HealthSnapshot.Status.UNKNOWN, invalidated.status());
    assertEquals(2L, invalidated.generation());

    long staleGeneration = cache.generation();
    cache.invalidate();
    cache.recordSendSuccess(staleGeneration);

    assertEquals(3L, cache.generation());
    HealthSnapshot after = cache.snapshotValue();
    assertEquals(HealthSnapshot.Status.UNKNOWN, after.status());
    assertEquals(3L, after.generation());
    assertFalse(after.brokerConnected());
  }

  @Test
  void invalidateRejectsLateRefreshResult() throws Exception {
    MutableClock clock = new MutableClock();
    CountDownLatch entered = new CountDownLatch(1);
    CountDownLatch release = new CountDownLatch(1);
    AtomicLong capturedGeneration = new AtomicLong(-1L);
    FakeProbe probe =
        new FakeProbe(
            (call, generation) -> {
              capturedGeneration.set(generation);
              entered.countDown();
              release.await();
              return BrokerProbeResult.up();
            });
    BridgeHealthCache cache = cache(probe, false, clock);
    try {
      tick(cache);
      assertTrue(entered.await(5, TimeUnit.SECONDS), "probe did not start");
      cache.invalidate();
      release.countDown();
      awaitWorkerIdle(createdWorker.get());

      assertEquals(1L, capturedGeneration.get());
      assertTrue(probe.invalidatedGenerations.contains(2L));

      HealthSnapshot snapshot = cache.current();
      assertEquals(HealthSnapshot.Status.UNKNOWN, snapshot.status());
      assertTrue(snapshot.healthy());
      assertFalse(snapshot.brokerConnected());
      assertEquals("broker status not yet checked", snapshot.message());
    } finally {
      release.countDown();
    }
  }

  @Test
  void invalidateDoesNotCreateSecondWorker() throws Exception {
    MutableClock clock = new MutableClock();
    CountDownLatch entered = new CountDownLatch(1);
    CountDownLatch release = new CountDownLatch(1);
    FakeProbe probe =
        new FakeProbe(
            (call, generation) -> {
              entered.countDown();
              release.await();
              return BrokerProbeResult.up();
            });
    BridgeHealthCache cache = cache(probe, false, clock);
    try {
      for (int call = 0; call < 5; call++) {
        tick(cache);
      }
      assertTrue(entered.await(5, TimeUnit.SECONDS), "probe did not start");
      for (int call = 0; call < 3; call++) {
        cache.invalidate();
      }
      cache.current();

      assertEquals(1, executorCreations.get());
      assertEquals(1, threadCount("jms-health-refresh"));
      assertEquals(4L, cache.generation());
      assertEquals(1, probe.probeCalls.get());
    } finally {
      release.countDown();
    }
  }

  @Test
  void invalidateReturnsPromptlyWhileProviderCloseBlocks() throws Exception {
    MutableClock clock = new MutableClock();
    CountDownLatch closeEntered = new CountDownLatch(1);
    CountDownLatch releaseClose = new CountDownLatch(1);
    FakeProbe probe =
        new FakeProbe((call, generation) -> BrokerProbeResult.up())
            .onClose(
                () -> {
                  closeEntered.countDown();
                  try {
                    releaseClose.await();
                  } catch (InterruptedException ignored) {
                    Thread.currentThread().interrupt();
                  }
                });
    BridgeHealthCache cache = cache(probe, false, clock);
    try {
      long started = System.nanoTime();
      cache.invalidate();
      long elapsedMillis = TimeUnit.NANOSECONDS.toMillis(System.nanoTime() - started);

      assertTrue(elapsedMillis < 100, "invalidate took " + elapsedMillis + " ms");
      assertEquals(List.of(2L), probe.invalidatedGenerations);
      assertEquals(0, probe.closeCalls.get());
      assertEquals(1, closeEntered.getCount(), "provider close ran on the invalidation thread");
      assertEquals(0, threadCount("jms-health-close"));
    } finally {
      releaseClose.countDown();
    }
  }

  @Test
  void lateSendSuccessCannotResurrectInvalidatedGeneration() throws Exception {
    MutableClock clock = new MutableClock();
    FakeProbe probe = new FakeProbe((call, generation) -> BrokerProbeResult.up());
    BridgeHealthCache cache = cache(probe, false, clock);

    long generation = cache.generation();
    cache.invalidate();
    cache.recordSendSuccess(generation);

    HealthSnapshot snapshot = cache.current();
    assertEquals(HealthSnapshot.Status.UNKNOWN, snapshot.status());
    assertEquals(2L, snapshot.generation());
    assertFalse(snapshot.brokerConnected());
  }

  @Test
  void closeReturnsWithinBoundWhenProbeCloseIgnoresInterrupt() throws Exception {
    MutableClock clock = new MutableClock();
    CountDownLatch probeEntered = new CountDownLatch(1);
    CountDownLatch releaseProbe = new CountDownLatch(1);
    CountDownLatch closeEntered = new CountDownLatch(1);
    CountDownLatch releaseClose = new CountDownLatch(1);
    AtomicBoolean probeInterrupted = new AtomicBoolean();
    AtomicBoolean closeInterrupted = new AtomicBoolean();
    FakeProbe probe =
        new FakeProbe(
                (call, generation) -> {
                  probeEntered.countDown();
                  awaitIgnoringInterrupts(releaseProbe, probeInterrupted);
                  return BrokerProbeResult.up();
                })
            .onClose(
                () -> {
                  closeEntered.countDown();
                  awaitIgnoringInterrupts(releaseClose, closeInterrupted);
                });
    ExecutorService closeWorker =
        Executors.newSingleThreadExecutor(
            runnable -> {
              Thread thread = new Thread(runnable, "jms-health-close");
              thread.setDaemon(true);
              return thread;
            });
    // The test executor waits for this close invocation to enter probe.close() before execute()
    // returns. BridgeHealthCache publishes closeRunner before that entry signal, so publication
    // precedes the termination-timeout decision. The handshake is bounded; the overall
    // close-duration assertion remains unchanged.
    ExecutorService entryHandshakeCloseWorker =
        new AbstractExecutorService() {
          @Override
          public void execute(Runnable command) {
            closeWorker.execute(command);
            try {
              if (!closeEntered.await(5, TimeUnit.SECONDS)) {
                fail("provider close did not enter within 5s");
              }
            } catch (InterruptedException error) {
              Thread.currentThread().interrupt();
              fail("interrupted while awaiting provider close entry");
            }
          }

          @Override
          public void shutdown() {
            closeWorker.shutdown();
          }

          @Override
          public List<Runnable> shutdownNow() {
            return closeWorker.shutdownNow();
          }

          @Override
          public boolean isShutdown() {
            return closeWorker.isShutdown();
          }

          @Override
          public boolean isTerminated() {
            return closeWorker.isTerminated();
          }

          @Override
          public boolean awaitTermination(long timeout, TimeUnit unit) throws InterruptedException {
            return closeWorker.awaitTermination(timeout, unit);
          }
        };
    FakeScheduler scheduler = new FakeScheduler();
    createdScheduler.set(scheduler);
    BridgeHealthCache cache =
        new BridgeHealthCache(
            probe,
            false,
            clock.supplier(),
            trackingExecutorFactory(),
            () -> entryHandshakeCloseWorker,
            () -> scheduler);
    try {
      tick(cache);
      assertTrue(probeEntered.await(5, TimeUnit.SECONDS), "probe did not start");

      long started = System.nanoTime();
      cache.close();
      long elapsedMillis = TimeUnit.NANOSECONDS.toMillis(System.nanoTime() - started);

      assertTrue(elapsedMillis <= 4_000, "close took " + elapsedMillis + " ms");
      // The refresh worker sets the flag in its interrupt catch block after shutdownNow(); close()
      // does not wait for that write, so observe it with a bounded poll instead of an immediate
      // read.
      awaitCondition("refresh worker observed interrupt", () -> probeInterrupted.get());
      assertTrue(closeEntered.await(5, TimeUnit.SECONDS), "provider close did not run");
      // Same unsupported immediate-observation assumption: closeInterrupted is written by the close
      // worker after it wakes from the release latch, which can lag close()'s return under load.
      awaitCondition("close worker observed interrupt", () -> closeInterrupted.get());

      int probeCalls = probe.probeCalls.get();
      HealthSnapshot afterClose = cache.current();
      assertEquals(HealthSnapshot.Status.UNKNOWN, afterClose.status());
      assertEquals(2L, afterClose.generation());
      assertFalse(afterClose.brokerConnected());
      assertEquals(probeCalls, probe.probeCalls.get());
      assertEquals(1, executorCreations.get());
    } finally {
      releaseProbe.countDown();
      releaseClose.countDown();
      closeWorker.shutdownNow();
    }
    awaitCondition("cache threads terminated", () -> cacheThreadCount() == 0);
  }

  @Test
  void secondCloseIsNoOpAndClosesProviderOnce() throws Exception {
    MutableClock clock = new MutableClock();
    FakeProbe probe = new FakeProbe((call, generation) -> BrokerProbeResult.up());
    BridgeHealthCache cache = cache(probe, false, clock);

    cache.close();
    long started = System.nanoTime();
    cache.close();
    long elapsedMillis = TimeUnit.NANOSECONDS.toMillis(System.nanoTime() - started);

    assertTrue(elapsedMillis < 100, "second close took " + elapsedMillis + " ms");
    assertEquals(1, probe.closeCalls.get());
    awaitCondition("cache threads terminated", () -> cacheThreadCount() == 0);
  }

  @Test
  void closeWithInterruptedCallerStillRunsQueuedProbeClose() throws Exception {
    MutableClock clock = new MutableClock();
    CountDownLatch closeWorkerStarted = new CountDownLatch(1);
    CountDownLatch releaseCloseWorker = new CountDownLatch(1);
    CountDownLatch closeCompleted = new CountDownLatch(1);
    FakeProbe probe =
        new FakeProbe((call, generation) -> BrokerProbeResult.up())
            .onClose(closeCompleted::countDown);
    ExecutorService gatedCloseWorker =
        Executors.newSingleThreadExecutor(
            runnable -> {
              Thread thread = new Thread(runnable, "jms-health-close");
              thread.setDaemon(true);
              return thread;
            });
    BridgeHealthCache cache =
        new BridgeHealthCache(
            probe,
            false,
            clock.supplier(),
            trackingExecutorFactory(),
            () -> gatedCloseWorker,
            FakeScheduler::new);
    Thread.interrupted();
    try {
      gatedCloseWorker.execute(
          () -> {
            closeWorkerStarted.countDown();
            awaitIgnoringInterrupts(releaseCloseWorker, new AtomicBoolean());
          });
      assertTrue(closeWorkerStarted.await(5, TimeUnit.SECONDS), "close worker did not start");

      Thread.currentThread().interrupt();
      long started = System.nanoTime();
      cache.close();
      long elapsedMillis = TimeUnit.NANOSECONDS.toMillis(System.nanoTime() - started);

      assertTrue(elapsedMillis <= 4_000, "close took " + elapsedMillis + " ms");
      assertTrue(Thread.currentThread().isInterrupted(), "caller interrupt flag was not preserved");
      Thread.interrupted();
      assertEquals(1, closeCompleted.getCount(), "queued close ran before the gate was released");

      releaseCloseWorker.countDown();
      assertTrue(closeCompleted.await(2, TimeUnit.SECONDS), "queued probe close never ran");
      assertTrue(gatedCloseWorker.awaitTermination(5, TimeUnit.SECONDS));
      assertEquals(1, probe.closeCalls.get());
    } finally {
      Thread.interrupted();
      releaseCloseWorker.countDown();
      gatedCloseWorker.shutdownNow();
    }
  }

  @Test
  void closeAndInvalidateRaceClosesResourceExactlyOnce() throws Exception {
    MutableClock clock = new MutableClock();
    CountDownLatch invalidate2Entered = new CountDownLatch(1);
    CountDownLatch releaseInvalidate2 = new CountDownLatch(1);
    List<Long> callbackOrder = new CopyOnWriteArrayList<>();
    AtomicLong acceptedGeneration = new AtomicLong();
    FakeProbe probe =
        new FakeProbe((call, generation) -> BrokerProbeResult.up())
            .onInvalidate(
                generation -> {
                  if (generation == 2L) {
                    invalidate2Entered.countDown();
                    awaitIgnoringInterrupts(releaseInvalidate2, new AtomicBoolean());
                  }
                  callbackOrder.add(generation);
                  acceptedGeneration.accumulateAndGet(generation, Math::max);
                });
    BridgeHealthCache cache = cache(probe, false, clock);
    AtomicLong invalidateElapsed = new AtomicLong(Long.MAX_VALUE);
    AtomicReference<Throwable> failure = new AtomicReference<>();

    Thread invalidator =
        new Thread(
            () -> {
              try {
                long started = System.nanoTime();
                cache.invalidate();
                invalidateElapsed.set(TimeUnit.NANOSECONDS.toMillis(System.nanoTime() - started));
              } catch (Throwable error) {
                failure.compareAndSet(null, error);
              }
            },
            "cache-invalidate-racer");
    invalidator.start();
    try {
      assertTrue(invalidate2Entered.await(5, TimeUnit.SECONDS), "generation 2 did not detach");

      long started = System.nanoTime();
      cache.close();
      long closeElapsed = TimeUnit.NANOSECONDS.toMillis(System.nanoTime() - started);

      assertTrue(closeElapsed <= 4_000, "close took " + closeElapsed + " ms");
      releaseInvalidate2.countDown();
      invalidator.join(TimeUnit.SECONDS.toMillis(5));
      assertFalse(invalidator.isAlive(), "invalidate did not return");
      if (failure.get() != null) {
        fail("invalidate failed: " + failure.get());
      }
      assertTrue(
          invalidateElapsed.get() <= 4_000, "invalidate took " + invalidateElapsed.get() + " ms");
    } finally {
      releaseInvalidate2.countDown();
    }

    assertEquals(List.of(3L, 2L), callbackOrder);
    assertEquals(3L, acceptedGeneration.get());
    assertEquals(1, probe.closeCalls.get());
    assertEquals(3L, cache.generation());

    HealthSnapshot snapshot = cache.current();
    assertEquals(HealthSnapshot.Status.UNKNOWN, snapshot.status());
    assertEquals(acceptedGeneration.get(), snapshot.generation());
    assertEquals(0, probe.probeCalls.get());
  }

  @Test
  void lateRefreshAndSendAfterCloseWriteNothing() throws Exception {
    MutableClock clock = new MutableClock();
    CountDownLatch entered = new CountDownLatch(1);
    CountDownLatch release = new CountDownLatch(1);
    AtomicBoolean interrupted = new AtomicBoolean();
    FakeProbe probe =
        new FakeProbe(
            (call, generation) -> {
              entered.countDown();
              awaitIgnoringInterrupts(release, interrupted);
              return BrokerProbeResult.up();
            });
    BridgeHealthCache cache = cache(probe, false, clock);
    long generation = cache.generation();
    int probeCalls = probe.probeCalls.get();
    long closeGeneration = generation + 1;
    try {
      tick(cache);
      assertTrue(entered.await(5, TimeUnit.SECONDS), "probe did not start");
      generation = cache.generation();
      probeCalls = probe.probeCalls.get();
      closeGeneration = generation + 1;

      cache.close();
      cache.recordSendSuccess(generation);
    } finally {
      release.countDown();
    }

    awaitCondition("cache threads terminated", () -> cacheThreadCount() == 0);
    HealthSnapshot snapshot = cache.snapshotValue();
    assertEquals(HealthSnapshot.Status.UNKNOWN, snapshot.status());
    assertEquals(closeGeneration, snapshot.generation());
    assertFalse(snapshot.brokerConnected());
    assertEquals(probeCalls, probe.probeCalls.get());
    assertEquals(1, executorCreations.get());
    HealthSnapshot afterClose = cache.current();
    assertEquals(HealthSnapshot.Status.UNKNOWN, afterClose.status());
    assertEquals(closeGeneration, afterClose.generation());
    assertEquals(1, probe.probeCalls.get());
  }

  @Test
  void invalidateClosesOwnedDiagnosticOnly() throws Exception {
    MutableClock clock = new MutableClock();
    FakeProbe probe = new FakeProbe((call, generation) -> BrokerProbeResult.up());
    BridgeHealthCache cache = cache(probe, false, clock);

    cache.invalidate();
    cache.close();

    assertEquals(List.of(2L, 3L), probe.invalidatedGenerations);
    assertEquals(1, probe.closeCalls.get());
    assertEquals(3L, cache.generation());
    assertEquals(HealthSnapshot.Status.UNKNOWN, cache.snapshotValue().status());

    String source =
        Files.readString(Path.of("src/main/java/org/rustcamel/jms/BridgeHealthCache.java"));
    assertFalse(source.contains("reset()"), "cache must never reset the client factory pool");
  }

  @Test
  void executorRejectionClearsInFlightForCurrentGeneration() throws Exception {
    MutableClock clock = new MutableClock();
    FakeProbe probe = new FakeProbe((call, generation) -> BrokerProbeResult.up());

    ExecutorService delegate = Executors.newSingleThreadExecutor(daemonThreads());
    workers.add(delegate);
    ExecutorService rejecting = mock(ExecutorService.class);
    AtomicInteger submissions = new AtomicInteger();
    doAnswer(
            invocation -> {
              if (submissions.incrementAndGet() == 1) {
                throw new RejectedExecutionException("rejected");
              }
              delegate.execute((Runnable) invocation.getArgument(0));
              return null;
            })
        .when(rejecting)
        .execute(any(Runnable.class));

    FakeScheduler scheduler = new FakeScheduler();
    BridgeHealthCache cache =
        new BridgeHealthCache(
            probe,
            false,
            clock.supplier(),
            () -> {
              executorCreations.incrementAndGet();
              return rejecting;
            },
            BridgeHealthCache::newCloseWorker,
            () -> scheduler);

    scheduler.runTick();
    HealthSnapshot first = cache.current();
    assertEquals(HealthSnapshot.Status.UNKNOWN, first.status());
    assertEquals(0, probe.probeCalls.get());

    scheduler.runTick();
    awaitCondition("probe invocation", () -> probe.probeCalls.get() == 1);
    awaitWorkerIdle(delegate);
    assertEquals(1, probe.probeCalls.get());
    assertEquals(1, executorCreations.get());
  }

  @Test
  void expiredRefreshesReuseOneDiagnosticForActiveMq() throws Exception {
    expiredRefreshesReuseOneDiagnostic("activemq");
  }

  @Test
  void expiredRefreshesReuseOneDiagnosticForArtemis() throws Exception {
    expiredRefreshesReuseOneDiagnostic("artemis");
  }

  private void expiredRefreshesReuseOneDiagnostic(String brokerType) throws Exception {
    MutableClock clock = new MutableClock();
    AtomicInteger opens = new AtomicInteger();
    AtomicInteger verifies = new AtomicInteger();
    JmsBrokerProbe.Diagnostic diagnostic =
        new JmsBrokerProbe.Diagnostic() {
          @Override
          public void verify() {
            verifies.incrementAndGet();
          }

          @Override
          public void close() {}
        };
    JmsBrokerProbe.DiagnosticFactory factory =
        (type, url, username, password) -> {
          opens.incrementAndGet();
          return diagnostic;
        };
    JmsBrokerProbe probe =
        new JmsBrokerProbe(
            bridgeConfig(brokerType), factory, () -> {}, () -> {}, () -> {}, () -> {});
    probes.add(probe);
    BridgeHealthCache cache = cache(probe, false, clock);

    tick(cache);
    awaitStatus(cache, HealthSnapshot.Status.UP);
    awaitWorkerIdle(createdWorker.get());

    clock.set(10_001L);
    tick(cache);
    awaitCondition(
        "expired refresh publish",
        () -> verifies.get() == 2 && cache.snapshotValue().checkedAtMillis() == 10_001L);
    awaitWorkerIdle(createdWorker.get());
    assertEquals(1, opens.get());
  }

  private BridgeHealthCache cache(BrokerProbe probe, boolean skip, MutableClock clock) {
    return cache(probe, skip, clock, new FakeScheduler());
  }

  private BridgeHealthCache cache(
      BrokerProbe probe, boolean skip, MutableClock clock, FakeScheduler scheduler) {
    createdScheduler.set(scheduler);
    return new BridgeHealthCache(
        probe,
        skip,
        clock.supplier(),
        trackingExecutorFactory(),
        BridgeHealthCache::newCloseWorker,
        () -> scheduler);
  }

  /** Drives one scheduler tick explicitly; reads no longer admit refresh work. */
  private void tick(BridgeHealthCache cache) {
    createdScheduler.get().runTick();
  }

  private Supplier<ExecutorService> trackingExecutorFactory() {
    return () -> {
      executorCreations.incrementAndGet();
      ExecutorService worker = Executors.newSingleThreadExecutor(daemonThreads());
      workers.add(worker);
      createdWorker.set(worker);
      return worker;
    };
  }

  private static ThreadFactory daemonThreads() {
    return runnable -> {
      Thread thread = new Thread(runnable, "jms-health-refresh");
      thread.setDaemon(true);
      return thread;
    };
  }

  private static void awaitWorkerIdle(ExecutorService worker) throws Exception {
    Future<?> marker = worker.submit(() -> {});
    marker.get(5, TimeUnit.SECONDS);
  }

  private static HealthSnapshot awaitStatus(
      BridgeHealthCache cache, HealthSnapshot.Status expected) {
    awaitCondition("snapshot status " + expected, () -> cache.snapshotValue().status() == expected);
    return cache.snapshotValue();
  }

  private static void awaitIgnoringInterrupts(CountDownLatch latch, AtomicBoolean interrupted) {
    while (true) {
      try {
        latch.await();
        return;
      } catch (InterruptedException ignored) {
        interrupted.set(true);
      }
    }
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

  private static BridgeConfig bridgeConfig(String brokerType) {
    Map<String, String> env = new HashMap<>();
    env.put("BRIDGE_BROKER_TYPE", brokerType);
    env.put("BRIDGE_BROKER_URL", "tcp://broker:61616");
    return new BridgeConfig(env);
  }

  /**
   * Reusable blocked-broker fixture for the real-handler T5/T6 tests.
   *
   * <p>Opens are counted by the {@link JmsBrokerProbe.DiagnosticFactory}, verifies on every probe,
   * and the single admitted probe blocks in {@code verify} until the caller releases the gate. The
   * probe invocation count tracks actual {@link BrokerProbe#probe(long)} calls, so a cache that
   * queues refresh work fails the flat-admission assertions.
   *
   * @param entered counts down once the admitted probe reaches the gate
   * @param release holds the probe until the test releases it
   * @param result outcome returned after the gate opens
   * @return a gated probe with open, verify, and invocation counters
   */
  static BlockedBrokerProbe gatedBrokerProbe(
      CountDownLatch entered, CountDownLatch release, BrokerProbeResult result) {
    AtomicInteger opens = new AtomicInteger();
    AtomicInteger verifies = new AtomicInteger();
    JmsBrokerProbe diagnosticProbe =
        new JmsBrokerProbe(
            bridgeConfig("activemq"),
            (type, url, username, password) -> {
              opens.incrementAndGet();
              return new JmsBrokerProbe.Diagnostic() {
                @Override
                public void verify() throws Exception {
                  verifies.incrementAndGet();
                  entered.countDown();
                  release.await();
                }

                @Override
                public void close() {}
              };
            },
            () -> {},
            () -> {},
            () -> {},
            () -> {});
    return new BlockedBrokerProbe(diagnosticProbe, result, opens, verifies);
  }

  static final class BlockedBrokerProbe implements BrokerProbe {
    private final BrokerProbe delegate;
    private final BrokerProbeResult result;
    private final AtomicInteger probeCalls = new AtomicInteger();
    private final AtomicInteger opens;
    private final AtomicInteger verifies;

    private BlockedBrokerProbe(
        BrokerProbe delegate,
        BrokerProbeResult result,
        AtomicInteger opens,
        AtomicInteger verifies) {
      this.delegate = delegate;
      this.result = result;
      this.opens = opens;
      this.verifies = verifies;
    }

    int probeCalls() {
      return probeCalls.get();
    }

    int opens() {
      return opens.get();
    }

    int verifies() {
      return verifies.get();
    }

    @Override
    public BrokerProbeResult probe(long generation) throws Exception {
      probeCalls.incrementAndGet();
      delegate.probe(generation);
      return result;
    }

    @Override
    public void invalidate(long generation) {
      delegate.invalidate(generation);
    }

    @Override
    public void close() {
      delegate.close();
    }
  }

  static final class MutableClock {
    private final AtomicLong now = new AtomicLong();

    long get() {
      return now.get();
    }

    void set(long value) {
      now.set(value);
    }

    LongSupplier supplier() {
      return this::get;
    }
  }

  /** Deterministic {@link BridgeHealthCache.RefreshScheduler} seam driven tick by tick. */
  static final class FakeScheduler implements BridgeHealthCache.RefreshScheduler {
    private final AtomicLong periodMillis = new AtomicLong(-1L);
    private final AtomicBoolean shutdown = new AtomicBoolean();
    private volatile Runnable tick;

    @Override
    public void schedule(Runnable tick, long periodMillis) {
      this.tick = tick;
      this.periodMillis.set(periodMillis);
    }

    void runTick() {
      Runnable scheduled = tick;
      if (scheduled != null) {
        scheduled.run();
      }
    }

    long periodMillis() {
      return periodMillis.get();
    }

    boolean isShutdown() {
      return shutdown.get();
    }

    @Override
    public void shutdown() {
      shutdown.set(true);
    }
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
    private volatile Runnable closeAction = () -> {};
    private volatile LongConsumer invalidateAction = invalidatedGenerations::add;

    FakeProbe(ProbeAction action) {
      this.action = action;
    }

    FakeProbe onClose(Runnable action) {
      this.closeAction = action;
      return this;
    }

    FakeProbe onInvalidate(LongConsumer action) {
      this.invalidateAction = action;
      return this;
    }

    @Override
    public BrokerProbeResult probe(long generation) throws Exception {
      return action.run(probeCalls.incrementAndGet(), generation);
    }

    @Override
    public void invalidate(long generation) {
      invalidateAction.accept(generation);
    }

    @Override
    public void close() {
      closeCalls.incrementAndGet();
      closeAction.run();
    }
  }
}
