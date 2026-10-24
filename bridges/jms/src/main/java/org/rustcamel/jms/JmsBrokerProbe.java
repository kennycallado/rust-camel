package org.rustcamel.jms;

import java.util.concurrent.ArrayBlockingQueue;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.RejectedExecutionException;
import java.util.concurrent.ThreadPoolExecutor;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.concurrent.atomic.AtomicReference;
import javax.jms.ExceptionListener;
import javax.jms.JMSException;
import org.apache.activemq.ActiveMQConnection;
import org.apache.activemq.ActiveMQConnectionFactory;
import org.apache.activemq.artemis.api.core.TransportConfiguration;
import org.apache.activemq.artemis.api.core.client.ClientSessionFactory;
import org.apache.activemq.artemis.api.core.client.FailoverEventType;
import org.apache.activemq.artemis.api.core.client.ServerLocator;
import org.apache.activemq.transport.TransportListener;
import org.jboss.logging.Logger;

/**
 * Retains one provider-specific diagnostic resource per probe generation.
 *
 * <p>A single health worker owns resource creation. The retained diagnostic is verified in place
 * across refreshes; failures, invalidations, and terminal close detach it exactly once and hand
 * provider cleanup to one bounded daemon executor. No provider I/O runs on the invalidation or
 * close caller thread, and no service-wide lock is held while provider code runs.
 */
public final class JmsBrokerProbe implements BrokerProbe {

  private static final Logger LOG = Logger.getLogger(JmsBrokerProbe.class);

  /** Package-private test seam for provider diagnostic resources. */
  interface Diagnostic {
    void verify() throws Exception;

    void close();
  }

  /** Package-private test seam for provider diagnostic construction. */
  interface DiagnosticFactory {
    Diagnostic open(String brokerType, String brokerUrl, String username, String password)
        throws Exception;
  }

  private record ProbeState(long generation, Diagnostic diagnostic, boolean closed) {}

  private final BridgeConfig config;
  private final DiagnosticFactory factory;
  private volatile Throwable lastBrokerFailure;
  private final AtomicReference<ProbeState> state =
      new AtomicReference<>(new ProbeState(1L, null, false));
  private final AtomicInteger retirementPending = new AtomicInteger();
  private final AtomicInteger activeProbes = new AtomicInteger();
  private final AtomicBoolean cleanupShutdown = new AtomicBoolean();
  private final CountDownLatch cleanupQuiescent = new CountDownLatch(1);
  private final ExecutorService cleanup;
  private final Runnable retirementTransitionHook;
  private final Runnable beforeTerminalCasHook;
  private final Runnable closeEntryHook;
  private final Runnable cleanupCompletedHook;

  /**
   * Production probe using the provider-native diagnostic adapters.
   *
   * @param config bridge configuration shared with the client factory
   */
  public JmsBrokerProbe(BridgeConfig config) {
    this(config, null, () -> {}, () -> {}, () -> {}, () -> {});
  }

  JmsBrokerProbe(
      BridgeConfig config,
      DiagnosticFactory factory,
      Runnable retirementTransitionHook,
      Runnable beforeTerminalCasHook,
      Runnable closeEntryHook,
      Runnable cleanupCompletedHook) {
    this.config = config;
    this.factory = factory != null ? factory : this::openDiagnostic;
    this.retirementTransitionHook = retirementTransitionHook;
    this.beforeTerminalCasHook = beforeTerminalCasHook;
    this.closeEntryHook = closeEntryHook;
    this.cleanupCompletedHook = cleanupCompletedHook;
    this.cleanup =
        new ThreadPoolExecutor(
            1,
            1,
            0L,
            TimeUnit.MILLISECONDS,
            new ArrayBlockingQueue<>(2),
            runnable -> {
              Thread thread = new Thread(runnable, "jms-probe-cleanup");
              thread.setDaemon(true);
              return thread;
            },
            new ThreadPoolExecutor.AbortPolicy());
  }

  @Override
  public BrokerProbeResult probe(long generation) {
    activeProbes.incrementAndGet();
    try {
      return probeInternal(generation);
    } finally {
      activeProbes.decrementAndGet();
      maybeShutdownCleanup();
    }
  }

  private BrokerProbeResult probeInternal(long generation) {
    ProbeState observed = state.get();
    if (observed.closed()) {
      return BrokerProbeResult.down("probe closed");
    }
    if (observed.generation() != generation) {
      return BrokerProbeResult.down("probe invalidated");
    }
    if (retirementPending.get() != 0) {
      return BrokerProbeResult.down("probe cleanup pending");
    }

    Diagnostic retained = observed.diagnostic();
    if (retained == null) {
      Diagnostic opened;
      try {
        opened =
            factory.open(
                config.brokerType(), config.brokerUrl(), config.username(), config.password());
      } catch (Exception error) {
        return failure(error);
      }
      if (!state.compareAndSet(observed, new ProbeState(generation, opened, false))) {
        retire(opened);
        return BrokerProbeResult.down("probe invalidated");
      }
      retained = opened;
    }

    try {
      retained.verify();
    } catch (Exception error) {
      detachAndRetire();
      return failure(error);
    }

    ProbeState current = state.get();
    if (!current.closed()
        && current.generation() == generation
        && current.diagnostic() == retained) {
      return BrokerProbeResult.up();
    }
    detachAndRetire();
    return BrokerProbeResult.down("probe invalidated");
  }

  @Override
  public void invalidate(long generation) {
    while (true) {
      ProbeState observed = state.get();
      if (generation <= observed.generation()) {
        return;
      }
      Diagnostic detached = observed.diagnostic();
      boolean accounted = accountRetirement(detached);
      beforeTerminalCasHook.run();
      ProbeState next = new ProbeState(generation, null, observed.closed());
      if (state.compareAndSet(observed, next)) {
        if (accounted) {
          try {
            retirementTransitionHook.run();
          } finally {
            submitAccountedRetirement(detached);
          }
        }
        return;
      }
      if (accounted) {
        releaseRetirementReservation();
      }
    }
  }

  @Override
  public void close() {
    closeEntryHook.run();
    while (true) {
      ProbeState observed = state.get();
      if (observed.closed()) {
        return;
      }
      Diagnostic detached = observed.diagnostic();
      boolean accounted = accountRetirement(detached);
      beforeTerminalCasHook.run();
      ProbeState next = new ProbeState(observed.generation(), null, true);
      if (state.compareAndSet(observed, next)) {
        if (accounted) {
          submitAccountedRetirement(detached);
        }
        break;
      }
      if (accounted) {
        releaseRetirementReservation();
      }
    }
    maybeShutdownCleanup();
    try {
      cleanupQuiescent.await(1, TimeUnit.SECONDS);
    } catch (InterruptedException error) {
      Thread.currentThread().interrupt();
    }
  }

  /** Test seam: true when no provider cleanup or open is outstanding. */
  boolean isCleanupIdle() {
    return retirementPending.get() == 0 && activeProbes.get() == 0;
  }

  /**
   * Detaches the currently retained diagnostic, but only if it is still the retained instance.
   * Identity checking makes the detach idempotent across verify, invalidate, and close races.
   */
  private void detachAndRetire() {
    while (true) {
      ProbeState observed = state.get();
      Diagnostic diagnostic = observed.diagnostic();
      if (diagnostic == null) {
        return;
      }
      boolean accounted = accountRetirement(diagnostic);
      ProbeState next = new ProbeState(observed.generation(), null, observed.closed());
      if (state.compareAndSet(observed, next)) {
        if (accounted) {
          submitAccountedRetirement(diagnostic);
        }
        return;
      }
      if (accounted) {
        releaseRetirementReservation();
      }
    }
  }

  /**
   * Accounts a retirement before the ownership-detaching CAS. A concurrent close therefore cannot
   * observe zero pending retirements and shut the cleanup worker down before this retirement is
   * submitted.
   */
  private boolean accountRetirement(Diagnostic diagnostic) {
    if (diagnostic == null) {
      return false;
    }
    retirementPending.incrementAndGet();
    return true;
  }

  /**
   * Releases a speculative retirement reservation (a losing CAS rollback or a completed cleanup)
   * and re-evaluates shutdown. Every decrement path must observe a terminal state that may have
   * become eligible while this reservation was still counted.
   */
  private void releaseRetirementReservation() {
    retirementPending.decrementAndGet();
    maybeShutdownCleanup();
  }

  private void retire(Diagnostic diagnostic) {
    if (accountRetirement(diagnostic)) {
      submitAccountedRetirement(diagnostic);
    }
  }

  /** Submits an already-accounted retirement; never drops the resource on rejection. */
  private void submitAccountedRetirement(Diagnostic diagnostic) {
    Runnable cleanupTask =
        () -> {
          try {
            diagnostic.close();
          } catch (RuntimeException ignored) {
            // Provider cleanup is best effort; the generation is already invalidated.
          } finally {
            releaseRetirementReservation();
            cleanupCompletedHook.run();
          }
        };
    try {
      cleanup.execute(cleanupTask);
    } catch (RejectedExecutionException rejected) {
      // Accounting precedes detach and shutdown is eligible only when no retirement is pending,
      // so the owned executor cannot legitimately reject an accounted retirement. Surface an
      // invariant violation instead of dropping the resource or spawning another worker.
      throw new IllegalStateException("probe cleanup rejected an accounted retirement", rejected);
    }
  }

  private void maybeShutdownCleanup() {
    if (state.get().closed()
        && retirementPending.get() == 0
        && activeProbes.get() == 0
        && cleanupShutdown.compareAndSet(false, true)) {
      cleanup.shutdown();
      cleanupQuiescent.countDown();
    }
  }

  /**
   * Retains the last connect-path failure for health reporting. The remote text stays in the local
   * warn log; the wire only ever carries the classification.
   */
  private BrokerProbeResult failure(Exception error) {
    lastBrokerFailure = error;
    LOG.warn("Broker probe failed: " + BrokerFailureClassifier.classify(error).name(), error);
    if (error instanceof InterruptedException) {
      Thread.currentThread().interrupt();
    }
    return BrokerProbeResult.down(error.getClass().getSimpleName() + " during broker probe");
  }

  /** Classification of the last connect-path failure, or {@code null} when none was recorded. */
  @Override
  public BrokerFailureClass lastFailureClass() {
    Throwable failure = lastBrokerFailure;
    return failure == null ? null : BrokerFailureClassifier.classify(failure);
  }

  private Diagnostic openDiagnostic(
      String brokerType, String brokerUrl, String username, String password) throws Exception {
    return switch (brokerType) {
      case "activemq" -> openActiveMq(brokerUrl, username, password);
      case "artemis" -> openArtemis(brokerUrl, username, password);
      default ->
          throw new IllegalArgumentException(
              "Unsupported broker_type: '" + brokerType + "'. Valid values: activemq, artemis");
    };
  }

  private Diagnostic openActiveMq(String url, String user, String pass) throws Exception {
    ActiveMQConnectionFactory factory = buildActiveMqDiagnosticFactory(url, user, pass);
    var connection = factory.createConnection();
    try {
      connection.start();
      return activeMqDiagnostic((ActiveMQConnection) connection);
    } catch (Exception error) {
      retire(new ActiveMqDiagnostic((ActiveMQConnection) connection, new AtomicBoolean()));
      throw error;
    }
  }

  static ActiveMQConnectionFactory buildActiveMqDiagnosticFactory(
      String url, String user, String pass) {
    ActiveMQConnectionFactory factory = JmsClientFactory.buildActiveMqFactory(url, user, pass);
    clearExplicitClientId(factory);
    return factory;
  }

  /**
   * Clears only the explicit client ID through the provider setter. The broker URL, including a
   * composite {@code failover:} wrapper and its {@code jms.clientIDPrefix}/{@code
   * jms.connectionIDPrefix} options, is never parsed or rewritten.
   */
  static void clearExplicitClientId(ActiveMQConnectionFactory factory) {
    factory.setClientID(null);
  }

  static Diagnostic activeMqDiagnostic(ActiveMQConnection connection) throws JMSException {
    AtomicBoolean broken = new AtomicBoolean();
    connection.setExceptionListener((ExceptionListener) error -> broken.set(true));
    connection.addTransportListener(
        new TransportListener() {
          @Override
          public void onCommand(Object command) {}

          @Override
          public void onException(java.io.IOException error) {
            broken.set(true);
          }

          @Override
          public void transportInterupted() {
            broken.set(true);
          }

          @Override
          public void transportResumed() {}
        });
    return new ActiveMqDiagnostic(connection, broken);
  }

  private Diagnostic openArtemis(String url, String user, String pass) throws Exception {
    org.apache.activemq.artemis.jms.client.ActiveMQConnectionFactory factory =
        buildArtemisDiagnosticFactory(url, user, pass);
    return openArtemisDiagnostic(factory);
  }

  Diagnostic openArtemisDiagnostic(
      org.apache.activemq.artemis.jms.client.ActiveMQConnectionFactory factory) throws Exception {
    return artemisDiagnostic(factory, this::retire);
  }

  /** Builds the native-image-safe Artemis factory and clears its explicit client ID. */
  static org.apache.activemq.artemis.jms.client.ActiveMQConnectionFactory
      buildArtemisDiagnosticFactory(String url, String user, String pass) {
    org.apache.activemq.artemis.jms.client.ActiveMQConnectionFactory factory =
        JmsClientFactory.buildArtemisFactory(url, user, pass);
    clearArtemisExplicitClientId(factory);
    return factory;
  }

  private static void clearArtemisExplicitClientId(
      org.apache.activemq.artemis.jms.client.ActiveMQConnectionFactory factory) {
    factory.setClientID(null);
  }

  /**
   * Keeps the native-image-safe Artemis path: static transport configuration plus {@code
   * createSessionFactory(tc)}. Never falls back to a generic {@code createConnection()}.
   */
  static Diagnostic artemisDiagnostic(
      org.apache.activemq.artemis.jms.client.ActiveMQConnectionFactory factory) throws Exception {
    return artemisDiagnostic(factory, acquired -> {});
  }

  private static Diagnostic artemisDiagnostic(
      org.apache.activemq.artemis.jms.client.ActiveMQConnectionFactory factory,
      java.util.function.Consumer<Diagnostic> onFailure)
      throws Exception {
    ServerLocator locator;
    try {
      locator = factory.getServerLocator();
    } catch (RuntimeException | Error error) {
      onFailure.accept(factoryOnly(factory));
      throw error;
    }

    TransportConfiguration transportConfiguration;
    try {
      transportConfiguration = locator.getStaticTransportConfigurations()[0];
    } catch (RuntimeException | Error error) {
      onFailure.accept(factoryOnly(factory));
      throw error;
    }

    ClientSessionFactory sessionFactory;
    try {
      sessionFactory = locator.createSessionFactory(transportConfiguration);
    } catch (Exception error) {
      onFailure.accept(factoryOnly(factory));
      throw error;
    }

    AtomicBoolean broken = new AtomicBoolean();
    try {
      sessionFactory.addFailoverListener(
          event -> {
            if (event == FailoverEventType.FAILURE_DETECTED
                || event == FailoverEventType.FAILOVER_FAILED) {
              broken.set(true);
            }
          });
    } catch (RuntimeException | Error error) {
      onFailure.accept(new ArtemisDiagnostic(factory, locator, sessionFactory, broken));
      throw error;
    }
    return new ArtemisDiagnostic(factory, locator, sessionFactory, broken);
  }

  private static Diagnostic factoryOnly(
      org.apache.activemq.artemis.jms.client.ActiveMQConnectionFactory factory) {
    return new Diagnostic() {
      @Override
      public void verify() {}

      @Override
      public void close() {
        factory.close();
      }
    };
  }

  private static final class ActiveMqDiagnostic implements Diagnostic {
    private final ActiveMQConnection connection;
    private final AtomicBoolean broken;

    ActiveMqDiagnostic(ActiveMQConnection connection, AtomicBoolean broken) {
      this.connection = connection;
      this.broken = broken;
    }

    @Override
    public void verify() throws Exception {
      if (broken.get()) {
        throw new IllegalStateException("broker reported a transport failure");
      }
      connection.getMetaData();
      if (broken.get()) {
        throw new IllegalStateException("broker reported a transport failure");
      }
    }

    @Override
    public void close() {
      try {
        connection.close();
      } catch (JMSException ignored) {
        // Retired diagnostic: provider cleanup is best effort.
      }
    }
  }

  static final class ArtemisDiagnostic implements Diagnostic {
    private final org.apache.activemq.artemis.jms.client.ActiveMQConnectionFactory factory;
    private final ServerLocator locator;
    private final ClientSessionFactory sessionFactory;
    private final AtomicBoolean broken;

    ArtemisDiagnostic(
        org.apache.activemq.artemis.jms.client.ActiveMQConnectionFactory factory,
        ServerLocator locator,
        ClientSessionFactory sessionFactory,
        AtomicBoolean broken) {
      this.factory = factory;
      this.locator = locator;
      this.sessionFactory = sessionFactory;
      this.broken = broken;
    }

    ClientSessionFactory retainedSessionFactory() {
      return sessionFactory;
    }

    @Override
    public void verify() throws Exception {
      if (broken.get()) {
        throw new IllegalStateException("broker reported a session-factory failure");
      }
      // Credentials come from the same configured factory as the client, never
      // from an anonymous no-argument session. The remaining flags/batching
      // mirror createSession()'s defaults (see ClientSessionFactoryImpl).
      var session =
          sessionFactory.createSession(
              factory.getUser(),
              factory.getPassword(),
              false,
              true,
              true,
              locator.isPreAcknowledge(),
              locator.getAckBatchSize());
      try {
        // Session creation is the reachability check on the native-image-safe path.
      } finally {
        if (session != null) {
          session.close();
        }
      }
      if (broken.get()) {
        throw new IllegalStateException("broker reported a session-factory failure");
      }
    }

    @Override
    public void close() {
      try {
        sessionFactory.close();
      } finally {
        factory.close();
      }
    }
  }
}
