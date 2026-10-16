package org.rustcamel.cxf;

import cxf_bridge.ConsumerRequest;
import cxf_bridge.ConsumerResponse;
import io.grpc.stub.StreamObserver;
import io.quarkus.runtime.StartupEvent;
import jakarta.annotation.PreDestroy;
import jakarta.enterprise.context.ApplicationScoped;
import jakarta.enterprise.event.Observes;
import jakarta.inject.Inject;
import java.time.Duration;
import java.time.Instant;
import java.util.ArrayList;
import java.util.List;
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

  /**
   * The consumer stream is not connected (or died mid-request). The publisher maps this typed
   * failure to the HTTP 503 transport-availability response (Retry-After, plaintext, no SOAP fault)
   * — never to the generic Receiver 500 fault path.
   */
  public static final class StreamUnavailableException extends Exception {
    private static final long serialVersionUID = 1L;

    public StreamUnavailableException(String message) {
      super(message);
    }

    public StreamUnavailableException(String message, Throwable cause) {
      super(message, cause);
    }
  }

  /**
   * One in-flight consumer request. {@code owner} is the stream identity (reference equality) the
   * request was registered on: closing a stream fails exactly its own pending requests — never the
   * requests of a replacement stream, and never by a blanket map clear.
   */
  static final class PendingRequest {
    final String requestId;
    final CompletableFuture<ConsumerResponse> future;
    final Instant createdAt;
    final StreamObserver<ConsumerRequest> owner;

    PendingRequest(
        String requestId,
        CompletableFuture<ConsumerResponse> future,
        Instant createdAt,
        StreamObserver<ConsumerRequest> owner) {
      this.requestId = requestId;
      this.future = future;
      this.createdAt = createdAt;
      this.owner = owner;
    }
  }

  private final Map<String, PendingRequest> pendingRequests = new ConcurrentHashMap<>();
  private final AtomicInteger inFlightCount = new AtomicInteger();
  private final AtomicBoolean cleanedUp = new AtomicBoolean(false);
  private final ScheduledExecutorService sweeper;

  /**
   * The ONE small lock coordinating the consumer-request observer and pending registration and
   * removal. Victim snapshots are taken and entries are removed under the lock (conditional {@link
   * Map#remove(Object, Object)} releases the capacity counter exactly once per registration);
   * futures are always completed OUTSIDE the lock so completion callbacks can reconnect, register,
   * and submit new requests without deadlocking or being dropped.
   */
  private final Object lock = new Object();

  /**
   * The live consumer-request observer of the current {@code OpenConsumerStream} RPC. Replaced by
   * identity on close ({@link #onConsumerStreamClosed}) so a canceled older stream can never clear
   * a newer replacement stream's liveness. Guarded by {@link #lock}.
   */
  private StreamObserver<ConsumerRequest> consumerRequestObserver;

  @Inject BridgeConfig bridgeConfig;

  @Inject SoapEndpointPublisher endpointPublisher;

  @Inject CxfClientManager clientManager;

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
   * Eager startup validation (ADR-0033): load and validate every profile's producer transport TLS
   * stores before the gRPC server announces {@code ready}. {@code CxfServerManager} is an
   * application-scoped bean, so a plain {@code @PostConstruct} would run lazily on first use —
   * after the ready line. Observing {@link StartupEvent} forces this bean to be constructed during
   * startup, before the HTTP verticle binds and PortAnnouncer emits the ready line. A store that
   * cannot be opened (wrong password, missing file) throws here and aborts Quarkus startup, so the
   * supervisor observes stdout EOF before any {@code {"status":"ready"}} line. Lazy per-dispatch
   * opening is not acceptable evidence of readiness.
   */
  void initTlsStores(@Observes StartupEvent event) {
    clientManager.initTlsStores();
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

    // Registration is coordinated with observer replacement and stream close under one lock;
    // the pending entry records the owning stream identity (reference ==) it is dispatched on.
    PendingRequest registered;
    StreamObserver<ConsumerRequest> observer;
    Throwable rejected;
    synchronized (lock) {
      if (cleanedUp.get()) {
        rejected = new StreamUnavailableException("bridge shutting down");
        registered = null;
        observer = null;
        inFlightCount.decrementAndGet();
      } else {
        observer = consumerRequestObserver;
        if (observer == null) {
          rejected = new StreamUnavailableException("no consumer stream connected");
          registered = null;
          inFlightCount.decrementAndGet();
        } else {
          PendingRequest candidate = new PendingRequest(requestId, future, Instant.now(), observer);
          if (pendingRequests.putIfAbsent(requestId, candidate) != null) {
            rejected = new IllegalStateException("Duplicate request_id: " + requestId);
            registered = null;
            observer = null;
            inFlightCount.decrementAndGet();
          } else {
            rejected = null;
            registered = candidate;
          }
        }
      }
    }

    if (rejected != null) {
      return CompletableFuture.failedFuture(rejected);
    }

    // Capacity and the map entry are released EXACTLY ONCE on any completion (response, timeout,
    // owning-stream close, dispatch failure) via conditional removal keyed by this instance.
    PendingRequest pending = registered;
    future.whenComplete((ok, err) -> releasePending(pending));

    try {
      observer.onNext(request);
    } catch (Exception e) {
      LOG.log(
          Level.WARNING,
          "Failed to dispatch consumer request {0}: {1}",
          new Object[] {requestId, e.getMessage()});
      future.completeExceptionally(new StreamUnavailableException("failed to dispatch request", e));
    }

    return future;
  }

  /**
   * Stores the consumer-request observer of a newly opened {@code OpenConsumerStream} RPC. It does
   * NOT publish the SOAP listener: publication happens synchronously at startup (Task 2.7), so
   * {@code BridgeProcess::start} can observe {@code soap-ready} before any consumer RPC exists.
   * Replacement never touches pending registrations: the replaced stream's requests stay owned by
   * it and fail on its close.
   */
  public void setConsumerRequestObserver(StreamObserver<ConsumerRequest> observer) {
    synchronized (lock) {
      consumerRequestObserver = observer;
    }
  }

  /** True only while a live consumer-request stream is connected. */
  public boolean hasConsumerObserver() {
    synchronized (lock) {
      return consumerRequestObserver != null;
    }
  }

  /**
   * A consumer stream closed or failed: clear its liveness (only when it is still the current
   * observer) and fail the requests THIS stream owns with the typed {@link
   * StreamUnavailableException} (HTTP 503 transport-availability) — irrespective of whether the
   * liveness swap succeeded. A replaced, older stream's close therefore fails only its own
   * in-flight requests and can never clear or fail the replacement stream's requests. The SOAP
   * listener stays published — new requests take the pre-stream {@code 503} until another {@code
   * OpenConsumerStream} arrives.
   */
  public void onConsumerStreamClosed(StreamObserver<ConsumerRequest> observer) {
    List<PendingRequest> victims = new ArrayList<>();
    synchronized (lock) {
      if (consumerRequestObserver == observer) {
        consumerRequestObserver = null;
      }
      for (PendingRequest pending : pendingRequests.values()) {
        if (pending.owner == observer) {
          victims.add(pending);
        }
      }
      for (PendingRequest pending : victims) {
        releasePendingLocked(pending);
      }
    }
    // Completing OUTSIDE the lock: completion callbacks may reconnect and submit new requests
    // without deadlocking, and those new registrations are never part of this cleanup.
    for (PendingRequest pending : victims) {
      pending.future.completeExceptionally(
          new StreamUnavailableException("consumer stream closed"));
    }
  }

  public void completeFromConsumer(ConsumerResponse response) {
    PendingRequest pending = pendingRequests.get(response.getRequestId());
    if (pending != null) {
      // Completion triggers releasePending via whenComplete: capacity freed exactly once.
      pending.future.complete(response);
    }
  }

  /**
   * Bean shutdown ({@code @PreDestroy}): stop new registrations and fail every in-flight request,
   * then tear down the SOAP listener. Preserves the baseline teardown role of this method.
   */
  public void cleanup() {
    if (!cleanedUp.compareAndSet(false, true)) {
      return;
    }
    List<PendingRequest> victims;
    synchronized (lock) {
      consumerRequestObserver = null;
      victims = new ArrayList<>(pendingRequests.values());
      for (PendingRequest pending : victims) {
        releasePendingLocked(pending);
      }
    }
    for (PendingRequest pending : victims) {
      pending.future.completeExceptionally(new StreamUnavailableException("bridge shutting down"));
    }
    endpointPublisher.stop();
    sweeper.shutdownNow();
  }

  /** Conditional removal + capacity release for one registration (call under {@link #lock}). */
  private void releasePendingLocked(PendingRequest pending) {
    if (pendingRequests.remove(pending.requestId, pending)) {
      inFlightCount.decrementAndGet();
    }
  }

  /** {@link #releasePendingLocked} under the lock; no-op on repeat calls (exactly-once release). */
  private void releasePending(PendingRequest pending) {
    synchronized (lock) {
      releasePendingLocked(pending);
    }
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

  /**
   * Times out stale requests by completing their futures; the per-registration release happens
   * through {@code whenComplete} → {@link #releasePending} (conditional removal, exactly once) —
   * never a chunked or global clear that could bypass stream ownership.
   */
  private void evictStaleRequests() {
    Instant cutoff = Instant.now().minus(Duration.ofMillis(bridgeConfig.consumerTimeoutMs()));
    for (PendingRequest pending : pendingRequests.values()) {
      if (pending.createdAt.isBefore(cutoff)) {
        pending.future.completeExceptionally(new IllegalStateException("504: request timed out"));
      }
    }
  }
}
