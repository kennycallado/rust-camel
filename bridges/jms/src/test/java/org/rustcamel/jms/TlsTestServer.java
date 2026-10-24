package org.rustcamel.jms;

import java.io.IOException;
import java.net.InetAddress;
import java.net.ServerSocket;
import java.net.Socket;
import java.nio.charset.StandardCharsets;
import java.security.GeneralSecurityException;
import java.util.ArrayList;
import java.util.Collections;
import java.util.List;
import java.util.concurrent.CompletableFuture;
import javax.net.ssl.ExtendedSSLSession;
import javax.net.ssl.HandshakeCompletedListener;
import javax.net.ssl.KeyManager;
import javax.net.ssl.SNIHostName;
import javax.net.ssl.SNIServerName;
import javax.net.ssl.SSLContext;
import javax.net.ssl.SSLServerSocket;
import javax.net.ssl.SSLSession;
import javax.net.ssl.SSLSocket;
import javax.net.ssl.TrustManager;

/**
 * In-process server harness for {@link TlsTransportMatrixTest} (task 7.1).
 *
 * <p>TLS mode: an {@link SSLServerSocket} on an ephemeral {@code 127.0.0.1:0} port, its identity
 * key managers built from the chosen server fixture pair and its trust managers from the given CA
 * bundle (only consulted when {@code needClientAuth} is set) — all PEM material through {@link
 * BrokerTlsMaterial}. Each accepted socket gets a {@link HandshakeCompletedListener} that captures
 * the client-requested SNI names from {@link ExtendedSSLSession#getRequestedServerNames()} — that
 * cast is where requested SNI is readable, and the capture runs in the listener where the completed
 * session is available. The accept thread then drives {@link SSLSocket#startHandshake()} and
 * records any failure. With {@code closeAfterHandshake} the accepted socket closes as soon as the
 * handshake completes, so a JMS-level client terminates on wire-format negotiation instead of
 * blocking on a peer that speaks no OpenWire.
 *
 * <p>Plain mode ({@link #startPlain()}): a non-TLS {@link ServerSocket} that answers with non-TLS
 * bytes — the plaintext peer a TLS client must classify as {@code TLS_PLAINTEXT_PEER}.
 *
 * <p>Ports are always ephemeral (bind {@code 127.0.0.1:0}); {@link #close()} closes every accepted
 * socket and the server socket. The accept thread is a daemon and needs no join.
 */
final class TlsTestServer implements AutoCloseable {

  /** Per accepted connection: the requested SNI names and the server-side handshake failure. */
  record Connection(
      CompletableFuture<List<String>> requestedServerNames,
      CompletableFuture<Throwable> handshakeFailure) {}

  private final ServerSocket serverSocket;
  private final boolean plain;
  private final boolean closeAfterHandshake;
  private final List<Socket> accepted = Collections.synchronizedList(new ArrayList<>());
  private final List<Connection> connections = Collections.synchronizedList(new ArrayList<>());

  private TlsTestServer(ServerSocket serverSocket, boolean plain, boolean closeAfterHandshake) {
    this.serverSocket = serverSocket;
    this.plain = plain;
    this.closeAfterHandshake = closeAfterHandshake;
    Thread acceptThread =
        new Thread(this::acceptLoop, "tls-test-server-" + serverSocket.getLocalPort());
    acceptThread.setDaemon(true);
    acceptThread.start();
  }

  static TlsTestServer start(
      String serverCertPem, String serverKeyPem, String trustCaPem, boolean needClientAuth)
      throws IOException {
    return start(serverCertPem, serverKeyPem, trustCaPem, needClientAuth, false);
  }

  static TlsTestServer start(
      String serverCertPem,
      String serverKeyPem,
      String trustCaPem,
      boolean needClientAuth,
      boolean closeAfterHandshake)
      throws IOException {
    KeyManager[] keyManagers = BrokerTlsMaterial.buildKeyManagers(serverCertPem, serverKeyPem);
    TrustManager[] trustManagers = BrokerTlsMaterial.buildTrustManagers(trustCaPem);
    SSLContext context;
    try {
      context = SSLContext.getInstance("TLS");
      context.init(keyManagers, trustManagers, BrokerTlsMaterial.newSecureRandom());
    } catch (GeneralSecurityException e) {
      throw new IllegalStateException("cannot build in-process TLS server context", e);
    }
    SSLServerSocket serverSocket =
        (SSLServerSocket)
            context
                .getServerSocketFactory()
                .createServerSocket(0, 16, InetAddress.getByName("127.0.0.1"));
    serverSocket.setNeedClientAuth(needClientAuth);
    return new TlsTestServer(serverSocket, false, closeAfterHandshake);
  }

  /** Non-TLS server that answers every connection with an HTTP 400 (non-TLS) response. */
  static TlsTestServer startPlain() throws IOException {
    ServerSocket serverSocket = new ServerSocket(0, 16, InetAddress.getByName("127.0.0.1"));
    return new TlsTestServer(serverSocket, true, true);
  }

  int port() {
    return serverSocket.getLocalPort();
  }

  /** Connection records in accept order. */
  Connection connection(int index) {
    synchronized (connections) {
      return connections.get(index);
    }
  }

  private void acceptLoop() {
    while (!serverSocket.isClosed()) {
      Socket socket;
      try {
        socket = serverSocket.accept();
      } catch (IOException closed) {
        return;
      }
      if (plain) {
        answerPlain(socket);
        continue;
      }
      handleTls((SSLSocket) socket);
    }
  }

  private void answerPlain(Socket socket) {
    accepted.add(socket);
    try {
      socket
          .getOutputStream()
          .write("HTTP/1.1 400 Bad Request\r\n\r\n".getBytes(StandardCharsets.US_ASCII));
      socket.getOutputStream().flush();
    } catch (IOException gone) {
      // peer already gone — teardown race, nothing to record
    } finally {
      closeQuietly(socket);
    }
  }

  private void handleTls(SSLSocket socket) {
    accepted.add(socket);
    Connection connection = new Connection(new CompletableFuture<>(), new CompletableFuture<>());
    connections.add(connection);
    HandshakeCompletedListener capture =
        event -> {
          SSLSession session = event.getSession();
          List<String> names = new ArrayList<>();
          for (SNIServerName name : ((ExtendedSSLSession) session).getRequestedServerNames()) {
            names.add(((SNIHostName) name).getAsciiName());
          }
          connection.requestedServerNames().complete(List.copyOf(names));
        };
    socket.addHandshakeCompletedListener(capture);
    try {
      socket.startHandshake();
    } catch (IOException failure) {
      connection.handshakeFailure().complete(failure);
    }
    if (closeAfterHandshake) {
      closeQuietly(socket);
    }
  }

  @Override
  public void close() {
    closeQuietly(serverSocket);
    synchronized (accepted) {
      for (Socket socket : accepted) {
        closeQuietly(socket);
      }
      accepted.clear();
    }
  }

  private static void closeQuietly(Socket socket) {
    try {
      socket.close();
    } catch (IOException ignore) {
      // teardown is best-effort
    }
  }

  private static void closeQuietly(ServerSocket socket) {
    try {
      socket.close();
    } catch (IOException ignore) {
      // teardown is best-effort
    }
  }
}
