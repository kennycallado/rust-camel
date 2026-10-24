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
 * <p>Plain mode ({@link #startPlain()}): a non-TLS {@link ServerSocket} that reads the client's
 * first bytes, then answers with non-TLS bytes — the plaintext peer a TLS client must classify as
 * {@code TLS_PLAINTEXT_PEER}. The answer drains before closing: closing with unread inbound data
 * makes the stack send RST, and on Windows an incoming RST discards already-received bytes, so the
 * client would see {@code SocketException} instead of the plaintext 400. The read captures byte 0
 * in {@link Connection#firstClientByte} (0x16 = TLS handshake record) and {@code shutdownOutput()}
 * sends a graceful FIN before the drain-and-close.
 *
 * <p>Ports are always ephemeral (bind {@code 127.0.0.1:0}); {@link #close()} closes every accepted
 * socket and the server socket. The accept thread is a daemon and needs no join.
 */
final class TlsTestServer implements AutoCloseable {

  /**
   * Per accepted connection: the requested SNI names, the server-side handshake failure, and the
   * first client byte captured by the plain-mode answer loop ({@code -1} when the peer closed or
   * errored before a byte arrived).
   */
  record Connection(
      CompletableFuture<List<String>> requestedServerNames,
      CompletableFuture<Throwable> handshakeFailure,
      CompletableFuture<Integer> firstClientByte) {}

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

  /** Bounds the ClientHello read and the post-answer drain of the plain-mode answer loop. */
  private static final int ANSWER_TIMEOUT_MS = 10_000;

  private void answerPlain(Socket socket) {
    accepted.add(socket);
    Connection connection =
        new Connection(
            new CompletableFuture<>(), new CompletableFuture<>(), new CompletableFuture<>());
    connections.add(connection);
    try {
      socket.setSoTimeout(ANSWER_TIMEOUT_MS);
      // Read the client's first bytes BEFORE answering (a TLS record header is 5 bytes). Byte 0
      // identifies the dialect: 0x16 = TLS handshake record, proving the client did not fall back
      // to plaintext OpenWire.
      int first = socket.getInputStream().read();
      connection.firstClientByte().complete(first);
      int total = first == -1 ? 0 : 1;
      while (total >= 1 && total < 5) {
        int next = socket.getInputStream().read();
        if (next == -1) {
          break;
        }
        total++;
      }
      socket
          .getOutputStream()
          .write("HTTP/1.1 400 Bad Request\r\n\r\n".getBytes(StandardCharsets.US_ASCII));
      socket.getOutputStream().flush();
      // Graceful FIN (never SO_LINGER 0): the client must be able to read the buffered 400 before
      // the connection dies — closing with unread inbound data would send RST, and on Windows an
      // incoming RST discards the already-buffered 400.
      socket.shutdownOutput();
      drainQuietly(socket);
    } catch (IOException gone) {
      // peer already gone or reset mid-exchange — complete the capture so the assertion fails
      // fast with a visible value instead of hanging on the future
      connection.firstClientByte().complete(-1);
    } finally {
      closeQuietly(socket);
    }
  }

  /**
   * Reads the peer's remaining bytes until EOF, timeout, or error, so close() sends FIN not RST.
   */
  private static void drainQuietly(Socket socket) {
    try {
      while (socket.getInputStream().read() != -1) {
        // discard — the drain is the point, not the bytes
      }
    } catch (IOException ignore) {
      // the peer's FIN is the exit we want; a reset or timeout ends the drain just as well
    }
  }

  private void handleTls(SSLSocket socket) {
    accepted.add(socket);
    Connection connection =
        new Connection(
            new CompletableFuture<>(), new CompletableFuture<>(), new CompletableFuture<>());
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
