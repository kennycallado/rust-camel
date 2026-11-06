package org.rustcamel.jms;

import java.util.ArrayList;
import java.util.Comparator;
import java.util.List;
import java.util.Locale;
import java.util.concurrent.Callable;
import java.util.concurrent.CopyOnWriteArrayList;
import java.util.concurrent.ExecutionException;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.Future;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.TimeoutException;
import java.util.function.BooleanSupplier;
import java.util.function.IntSupplier;
import javax.jms.Connection;
import javax.jms.JMSException;
import org.apache.activemq.pool.PooledConnectionFactory;
import org.messaginghub.pooled.jms.JmsPoolConnectionFactory;

/**
 * Broker-free verifier for the connection-pool idle evictor on both bridges branches.
 *
 * <p>Run with {@code java org.rustcamel.jms.PoolEvictorVerifier}. The process is fail-closed: an
 * armed daemon watchdog halts the JVM with exit code 2 when the global budget expires, so a blocked
 * pool call, a blocked check-in, or a blocked shutdown hook can never hang the run. Only {@link
 * #main(String[])} exits normally, with code 0 when both branches prove {@code START}, {@code
 * RETAINED}, {@code EVICT}, {@code SHUTDOWN}.
 */
final class PoolEvictorVerifier {

  private static final long IDLE_TIMEOUT_MS = 1_000L;
  private static final long CHECK_INTERVAL_MS = 1_000L;
  private static final long START_POLL_MS = 10_000L;
  private static final long EVICT_DEADLINE_MS = 15_000L;
  private static final long IDLE_FLOOR_MS = IDLE_TIMEOUT_MS;
  private static final long EVICT_UPPER_MS = IDLE_TIMEOUT_MS + 2 * CHECK_INTERVAL_MS + 3_000L;
  private static final long STOP_BOUND_MS = 10_000L;
  private static final long SHUTDOWN_POLL_MS = 10_000L;
  private static final long GLOBAL_BUDGET_MS = 120_000L;

  private static final long POLL_INTERVAL_MS = 250L;
  // The poll owns its deadline; the outer bounded guard must outlive it so that a
  // legitimately exhausted poll reports POLL EXHAUSTED, not UNBOUNDED.
  private static final long POLL_GUARD_MS = 2_000L;
  private static final long WATCHDOG_GRACE_MS = 1_500L;
  private static final long NANOS_PER_MS = 1_000_000L;

  private static final String EVICTOR_THREAD_NEEDLE = "commons-pool-evictor";
  private static final String FAULT_ENV = "POOL_EVICTOR_FAULT";
  private static final String BUDGET_ENV = "POOL_EVICTOR_BUDGET_MS";
  private static final String BLOCK_CHECKIN_FAULT = "block-checkin";
  private static final String BLOCK_HOOK_FAULT = "block-hook";
  private static final String FAIL_ARTEMIS_FAULT = "fail-artemis";
  private static final String SKIP_ARTEMIS_FAULT = "skip-artemis";

  private static final Object BLOCK_MONITOR = new Object();

  // Lock-free diagnostic sources only: the watchdog and bounded-step timeouts must never touch a
  // pool getter. The lists are live CopyOnWriteArrayList views owned by the verifier thread.
  private static volatile List<StubClassicConnection> classicConnections = List.of();
  private static volatile List<StubJmsConnection> artemisConnections = List.of();

  private PoolEvictorVerifier() {}

  public static void main(String[] args) {
    startWatchdog();
    if (isFault(BLOCK_HOOK_FAULT)) {
      System.out.println("FAULT entered block-hook");
      System.out.flush();
      Runtime.getRuntime()
          .addShutdownHook(
              new Thread(
                  () -> {
                    System.out.println("FAULT shutdown hook blocking");
                    System.out.flush();
                    blockForever();
                  },
                  "pool-evictor-fault-block-hook"));
    }
    int classicCode = verifyClassic();
    int artemisCode = verifyArtemis();
    int total = classicCode + artemisCode;
    if (total == 0) {
      System.out.println("PASS both");
      System.out.flush();
    }
    System.exit(total == 0 ? 0 : 1);
  }

  // ---------------------------------------------------------------------------------------------
  // Watchdog

  private static void startWatchdog() {
    long budgetMs = effectiveBudgetMs();
    Thread watchdog =
        new Thread(
            () -> {
              try {
                Thread.sleep(budgetMs);
              } catch (InterruptedException e) {
                Thread.currentThread().interrupt();
              }
              Thread diagnostics =
                  new Thread(
                      () -> {
                        diagnoseLockFree();
                        System.out.println("WATCHDOG budget=" + budgetMs + "ms exhausted");
                        System.out.flush();
                      },
                      "pool-evictor-watchdog-diagnostics");
              diagnostics.setDaemon(true);
              diagnostics.start();
              try {
                Thread.sleep(WATCHDOG_GRACE_MS);
              } catch (InterruptedException e) {
                Thread.currentThread().interrupt();
              }
              Runtime.getRuntime().halt(2);
            },
            "pool-evictor-watchdog");
    watchdog.setDaemon(true);
    watchdog.start();
  }

  private static long effectiveBudgetMs() {
    String override = System.getenv(BUDGET_ENV);
    if (override != null && !override.isBlank()) {
      try {
        return Long.parseLong(override.trim());
      } catch (NumberFormatException e) {
        System.out.println(
            "WATCHDOG invalid "
                + BUDGET_ENV
                + "='"
                + override
                + "', using default "
                + GLOBAL_BUDGET_MS
                + "ms");
      }
    }
    return GLOBAL_BUDGET_MS;
  }

  // ---------------------------------------------------------------------------------------------
  // Bounded execution

  private static <T> T bounded(String what, long boundMs, Callable<T> call) {
    ExecutorService executor =
        Executors.newSingleThreadExecutor(
            runnable -> {
              Thread thread = new Thread(runnable, "pool-evictor-step-" + what.replace(' ', '-'));
              thread.setDaemon(true);
              return thread;
            });
    try {
      Future<T> future = executor.submit(call);
      try {
        return future.get(boundMs, TimeUnit.MILLISECONDS);
      } catch (TimeoutException e) {
        diagnoseLockFree();
        System.out.println("UNBOUNDED step=\"" + what + "\" bound=" + boundMs + "ms");
        System.out.flush();
        throw new IllegalStateException("UNBOUNDED: " + what);
      } catch (ExecutionException e) {
        Throwable cause = e.getCause();
        if (cause instanceof RuntimeException) {
          throw (RuntimeException) cause;
        }
        if (cause instanceof Error) {
          throw (Error) cause;
        }
        throw new IllegalStateException("STEP FAILED: " + what, cause);
      } catch (InterruptedException e) {
        Thread.currentThread().interrupt();
        throw new IllegalStateException("INTERRUPTED: " + what, e);
      }
    } finally {
      executor.shutdownNow();
    }
  }

  private static boolean pollUntil(BooleanSupplier condition, long deadlineMs) {
    long deadlineNanos = System.nanoTime() + deadlineMs * NANOS_PER_MS;
    while (true) {
      if (condition.getAsBoolean()) {
        return true;
      }
      if (System.nanoTime() >= deadlineNanos) {
        return false;
      }
      try {
        Thread.sleep(POLL_INTERVAL_MS);
      } catch (InterruptedException e) {
        Thread.currentThread().interrupt();
        return false;
      }
    }
  }

  // Distinguishes a poll that used up its own deadline from a step that is blocked
  // mid-call: only the latter may be reported as UNBOUNDED by bounded().
  private static void reportPollExhausted(String step, long deadlineMs) {
    System.out.println("POLL EXHAUSTED step=\"" + step + "\" deadline=" + deadlineMs + "ms");
    System.out.flush();
  }

  private static List<String> threadNamesContaining(String needle) {
    String lowerNeedle = needle.toLowerCase(Locale.ROOT);
    List<String> matches = new ArrayList<>();
    for (Thread thread : Thread.getAllStackTraces().keySet()) {
      String name = thread.getName();
      if (name != null && name.toLowerCase(Locale.ROOT).contains(lowerNeedle)) {
        matches.add(name);
      }
    }
    matches.sort(Comparator.naturalOrder());
    return matches;
  }

  // ---------------------------------------------------------------------------------------------
  // Classic branch

  private static int verifyClassic() {
    List<StubClassicConnection> created = new CopyOnWriteArrayList<>();
    classicConnections = created;
    StubClassicConnectionFactory delegate = new StubClassicConnectionFactory();
    delegate.setCreationListener(created::add);
    PooledConnectionFactory pool = new PooledConnectionFactory(delegate);

    long preStartNanos = System.nanoTime();
    try {
      bounded(
          "pool start",
          STOP_BOUND_MS,
          () -> {
            pool.setMaxConnections(5);
            pool.setIdleTimeout((int) IDLE_TIMEOUT_MS);
            pool.setTimeBetweenExpirationCheckMillis(CHECK_INTERVAL_MS);
            pool.start();
            return null;
          });
    } catch (RuntimeException e) {
      return failClassic("START", "pool start failed: " + e, pool);
    }

    // (a) START
    boolean evictorStarted;
    try {
      evictorStarted =
          bounded(
              "classic start poll",
              START_POLL_MS + POLL_GUARD_MS,
              () ->
                  pollUntil(
                      () -> !threadNamesContaining(EVICTOR_THREAD_NEEDLE).isEmpty(),
                      START_POLL_MS));
    } catch (RuntimeException e) {
      return failClassic("START", "evictor thread not observed: " + e, pool);
    }
    if (!evictorStarted) {
      reportPollExhausted("classic start poll", START_POLL_MS);
      return failClassic("START", "no evictor thread within " + START_POLL_MS + "ms", pool);
    }

    // (b) RETAINED
    final Connection borrowed;
    int createdBeforeBorrow = created.size();
    try {
      borrowed = bounded("borrow", STOP_BOUND_MS, pool::createConnection);
    } catch (RuntimeException e) {
      return failClassic("RETAINED", "borrow failed: " + e, pool);
    }
    if (borrowed == null) {
      return failClassic("RETAINED", "borrow returned null", pool);
    }
    long idleOriginNanos = System.nanoTime();
    try {
      bounded(
          "check-in",
          STOP_BOUND_MS,
          () -> {
            classicCheckIn(borrowed);
            return null;
          });
    } catch (RuntimeException e) {
      return failClassic("RETAINED", "check-in failed: " + e, pool);
    }
    int retained = pool.getNumConnections();
    if (retained < 1) {
      return failClassic("RETAINED", "retained=" + retained + " (expected >= 1)", pool);
    }
    System.out.println("BASE branch=classic retained=" + retained);
    System.out.flush();

    // (c) EVICT
    boolean evicted;
    try {
      evicted =
          bounded(
              "classic evict poll",
              EVICT_DEADLINE_MS + POLL_GUARD_MS,
              () -> pollUntil(() -> pool.getNumConnections() == 0, EVICT_DEADLINE_MS));
    } catch (RuntimeException e) {
      return failClassic("EVICT", "retained count did not reach 0: " + e, pool);
    }
    long evictedAtNanos = System.nanoTime();
    if (!evicted) {
      reportPollExhausted("classic evict poll", EVICT_DEADLINE_MS);
      return failClassic(
          "EVICT",
          "retained=" + pool.getNumConnections() + " after " + EVICT_DEADLINE_MS + "ms",
          pool);
    }

    StubClassicConnection borrowedConnection =
        createdBeforeBorrow < created.size() ? created.get(createdBeforeBorrow) : null;
    long latestIdleOriginNanos = Math.max(preStartNanos, idleOriginNanos);
    long evictElapsedNanos = evictedAtNanos - latestIdleOriginNanos;
    if (evictElapsedNanos > EVICT_UPPER_MS * NANOS_PER_MS) {
      return failClassic(
          "EVICT",
          "observed elapsed="
              + evictElapsedNanos / NANOS_PER_MS
              + "ms > upper="
              + EVICT_UPPER_MS
              + "ms",
          pool);
    }

    int stopRecords = 0;
    List<String> violations = new ArrayList<>();
    for (StubClassicConnection connection : created) {
      StubTransport transport = connection.stubTransport();
      boolean isBorrowed = connection == borrowedConnection;
      long originNanos = isBorrowed ? idleOriginNanos : preStartNanos;
      if (transport.stopTimes().isEmpty()) {
        violations.add("no physical stop recorded (origin=" + originName(isBorrowed) + ")");
      }
      for (Long stopNanos : transport.stopTimes()) {
        stopRecords++;
        long elapsedNanos = stopNanos - originNanos;
        if (elapsedNanos < IDLE_FLOOR_MS * NANOS_PER_MS) {
          violations.add(
              "stop elapsed="
                  + elapsedNanos / NANOS_PER_MS
                  + "ms < floor="
                  + IDLE_FLOOR_MS
                  + "ms (origin="
                  + originName(isBorrowed)
                  + ")");
        }
      }
    }
    if (!violations.isEmpty()) {
      return failClassic("EVICT", String.join("; ", violations), pool);
    }
    if (stopRecords != retained) {
      return failClassic("EVICT", "stop records=" + stopRecords + " != retained=" + retained, pool);
    }

    // (d) SHUTDOWN
    try {
      bounded(
          "stop",
          STOP_BOUND_MS,
          () -> {
            pool.stop();
            return null;
          });
    } catch (RuntimeException e) {
      return failClassic("SHUTDOWN", "stop failed: " + e, pool);
    }
    boolean shutDown;
    try {
      shutDown =
          bounded(
              "classic shutdown poll",
              SHUTDOWN_POLL_MS + POLL_GUARD_MS,
              () ->
                  pollUntil(
                      () ->
                          threadNamesContaining(EVICTOR_THREAD_NEEDLE).isEmpty()
                              && pool.getNumConnections() == 0,
                      SHUTDOWN_POLL_MS));
    } catch (RuntimeException e) {
      return failClassic("SHUTDOWN", "evictor did not stop: " + e, pool);
    }
    if (!shutDown) {
      reportPollExhausted("classic shutdown poll", SHUTDOWN_POLL_MS);
      return failClassic(
          "SHUTDOWN",
          "evictor threads="
              + threadNamesContaining(EVICTOR_THREAD_NEEDLE)
              + " retained="
              + pool.getNumConnections(),
          pool);
    }

    System.out.println("PASS branch=classic proofs=START,RETAINED,EVICT,SHUTDOWN");
    System.out.flush();
    return 0;
  }

  private static void classicCheckIn(Connection borrowed) throws JMSException {
    if (isFault(BLOCK_CHECKIN_FAULT)) {
      System.out.println("FAULT entered block-checkin");
      System.out.flush();
      blockForever();
    }
    borrowed.close();
  }

  private static String originName(boolean borrowed) {
    return borrowed ? "check-in" : "pre-start";
  }

  // ---------------------------------------------------------------------------------------------
  // Artemis branch

  private static int verifyArtemis() {
    if (isFault(SKIP_ARTEMIS_FAULT)) {
      System.out.println("FAULT entered skip-artemis");
      System.out.flush();
      return 0;
    }

    List<StubJmsConnection> created = new CopyOnWriteArrayList<>();
    artemisConnections = created;
    StubJmsConnectionFactory delegate =
        new StubJmsConnectionFactory(
            () -> {
              StubJmsConnection connection = new StubJmsConnection();
              created.add(connection);
              return connection;
            });
    JmsPoolConnectionFactory pool = new JmsPoolConnectionFactory();

    long preStartNanos = System.nanoTime();
    try {
      bounded(
          "pool start",
          STOP_BOUND_MS,
          () -> {
            pool.setConnectionFactory(delegate);
            pool.setMaxConnections(5);
            pool.setConnectionIdleTimeout((int) IDLE_TIMEOUT_MS);
            pool.setConnectionCheckInterval(CHECK_INTERVAL_MS);
            pool.start();
            return null;
          });
    } catch (RuntimeException e) {
      return failArtemis("START", "pool start failed: " + e, pool);
    }

    // (a) START
    boolean evictorStarted;
    try {
      evictorStarted =
          bounded(
              "artemis start poll",
              START_POLL_MS + POLL_GUARD_MS,
              () ->
                  pollUntil(
                      () -> !threadNamesContaining(EVICTOR_THREAD_NEEDLE).isEmpty(),
                      START_POLL_MS));
    } catch (RuntimeException e) {
      return failArtemis("START", "evictor thread not observed: " + e, pool);
    }
    if (!evictorStarted) {
      reportPollExhausted("artemis start poll", START_POLL_MS);
      return failArtemis("START", "no evictor thread within " + START_POLL_MS + "ms", pool);
    }
    if (isFault(FAIL_ARTEMIS_FAULT)) {
      return failArtemis("START", "fault-injected", pool);
    }

    // (b) RETAINED
    final Connection borrowed;
    int createdBeforeBorrow = created.size();
    try {
      borrowed = bounded("borrow", STOP_BOUND_MS, pool::createConnection);
    } catch (RuntimeException e) {
      return failArtemis("RETAINED", "borrow failed: " + e, pool);
    }
    if (borrowed == null) {
      return failArtemis("RETAINED", "borrow returned null", pool);
    }
    long idleOriginNanos = System.nanoTime();
    try {
      bounded(
          "check-in",
          STOP_BOUND_MS,
          () -> {
            borrowed.close();
            return null;
          });
    } catch (RuntimeException e) {
      return failArtemis("RETAINED", "check-in failed: " + e, pool);
    }
    int retained = pool.getNumConnections();
    if (retained < 1) {
      return failArtemis("RETAINED", "retained=" + retained + " (expected >= 1)", pool);
    }
    System.out.println("BASE branch=artemis retained=" + retained);
    System.out.flush();

    // (c) EVICT
    boolean evicted;
    try {
      evicted =
          bounded(
              "artemis evict poll",
              EVICT_DEADLINE_MS + POLL_GUARD_MS,
              () -> pollUntil(() -> pool.getNumConnections() == 0, EVICT_DEADLINE_MS));
    } catch (RuntimeException e) {
      return failArtemis("EVICT", "retained count did not reach 0: " + e, pool);
    }
    long evictedAtNanos = System.nanoTime();
    if (!evicted) {
      reportPollExhausted("artemis evict poll", EVICT_DEADLINE_MS);
      return failArtemis(
          "EVICT",
          "retained=" + pool.getNumConnections() + " after " + EVICT_DEADLINE_MS + "ms",
          pool);
    }

    long latestIdleOriginNanos = Math.max(preStartNanos, idleOriginNanos);
    long evictElapsedNanos = evictedAtNanos - latestIdleOriginNanos;
    if (evictElapsedNanos > EVICT_UPPER_MS * NANOS_PER_MS) {
      return failArtemis(
          "EVICT",
          "observed elapsed="
              + evictElapsedNanos / NANOS_PER_MS
              + "ms > upper="
              + EVICT_UPPER_MS
              + "ms",
          pool);
    }

    int closeRecords = 0;
    List<String> violations = new ArrayList<>();
    for (int index = 0; index < created.size(); index++) {
      StubJmsConnection connection = created.get(index);
      boolean isBorrowed = index >= createdBeforeBorrow;
      long originNanos = isBorrowed ? idleOriginNanos : preStartNanos;
      if (connection.closeTimes().isEmpty()) {
        violations.add("no physical close recorded (origin=" + originName(isBorrowed) + ")");
      }
      for (Long closeNanos : connection.closeTimes()) {
        closeRecords++;
        long elapsedNanos = closeNanos - originNanos;
        if (elapsedNanos < IDLE_FLOOR_MS * NANOS_PER_MS) {
          violations.add(
              "close elapsed="
                  + elapsedNanos / NANOS_PER_MS
                  + "ms < floor="
                  + IDLE_FLOOR_MS
                  + "ms (origin="
                  + originName(isBorrowed)
                  + ")");
        }
      }
    }
    if (!violations.isEmpty()) {
      return failArtemis("EVICT", String.join("; ", violations), pool);
    }
    if (closeRecords != retained) {
      return failArtemis(
          "EVICT", "close records=" + closeRecords + " != retained=" + retained, pool);
    }

    // (d) SHUTDOWN
    try {
      bounded(
          "stop",
          STOP_BOUND_MS,
          () -> {
            pool.stop();
            return null;
          });
    } catch (RuntimeException e) {
      return failArtemis("SHUTDOWN", "stop failed: " + e, pool);
    }
    boolean shutDown;
    try {
      shutDown =
          bounded(
              "artemis shutdown poll",
              SHUTDOWN_POLL_MS + POLL_GUARD_MS,
              () ->
                  pollUntil(
                      () ->
                          threadNamesContaining(EVICTOR_THREAD_NEEDLE).isEmpty()
                              && pool.getNumConnections() == 0,
                      SHUTDOWN_POLL_MS));
    } catch (RuntimeException e) {
      return failArtemis("SHUTDOWN", "evictor did not stop: " + e, pool);
    }
    if (!shutDown) {
      reportPollExhausted("artemis shutdown poll", SHUTDOWN_POLL_MS);
      return failArtemis(
          "SHUTDOWN",
          "evictor threads="
              + threadNamesContaining(EVICTOR_THREAD_NEEDLE)
              + " retained="
              + pool.getNumConnections(),
          pool);
    }

    System.out.println("PASS branch=artemis proofs=START,RETAINED,EVICT,SHUTDOWN");
    System.out.flush();
    return 0;
  }

  // ---------------------------------------------------------------------------------------------
  // Failure diagnostics

  private static int failClassic(String proof, String detail, PooledConnectionFactory pool) {
    System.out.println("FAIL branch=classic " + proof + " " + detail);
    diagnose("classic", pool::getNumConnections);
    return 1;
  }

  private static int failArtemis(String proof, String detail, JmsPoolConnectionFactory pool) {
    System.out.println("FAIL branch=artemis " + proof + " " + detail);
    diagnose("artemis", pool::getNumConnections);
    return 1;
  }

  // The verifier's own thread only: this is the only place that reads a pool getter for
  // diagnostics, and it is never called from the watchdog or a bounded-step timeout.
  private static void diagnose(String branch, IntSupplier retained) {
    try {
      System.out.println("DIAG branch=" + branch + " retained=" + retained.getAsInt());
    } catch (RuntimeException e) {
      System.out.println("DIAG branch=" + branch + " retained=<unavailable: " + e + ">");
    }
    diagnoseLockFree();
  }

  // Lock-free diagnostics: thread table and volatile stub records only, never a pool getter.
  private static void diagnoseLockFree() {
    System.out.println("DIAG threadDump=" + describeThreads());
    System.out.println("DIAG classic.connections=" + describeClassicStubs());
    System.out.println("DIAG artemis.connections=" + describeArtemisStubs());
    System.out.flush();
  }

  private static String describeThreads() {
    List<Thread> threads = new ArrayList<>(Thread.getAllStackTraces().keySet());
    threads.sort(
        Comparator.comparing(Thread::getName, Comparator.nullsFirst(Comparator.naturalOrder())));
    StringBuilder builder = new StringBuilder();
    for (Thread thread : threads) {
      builder
          .append('[')
          .append(thread.getName())
          .append(" daemon=")
          .append(thread.isDaemon())
          .append(" state=")
          .append(thread.getState())
          .append(']');
    }
    return builder.toString();
  }

  private static String describeClassicStubs() {
    StringBuilder builder = new StringBuilder();
    for (StubClassicConnection connection : classicConnections) {
      StubTransport transport = connection.stubTransport();
      builder
          .append("[stops=")
          .append(transport.stopTimes().size())
          .append(" stopped=")
          .append(transport.isStopped())
          .append(']');
    }
    return builder.toString();
  }

  private static String describeArtemisStubs() {
    StringBuilder builder = new StringBuilder();
    for (StubJmsConnection connection : artemisConnections) {
      builder
          .append("[closes=")
          .append(connection.closeTimes().size())
          .append(" closed=")
          .append(connection.isClosed())
          .append(']');
    }
    return builder.toString();
  }

  // ---------------------------------------------------------------------------------------------
  // Fault hooks

  private static boolean isFault(String candidate) {
    return candidate.equals(System.getenv(FAULT_ENV));
  }

  // Never returns and ignores interrupts, so a fault regression can prove the watchdog halts the
  // process even while a verifier step or a shutdown hook is stuck on a monitor.
  private static void blockForever() {
    synchronized (BLOCK_MONITOR) {
      while (true) {
        try {
          BLOCK_MONITOR.wait();
        } catch (InterruptedException e) {
          // Deliberately swallowed: the fault must stay blocked until the watchdog halts.
        }
      }
    }
  }
}
