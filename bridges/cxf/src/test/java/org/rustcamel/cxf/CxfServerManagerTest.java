package org.rustcamel.cxf;

import static org.junit.jupiter.api.Assertions.*;
import static org.mockito.Mockito.*;

import cxf_bridge.ConsumerRequest;
import cxf_bridge.ConsumerResponse;
import io.grpc.stub.StreamObserver;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.CompletionException;
import java.util.concurrent.ExecutionException;
import java.util.concurrent.atomic.AtomicReference;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;
import org.mockito.ArgumentCaptor;
import org.mockito.Mock;
import org.mockito.MockitoAnnotations;

/**
 * Pending-consumer-request ownership across stream registration, replacement, and close (Task 2.7
 * review ses_ec9f34909ffe). Every pending future is owned by the stream it was dispatched on
 * (reference identity): closing a stream fails ONLY its own pending requests — even when it had
 * already been replaced and no longer owns the liveness flag — and never the replacement stream's
 * requests. Cleanup snapshots the owned set, removes it under one lock (conditional removal, so the
 * capacity counter decrements exactly once), and completes the futures OUTSIDE the lock so
 * completion callbacks can reconnect and submit new requests without deadlock or drops.
 */
class CxfServerManagerTest {

  @Mock BridgeConfig bridgeConfig;
  @Mock SoapEndpointPublisher endpointPublisher;

  CxfServerManager manager;

  @BeforeEach
  void setUp() {
    MockitoAnnotations.openMocks(this);
    when(bridgeConfig.maxConcurrentRequests()).thenReturn(2);
    when(bridgeConfig.consumerTimeoutMs()).thenReturn(60_000);
    manager = new CxfServerManager();
    manager.bridgeConfig = bridgeConfig;
    manager.endpointPublisher = endpointPublisher;
  }

  @AfterEach
  void tearDown() {
    manager.cleanup();
  }

  private static ConsumerRequest request(String profile, String id) {
    return ConsumerRequest.newBuilder().setRequestId(id).setSecurityProfile(profile).build();
  }

  private static ConsumerResponse response(String id) {
    return ConsumerResponse.newBuilder().setRequestId(id).setSecurityProfile("v11").build();
  }

  /**
   * Root cause of a failed future (unwraps {@link CompletionException}/{@link ExecutionException}).
   */
  private static Throwable rootCause(CompletableFuture<?> future) {
    Throwable thrown = future.handle((ok, err) -> err).join();
    while ((thrown instanceof CompletionException || thrown instanceof ExecutionException)
        && thrown.getCause() != null) {
      thrown = thrown.getCause();
    }
    return thrown;
  }

  /**
   * Contract (review ses_ec9f34909ffe): stream A holds a pending request, stream B replaces A, then
   * A's stream closes. A's pending future SHALL fail with the typed {@link
   * CxfServerManager.StreamUnavailableException} (which the publisher maps to the HTTP 503
   * transport-availability response — asserted end-to-end in {@code
   * SoapEndpointPublisherTest.lateStreamLossYieldsSame503}), never a sweeper-timeout generic error.
   * B SHALL stay live and its future SHALL NOT be failed by A's close; it completes only on its
   * correlated response. The requestId map entry survives A's close, and capacity is released
   * exactly once (proven by over-admission boundaries at max=2).
   */
  @Test
  void replacedStreamCloseFailsOnlyItsPendingRequests() {
    @SuppressWarnings("unchecked")
    StreamObserver<ConsumerRequest> observerA = mock(StreamObserver.class);
    @SuppressWarnings("unchecked")
    StreamObserver<ConsumerRequest> observerB = mock(StreamObserver.class);

    manager.setConsumerRequestObserver("v11", observerA);
    CompletableFuture<ConsumerResponse> futureA = manager.handleSoapRequest(request("v11", "a"));
    // B replaces A while A's request is in flight; B's request is registered on B.
    manager.setConsumerRequestObserver("v11", observerB);
    CompletableFuture<ConsumerResponse> futureB = manager.handleSoapRequest(request("v11", "b"));

    // A closes AFTER being replaced: it no longer owns the liveness flag (CAS fails), but its
    // own pending future must still fail typed.
    manager.onConsumerStreamClosed("v11", observerA);

    assertTrue(
        futureA.isCompletedExceptionally(),
        "A's pending future must be failed by A's close (not left to the sweeper timeout)");
    Throwable causeA = rootCause(futureA);
    assertInstanceOf(
        CxfServerManager.StreamUnavailableException.class,
        causeA,
        "A's close must fail A's pending future with typed stream unavailability: " + causeA);

    // The newer stream is untouched: still live, its future not failed.
    assertTrue(manager.hasConsumerObserver("v11"), "A's close must not clear B's liveness");
    assertFalse(futureB.isDone(), "B's pending future must not be failed by A's close");

    // Capacity accounting is exact (max=2): A's slot is released exactly once — C is admitted
    // into it, D is rejected as over capacity (a missed release would reject C; a double
    // release would wrongly admit D).
    CompletableFuture<ConsumerResponse> futureC = manager.handleSoapRequest(request("v11", "c"));
    assertFalse(futureC.isDone(), "C must be admitted into A's released slot");
    CompletableFuture<ConsumerResponse> futureD = manager.handleSoapRequest(request("v11", "d"));
    assertTrue(futureD.isCompletedExceptionally(), "D must exceed the max=2 capacity");
    Throwable causeD = rootCause(futureD);
    assertInstanceOf(IllegalStateException.class, causeD, String.valueOf(causeD));
    assertTrue(
        causeD.getMessage().contains("Max concurrent"),
        "capacity rejection must name the limit: " + causeD.getMessage());

    // The pending map is keyed by requestId and survived A's close: B's request completes on
    // its correlated response — and that response completes ONLY B's future.
    manager.completeFromConsumer(response("b"));
    assertTrue(futureB.isDone(), "B's request must complete on its correlated response");
    assertEquals("b", futureB.join().getRequestId());
    assertFalse(futureC.isDone(), "B's response must complete only B's future");

    manager.completeFromConsumer(response("c"));
    assertTrue(futureC.isDone());
    CompletableFuture<ConsumerResponse> futureE = manager.handleSoapRequest(request("v11", "e"));
    assertFalse(
        futureE.isCompletedExceptionally(),
        "all slots must be released after completion (counts back to zero)");
    assertTrue(manager.hasConsumerObserver("v11"), "B must still be live at the end");
  }

  /**
   * Contract (review ses_ec9f34909ffe): when A's close-completion cleanup runs, a completion
   * callback may reconnect as B and submit a new request DURING the cleanup. That request SHALL be
   * retained (not failed, not dropped from the map), B SHALL be live, the correlated response SHALL
   * complete it, and capacity SHALL be zero afterwards (proven by admission at max=1).
   * Deterministic: the completion callback runs inline on the completing thread — no sleeps or
   * polling.
   */
  @Test
  void reconnectDuringPendingCleanupPreservesNewRequests() {
    when(bridgeConfig.maxConcurrentRequests()).thenReturn(1);
    @SuppressWarnings("unchecked")
    StreamObserver<ConsumerRequest> observerA = mock(StreamObserver.class);
    @SuppressWarnings("unchecked")
    StreamObserver<ConsumerRequest> observerB = mock(StreamObserver.class);

    manager.setConsumerRequestObserver("v11", observerA);
    CompletableFuture<ConsumerResponse> futureA = manager.handleSoapRequest(request("v11", "a"));

    AtomicReference<CompletableFuture<ConsumerResponse>> reconnected = new AtomicReference<>();
    futureA.whenComplete(
        (ok, err) -> {
          // Runs DURING A's close-completion cleanup (inline on the completing thread,
          // outside the manager lock): the stream reconnects as B and submits a new request
          // before cleanup returns.
          manager.setConsumerRequestObserver("v11", observerB);
          reconnected.set(manager.handleSoapRequest(request("v11", "d")));
        });

    manager.onConsumerStreamClosed("v11", observerA);

    assertTrue(futureA.isCompletedExceptionally(), "A's pending future must be failed");
    assertInstanceOf(
        CxfServerManager.StreamUnavailableException.class,
        rootCause(futureA),
        "A's close must fail typed");
    assertTrue(
        manager.hasConsumerObserver("v11"), "B must be live after reconnecting during cleanup");

    CompletableFuture<ConsumerResponse> futureD = reconnected.get();
    assertNotNull(futureD, "the reconnecting stream must have submitted its request");
    assertFalse(
        futureD.isDone(),
        "B's request registered during cleanup must be retained, not failed or dropped");

    // The map entry was not wiped by A's cleanup: the correlated response completes it.
    manager.completeFromConsumer(response("d"));
    assertTrue(futureD.isDone(), "B's request must complete on its correlated response");
    assertEquals("d", futureD.join().getRequestId());
    verify(observerB).onNext(argThat(r -> "d".equals(r.getRequestId())));
    verify(observerA, never()).onNext(argThat(r -> "d".equals(r.getRequestId())));

    // Counts are zero after completion: with max=1 a new request is admitted only if D's slot
    // was released exactly once.
    CompletableFuture<ConsumerResponse> futureE = manager.handleSoapRequest(request("v11", "e"));
    assertFalse(futureE.isCompletedExceptionally(), "capacity must be zero after completion");
  }

  @SuppressWarnings("unchecked")
  private static StreamObserver<ConsumerRequest> observerMock() {
    return mock(StreamObserver.class);
  }

  private static java.util.List<String> receivedIds(StreamObserver<ConsumerRequest> observer) {
    ArgumentCaptor<ConsumerRequest> captor = ArgumentCaptor.forClass(ConsumerRequest.class);
    verify(observer, atLeast(0)).onNext(captor.capture());
    return captor.getAllValues().stream().map(ConsumerRequest::getRequestId).toList();
  }

  @Test
  void secondProfileStreamDoesNotReplaceFirst() {
    StreamObserver<ConsumerRequest> a = observerMock();
    StreamObserver<ConsumerRequest> b = observerMock();

    manager.setConsumerRequestObserver("v11", a);
    manager.setConsumerRequestObserver("v12", b);
    manager.handleSoapRequest(request("v11", "x"));

    assertTrue(manager.hasConsumerObserver("v11"));
    assertTrue(manager.hasConsumerObserver("v12"));
    verify(a, times(1)).onNext(any());
    verify(b, never()).onNext(any());
  }

  @Test
  void requestDispatchesOnlyToItsProfileStream() {
    StreamObserver<ConsumerRequest> a = observerMock();
    StreamObserver<ConsumerRequest> b = observerMock();
    manager.setConsumerRequestObserver("v11", a);
    manager.setConsumerRequestObserver("v12", b);

    manager.handleSoapRequest(request("v11", "r1"));
    manager.handleSoapRequest(request("v12", "r2"));

    assertEquals(java.util.List.of("r1"), receivedIds(a));
    assertEquals(java.util.List.of("r2"), receivedIds(b));
  }

  @Test
  void missingProfileStreamFailsWithoutFallback() {
    StreamObserver<ConsumerRequest> b = observerMock();
    manager.setConsumerRequestObserver("v12", b);

    CompletableFuture<ConsumerResponse> missing = manager.handleSoapRequest(request("v11", "r"));

    assertTrue(missing.isCompletedExceptionally());
    Throwable cause = rootCause(missing);
    assertInstanceOf(CxfServerManager.StreamUnavailableException.class, cause);
    assertTrue(cause.getMessage().contains("v11"), String.valueOf(cause.getMessage()));
    verify(b, never()).onNext(any());

    // The rejected v11 request released its capacity slot: both v12 requests fit at max=2.
    assertFalse(manager.handleSoapRequest(request("v12", "s1")).isDone());
    assertFalse(manager.handleSoapRequest(request("v12", "s2")).isDone());
  }

  @Test
  void closingOneProfileFailsOnlyItsRequests() {
    StreamObserver<ConsumerRequest> a = observerMock();
    StreamObserver<ConsumerRequest> b = observerMock();
    manager.setConsumerRequestObserver("v11", a);
    manager.setConsumerRequestObserver("v12", b);
    CompletableFuture<ConsumerResponse> fa = manager.handleSoapRequest(request("v11", "a"));
    CompletableFuture<ConsumerResponse> fb = manager.handleSoapRequest(request("v12", "b"));

    manager.onConsumerStreamClosed("v11", a);

    assertTrue(fa.isCompletedExceptionally());
    assertInstanceOf(CxfServerManager.StreamUnavailableException.class, rootCause(fa));
    assertFalse(fb.isDone());
    assertFalse(manager.hasConsumerObserver("v11"));
    assertTrue(manager.hasConsumerObserver("v12"));

    manager.completeFromConsumer(response("b"));
    assertTrue(fb.isDone());
    assertEquals("b", fb.join().getRequestId());
  }

  @Test
  void cleanupClearsEveryProfile() {
    manager.setConsumerRequestObserver("v11", observerMock());
    manager.setConsumerRequestObserver("v12", observerMock());

    manager.cleanup();

    assertFalse(manager.hasConsumerObserver("v11"));
    assertFalse(manager.hasConsumerObserver("v12"));
  }
}
