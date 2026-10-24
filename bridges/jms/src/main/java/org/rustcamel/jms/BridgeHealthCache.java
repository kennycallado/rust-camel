package org.rustcamel.jms;

import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.RejectedExecutionException;
import java.util.concurrent.ScheduledExecutorService;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicReference;
import java.util.function.LongSupplier;
import java.util.function.Supplier;

/**
 * Immutable broker-health snapshot cache with scheduler-driven single-flight refresh.
 *
 * <p>An owned daemon scheduler requests refresh at startup and every {@link #REFRESH_AFTER_MS},
 * independently of reads or sends; {@link #current()} never blocks, never probes the broker, and
 * never admits refresh work. At most one refresh is admitted at a time on the one daemon worker
 * owned by this cache. Invalidation advances the generation and preserves an in-flight timestamp
 * until that worker exits, so a late result or send cannot publish into the new generation.
 * Terminal {@link #close()} cancels the scheduler before worker and provider teardown, runs
 * provider cleanup on one dedicated daemon worker, and returns within a fixed bound even when that
 * cleanup hangs.
 */
public final class BridgeHealthCache {

  static final long HEALTH_TTL_MS = 10_000L;
  static final long REFRESH_AFTER_MS = 5_000L;

  private record CacheState(
      long generation, HealthSnapshot snapshot, Long refreshStartedAtMillis, boolean closed) {}

  private final BrokerProbe probe;
  private final boolean skip;
  private final LongSupplier clock;
  private final ExecutorService worker;
  private final ExecutorService closeWorker;
  private final RefreshScheduler scheduler;
  private final AtomicReference<Thread> closeRunner = new AtomicReference<>();
  private final AtomicReference<CacheState> state;

  /**
   * Production constructor.
   *
   * @param probe generation-scoped broker probe
   * @param skip when true, health reports skipped without broker I/O
   * @param clock millisecond clock used for status ages
   */
  public BridgeHealthCache(BrokerProbe probe, boolean skip, LongSupplier clock) {
    this(
        probe,
        skip,
        clock,
        BridgeHealthCache::newWorker,
        BridgeHealthCache::newCloseWorker,
        BridgeHealthCache::newScheduler);
  }

  /** Package-private constructor with an executor seam for deterministic test teardown. */
  BridgeHealthCache(
      BrokerProbe probe,
      boolean skip,
      LongSupplier clock,
      Supplier<ExecutorService> executorFactory) {
    this(
        probe,
        skip,
        clock,
        executorFactory,
        BridgeHealthCache::newCloseWorker,
        BridgeHealthCache::newScheduler);
  }

  /**
   * Package-private constructor with executor and scheduler seams for deterministic tests.
   *
   * @param probe generation-scoped broker probe
   * @param skip when true, health reports skipped without broker I/O and no refresh is scheduled
   * @param clock millisecond clock used for status ages
   * @param executorFactory refresh worker factory
   * @param closeWorkerFactory dedicated close worker factory
   * @param schedulerFactory refresh scheduler factory
   */
  BridgeHealthCache(
      BrokerProbe probe,
      boolean skip,
      LongSupplier clock,
      Supplier<ExecutorService> executorFactory,
      Supplier<ExecutorService> closeWorkerFactory,
      Supplier<RefreshScheduler> schedulerFactory) {
    this.probe = probe;
    this.skip = skip;
    this.clock = clock;
    this.worker = executorFactory.get();
    this.closeWorker = closeWorkerFactory.get();
    this.scheduler = schedulerFactory.get();
    long now = clock.getAsLong();
    this.state =
        new AtomicReference<>(new CacheState(1L, HealthSnapshot.unknown(1L, now), null, false));
    if (!skip) {
      this.scheduler.schedule(this::onScheduledRefresh, REFRESH_AFTER_MS);
    }
  }

  /**
   * Returns the cached snapshot without blocking and without probing the broker. The mapping is
   * pure: a read never admits refresh work; the owned scheduler requests refresh at startup and
   * every {@link #REFRESH_AFTER_MS} independently of reads or sends.
   */
  public HealthSnapshot current() {
    long now = clock.getAsLong();
    CacheState observed = state.get();

    if (observed.closed()) {
      return HealthSnapshot.unknown(observed.generation(), now);
    }
    if (skip) {
      return HealthSnapshot.skipped(observed.generation(), now);
    }

    long snapshotAge = now - observed.snapshot().checkedAtMillis();
    Long refreshStartedAt = observed.refreshStartedAtMillis();
    boolean refreshInFlight = refreshStartedAt != null;
    long inflightAge = refreshInFlight ? now - refreshStartedAt : 0L;

    if (snapshotAge >= HEALTH_TTL_MS || (refreshInFlight && inflightAge >= HEALTH_TTL_MS)) {
      long age = refreshInFlight ? Math.max(snapshotAge, inflightAge) : snapshotAge;
      return HealthSnapshot.stale(observed.generation(), age, now);
    }

    if (observed.snapshot().status() == HealthSnapshot.Status.UNKNOWN && refreshInFlight) {
      return HealthSnapshot.unknown(observed.generation(), now);
    }

    return observed.snapshot();
  }

  /** Current generation, advanced only by {@link #invalidate()}. */
  public long generation() {
    return state.get().generation();
  }

  /** Classification of the probe's last connect-path failure, or {@code null} when none. */
  BrokerFailureClass lastFailureClass() {
    return probe.lastFailureClass();
  }

  /** Package-private test accessor for the published snapshot. */
  HealthSnapshot snapshotValue() {
    return state.get().snapshot();
  }

  /**
   * Publishes a verified-up snapshot from a successful send when the generation is still current. A
   * stale generation writes nothing.
   *
   * @param expectedGeneration generation captured before the send
   */
  public void recordSendSuccess(long expectedGeneration) {
    CacheState observed = state.get();
    if (observed.closed() || observed.generation() != expectedGeneration) {
      return;
    }
    publish(expectedGeneration, HealthSnapshot.up(expectedGeneration, clock.getAsLong()));
  }

  /**
   * Advances the generation, resets the snapshot to unknown, preserves any in-flight admission, and
   * asks the probe to retire the old diagnostic without blocking the caller.
   *
   * <p>An already closed cache ignores invalidation: terminal teardown already advanced the
   * generation and detached the owned diagnostic exactly once.
   */
  public void invalidate() {
    CacheState observed;
    long nextGeneration;
    while (true) {
      observed = state.get();
      if (observed.closed()) {
        return;
      }
      nextGeneration = observed.generation() + 1;
      CacheState next =
          new CacheState(
              nextGeneration,
              HealthSnapshot.unknown(nextGeneration, clock.getAsLong()),
              observed.refreshStartedAtMillis(),
              false);
      if (state.compareAndSet(observed, next)) {
        break;
      }
    }
    probe.invalidate(nextGeneration);
  }

  /**
   * Terminal teardown with once-only semantics.
   *
   * <p>The winning caller advances the generation, installs an unknown snapshot, and preserves the
   * in-flight admission until the refresh worker exits. Scheduled refresh admissions are cancelled
   * first so no tick can admit work after teardown starts. Provider detach is non-blocking, the
   * refresh worker is stopped for at most two seconds, and provider close runs on the one dedicated
   * close worker for at most one second before it is interrupted. A queued terminal close is never
   * cancelled: graceful shutdown lets it run once the worker is free, even when the caller was
   * interrupted before it started. Concurrent and later calls return immediately without repeating
   * teardown.
   */
  public void close() {
    CacheState observed;
    long nextGeneration;
    while (true) {
      observed = state.get();
      if (observed.closed()) {
        return;
      }
      nextGeneration = observed.generation() + 1;
      CacheState next =
          new CacheState(
              nextGeneration,
              HealthSnapshot.unknown(nextGeneration, clock.getAsLong()),
              observed.refreshStartedAtMillis(),
              true);
      if (state.compareAndSet(observed, next)) {
        break;
      }
    }

    scheduler.shutdown();

    try {
      probe.invalidate(nextGeneration);
    } catch (RuntimeException ignored) {
      // Terminal teardown is still bounded when a provider detach fails.
    }

    worker.shutdownNow();
    if (awaitTerminationUninterruptibly(worker, 2, TimeUnit.SECONDS)) {
      clearRefreshTimestamp();
    }

    try {
      closeWorker.execute(
          () -> {
            closeRunner.set(Thread.currentThread());
            try {
              probe.close();
            } catch (RuntimeException ignored) {
              // Provider cleanup is best effort; the generation is already terminal.
            } finally {
              closeRunner.compareAndSet(Thread.currentThread(), null);
            }
          });
    } catch (RejectedExecutionException rejected) {
      // A once-only close never shuts the close worker down before submitting its task.
    }
    closeWorker.shutdown();
    try {
      if (!closeWorker.awaitTermination(1, TimeUnit.SECONDS)) {
        interruptRunningClose();
      }
    } catch (InterruptedException error) {
      Thread.currentThread().interrupt();
      interruptRunningClose();
    }
  }

  /**
   * Interrupts only a provider close that is already running. Shutdown is graceful, so a terminal
   * task that has not started stays queued and still runs exactly once.
   */
  private void interruptRunningClose() {
    Thread running = closeRunner.get();
    if (running != null && running != Thread.currentThread()) {
      running.interrupt();
    }
  }

  /**
   * Runs one scheduler-driven refresh admission. Admission reuses the worker single-flight CAS, so
   * a tick that lands while a refresh is in flight is a no-op, and a tick after close or on a
   * generation superseded by {@link #invalidate()} admits nothing.
   */
  private void onScheduledRefresh() {
    if (skip) {
      return;
    }
    try {
      CacheState observed = state.get();
      if (!observed.closed()) {
        admitRefresh(observed.generation(), clock.getAsLong());
      }
    } catch (RuntimeException ignored) {
      // A failing tick must never suppress the scheduler's subsequent executions.
    }
  }

  private void admitRefresh(long generation, long now) {
    CacheState admitted;
    while (true) {
      CacheState observed = state.get();
      if (observed.closed()
          || observed.generation() != generation
          || observed.refreshStartedAtMillis() != null) {
        return;
      }
      admitted = new CacheState(observed.generation(), observed.snapshot(), now, observed.closed());
      if (state.compareAndSet(observed, admitted)) {
        break;
      }
    }
    CacheState submission = admitted;
    try {
      worker.execute(() -> runRefresh(submission.generation()));
    } catch (RejectedExecutionException rejected) {
      clearAdmission(submission);
    }
  }

  private void runRefresh(long capturedGeneration) {
    try {
      BrokerProbeResult result = probe.probe(capturedGeneration);
      long now = clock.getAsLong();
      HealthSnapshot snapshot =
          result.connected()
              ? HealthSnapshot.up(capturedGeneration, now)
              : HealthSnapshot.down(capturedGeneration, result.message(), now);
      publish(capturedGeneration, snapshot);
    } catch (Exception error) {
      if (error instanceof InterruptedException) {
        Thread.currentThread().interrupt();
      }
      publish(
          capturedGeneration,
          HealthSnapshot.down(capturedGeneration, summarize(error), clock.getAsLong()));
    } finally {
      clearRefreshTimestamp();
    }
  }

  private void publish(long expectedGeneration, HealthSnapshot snapshot) {
    while (true) {
      CacheState observed = state.get();
      if (observed.closed() || observed.generation() != expectedGeneration) {
        return;
      }
      CacheState next =
          new CacheState(
              observed.generation(),
              snapshot,
              observed.refreshStartedAtMillis(),
              observed.closed());
      if (state.compareAndSet(observed, next)) {
        return;
      }
    }
  }

  private void clearAdmission(CacheState admitted) {
    Long admission = admitted.refreshStartedAtMillis();
    while (true) {
      CacheState observed = state.get();
      if (observed.refreshStartedAtMillis() == null
          || !observed.refreshStartedAtMillis().equals(admission)) {
        return;
      }
      CacheState cleared =
          new CacheState(observed.generation(), observed.snapshot(), null, observed.closed());
      if (state.compareAndSet(observed, cleared)) {
        return;
      }
    }
  }

  private void clearRefreshTimestamp() {
    while (true) {
      CacheState observed = state.get();
      if (observed.refreshStartedAtMillis() == null) {
        return;
      }
      CacheState next =
          new CacheState(observed.generation(), observed.snapshot(), null, observed.closed());
      if (state.compareAndSet(observed, next)) {
        return;
      }
    }
  }

  private static String summarize(Throwable error) {
    if (error == null) {
      return "broker probe failed";
    }
    return error.getClass().getSimpleName() + " during broker probe";
  }

  private static ExecutorService newWorker() {
    return Executors.newSingleThreadExecutor(
        runnable -> {
          Thread thread = new Thread(runnable, "jms-health-refresh");
          thread.setDaemon(true);
          return thread;
        });
  }

  /** Package-private close worker factory, visible for the deterministic test seam. */
  static ExecutorService newCloseWorker() {
    return Executors.newSingleThreadExecutor(
        runnable -> {
          Thread thread = new Thread(runnable, "jms-health-close");
          thread.setDaemon(true);
          return thread;
        });
  }

  private static RefreshScheduler newScheduler() {
    ScheduledExecutorService executor =
        Executors.newSingleThreadScheduledExecutor(
            runnable -> {
              Thread thread = new Thread(runnable, "jms-health-refresh-scheduler");
              thread.setDaemon(true);
              return thread;
            });
    return new RefreshScheduler() {
      @Override
      public void schedule(Runnable tick, long periodMillis) {
        executor.scheduleAtFixedRate(tick, 0L, periodMillis, TimeUnit.MILLISECONDS);
      }

      @Override
      public void shutdown() {
        executor.shutdownNow();
      }
    };
  }

  /** Package-private refresh scheduling seam for deterministic tests. */
  interface RefreshScheduler {

    /** Schedules {@code tick} to run once immediately and then every {@code periodMillis}. */
    void schedule(Runnable tick, long periodMillis);

    /** Cancels future ticks; idempotent. */
    void shutdown();
  }

  /** Waits up to {@code timeout} while preserving the caller's interrupt flag and total bound. */
  private static boolean awaitTerminationUninterruptibly(
      ExecutorService executor, long timeout, TimeUnit unit) {
    long deadline = System.nanoTime() + unit.toNanos(timeout);
    boolean interrupted = false;
    try {
      while (true) {
        long remaining = deadline - System.nanoTime();
        if (remaining <= 0L) {
          return executor.isTerminated();
        }
        try {
          return executor.awaitTermination(remaining, TimeUnit.NANOSECONDS);
        } catch (InterruptedException error) {
          interrupted = true;
        }
      }
    } finally {
      if (interrupted) {
        Thread.currentThread().interrupt();
      }
    }
  }
}
