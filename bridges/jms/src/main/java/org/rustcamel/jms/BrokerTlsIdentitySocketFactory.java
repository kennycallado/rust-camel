package org.rustcamel.jms;

import java.io.IOException;
import java.io.InputStream;
import java.net.InetAddress;
import java.net.InetSocketAddress;
import java.net.Socket;
import java.net.SocketException;
import javax.net.ssl.SSLSocket;
import javax.net.ssl.SSLSocketFactory;

/**
 * Identity-binding {@link SSLSocketFactory} seam (task 5.1): wraps the platform TLS socket factory
 * of the ActiveMQ {@code SslContext} (or {@code SSLContext.getDefault()}) and forces every broker
 * socket down the connected-socket path with the bridge TLS identity binding applied.
 *
 * <p>ActiveMQ's {@code TcpTransport} constructor calls {@link #createSocket()} (no-arg) and, on
 * {@link SocketException}, leaves the socket unset so {@code TcpTransport.connect()} takes the
 * connected-socket path: {@code createSocket(InetAddress, int)}, or {@code
 * createSocket(InetAddress, int, InetAddress, int)} when the transport URI carries a local bind
 * (the {@code ssl://host:port/localHost:localPort} path form). The no-arg factory deliberately
 * throws so a connected-socket overload is always taken — and <em>every</em> dialing overload
 * relabels the dialed peer through one shared helper: {@link InetAddress#getByAddress(String,
 * byte[])} with the configured {@code serverName} (same resolved IP, no DNS lookup) before {@link
 * ServerNamePreservingSSLSocket} pins SNI and endpoint identification to that name. SNI
 * re-assertion alone is not enough — the JDK's identity check falls back to the dialed peer host,
 * so without the relabel a certificate matching only the dialed host is accepted under a wrong
 * {@code serverName}. With a {@code null} {@code serverName} the original host is kept and the
 * platform factory is called directly.
 *
 * <p>Default cipher and protocol lists are never restricted. {@code connectTimeout} mirrors
 * ActiveMQ {@code TcpTransport}'s default {@code connectionTimeout} of 30 seconds.
 */
public final class BrokerTlsIdentitySocketFactory extends SSLSocketFactory {

  private static final int CONNECT_TIMEOUT_MS = 30_000;

  private final SSLSocketFactory delegate;
  private final String serverName;

  public BrokerTlsIdentitySocketFactory(SSLSocketFactory delegate, String serverName) {
    if (delegate == null) {
      throw new IllegalArgumentException("delegate SSLSocketFactory is required");
    }
    this.delegate = delegate;
    this.serverName = serverName;
  }

  /**
   * Never usable: throws so {@code TcpTransport} takes its connected-socket path, which is where
   * the identity binding can be applied.
   */
  @Override
  public Socket createSocket() throws SocketException {
    throw new SocketException("bridge TLS socket factory requires the connected-socket path");
  }

  @Override
  public Socket createSocket(InetAddress host, int port) throws IOException {
    return dial(host, port, null, 0, false);
  }

  @Override
  public Socket createSocket(String host, int port) throws IOException {
    return createSocket(InetAddress.getByName(host), port);
  }

  @Override
  public Socket createSocket(InetAddress host, int port, InetAddress localAddress, int localPort)
      throws IOException {
    return dial(host, port, localAddress, localPort, true);
  }

  @Override
  public Socket createSocket(String host, int port, InetAddress localAddress, int localPort)
      throws IOException {
    return createSocket(InetAddress.getByName(host), port, localAddress, localPort);
  }

  @Override
  public Socket createSocket(Socket s, String host, int port, boolean autoClose)
      throws IOException {
    return maybeWrap(delegate.createSocket(s, host, port, autoClose));
  }

  @Override
  public Socket createSocket(Socket s, InputStream consumed, boolean autoClose) throws IOException {
    return maybeWrap(delegate.createSocket(s, consumed, autoClose));
  }

  /**
   * The one peer-dialing seam every connected-socket overload goes through, so no overload can
   * forget the identity binding. With a configured {@code serverName} the resolved peer is
   * relabelled via {@link InetAddress#getByAddress(String, byte[])} (same IP bytes, no DNS lookup)
   * and wrapped in {@link ServerNamePreservingSSLSocket} before connecting — SNI re-assertion alone
   * leaves the JDK's fallback-to-dialed-peer-host identity check open, so without the relabel a
   * certificate that matches only the dialed host would be accepted under a wrong {@code
   * serverName}. {@code bound} marks the local-bind overloads: their local endpoint is bound first
   * ({@code localPort} 0 lets the kernel assign an ephemeral port). With a {@code null} {@code
   * serverName} the platform factory dials the original host exactly as before.
   */
  private Socket dial(
      InetAddress host, int port, InetAddress localAddress, int localPort, boolean bound)
      throws IOException {
    if (serverName == null) {
      return bound
          ? delegate.createSocket(host, port, localAddress, localPort)
          : delegate.createSocket(host, port);
    }
    // Relabel the resolved address with the identity name: same IP bytes, no DNS lookup.
    InetAddress identity = InetAddress.getByAddress(serverName, host.getAddress());
    SSLSocket raw = (SSLSocket) delegate.createSocket();
    ServerNamePreservingSSLSocket wrapped = new ServerNamePreservingSSLSocket(raw, serverName);
    if (bound) {
      wrapped.bind(new InetSocketAddress(localAddress, localPort));
    }
    wrapped.connect(new InetSocketAddress(identity, port), CONNECT_TIMEOUT_MS);
    return wrapped;
  }

  /**
   * Pure delegate + wrap for the layered (already-connected) overloads: they never dial a peer —
   * the handshake runs over the existing transport connection — so there is no dialed-peer host to
   * relabel; the wrap still pins SNI and endpoint identification to {@code serverName}.
   */
  private Socket maybeWrap(Socket socket) {
    if (serverName == null || !(socket instanceof SSLSocket sslSocket)) {
      return socket;
    }
    return new ServerNamePreservingSSLSocket(sslSocket, serverName);
  }

  @Override
  public String[] getDefaultCipherSuites() {
    return delegate.getDefaultCipherSuites();
  }

  @Override
  public String[] getSupportedCipherSuites() {
    return delegate.getSupportedCipherSuites();
  }
}
