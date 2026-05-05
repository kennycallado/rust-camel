package org.rustcamel.cxf;

import cxf_bridge.ConsumerRequest;
import cxf_bridge.ConsumerResponse;
import io.grpc.stub.StreamObserver;
import jakarta.annotation.PreDestroy;
import jakarta.enterprise.context.ApplicationScoped;
import jakarta.inject.Inject;
import java.time.Duration;
import java.time.Instant;
import java.util.Map;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.Executors;
import java.util.concurrent.ScheduledExecutorService;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.logging.Level;
import java.util.logging.Logger;

@ApplicationScoped
public class CxfServerManager {
  private static final Logger LOG = Logger.getLogger(CxfServerManager.class.getName());

  static final class PendingRequest {
    final String requestId;
    final CompletableFuture<ConsumerResponse> future;
    final Instant createdAt;

    PendingRequest(
        String requestId, CompletableFuture<ConsumerResponse> future, Instant createdAt) {
      this.requestId = requestId;
      this.future = future;
      this.createdAt = createdAt;
    }
  }

  private final Map<String, PendingRequest> pendingRequests = new ConcurrentHashMap<>();
  private final AtomicInteger inFlightCount = new AtomicInteger();
  private final AtomicBoolean cleanedUp = new AtomicBoolean(false);
  private final ScheduledExecutorService sweeper;
  private volatile StreamObserver<ConsumerRequest> consumerRequestObserver;

  @Inject BridgeConfig bridgeConfig;

  @Inject SoapEndpointPublisher endpointPublisher;

  public CxfServerManager() {
    this.sweeper =
        Executors.newSingleThreadScheduledExecutor(
            r -> {
              Thread t = new Thread(r, "cxf-consumer-sweeper");
              t.setDaemon(true);
              return t;
            });
    this.sweeper.scheduleAtFixedRate(this::evictStaleRequests, 10, 10, TimeUnit.SECONDS);
  }

  /**
   * Forwards an incoming SOAP consumer request (from {@link SoapEndpointPublisher}) to the active
   * gRPC consumer stream and returns a future completed by {@link #completeFromConsumer}.
   */
  public CompletableFuture<ConsumerResponse> handleSoapRequest(ConsumerRequest request) {
    String requestId = request.getRequestId();
    if (requestId == null || requestId.isBlank()) {
      return CompletableFuture.failedFuture(new IllegalArgumentException("request_id is required"));
    }

    int maxConcurrentRequests = bridgeConfig.maxConcurrentRequests();
    if (!tryIncrementInFlight(maxConcurrentRequests)) {
      return CompletableFuture.failedFuture(
          new IllegalStateException("Max concurrent requests exceeded: " + maxConcurrentRequests));
    }

    CompletableFuture<ConsumerResponse> future = new CompletableFuture<>();
    PendingRequest pending = new PendingRequest(requestId, future, Instant.now());
    PendingRequest existing = pendingRequests.putIfAbsent(requestId, pending);
    if (existing != null) {
      inFlightCount.decrementAndGet();
      return CompletableFuture.failedFuture(
          new IllegalStateException("Duplicate request_id: " + requestId));
    }

    future.whenComplete((ok, err) -> completeRequest(requestId));

    StreamObserver<ConsumerRequest> observer = consumerRequestObserver;
    if (observer == null) {
      future.completeExceptionally(new IllegalStateException("503: no consumer stream connected"));
      return future;
    }

    try {
      observer.onNext(request);
    } catch (Exception e) {
      LOG.log(
          Level.WARNING,
          "Failed to dispatch consumer request {0}: {1}",
          new Object[] {requestId, e.getMessage()});
      future.completeExceptionally(new IllegalStateException("503: failed to dispatch request", e));
    }

    return future;
  }

  public void setConsumerRequestObserver(StreamObserver<ConsumerRequest> observer) {
    this.consumerRequestObserver = observer;
    endpointPublisher.publish();
  }

  public void completeFromConsumer(ConsumerResponse response) {
    PendingRequest pending = pendingRequests.get(response.getRequestId());
    if (pending != null) {
      pending.future.complete(response);
    }
  }

  public void cleanup() {
    if (!cleanedUp.compareAndSet(false, true)) {
      return;
    }
    for (PendingRequest pending : pendingRequests.values()) {
      pending.future.completeExceptionally(
          new IllegalStateException("503: consumer stream closed"));
    }
    pendingRequests.clear();
    consumerRequestObserver = null;
    inFlightCount.set(0);
    endpointPublisher.stop();
    sweeper.shutdownNow();
  }

  @PreDestroy
  void onDestroy() {
    cleanup();
  }

  private boolean tryIncrementInFlight(int maxConcurrentRequests) {
    while (true) {
      int current = inFlightCount.get();
      if (current >= maxConcurrentRequests) {
        return false;
      }
      if (inFlightCount.compareAndSet(current, current + 1)) {
        return true;
      }
    }
  }

  private void completeRequest(String requestId) {
    PendingRequest removed = pendingRequests.remove(requestId);
    if (removed != null) {
      inFlightCount.decrementAndGet();
    }
  }

  private void evictStaleRequests() {
    Instant cutoff = Instant.now().minus(Duration.ofMillis(bridgeConfig.consumerTimeoutMs()));
    for (PendingRequest pending : pendingRequests.values()) {
      if (pending.createdAt.isBefore(cutoff)) {
        pending.future.completeExceptionally(new IllegalStateException("504: request timed out"));
      }
    }
  }
}
