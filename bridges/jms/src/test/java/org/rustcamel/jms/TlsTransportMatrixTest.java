package org.rustcamel.jms;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertTrue;
import static org.junit.jupiter.api.Assertions.fail;

import java.io.IOException;
import java.lang.reflect.Field;
import java.net.InetAddress;
import java.net.ServerSocket;
import java.net.URI;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.Callable;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.ExecutionException;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.Future;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.TimeoutException;
import javax.net.ssl.KeyManager;
import javax.net.ssl.TrustManager;
import org.apache.activemq.broker.SslContext;
import org.apache.activemq.transport.Transport;
import org.apache.activemq.transport.TransportFactory;
import org.apache.activemq.transport.TransportListener;
import org.apache.activemq.transport.failover.FailoverTransport;
import org.junit.jupiter.api.DisplayName;
import org.junit.jupiter.api.Test;

/**
 * In-process TLS matrix for the JMS bridge transport seam (task 7.1): the client side is the real
 * bridge path — {@link JmsClientFactory#registerTransportFactories()} plus {@link
 * TransportFactory#findTransportFactory} over the bridge-owned {@link CamelSslTransportFactory},
 * whose {@code createSocketFactory} override builds the {@link BrokerTlsIdentitySocketFactory} from
 * the production seams (ActiveMQ {@link SslContext} thread-local for trust material, {@link
 * BridgeActiveMqConnectionFactory.TlsConnectionState} for the broker identity name) — and the
 * server side is {@link TlsTestServer} over PEM fixtures. Failures are asserted through {@link
 * BrokerFailureClassifier#classify}.
 *
 * <p>CI-owned rows (stated here as the module docs for this matrix): the platform-trust
 * <em>positive</em> against a public CA cannot be simulated hermetically — only the hermetic
 * negative ({@code platformTrustFailsPrivateCa}) runs here — and the FULL native matrix (real
 * broker, GraalVM native image) lives in the CI smoke module, not in this JVM-level suite.
 */
@DisplayName("JMS bridge in-process TLS matrix")
class TlsTransportMatrixTest {

  private static final String MQ_INTERNAL = "mq.internal.example";
  private static final long BOUND_SECONDS = 10;

  // ---- fixtures ----

  private static String fixture(String name) {
    try {
      return Path.of(TlsTransportMatrixTest.class.getResource("/tls-fixtures/" + name).toURI())
          .toString();
    } catch (Exception e) {
      throw new IllegalStateException("missing tls fixture: " + name, e);
    }
  }

  private static TrustManager[] testCaTrust() {
    return BrokerTlsMaterial.buildTrustManagers(fixture("test-ca.pem"));
  }

  private static KeyManager[] clientIdentity() {
    return BrokerTlsMaterial.buildKeyManagers(
        fixture("client-cert.pem"), fixture("client-key-pkcs8.pem"));
  }

  // ---- production-seam scopes ----

  /**
   * Trust-material seam: ActiveMQ's {@code ActiveMQSslConnectionFactory.createTransport} installs
   * an {@link SslContext} around transport construction and {@code FailoverTransport} captures it
   * for later member creation — the test installs the same thread-local the same way. {@code
   * pinned} mirrors the {@code tls} table branch; {@code platformTrust} mirrors its absence
   * (platform default trust).
   */
  private static final class SslContextScope implements AutoCloseable {
    private final SslContext previous = SslContext.getCurrentSslContext();

    static SslContextScope pinned(KeyManager[] keyManagers, TrustManager[] trustManagers) {
      SslContextScope scope = new SslContextScope();
      SslContext.setCurrentSslContext(
          new SslContext(keyManagers, trustManagers, BrokerTlsMaterial.newSecureRandom()));
      return scope;
    }

    static SslContextScope platformTrust() {
      SslContextScope scope = new SslContextScope();
      SslContext.setCurrentSslContext(null);
      return scope;
    }

    @Override
    public void close() {
      SslContext.setCurrentSslContext(previous);
    }
  }

  /**
   * TLS identity-state seam: {@link BridgeActiveMqConnectionFactory#createConnection(String,
   * String)} publishes {@code TlsConnectionState} on a private {@link InheritableThreadLocal}
   * before transport construction. The test publishes the same state the same way ("mirroring what
   * createConnection establishes"); the holder field is private, so the test reaches it
   * reflectively and keeps the production seam untouched.
   */
  private static final class TlsStateScope implements AutoCloseable {
    private final InheritableThreadLocal<BridgeActiveMqConnectionFactory.TlsConnectionState> holder;
    private final BridgeActiveMqConnectionFactory.TlsConnectionState previous;

    static TlsStateScope published(String serverName) {
      return new TlsStateScope(serverName);
    }

    @SuppressWarnings("unchecked")
    private TlsStateScope(String serverName) {
      try {
        Field field =
            BridgeActiveMqConnectionFactory.class.getDeclaredField("TLS_CONNECTION_STATE");
        field.setAccessible(true);
        this.holder =
            (InheritableThreadLocal<BridgeActiveMqConnectionFactory.TlsConnectionState>)
                field.get(null);
      } catch (ReflectiveOperationException e) {
        throw new IllegalStateException("cannot reach TlsConnectionState holder", e);
      }
      this.previous = holder.get();
      holder.set(new BridgeActiveMqConnectionFactory.TlsConnectionState(serverName, true));
    }

    @Override
    public void close() {
      if (previous == null) {
        holder.remove();
      } else {
        holder.set(previous);
      }
    }
  }

  // ---- client-side transport dial ----

  /** Client-side handshake outcome: both failure channels of the transport path. */
  private record ClientOutcome(
      Throwable startFailure, CompletableFuture<Throwable> listenerFailure, Transport transport) {
    /**
     * Every handshake failure the client observed. The handshake fires on the transport's first
     * write ({@code WireFormatNegotiator.start}) racing the transport read thread, so a failed
     * handshake surfaces either synchronously from {@code start()} or asynchronously through {@link
     * TransportListener#onException} — both channels are captured and either may carry the
     * classifiable chain.
     */
    List<Throwable> awaitFailures() throws Exception {
      List<Throwable> failures = new ArrayList<>();
      if (startFailure != null) {
        failures.add(startFailure);
      }
      try {
        Throwable listener = listenerFailure.get(BOUND_SECONDS, TimeUnit.SECONDS);
        if (listener != null && !failures.contains(listener)) {
          failures.add(listener);
        }
      } catch (TimeoutException noneArrived) {
        // listener channel silent — the synchronous failure (if any) is all there is
      }
      return failures;
    }
  }

  /** Listener capturing transport failures and the failover resume signal. */
  private static final class CapturingListener implements TransportListener {
    private final CompletableFuture<Throwable> failure = new CompletableFuture<>();
    private final CompletableFuture<Void> resumed = new CompletableFuture<>();

    @Override
    public void onCommand(Object command) {}

    @Override
    public void onException(IOException error) {
      failure.complete(error);
    }

    @Override
    public void transportInterupted() {}

    @Override
    public void transportResumed() {
      resumed.complete(null);
    }
  }

  /** Dials {@code ssl://127.0.0.1:<port>} through the real bridge transport seam. */
  private static ClientOutcome dial(int port) throws Exception {
    return dial(new URI("ssl://127.0.0.1:" + port));
  }

  /** Dials {@code uri} through the real bridge transport seam. */
  private static ClientOutcome dial(URI uri) throws Exception {
    JmsClientFactory.registerTransportFactories();
    Transport transport = TransportFactory.findTransportFactory(uri).doConnect(uri);
    CapturingListener listener = new CapturingListener();
    transport.setTransportListener(listener);
    Throwable startFailure = null;
    try {
      transport.start();
    } catch (Exception error) {
      startFailure = error;
    }
    return new ClientOutcome(startFailure, listener.failure, transport);
  }

  /** Failure classification over both transport channels; contradictory TLS classes fail hard. */
  private static void assertFailureClassified(BrokerFailureClass expected, ClientOutcome outcome)
      throws Exception {
    List<Throwable> failures = outcome.awaitFailures();
    assertFalse(failures.isEmpty(), "expected a client-side handshake failure");
    boolean matched = false;
    for (Throwable failure : failures) {
      BrokerFailureClass actual = BrokerFailureClassifier.classify(failure);
      if (actual == expected) {
        matched = true;
      } else if (actual.name().startsWith("TLS_")) {
        fail(
            "contradictory classification: expected "
                + expected
                + " but classified "
                + actual
                + " for "
                + failure);
      }
    }
    assertTrue(
        matched, () -> "expected " + expected + " but classified " + describeFailures(failures));
  }

  private static String describeFailures(List<Throwable> failures) {
    StringBuilder text = new StringBuilder();
    for (Throwable failure : failures) {
      text.append(BrokerFailureClassifier.classify(failure))
          .append(": ")
          .append(failure)
          .append("; ");
    }
    return text.toString();
  }

  /** Server-side handshake completed with exactly the requested SNI names. */
  private static void assertHandshakeSucceeded(ClientOutcome outcome, TlsTestServer server)
      throws Exception {
    assertNull(
        outcome.startFailure(), () -> "unexpected client failure: " + outcome.startFailure());
    List<String> names =
        server.connection(0).requestedServerNames().get(BOUND_SECONDS, TimeUnit.SECONDS);
    assertNotNull(names, "server-side handshake did not complete");
  }

  private static int reserveClosedPort() throws IOException {
    try (ServerSocket probe = new ServerSocket(0, 1, InetAddress.getByName("127.0.0.1"))) {
      return probe.getLocalPort();
    }
  }

  // ---- matrix cases ----

  @Test
  void dnsSanByIpWithoutServerNameFails() throws Exception {
    try (TlsTestServer server =
            TlsTestServer.start(
                fixture("server-dns-san.pem"),
                fixture("server-dns-san.key"),
                fixture("test-ca.pem"),
                false);
        SslContextScope trust = SslContextScope.pinned(null, testCaTrust());
        TlsStateScope state = TlsStateScope.published(null)) {
      ClientOutcome outcome = dial(server.port());
      try {
        assertFailureClassified(BrokerFailureClass.TLS_HOSTNAME, outcome);
      } finally {
        outcome.transport().stop();
      }
    }
  }

  @Test
  void dnsSanByIpWithServerNamePasses() throws Exception {
    try (TlsTestServer server =
            TlsTestServer.start(
                fixture("server-dns-san.pem"),
                fixture("server-dns-san.key"),
                fixture("test-ca.pem"),
                false);
        SslContextScope trust = SslContextScope.pinned(null, testCaTrust());
        TlsStateScope state = TlsStateScope.published(MQ_INTERNAL)) {
      ClientOutcome outcome = dial(server.port());
      try {
        assertHandshakeSucceeded(outcome, server);
        // The server-side listener capture proves the delegate-forwarded SNI correction reached
        // the real handshake — exactly the configured identity name, nothing else.
        assertEquals(
            List.of(MQ_INTERNAL),
            server.connection(0).requestedServerNames().get(BOUND_SECONDS, TimeUnit.SECONDS));
      } finally {
        outcome.transport().stop();
      }
    }
  }

  @Test
  void wrongServerNameFailsEvenWhenDialedHostWouldMatch() throws Exception {
    // ip-san cert matches the dialed 127.0.0.1, but the configured identity name relabels the peer
    // host and pins SNI to other.example — it must fail through the REAL transport path.
    try (TlsTestServer server =
            TlsTestServer.start(
                fixture("server-ip-san.pem"),
                fixture("server-ip-san.key"),
                fixture("test-ca.pem"),
                false);
        SslContextScope trust = SslContextScope.pinned(null, testCaTrust());
        TlsStateScope state = TlsStateScope.published("other.example")) {
      ClientOutcome outcome = dial(server.port());
      try {
        assertFailureClassified(BrokerFailureClass.TLS_HOSTNAME, outcome);
      } finally {
        outcome.transport().stop();
      }
    }
  }

  @Test
  void wrongServerNameViaLocalBindOverloadFails() throws Exception {
    // Same matrix as wrongServerNameFailsEvenWhenDialedHostWouldMatch — ip-san cert matches the
    // dialed 127.0.0.1, the configured identity name pins other.example — but dialed through the
    // local-bind createSocket overload: the transport URI's path binds the local side to loopback
    // ephemeral port 0, so TcpTransport.connect() takes
    // createSocket(InetAddress, int, InetAddress, int) instead of createSocket(InetAddress, int).
    // That overload must relabel the dialed peer exactly like the connected-socket path: SNI
    // re-assertion alone leaves the JDK's fallback-to-dialed-peer-host identity check open, and the
    // IP-only certificate would be ACCEPTED (expert-reproduced hole).
    try (TlsTestServer server =
            TlsTestServer.start(
                fixture("server-ip-san.pem"),
                fixture("server-ip-san.key"),
                fixture("test-ca.pem"),
                false);
        SslContextScope trust = SslContextScope.pinned(null, testCaTrust());
        TlsStateScope state = TlsStateScope.published("other.example")) {
      ClientOutcome outcome = dial(new URI("ssl://127.0.0.1:" + server.port() + "/127.0.0.1:0"));
      try {
        assertFailureClassified(BrokerFailureClass.TLS_HOSTNAME, outcome);
      } finally {
        outcome.transport().stop();
      }
    }
  }

  @Test
  void ipSanByIpPasses() throws Exception {
    try (TlsTestServer server =
            TlsTestServer.start(
                fixture("server-ip-san.pem"),
                fixture("server-ip-san.key"),
                fixture("test-ca.pem"),
                false);
        SslContextScope trust = SslContextScope.pinned(null, testCaTrust());
        TlsStateScope state = TlsStateScope.published(null)) {
      ClientOutcome outcome = dial(server.port());
      try {
        assertHandshakeSucceeded(outcome, server);
      } finally {
        outcome.transport().stop();
      }
    }
  }

  @Test
  void wrongCaFailsClosed() throws Exception {
    // server-other-name is signed by other-ca; the client pins test-ca only.
    try (TlsTestServer server =
            TlsTestServer.start(
                fixture("server-other-name.pem"),
                fixture("server-other-name.key"),
                fixture("other-ca.pem"),
                false);
        SslContextScope trust = SslContextScope.pinned(null, testCaTrust());
        TlsStateScope state = TlsStateScope.published(null)) {
      ClientOutcome outcome = dial(server.port());
      try {
        assertFailureClassified(BrokerFailureClass.TLS_TRUST, outcome);
      } finally {
        outcome.transport().stop();
      }
    }
  }

  @Test
  void platformTrustFailsPrivateCa() throws Exception {
    // No tls table semantics at all: platform default trust, which does not know the private CA.
    try (TlsTestServer server =
            TlsTestServer.start(
                fixture("server-dns-san.pem"),
                fixture("server-dns-san.key"),
                fixture("test-ca.pem"),
                false);
        SslContextScope trust = SslContextScope.platformTrust();
        TlsStateScope state = TlsStateScope.published(null)) {
      ClientOutcome outcome = dial(server.port());
      try {
        assertFailureClassified(BrokerFailureClass.TLS_TRUST, outcome);
      } finally {
        outcome.transport().stop();
      }
    }
  }

  @Test
  void needClientAuthWithoutClientCertFails() throws Exception {
    try (TlsTestServer server =
            TlsTestServer.start(
                fixture("server-ip-san.pem"),
                fixture("server-ip-san.key"),
                fixture("test-ca.pem"),
                true);
        SslContextScope trust = SslContextScope.pinned(null, testCaTrust());
        TlsStateScope state = TlsStateScope.published(null)) {
      ClientOutcome outcome = dial(server.port());
      try {
        assertFailureClassified(BrokerFailureClass.TLS_CLIENT_AUTH, outcome);
        // the server socket must have observed the handshake failure too
        Throwable serverFailure =
            server.connection(0).handshakeFailure().get(BOUND_SECONDS, TimeUnit.SECONDS);
        assertNotNull(serverFailure, "server observed no handshake failure");
      } finally {
        outcome.transport().stop();
      }
    }
  }

  @Test
  void needClientAuthWithClientCertPasses() throws Exception {
    try (TlsTestServer server =
            TlsTestServer.start(
                fixture("server-ip-san.pem"),
                fixture("server-ip-san.key"),
                fixture("test-ca.pem"),
                true);
        SslContextScope trust = SslContextScope.pinned(clientIdentity(), testCaTrust());
        TlsStateScope state = TlsStateScope.published(null)) {
      ClientOutcome outcome = dial(server.port());
      try {
        assertHandshakeSucceeded(outcome, server);
      } finally {
        outcome.transport().stop();
      }
    }
  }

  @Test
  void plainServerOnTlsClientFailsPlaintextPeer() throws Exception {
    try (TlsTestServer server = TlsTestServer.startPlain();
        SslContextScope trust = SslContextScope.pinned(null, testCaTrust());
        TlsStateScope state = TlsStateScope.published(null)) {
      ClientOutcome outcome = dial(server.port());
      try {
        assertFailureClassified(BrokerFailureClass.TLS_PLAINTEXT_PEER, outcome);
      } finally {
        outcome.transport().stop();
      }
    }
  }

  @Test
  void ordinaryFailoverReachesSecondMember() throws Exception {
    int deadPort = reserveClosedPort();
    try (TlsTestServer server =
            TlsTestServer.start(
                fixture("server-ip-san.pem"),
                fixture("server-ip-san.key"),
                fixture("test-ca.pem"),
                false);
        SslContextScope trust = SslContextScope.pinned(null, testCaTrust());
        TlsStateScope state = TlsStateScope.published(null)) {
      URI uri =
          new URI(
              "failover:(ssl://127.0.0.1:"
                  + deadPort
                  + ",ssl://127.0.0.1:"
                  + server.port()
                  + ")?randomize=false");
      JmsClientFactory.registerTransportFactories();
      Transport transport = TransportFactory.findTransportFactory(uri).doCompositeConnect(uri);
      try {
        // PRODUCTION guard through the shared seam — not a test-local reimplementation
        BridgeActiveMqConnectionFactory.applyCompositeTlsPolicy(transport, true);
        FailoverTransport failover = transport.narrow(FailoverTransport.class);
        assertNotNull(failover, "composite yields a FailoverTransport");
        CapturingListener listener = new CapturingListener();
        transport.setTransportListener(listener);
        transport.start();
        // dead member refuses (fast on loopback), failover cycles to the live TLS member
        listener.resumed.get(BOUND_SECONDS, TimeUnit.SECONDS);
        assertFalse(failover.isUpdateURIsSupported(), "updateURIs still disabled after failover");
        assertFalse(failover.isReconnectSupported(), "reconnect still disabled after failover");
      } finally {
        transport.stop();
      }
    }
  }

  @Test
  void failoverSecondMemberReceivesServerName() throws Exception {
    int deadPort = reserveClosedPort();
    try (TlsTestServer server =
            TlsTestServer.start(
                fixture("server-dns-san.pem"),
                fixture("server-dns-san.key"),
                fixture("test-ca.pem"),
                false);
        SslContextScope trust = SslContextScope.pinned(null, testCaTrust());
        TlsStateScope state = TlsStateScope.published(MQ_INTERNAL)) {
      URI uri =
          new URI(
              "failover:(ssl://127.0.0.1:"
                  + deadPort
                  + ",ssl://127.0.0.1:"
                  + server.port()
                  + ")?randomize=false");
      JmsClientFactory.registerTransportFactories();
      Transport transport = TransportFactory.findTransportFactory(uri).doCompositeConnect(uri);
      try {
        BridgeActiveMqConnectionFactory.applyCompositeTlsPolicy(transport, true);
        CapturingListener listener = new CapturingListener();
        transport.setTransportListener(listener);
        transport.start();
        listener.resumed.get(BOUND_SECONDS, TimeUnit.SECONDS);
        // The live server only ever sees the SECOND member; its captured requested SNI proves the
        // InheritableThreadLocal state survived asynchronous failover member creation.
        assertEquals(
            List.of(MQ_INTERNAL),
            server.connection(0).requestedServerNames().get(BOUND_SECONDS, TimeUnit.SECONDS));
      } finally {
        transport.stop();
      }
    }
  }

  @Test
  void serverNameThroughProductionFactoryPath() throws Exception {
    // REAL trust material (the test-CA-signed server would otherwise be rejected by platform trust
    // before identity matters — that rejection stays covered by platformTrustFailsPrivateCa); the
    // server completes the TLS handshake and then closes so createConnection terminates on
    // wire-format negotiation instead of blocking against a peer that speaks no OpenWire.
    try (TlsTestServer server =
        TlsTestServer.start(
            fixture("server-dns-san.pem"),
            fixture("server-dns-san.key"),
            fixture("test-ca.pem"),
            false,
            true)) {
      JmsClientFactory.registerTransportFactories();
      BridgeActiveMqConnectionFactory factory =
          new BridgeActiveMqConnectionFactory(
              "ssl://127.0.0.1:" + server.port(), null, testCaTrust(), MQ_INTERNAL);
      ExecutorService caller = Executors.newSingleThreadExecutor();
      try {
        Callable<javax.jms.Connection> create = factory::createConnection;
        Future<javax.jms.Connection> attempt = caller.submit(create);
        try {
          javax.jms.Connection connection = attempt.get(20, TimeUnit.SECONDS);
          connection.close();
        } catch (ExecutionException accepted) {
          // post-handshake close surfaces as a client exception — the state-establishment segment
          // is already proven by the server-side capture below
        }
        assertEquals(
            List.of(MQ_INTERNAL),
            server.connection(0).requestedServerNames().get(BOUND_SECONDS, TimeUnit.SECONDS));
      } finally {
        caller.shutdownNow();
      }
    }
  }
}
