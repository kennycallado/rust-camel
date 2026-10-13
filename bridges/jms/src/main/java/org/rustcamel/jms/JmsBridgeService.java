package org.rustcamel.jms;

import io.grpc.Status;
import io.grpc.stub.ServerCallStreamObserver;
import io.grpc.stub.StreamObserver;
import io.quarkus.grpc.GrpcService;
import io.smallrye.common.annotation.Blocking;
import jakarta.annotation.PreDestroy;
import jakarta.inject.Inject;
import java.nio.file.Paths;
import java.util.Map;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.atomic.AtomicBoolean;
import jms_bridge.BridgeServiceGrpc;
import jms_bridge.CompletionRequest;
import jms_bridge.CompletionResponse;
import jms_bridge.HealthRequest;
import jms_bridge.HealthResponse;
import jms_bridge.JmsMessage;
import jms_bridge.SendRequest;
import jms_bridge.SendResponse;
import jms_bridge.SubscribeRequest;
import org.jboss.logging.Logger;

@GrpcService
@Blocking
public class JmsBridgeService extends BridgeServiceGrpc.BridgeServiceImplBase {
  private static final Logger LOG = Logger.getLogger(JmsBridgeService.class);

  @Inject JmsProducer producer;
  @Inject jakarta.enterprise.inject.Instance<JmsConsumer> consumerFactory;
  @Inject JmsClientFactory clientFactory;
  @Inject BridgeConfig config;

  private final ConcurrentHashMap<String, JmsConsumer> activeConsumers = new ConcurrentHashMap<>();
  private final Object shutdownLock = new Object();
  private volatile boolean shutdown = false;

  /**
   * The one shared journal runtime for this bridge process, resolved once from {@link
   * BridgeConfig}. Every queue consumer reuses the same {@link JournalService}, so two
   * subscriptions on one queue domain share one budget and the OS lock is released when the last
   * subscription ends.
   */
  private volatile JournalRuntime journalRuntime;

  private final Object journalRuntimeLock = new Object();

  /**
   * One bridge's shared journal state. When the journal directory or broker key is unset the
   * service holds a {@code missingEnv} name instead: readiness, producers, and topics still work,
   * and only a queue subscribe fails {@code FAILED_PRECONDITION} naming the variable.
   */
  record JournalRuntime(
      JournalService journal,
      String brokerKey,
      String missingEnv,
      long lockWaitMs,
      long maxAttempts) {}

  private volatile boolean lastHealthy = false;
  private volatile long lastHealthCheck = 0L;
  private volatile String lastHealthMessage = "ok";
  private static final long HEALTH_TTL_MS = 10_000L;

  @Override
  public void send(SendRequest request, StreamObserver<SendResponse> responseObserver) {
    try {
      String msgId =
          producer.send(
              request.getDestination(),
              request.getBody().toByteArray(),
              request.getHeadersMap(),
              request.getContentType());
      responseObserver.onNext(
          SendResponse.newBuilder().setMessageId(msgId == null ? "" : msgId).build());
      responseObserver.onCompleted();
      lastHealthy = true;
      lastHealthMessage = "ok";
    } catch (Exception e) {
      LOG.error(
          "JMS send failed for destination '" + request.getDestination() + "': " + e.getMessage(),
          e);
      responseObserver.onError(Status.INTERNAL.withDescription(e.getMessage()).asException());
    }
  }

  @Override
  public void subscribe(SubscribeRequest request, StreamObserver<JmsMessage> responseObserver) {
    JmsConsumer consumer = consumerFactory.get();
    String subId = request.getSubscriptionId();

    JournalRuntime runtime;
    try {
      runtime = journalRuntime();
    } catch (JournalPermanentException e) {
      // A set-but-invalid journal configuration is a startup permanent failure; Phase 4 later
      // signals it before readiness. Here it fails the affected subscribe closed.
      consumerFactory.destroy(consumer);
      responseObserver.onError(
          Status.FAILED_PRECONDITION.withDescription(e.getMessage()).asException());
      return;
    }
    if (!isTopic(request.getDestination()) && runtime.missingEnv() != null) {
      // Unset journal env: readiness, producers, and topics still work; only a queue subscribe
      // fails. The message names the missing variable.
      consumerFactory.destroy(consumer);
      responseObserver.onError(
          Status.FAILED_PRECONDITION
              .withDescription(
                  runtime.missingEnv()
                      + " is not set; queue consumption requires a configured attempt journal")
              .asException());
      return;
    }
    consumer.configureJournal(
        runtime.journal(),
        runtime.brokerKey(),
        runtime.lockWaitMs(),
        runtime.missingEnv(),
        runtime.maxAttempts());

    boolean refused;
    JmsConsumer existing;
    synchronized (shutdownLock) {
      if (shutdown) {
        refused = true;
        existing = null;
      } else {
        refused = false;
        existing = activeConsumers.putIfAbsent(subId, consumer);
      }
    }
    if (refused) {
      // Shutdown raced the registration: destroy the fresh consumer (no leak) and refuse the
      // stream — it never entered the map nor reached the broker.
      consumerFactory.destroy(consumer);
      responseObserver.onError(
          Status.UNAVAILABLE.withDescription("bridge shutting down").asException());
      return;
    }
    if (existing != null) {
      // Duplicate subscription_id: destroy the fresh consumer (no leak) and reject the stream
      // before any cancel-handler registration or broker subscription — the live stream for this
      // subscription_id must stay the sole owner of the map entry.
      consumerFactory.destroy(consumer);
      responseObserver.onError(
          Status.ALREADY_EXISTS
              .withDescription("subscription_id already active: " + subId)
              .asException());
      return;
    }

    // Tracks whether the stream has been terminated (by client cancel, error,
    // or completion). compareAndSet ensures exactly ONE of the three paths
    // wins the race and touches the responseObserver — preventing the
    // StatusRuntimeException that tears down the shared H2 connection.
    AtomicBoolean finished = new AtomicBoolean(false);

    if (responseObserver instanceof ServerCallStreamObserver<JmsMessage> serverObs) {
      serverObs.setOnCancelHandler(() -> cleanupSubscription(consumer, subId, finished));
    }

    // A socket cut drops the TCP connection without an HTTP/2 RST_STREAM, so Vert.x never calls
    // onCancel. TransportCloseFilter signals that premature close through this future; reuse the
    // same finished CAS so cancel, transport close, and normal completion release exactly once.
    CompletableFuture<Void> transportClose = TransportCloseFilter.current();
    if (transportClose != null) {
      transportClose.whenComplete(
          (ignored, failure) -> {
            try {
              cleanupSubscription(consumer, subId, finished);
            } catch (RuntimeException e) {
              // whenComplete would otherwise swallow the failure into a discarded stage.
              LOG.error(
                  "transport-close cleanup failed for subscription '"
                      + subId
                      + "': "
                      + e.getMessage(),
                  e);
            }
          });
    }

    // The transport may already have closed before this consumer reached the map: the callback
    // above ran synchronously and released exactly once. Never subscribe a closed consumer.
    if (finished.get()) {
      return;
    }

    try {
      consumer.subscribe(
          request.getDestination(),
          subId,
          new StreamObserver<>() {
            @Override
            public void onNext(JmsMessage msg) {
              if (!finished.get()) {
                try {
                  responseObserver.onNext(msg);
                } catch (Exception ignored) {
                }
              }
            }

            @Override
            public void onError(Throwable t) {
              if (cleanupSubscription(consumer, subId, finished)) {
                safeRespond(responseObserver, t);
              }
            }

            @Override
            public void onCompleted() {
              if (cleanupSubscription(consumer, subId, finished)) {
                safeComplete(responseObserver);
              }
            }
          },
          finished);
    } catch (IllegalStateException e) {
      // Startup validation failed (ADR-0033): a malformed JMS_MAX_BODY_BYTES must fail loud here,
      // not escape as grpc UNKNOWN — and without leaving this consumer in activeConsumers.
      if (cleanupSubscription(consumer, subId, finished)) {
        LOG.error("JMS subscribe failed for subscription '" + subId + "': " + e.getMessage(), e);
        responseObserver.onError(
            Status.FAILED_PRECONDITION.withDescription(e.getMessage()).asException());
      }
    }
  }

  /**
   * Hand one terminal decision to the subscription's owner thread and reply after the owner
   * settles. The subscription, generation, and delivery token are all validated inside {@link
   * JmsConsumer#complete}; an unknown subscription is rejected without touching any session. This
   * handler never performs a JMS operation itself.
   */
  @Override
  public void complete(
      CompletionRequest request, StreamObserver<CompletionResponse> responseObserver) {
    JmsConsumer consumer = activeConsumers.get(request.getSubscriptionId());
    boolean accepted =
        consumer != null
            && consumer.complete(
                request.getSubscriptionGeneration(),
                request.getDeliveryToken(),
                request.getOutcome());
    responseObserver.onNext(CompletionResponse.newBuilder().setAccepted(accepted).build());
    responseObserver.onCompleted();
  }

  @Override
  public void health(HealthRequest request, StreamObserver<HealthResponse> responseObserver) {
    long now = System.currentTimeMillis();
    if (now - lastHealthCheck > HEALTH_TTL_MS) {
      synchronized (this) {
        now = System.currentTimeMillis();
        if (now - lastHealthCheck > HEALTH_TTL_MS) {
          try {
            clientFactory.checkHealth();
            lastHealthy = true;
            lastHealthMessage = "ok";
          } catch (Exception e) {
            lastHealthy = false;
            lastHealthMessage = summarizeThrowable(e);
            LOG.warn("JMS broker health check failed: " + lastHealthMessage);
          }
          lastHealthCheck = now;
        }
      }
    }

    responseObserver.onNext(
        HealthResponse.newBuilder()
            .setHealthy(lastHealthy)
            .setBrokerConnected(lastHealthy)
            .setMessage(lastHealthMessage)
            .build());
    responseObserver.onCompleted();
  }

  /**
   * Exactly-once teardown for a subscription: wins the {@code finished} race at most once and, on
   * winning AND still owning the map entry, stops and destroys the consumer. Returns whether this
   * call won the CAS (and therefore may terminate the stream response).
   */
  private boolean cleanupSubscription(JmsConsumer consumer, String subId, AtomicBoolean finished) {
    if (!finished.compareAndSet(false, true)) {
      return false;
    }
    // Owner-checked remove gates BOTH stop and destroy: the entry may no longer be ours
    // (e.g. @PreDestroy drained the map and a new owner registered) — the stale teardown
    // must not stop/destroy twice nor evict the new owner's consumer.
    if (activeConsumers.remove(subId, consumer)) {
      consumer.stop();
      consumerFactory.destroy(consumer);
    }
    return true;
  }

  @PreDestroy
  public void shutdown() {
    synchronized (shutdownLock) {
      shutdown = true;
    }
    // Drain entry by entry instead of wiping the map wholesale: every removal is
    // owner-checked and performs exactly one stop+destroy, so a concurrent stream
    // teardown can never double-destroy a consumer.
    while (!activeConsumers.isEmpty()) {
      for (Map.Entry<String, JmsConsumer> e : activeConsumers.entrySet()) {
        if (activeConsumers.remove(e.getKey(), e.getValue())) {
          e.getValue().stop();
          consumerFactory.destroy(e.getValue());
        }
      }
    }
  }

  /**
   * One shared journal runtime per bridge, resolved once and cached. Concurrent first subscribes
   * observe the same {@link JournalService}, so they share one budget per queue domain.
   */
  private JournalRuntime journalRuntime() throws JournalPermanentException {
    JournalRuntime runtime = journalRuntime;
    if (runtime != null) {
      return runtime;
    }
    synchronized (journalRuntimeLock) {
      if (journalRuntime == null) {
        journalRuntime = buildJournalRuntime(config);
      }
      return journalRuntime;
    }
  }

  /**
   * Builds the shared journal runtime from the bridge environment. Unset values yield a runtime
   * that defers the failure to a queue subscribe; set values are validated and produce the one
   * process-wide {@link JournalService}.
   *
   * @param config the bridge configuration; production injects the CDI bean
   * @return the shared runtime
   * @throws JournalPermanentException when a set value is invalid
   */
  static JournalRuntime buildJournalRuntime(BridgeConfig config) throws JournalPermanentException {
    long lockWaitMs = config.journalLockWaitMs();
    long maxAttempts = config.maxDeliveryAttempts();
    String dir = config.attemptJournalDir();
    String key = config.brokerKey();
    if (dir == null || dir.isBlank()) {
      return new JournalRuntime(
          null, null, JournalService.JOURNAL_DIRECTORY_ENV, lockWaitMs, maxAttempts);
    }
    if (key == null || key.isBlank()) {
      return new JournalRuntime(null, null, JournalService.BROKER_KEY_ENV, lockWaitMs, maxAttempts);
    }
    JournalService.validateStartup(dir, key);
    JournalService service =
        new JournalService(
            Paths.get(dir),
            config.journalMaxBytes(),
            config.journalRetentionMs(),
            System::currentTimeMillis);
    return new JournalRuntime(service, key, null, lockWaitMs, maxAttempts);
  }

  /** Whether the destination addresses a topic rather than a queue. */
  private static boolean isTopic(String destination) {
    return destination.startsWith("topic:");
  }

  private static void safeRespond(StreamObserver<JmsMessage> responseObserver, Throwable t) {
    try {
      responseObserver.onError(t);
    } catch (Exception ignored) {
    }
  }

  private static void safeComplete(StreamObserver<JmsMessage> responseObserver) {
    try {
      responseObserver.onCompleted();
    } catch (Exception ignored) {
    }
  }

  private static String summarizeThrowable(Throwable error) {
    if (error == null) {
      return "connection failed";
    }

    StringBuilder summary = new StringBuilder();
    Throwable cur = error;
    int depth = 0;
    while (cur != null && depth < 6) {
      if (depth > 0) {
        summary.append(" <- ");
      }
      summary.append(cur.getClass().getName());
      String msg = cur.getMessage();
      if (msg != null && !msg.isBlank()) {
        summary.append(": ").append(msg);
      }
      cur = cur.getCause();
      depth++;
    }

    if (summary.length() == 0) {
      return "connection failed";
    }
    return summary.toString();
  }
}
