package org.rustcamel.jms;

import io.grpc.Status;
import io.grpc.stub.ServerCallStreamObserver;
import io.grpc.stub.StreamObserver;
import io.quarkus.grpc.GrpcService;
import io.smallrye.common.annotation.Blocking;
import jakarta.annotation.PreDestroy;
import jakarta.inject.Inject;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.atomic.AtomicBoolean;
import jms_bridge.BridgeServiceGrpc;
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

  private final ConcurrentHashMap<String, JmsConsumer> activeConsumers = new ConcurrentHashMap<>();
  private final Object shutdownLock = new Object();
  private volatile boolean shutdown = false;
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
