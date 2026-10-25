package org.rustcamel.jms;

import java.io.EOFException;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.net.InetAddress;
import java.net.ServerSocket;
import java.net.Socket;
import java.net.SocketException;
import java.net.SocketTimeoutException;
import java.nio.ByteBuffer;
import java.nio.charset.StandardCharsets;
import java.security.GeneralSecurityException;
import java.util.ArrayList;
import java.util.Collections;
import java.util.List;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.Executors;
import java.util.concurrent.RejectedExecutionException;
import java.util.concurrent.ScheduledExecutorService;
import java.util.concurrent.ScheduledFuture;
import java.util.concurrent.TimeUnit;
import javax.net.ssl.ExtendedSSLSession;
import javax.net.ssl.HandshakeCompletedListener;
import javax.net.ssl.KeyManager;
import javax.net.ssl.SNIHostName;
import javax.net.ssl.SNIServerName;
import javax.net.ssl.SSLContext;
import javax.net.ssl.SSLEngine;
import javax.net.ssl.SSLEngineResult;
import javax.net.ssl.SSLException;
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
 * records any failure. A failed handshake tears down gracefully — bounded drain, then
 * shutdownOutput, then close (the same doctrine the plain-mode answer follows) — so the client can
 * still read the fatal alert instead of a bare connection abort. With {@code closeAfterHandshake}
 * the accepted socket closes as soon as the handshake completes, so a JMS-level client terminates
 * on wire-format negotiation instead of blocking on a peer that speaks no OpenWire.
 *
 * <p>Engine-driven mode ({@link #startEngine}): a plain {@link ServerSocket} plus a per-connection
 * server-mode {@link SSLEngine} built from the same {@link BrokerTlsMaterial} key and trust
 * managers, with {@code setNeedClientAuth(true)}. This path exists for the client-auth row: JSSE's
 * {@link SSLSocket#startHandshake()} closes the transport inside the call when it raises a fatal
 * error, so a fixture built on it cannot flush the engine-generated fatal alert and drain the peer
 * gracefully — the behaviour the native release configuration (GraalVM 25, separate process)
 * exposes. Owning the unwrap/wrap loop lets this fixture complete {@link
 * Connection#handshakeFailure()} with the engine's own exception, flush the engine's pending fatal
 * alert (the only correct way to emit it: TLS 1.3 protects the alert record with the active
 * handshake keys, so a synthetic plaintext record would be rejected as a decode error), half-close
 * the output, and drain inbound data under a 15s overall deadline. The deadline is only real
 * because a per-connection watchdog closes the accepted socket at that instant: the checks that
 * precede each read and write cannot interrupt a write already blocked in the kernel (a stalled
 * peer with a zero TCP window), and {@code SO_TIMEOUT} bounds reads only. The forced close makes
 * that blocked call fail with a {@link SocketException}, which the existing catch paths already
 * treat as teardown; the watchdog is cancelled once the connection completes. A post-deadline
 * forced close may leave inbound bytes unread — the already-documented deadline exception to the
 * graceful-close doctrine. SNI is snapshotted from {@link SSLEngine#getHandshakeSession()} right
 * after the ClientHello is consumed, because no completed-session listener fires on that path. Only
 * {@code needClientAuthWithoutClientCertFails} uses this path; every other row stays on the JSSE
 * TLS or plain path.
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

  /** Bounds the handshake read, the plain-mode answer loop, and every engine-path read. */
  private static final int ANSWER_TIMEOUT_MS = 10_000;

  /** Overall budget for the engine-driven handshake and its graceful teardown. */
  private static final long ENGINE_DEADLINE_MS = 15_000L;

  private static final System.Logger LOG = System.getLogger(TlsTestServer.class.getName());

  /**
   * Per accepted connection: the requested SNI names, the server-side handshake failure, and the
   * first client byte captured by the plain-mode answer loop ({@code -1} when the peer closed or
   * errored before a byte arrived, and always {@code -1} on the engine-driven path, which never
   * reads plaintext).
   */
  record Connection(
      CompletableFuture<List<String>> requestedServerNames,
      CompletableFuture<Throwable> handshakeFailure,
      CompletableFuture<Integer> firstClientByte) {}

  private final ServerSocket serverSocket;
  private final boolean plain;
  private final boolean closeAfterHandshake;
  private final SSLContext engineContext;

  /**
   * Engine-path deadline enforcement: a daemon single-thread scheduler whose only job is to close a
   * connection's socket when its overall deadline expires, so a write blocked in the kernel cannot
   * outlive the budget. {@code null} on the JSSE and plain paths.
   */
  private final ScheduledExecutorService engineWatchdog;

  private final List<Socket> accepted = Collections.synchronizedList(new ArrayList<>());
  private final List<Connection> connections = Collections.synchronizedList(new ArrayList<>());

  private TlsTestServer(
      ServerSocket serverSocket,
      boolean plain,
      boolean closeAfterHandshake,
      SSLContext engineContext) {
    this.serverSocket = serverSocket;
    this.plain = plain;
    this.closeAfterHandshake = closeAfterHandshake;
    this.engineContext = engineContext;
    this.engineWatchdog =
        engineContext == null
            ? null
            : Executors.newSingleThreadScheduledExecutor(
                runnable -> {
                  Thread thread =
                      new Thread(
                          runnable, "tls-test-server-watchdog-" + serverSocket.getLocalPort());
                  thread.setDaemon(true);
                  return thread;
                });
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
    SSLContext context = buildServerContext(keyManagers, trustCaPem);
    SSLServerSocket serverSocket =
        (SSLServerSocket)
            context
                .getServerSocketFactory()
                .createServerSocket(0, 16, InetAddress.getByName("127.0.0.1"));
    serverSocket.setNeedClientAuth(needClientAuth);
    return new TlsTestServer(serverSocket, false, closeAfterHandshake, null);
  }

  /**
   * Server-mode {@link SSLEngine} path: a plain server socket whose accepted connection is driven
   * by an engine, so this fixture — not JSSE — owns the fatal-alert flush and the graceful
   * shutdown. Always requires a client certificate.
   */
  static TlsTestServer startEngine(String serverCertPem, String serverKeyPem, String trustCaPem)
      throws IOException {
    KeyManager[] keyManagers = BrokerTlsMaterial.buildKeyManagers(serverCertPem, serverKeyPem);
    SSLContext context = buildServerContext(keyManagers, trustCaPem);
    ServerSocket serverSocket = new ServerSocket(0, 16, InetAddress.getByName("127.0.0.1"));
    return new TlsTestServer(serverSocket, false, false, context);
  }

  private static SSLContext buildServerContext(KeyManager[] keyManagers, String trustCaPem) {
    TrustManager[] trustManagers = BrokerTlsMaterial.buildTrustManagers(trustCaPem);
    try {
      SSLContext context = SSLContext.getInstance("TLS");
      context.init(keyManagers, trustManagers, BrokerTlsMaterial.newSecureRandom());
      return context;
    } catch (GeneralSecurityException e) {
      throw new IllegalStateException("cannot build in-process TLS server context", e);
    }
  }

  /** Non-TLS server that answers every connection with an HTTP 400 (non-TLS) response. */
  static TlsTestServer startPlain() throws IOException {
    ServerSocket serverSocket = new ServerSocket(0, 16, InetAddress.getByName("127.0.0.1"));
    return new TlsTestServer(serverSocket, true, true, null);
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
      if (engineContext != null) {
        handleEngine(socket);
        continue;
      }
      handleTls((SSLSocket) socket);
    }
  }

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

  /**
   * Graceful teardown of a failed handshake: bound the drain with SO_TIMEOUT, drain the peer's
   * remaining bytes so close() sends FIN not RST, half-close the output, then close. Never abrupt,
   * never SO_LINGER 0. Best-effort — after a fatal handshake error the JDK has usually already
   * closed the transport, so every step here may find the socket dead; that is fine, the point is
   * that the fixture's own close path stays graceful on every JDK where it can still act.
   */
  private static void gracefulFailureClose(SSLSocket socket) {
    try {
      socket.setSoTimeout(ANSWER_TIMEOUT_MS);
    } catch (SocketException ignore) {
      // socket already dead — nothing to drain
    }
    drainQuietly(socket);
    try {
      socket.shutdownOutput();
    } catch (IOException ignore) {
      // best-effort half-close; close() below still ends the connection
    }
    closeQuietly(socket);
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
      // Graceful failure teardown (same doctrine as the plain-mode answer loop): the fatal alert
      // JSSE just flushed must reach the client — an abrupt close with unread inbound data sends
      // RST, and on Windows an incoming RST discards already-received bytes, so the client reports
      // a bare SocketException instead of the TLS failure JSSE actually raised. Modern JDKs close
      // the transport themselves on a fatal handshake error with a best-effort non-blocking drain;
      // this teardown guarantees the fixture never adds its own abrupt close on top of that.
      gracefulFailureClose(socket);
      return;
    }
    if (closeAfterHandshake) {
      closeQuietly(socket);
    }
  }

  /**
   * Engine-driven accept: drives one {@link SSLEngine} through the handshake, records the engine's
   * own failure, flushes its pending fatal alert, then tears the raw socket down gracefully. A
   * watchdog closes the socket at the overall deadline so a blocked read or write cannot outlive
   * it; the watchdog is cancelled when this connection's handling ends.
   */
  private void handleEngine(Socket socket) {
    accepted.add(socket);
    Connection connection =
        new Connection(
            new CompletableFuture<>(), new CompletableFuture<>(), new CompletableFuture<>());
    connections.add(connection);
    // The engine path never reads plaintext, so there is no first client byte to report.
    connection.firstClientByte().complete(-1);
    SSLEngine engine = engineContext.createSSLEngine();
    engine.setUseClientMode(false);
    engine.setNeedClientAuth(true);
    long deadlineNanos = System.nanoTime() + TimeUnit.MILLISECONDS.toNanos(ENGINE_DEADLINE_MS);
    ScheduledFuture<?> watchdog = scheduleEngineDeadlineClose(socket, deadlineNanos);
    try {
      Throwable failure = null;
      boolean finished = false;
      try {
        engine.beginHandshake();
        finished = driveEngineHandshake(socket, engine, connection, deadlineNanos);
      } catch (SSLException engineFailure) {
        failure = engineFailure;
      } catch (IOException ioFailure) {
        failure = ioFailure;
      } catch (RuntimeException runtimeFailure) {
        failure = runtimeFailure;
      }
      if (failure != null) {
        // The engine's own exception is the evidence; never a synthetic or bare-IO substitute.
        connection.handshakeFailure().complete(failure);
      } else if (!finished) {
        LOG.log(
            System.Logger.Level.WARNING,
            "engine-driven TLS server closed before the handshake finished without throwing");
      }
      if (!finished) {
        // Flush the engine's own pending fatal alert. TLS 1.3 protects it with the active handshake
        // keys, so the engine is the only entity that can encode it; a synthetic plaintext alert
        // record would be rejected by the client as a decode error.
        int flushed = flushEngineAlert(engine, socket, deadlineNanos);
        LOG.log(
            System.Logger.Level.INFO,
            "engine-driven TLS server flushed {0} byte(s) of engine-generated alert data",
            flushed);
        gracefulEngineClose(socket, deadlineNanos);
        return;
      }
      closeQuietly(socket);
    } finally {
      if (watchdog != null) {
        watchdog.cancel(false);
      }
    }
  }

  /**
   * Schedules the connection's deadline close. On expiry the daemon scheduler closes the socket
   * from another thread, which unblocks a read or write already waiting in the kernel with a {@link
   * SocketException}; the engine loop's catch paths treat that as teardown. When the executor is
   * already shut down by {@link #close()} (this accept lost the shutdown race), the socket is
   * closed here instead: an unprotected socket would leak, and the engine loop's failure path
   * records the resulting exception as the teardown signal.
   */
  private ScheduledFuture<?> scheduleEngineDeadlineClose(Socket socket, long deadlineNanos) {
    if (engineWatchdog == null) {
      return null;
    }
    long delayMillis =
        Math.max(0L, TimeUnit.NANOSECONDS.toMillis(deadlineNanos - System.nanoTime()));
    ScheduledFuture<?> watchdog;
    try {
      watchdog =
          engineWatchdog.schedule(() -> closeQuietly(socket), delayMillis, TimeUnit.MILLISECONDS);
    } catch (RejectedExecutionException closing) {
      watchdog = null;
    }
    if (watchdog == null) {
      // Shutdown won the race: no watchdog can ever fire, so close the socket now rather than
      // leaving it registered but unguarded outside teardown.
      closeQuietly(socket);
    }
    return watchdog;
  }

  /**
   * Full SSLEngine handshake loop: task delegation, wrap/unwrap, partial records across reads, and
   * both buffer-exhaustion statuses. Returns {@code true} when the handshake reaches FINISHED or a
   * settled NOT_HANDSHAKING state, {@code false} when the engine closes first.
   */
  private static boolean driveEngineHandshake(
      Socket socket, SSLEngine engine, Connection connection, long deadlineNanos)
      throws IOException {
    SSLSession session = engine.getSession();
    ByteBuffer netIn = ByteBuffer.allocate(session.getPacketBufferSize() * 2);
    ByteBuffer appIn = ByteBuffer.allocate(session.getApplicationBufferSize());
    ByteBuffer netOut = ByteBuffer.allocate(session.getPacketBufferSize() * 2);
    ByteBuffer empty = ByteBuffer.allocate(0);
    InputStream in = socket.getInputStream();
    OutputStream out = socket.getOutputStream();
    SSLEngineResult.HandshakeStatus status = engine.getHandshakeStatus();
    boolean sniCaptured = false;
    boolean needsRead = true;

    while (true) {
      if (System.nanoTime() >= deadlineNanos) {
        throw new SocketTimeoutException(
            "engine-driven TLS handshake exceeded its overall deadline");
      }
      switch (status) {
        case NEED_TASK:
          runDelegatedTasks(engine);
          status = engine.getHandshakeStatus();
          break;
        case NEED_WRAP:
          netOut.clear();
          SSLEngineResult wrapResult = engine.wrap(empty, netOut);
          if (wrapResult.getStatus() == SSLEngineResult.Status.BUFFER_OVERFLOW) {
            netOut = enlarge(netOut, session.getPacketBufferSize());
            break;
          }
          flushFully(out, netOut, deadlineNanos);
          if (wrapResult.getStatus() == SSLEngineResult.Status.CLOSED) {
            return false;
          }
          status = wrapResult.getHandshakeStatus();
          break;
        case NEED_UNWRAP:
          if (needsRead || netIn.position() == 0) {
            netIn = readInto(socket, in, netIn, deadlineNanos);
            needsRead = false;
            status = engine.getHandshakeStatus();
            break;
          }
        // buffered record bytes remain — unwrap them before asking the socket for more
        // fall through
        case NEED_UNWRAP_AGAIN:
          // NEED_UNWRAP_AGAIN means run unwrap again on the already-buffered bytes WITHOUT first
          // waiting for new network data: the engine can still owe handshake progress (e.g. TLS
          // 1.3 post-handshake messages) while netIn is empty. Only NEED_UNWRAP reads.
          netIn.flip();
          SSLEngineResult unwrapResult;
          try {
            unwrapResult = engine.unwrap(netIn, appIn);
          } finally {
            netIn.compact();
          }
          if (!sniCaptured && unwrapResult.getStatus() == SSLEngineResult.Status.OK) {
            // The ClientHello is consumed at this point, so the handshake session exposes the
            // client's SNI; guard every step because a failed handshake can drop the session.
            sniCaptured = captureServerNames(engine, connection);
          }
          switch (unwrapResult.getStatus()) {
            case OK:
              appIn.clear();
              status = unwrapResult.getHandshakeStatus();
              break;
            case BUFFER_UNDERFLOW:
              // a partial TLS record is already accumulated in netIn — read the rest
              needsRead = true;
              status = unwrapResult.getHandshakeStatus();
              break;
            case BUFFER_OVERFLOW:
              appIn = enlarge(appIn, session.getApplicationBufferSize());
              status = unwrapResult.getHandshakeStatus();
              break;
            case CLOSED:
              return false;
            default:
              throw new SSLException(
                  "unexpected engine unwrap status: " + unwrapResult.getStatus());
          }
          break;
        case FINISHED:
        case NOT_HANDSHAKING:
          if (!sniCaptured) {
            captureServerNames(engine, connection);
          }
          return true;
        default:
          throw new SSLException("unexpected engine handshake status: " + status);
      }
    }
  }

  /** Runs every delegated task until the engine has none left. */
  private static void runDelegatedTasks(SSLEngine engine) {
    Runnable task;
    while ((task = engine.getDelegatedTask()) != null) {
      task.run();
    }
  }

  /**
   * Reads more raw socket bytes into the inbound accumulator; grows it when a partial record fills
   * the buffer. Each read is bounded by {@link #ANSWER_TIMEOUT_MS} and the remaining overall
   * deadline.
   */
  private static ByteBuffer readInto(
      Socket socket, InputStream in, ByteBuffer netIn, long deadlineNanos) throws IOException {
    if (!netIn.hasRemaining()) {
      netIn = enlarge(netIn, netIn.capacity() * 2);
    }
    long remainingMillis = TimeUnit.NANOSECONDS.toMillis(deadlineNanos - System.nanoTime());
    if (remainingMillis <= 0) {
      throw new SocketTimeoutException("engine-driven TLS handshake reached its overall deadline");
    }
    socket.setSoTimeout((int) Math.min(ANSWER_TIMEOUT_MS, remainingMillis));
    byte[] chunk = new byte[Math.min(netIn.remaining(), 16 * 1024)];
    int read = in.read(chunk);
    if (read < 0) {
      throw new EOFException("client closed before the TLS handshake completed");
    }
    netIn.put(chunk, 0, read);
    return netIn;
  }

  private static void flushFully(OutputStream out, ByteBuffer netOut, long deadlineNanos)
      throws IOException {
    // SO_TIMEOUT bounds reads only, so the deadline-triggered watchdog close is what actually
    // bounds a write already blocked on a stalled peer. This pre-write check only avoids starting
    // a write past the budget.
    if (System.nanoTime() >= deadlineNanos) {
      throw new SocketTimeoutException(
          "engine-driven TLS handshake reached its overall deadline before a write");
    }
    netOut.flip();
    out.write(netOut.array(), netOut.arrayOffset() + netOut.position(), netOut.remaining());
    out.flush();
  }

  /** Enlarges a write-mode buffer, preserving accumulated bytes. */
  private static ByteBuffer enlarge(ByteBuffer buffer, int minimumCapacity) {
    ByteBuffer grown = ByteBuffer.allocate(Math.max(buffer.capacity() * 2, minimumCapacity));
    buffer.flip();
    grown.put(buffer);
    return grown;
  }

  /**
   * Snapshots the client's requested SNI names from the in-progress handshake session. Returns
   * whether the names were captured; a null session (engine already closed) or a non-extended
   * session leaves the future pending rather than inventing evidence.
   */
  private static boolean captureServerNames(SSLEngine engine, Connection connection) {
    try {
      SSLSession handshake = engine.getHandshakeSession();
      if (handshake instanceof ExtendedSSLSession extended) {
        List<String> names = new ArrayList<>();
        for (SNIServerName name : extended.getRequestedServerNames()) {
          if (name instanceof SNIHostName host) {
            names.add(host.getAsciiName());
          }
        }
        connection.requestedServerNames().complete(List.copyOf(names));
        return true;
      }
    } catch (RuntimeException disappearing) {
      // a closing engine can drop the handshake session mid-capture; SNI evidence is best-effort
    }
    return false;
  }

  /**
   * Emits the engine's own pending fatal alert by looping {@code wrap} until it produces no more
   * bytes. After a fatal handshake error the engine is outbound-closed but still holds the alert
   * record; only the engine can encode it. Every produced record's outer content type is logged so
   * the park evidence can say whether a plaintext 0x15 alert was visible or the alert was
   * engine-protected (TLS 1.3 sends it inside an application_data record).
   *
   * <p>Every wrap iteration and every write is checked against the overall deadline; on expiry the
   * flush stops (recording the exhaustion) and the caller proceeds to the graceful teardown instead
   * of blocking on a stalled peer.
   */
  private static int flushEngineAlert(SSLEngine engine, Socket socket, long deadlineNanos) {
    OutputStream out;
    try {
      out = socket.getOutputStream();
    } catch (IOException dead) {
      return 0;
    }
    ByteBuffer empty = ByteBuffer.allocate(0);
    ByteBuffer netOut = ByteBuffer.allocate(engine.getSession().getPacketBufferSize() * 2);
    int written = 0;
    for (int attempt = 0; attempt < 32; attempt++) {
      if (System.nanoTime() >= deadlineNanos) {
        logAlertDeadlineExhausted(written);
        break;
      }
      runDelegatedTasks(engine);
      netOut.clear();
      SSLEngineResult result;
      try {
        result = engine.wrap(empty, netOut);
      } catch (SSLException engineClosed) {
        // nothing left to drain
        break;
      }
      if (result.getStatus() == SSLEngineResult.Status.BUFFER_OVERFLOW) {
        netOut = enlarge(netOut, engine.getSession().getPacketBufferSize());
        continue;
      }
      int produced = netOut.position();
      if (produced == 0) {
        break;
      }
      logEmittedRecords(netOut);
      netOut.flip();
      if (System.nanoTime() >= deadlineNanos) {
        logAlertDeadlineExhausted(written);
        break;
      }
      try {
        out.write(netOut.array(), netOut.arrayOffset() + netOut.position(), netOut.remaining());
        out.flush();
        written += produced;
      } catch (IOException gone) {
        break;
      }
    }
    return written;
  }

  private static void logAlertDeadlineExhausted(int written) {
    LOG.log(
        System.Logger.Level.WARNING,
        "engine-driven TLS server stopped flushing its fatal alert at the overall deadline after"
            + " {0} byte(s)",
        written);
  }

  /** Logs each TLS record's outer content type from a write-mode buffer (park evidence only). */
  private static void logEmittedRecords(ByteBuffer produced) {
    int offset = 0;
    int end = produced.position();
    byte[] bytes = produced.array();
    int base = produced.arrayOffset();
    while (offset + 5 <= end) {
      int type = bytes[base + offset] & 0xFF;
      int length = ((bytes[base + offset + 3] & 0xFF) << 8) | (bytes[base + offset + 4] & 0xFF);
      if (type == 0x15 && offset + 5 <= end) {
        int level = offset + 5 < end ? bytes[base + offset + 5] & 0xFF : -1;
        int description = offset + 6 < end ? bytes[base + offset + 6] & 0xFF : -1;
        LOG.log(
            System.Logger.Level.INFO,
            "engine-driven TLS server emitted alert record 0x15 (level {0}, description {1})",
            level,
            description);
      } else {
        LOG.log(
            System.Logger.Level.INFO,
            "engine-driven TLS server emitted record type 0x{0} ({1} byte(s)); a TLS 1.3 alert"
                + " is engine-protected inside an application_data record",
            Integer.toHexString(type),
            length + 5);
      }
      if (length > end - offset - 5) {
        break;
      }
      offset += 5 + length;
    }
  }

  /**
   * Graceful engine-path teardown: half-close the output, then drain inbound bytes until EOF, a
   * read error other than a per-read timeout, or the overall deadline, then close. A per-read
   * {@link SocketTimeoutException} is not terminal: the drain continues while the overall budget
   * remains, so a slow peer is not cut off early. If the overall deadline expires first, the socket
   * is closed with inbound bytes potentially unread (accepting an RST) rather than blocking past
   * the budget — the only intentional exception to the no-close-with-unread-bytes doctrine. Never
   * linger.
   */
  private static void gracefulEngineClose(Socket socket, long deadlineNanos) {
    try {
      socket.shutdownOutput();
    } catch (IOException ignore) {
      // output already closed — the drain below still ends the connection gracefully
    }
    byte[] discard = new byte[4096];
    try {
      InputStream in = socket.getInputStream();
      while (true) {
        long remainingMillis = TimeUnit.NANOSECONDS.toMillis(deadlineNanos - System.nanoTime());
        if (remainingMillis <= 0) {
          LOG.log(
              System.Logger.Level.WARNING,
              "engine-driven TLS server stopped draining at the overall deadline; closing with"
                  + " inbound bytes possibly unread");
          break;
        }
        socket.setSoTimeout((int) Math.min(ANSWER_TIMEOUT_MS, remainingMillis));
        try {
          if (in.read(discard) < 0) {
            break;
          }
        } catch (SocketTimeoutException stillDraining) {
          // a per-read timeout is not terminal — keep draining while the overall budget remains
        }
      }
    } catch (IOException ignore) {
      // a read error other than a timeout (reset, closed) ends the drain; the alert was flushed
    } finally {
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
    if (engineWatchdog != null) {
      engineWatchdog.shutdownNow();
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
