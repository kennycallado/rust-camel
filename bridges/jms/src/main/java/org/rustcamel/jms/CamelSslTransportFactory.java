package org.rustcamel.jms;

import java.io.IOException;
import java.security.GeneralSecurityException;
import javax.net.SocketFactory;
import javax.net.ssl.SSLContext;
import javax.net.ssl.SSLSocketFactory;
import org.apache.activemq.broker.SslContext;
import org.apache.activemq.transport.tcp.SslTransportFactory;

/**
 * Bridge-owned {@code ssl} transport factory (task 4.1), with the broker TLS socket-factory seam
 * from task 5.1. Registering a bridge-owned class (rather than the stock factory) makes that seam
 * possible without URL reflection and keeps {@code TransportFactory.findTransportFactory("ssl")}
 * resolution stable in native image.
 *
 * <p><b>5.19.10 seam verification (javap on {@code activemq-client-5.19.10.jar}):</b> {@code
 * createSocketFactory()} is {@code protected javax.net.SocketFactory createSocketFactory() throws
 * java.io.IOException} in both {@code TcpTransportFactory} and its {@code SslTransportFactory}
 * subclass — overridable, not private — and is the virtual call {@code
 * TcpTransportFactory.doConnect} makes when building the transport socket. It is exactly the
 * creator the class constant pool advertises as {@code ()Ljavax/net/SocketFactory;}. This override
 * replaces the stock body: the delegate is the socket factory of the current {@code
 * org.apache.activemq.broker.SslContext} thread-local (installed by {@code
 * ActiveMQSslConnectionFactory.setKeyAndTrustManagers} during transport construction) when present,
 * else {@code SSLContext.getDefault().getSocketFactory()}, and the {@code serverName} comes from
 * the {@link BridgeActiveMqConnectionFactory.TlsConnectionState} {@link InheritableThreadLocal}
 * published by {@code BridgeActiveMqConnectionFactory.createConnection} before transport
 * construction (run-time only, native-safe). The returned {@link BrokerTlsIdentitySocketFactory}
 * forces the connected-socket path so the identity binding can be applied; with no bridge state the
 * {@code serverName} is {@code null} and sockets keep their original host.
 */
public class CamelSslTransportFactory extends SslTransportFactory {

  @Override
  protected SocketFactory createSocketFactory() throws IOException {
    SSLSocketFactory delegate = platformSocketFactory();
    BridgeActiveMqConnectionFactory.TlsConnectionState state =
        BridgeActiveMqConnectionFactory.currentTlsConnectionState();
    String serverName = state != null && state.tlsActive() ? state.serverName() : null;
    return new BrokerTlsIdentitySocketFactory(delegate, serverName);
  }

  private static SSLSocketFactory platformSocketFactory() throws IOException {
    SslContext current = SslContext.getCurrentSslContext();
    try {
      if (current != null) {
        return current.getSSLContext().getSocketFactory();
      }
      return SSLContext.getDefault().getSocketFactory();
    } catch (GeneralSecurityException e) {
      throw new IOException(
          "cannot build TLS socket factory (SslContext or SSLContext default)", e);
    }
  }
}
