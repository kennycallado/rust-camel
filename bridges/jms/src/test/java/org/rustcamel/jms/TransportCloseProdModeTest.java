package org.rustcamel.jms;

import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertTrue;
import static org.mockito.ArgumentMatchers.any;
import static org.mockito.ArgumentMatchers.anyString;
import static org.mockito.Mockito.doAnswer;
import static org.mockito.Mockito.mock;
import static org.mockito.Mockito.times;
import static org.mockito.Mockito.verify;
import static org.mockito.Mockito.when;

import io.grpc.ManagedChannel;
import io.grpc.ManagedChannelBuilder;
import io.grpc.stub.StreamObserver;
import io.quarkus.test.common.http.TestHTTPResource;
import io.quarkus.test.junit.QuarkusTest;
import io.quarkus.test.junit.QuarkusTestProfile;
import io.quarkus.test.junit.TestProfile;
import jakarta.enterprise.inject.Any;
import jakarta.enterprise.inject.Instance;
import jakarta.inject.Inject;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.net.InetAddress;
import java.net.ServerSocket;
import java.net.Socket;
import java.net.URI;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicReference;
import jms_bridge.BridgeServiceGrpc;
import jms_bridge.JmsMessage;
import jms_bridge.SubscribeRequest;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;

/**
 * Spike 5.2a.1 (rc-icvzl): the transport-close future produced by {@link TransportCloseFilter} must
 * reach the {@code @Blocking} {@link JmsBridgeService} worker and complete ONLY on a premature
 * socket cut (TCP drop without an HTTP/2 {@code RST_STREAM}) — never on a normal stream end.
 *
 * <p>The profile turns on the production unified server ({@code use-separate-server=false}), so
 * gRPC is served by the main HTTP server and the filter runs on the request path. The consumer is
 * mocked so the subscribe stream stays in flight; the mock's {@code subscribe} callback runs on the
 * same {@code @Blocking} worker as the real service and captures {@link
 * TransportCloseFilter#current()} there, proving the context local crossed the HTTP-to-worker
 * boundary.
 *
 * <p>Two scenarios: (1) a raw TCP proxy is cut, and the captured future must complete within 2 s;
 * (2) the RPC completes normally, and the captured future must stay pending (Vert.x fires the
 * response {@code closeHandler} on a normal HTTP/2 close too, so the filter distinguishes via
 * {@code response().ended()}). Task 5.2a.2 adds the cleanup wiring and the cleanup assertions.
 */
@QuarkusTest
@TestProfile(TransportCloseProdModeTest.ProdModeProfile.class)
class TransportCloseProdModeTest {

  /** Production-shaped unified server: gRPC shares the main HTTP server. */
  public static class ProdModeProfile implements QuarkusTestProfile {
    @Override
    public Map<String, String> getConfigOverrides() {
      return Map.of(
          "quarkus.grpc.server.use-separate-server", "false",
          "quarkus.grpc.server.plain-text", "true",
          "quarkus.http.test-port", "0",
          "quarkus.http.test-ssl-port", "0",
          "quarkus.http.insecure-requests", "enabled",
          "quarkus.http.ssl-port", "-1",
          "quarkus.http.ssl.client-auth", "none");
    }
  }

  @Inject @Any JmsBridgeService service;

  @TestHTTPResource("/")
  URI baseUri;

  private final AtomicReference<String> workerThread = new AtomicReference<>();
  private final AtomicReference<CompletableFuture<Void>> workerFuture = new AtomicReference<>();
  private Instance<JmsConsumer> capturingFactory;
  private JmsConsumer capturingConsumer;

  /**
   * Counts down on the mock {@code stop()} and {@code destroy()} callbacks so both cleanup effects
   * can be awaited under a single deadline, with no fixed sleep and no per-verify timeout race.
   */
  private CountDownLatch stopAndDestroy;

  private TcpCutProxy proxy;

  @BeforeEach
  void startProxy() throws IOException {
    workerThread.set(null);
    workerFuture.set(null);
    stopAndDestroy = new CountDownLatch(2);
    proxy = new TcpCutProxy(baseUri.getPort());
  }

  @AfterEach
  void tearDown() {
    if (proxy != null) {
      proxy.close();
    }
  }

  /**
   * Installs a capturing consumer factory. The callback runs on the {@code @Blocking} {@link
   * JmsBridgeService} worker. {@code workerThread} is published before {@code workerFuture} so that
   * observing a non-null future also means a non-null thread name (safe release).
   *
   * @param completeNormally when true the mock ends the stream normally, exercising the
   *     normal-completion path instead of leaving the stream in flight
   */
  @SuppressWarnings("unchecked")
  private void installBlockingConsumer(boolean completeNormally) {
    Instance<JmsConsumer> factory = mock(Instance.class);
    JmsConsumer consumer = mock(JmsConsumer.class);
    this.capturingFactory = factory;
    this.capturingConsumer = consumer;
    when(factory.get()).thenReturn(consumer);
    doAnswer(
            invocation -> {
              StreamObserver<JmsMessage> observer = invocation.getArgument(2);
              workerThread.set(Thread.currentThread().getName());
              workerFuture.set(TransportCloseFilter.current());
              if (completeNormally) {
                observer.onCompleted();
              }
              return null;
            })
        .when(consumer)
        .subscribe(anyString(), anyString(), any(), any());
    doAnswer(
            invocation -> {
              stopAndDestroy.countDown();
              return null;
            })
        .when(consumer)
        .stop();
    doAnswer(
            invocation -> {
              stopAndDestroy.countDown();
              return null;
            })
        .when(factory)
        .destroy(consumer);
    service.consumerFactory = factory;
  }

  @Test
  void transportCloseProdModeDetectsSocketCut() throws Exception {
    installBlockingConsumer(false);
    ManagedChannel channel =
        ManagedChannelBuilder.forAddress("127.0.0.1", proxy.port()).usePlaintext().build();
    try {
      subscribe(channel);

      CompletableFuture<Void> transportClose = awaitWorkerFuture(5, TimeUnit.SECONDS);
      assertNotNull(
          transportClose, "context-local transport future must reach the @Blocking worker");
      assertFalse(
          workerThread.get().contains("eventloop"),
          () -> "subscribe must run on the @Blocking worker, thread was " + workerThread.get());
      System.out.println(
          "TransportCloseProdModeTest @Blocking worker thread: " + workerThread.get());
      assertFalse(
          transportClose.isDone(), "future must stay pending while the stream is in flight");

      // One deadline for the whole cut-to-release contract, measured just before the cut.
      long cutDeadlineNanos = System.nanoTime() + TimeUnit.SECONDS.toNanos(2);
      proxy.cut();

      transportClose.get(remainingNanos(cutDeadlineNanos), TimeUnit.NANOSECONDS);
      assertTrue(transportClose.isDone(), "socket cut must complete the transport future");
      assertTrue(
          stopAndDestroy.await(remainingNanos(cutDeadlineNanos), TimeUnit.NANOSECONDS),
          "stop and destroy must both run within 2 s of the cut");

      // The wiring must release the subscription exactly once, not only complete the signal.
      verify(capturingConsumer, times(1)).stop();
      verify(capturingFactory, times(1)).destroy(capturingConsumer);
    } finally {
      channel.shutdownNow().awaitTermination(5, TimeUnit.SECONDS);
    }
  }

  @Test
  void normalGrpcStreamCompletionLeavesTransportFuturePending() throws Exception {
    installBlockingConsumer(true);
    ManagedChannel channel =
        ManagedChannelBuilder.forAddress("127.0.0.1", proxy.port()).usePlaintext().build();
    try {
      CompletableFuture<Void> rpcCompletion = subscribe(channel);

      CompletableFuture<Void> transportClose = awaitWorkerFuture(5, TimeUnit.SECONDS);
      assertNotNull(
          transportClose, "context-local transport future must reach the @Blocking worker");
      // Complete only on a TRUE successful onCompleted: onError completes exceptionally, so get()
      // throws ExecutionException and fails the test instead of counting an error as success.
      rpcCompletion.get(5, TimeUnit.SECONDS);

      // Vert.x fires the response closeHandler on a normal HTTP/2 close too; the future must stay
      // pending because the response ended normally (response().ended() == true).
      assertStillPending(transportClose, 1_000);
    } finally {
      channel.shutdownNow().awaitTermination(5, TimeUnit.SECONDS);
    }
  }

  /**
   * Starts the subscribe RPC. The returned future completes normally only on a genuine {@code
   * onCompleted} callback; an {@code onError} completes it exceptionally so callers cannot mistake
   * an RPC failure for a normal stream end.
   */
  private CompletableFuture<Void> subscribe(ManagedChannel channel) {
    CompletableFuture<Void> terminal = new CompletableFuture<>();
    BridgeServiceGrpc.newStub(channel)
        .subscribe(
            SubscribeRequest.newBuilder()
                .setDestination("topic:transport-close-spike")
                .setSubscriptionId("spike-1")
                .build(),
            new StreamObserver<>() {
              @Override
              public void onNext(JmsMessage value) {}

              @Override
              public void onError(Throwable t) {
                terminal.completeExceptionally(t);
              }

              @Override
              public void onCompleted() {
                terminal.complete(null);
              }
            });
    return terminal;
  }

  private CompletableFuture<Void> awaitWorkerFuture(long timeout, TimeUnit unit)
      throws InterruptedException {
    long deadline = System.nanoTime() + unit.toNanos(timeout);
    while (System.nanoTime() < deadline) {
      CompletableFuture<Void> future = workerFuture.get();
      if (future != null) {
        return future;
      }
      Thread.sleep(20);
    }
    return null;
  }

  private static void assertStillPending(CompletableFuture<Void> future, long millis)
      throws InterruptedException {
    long deadline = System.nanoTime() + TimeUnit.MILLISECONDS.toNanos(millis);
    while (System.nanoTime() < deadline) {
      assertFalse(
          future.isDone(), "normal stream completion must NOT complete the transport-close future");
      Thread.sleep(25);
    }
  }

  /** Remaining nanos to a deadline, floored at 1 ns so a timed wait never gets a zero value. */
  private static long remainingNanos(long deadlineNanos) {
    return Math.max(1L, deadlineNanos - System.nanoTime());
  }

  /**
   * Minimal raw TCP relay in front of the unified server. {@link #cut()} closes every tracked
   * socket with a plain {@code close()} (graceful FIN, no {@code SO_LINGER(0)}): the bridge sees
   * the connection drop without any HTTP/2 {@code RST_STREAM}.
   */
  private static final class TcpCutProxy implements AutoCloseable {
    private final ServerSocket server;
    private final int backendPort;
    private final List<Socket> sockets = new ArrayList<>();
    private volatile boolean running = true;

    TcpCutProxy(int backendPort) throws IOException {
      this.backendPort = backendPort;
      this.server = new ServerSocket(0, 50, InetAddress.getLoopbackAddress());
      Thread acceptor = new Thread(this::acceptLoop, "tcp-cut-proxy");
      acceptor.setDaemon(true);
      acceptor.start();
    }

    int port() {
      return server.getLocalPort();
    }

    private void acceptLoop() {
      while (running) {
        try {
          Socket client = server.accept();
          Socket backend = new Socket(InetAddress.getLoopbackAddress(), backendPort);
          synchronized (sockets) {
            sockets.add(client);
            sockets.add(backend);
          }
          pump(client, backend);
          pump(backend, client);
        } catch (IOException e) {
          return;
        }
      }
    }

    private void pump(Socket from, Socket to) {
      Thread thread =
          new Thread(
              () -> {
                byte[] buffer = new byte[8192];
                try (InputStream in = from.getInputStream();
                    OutputStream out = to.getOutputStream()) {
                  int read;
                  while ((read = in.read(buffer)) >= 0) {
                    out.write(buffer, 0, read);
                    out.flush();
                  }
                } catch (IOException ignored) {
                  // Socket cut: the peer closed underneath the pump.
                }
              },
              "tcp-cut-proxy-pump");
      thread.setDaemon(true);
      thread.start();
    }

    void cut() {
      synchronized (sockets) {
        for (Socket socket : sockets) {
          try {
            socket.close();
          } catch (IOException ignored) {
          }
        }
        sockets.clear();
      }
    }

    @Override
    public void close() {
      running = false;
      cut();
      try {
        server.close();
      } catch (IOException ignored) {
      }
    }
  }
}
