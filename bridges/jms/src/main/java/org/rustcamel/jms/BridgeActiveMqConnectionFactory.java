package org.rustcamel.jms;

import javax.jms.Connection;
import javax.jms.JMSException;
import javax.net.ssl.KeyManager;
import javax.net.ssl.TrustManager;
import org.apache.activemq.ActiveMQConnection;
import org.apache.activemq.ActiveMQSslConnectionFactory;
import org.apache.activemq.management.JMSStatsImpl;
import org.apache.activemq.transport.Transport;
import org.apache.activemq.transport.failover.FailoverTransport;

/**
 * The single bridge-owned ActiveMQ Classic connection factory (task 5.1), used by <em>both</em> TLS
 * selection branches — pinned PEM material and platform-trust — so the composite downgrade guard
 * applies with or without a {@code tls} table.
 *
 * <p>Two seams live here:
 *
 * <ul>
 *   <li><b>Composite downgrade policy.</b> {@link #applyCompositeTlsPolicy(Transport, boolean)}
 *       disables failover {@code updateURIs} and broker-directed reconnect whenever a composite URL
 *       has an {@code ssl} member. Without it the failover task runner could silently replace an
 *       audited {@code ssl://} member with a broker-advertised one mid-flight, downgrading the
 *       transport security envelope. Plain {@code ssl://} transports narrow to {@code null} and are
 *       left untouched.
 *   <li><b>TLS identity state.</b> {@link #createConnection(String, String)} publishes {@link
 *       TlsConnectionState} on an {@link InheritableThreadLocal} <em>before</em> {@code super}
 *       runs: {@code super.createConnection} builds the transport, and {@link
 *       CamelSslTransportFactory}'s socket-factory creation consults that state during construction
 *       — before {@link #createActiveMQConnection(Transport, JMSStatsImpl)} ever executes. {@link
 *       InheritableThreadLocal} (not plain {@link ThreadLocal}) because failover member creation
 *       for members ≥ 2 happens on the failover task-runner thread spawned during {@code
 *       transport.start()} under the still-set parent state; child threads inherit at creation.
 *       This mirrors the lifetime envelope of ActiveMQ's own {@code broker.SslContext}
 *       thread-local, and broker-directed member replacement is already disabled by the guard
 *       above, bounding the async surface. The state is cleared in {@code finally}.
 * </ul>
 */
public class BridgeActiveMqConnectionFactory extends ActiveMQSslConnectionFactory {

  private static final InheritableThreadLocal<TlsConnectionState> TLS_CONNECTION_STATE =
      new InheritableThreadLocal<>();

  private final KeyManager[] keyManagers;
  private final TrustManager[] trustManagers;
  private final String serverName;
  private final boolean urlHasSslMember;

  /**
   * @param km client identity managers, or {@code null} for none
   * @param tm trust managers; non-null activates {@code setKeyAndTrustManagers} (TLS env branch),
   *     null skips it entirely (platform-trust branch)
   * @param serverName broker identity name for SNI/endpoint-identification binding, or {@code null}
   */
  public BridgeActiveMqConnectionFactory(
      String url, KeyManager[] km, TrustManager[] tm, String serverName) {
    super(url);
    if (tm != null) {
      setKeyAndTrustManagers(km, tm, BrokerTlsMaterial.newSecureRandom());
    }
    this.keyManagers = km;
    this.trustManagers = tm;
    this.serverName = serverName;
    this.urlHasSslMember = JmsClientFactory.urlHasSslMember(url);
  }

  /** Test/observable seam: the client identity managers configured at construction. */
  KeyManager[] configuredKeyManagers() {
    return keyManagers;
  }

  /**
   * Test/observable seam: the trust managers configured at construction ({@code null} = default).
   */
  TrustManager[] configuredTrustManagers() {
    return trustManagers;
  }

  @Override
  public Connection createConnection() throws JMSException {
    return createConnection(getUserName(), getPassword());
  }

  @Override
  public Connection createConnection(String userName, String password) throws JMSException {
    TlsConnectionState previous = TLS_CONNECTION_STATE.get();
    TLS_CONNECTION_STATE.set(new TlsConnectionState(serverName, urlHasSslMember));
    try {
      return super.createConnection(userName, password);
    } finally {
      if (previous == null) {
        TLS_CONNECTION_STATE.remove();
      } else {
        TLS_CONNECTION_STATE.set(previous);
      }
    }
  }

  @Override
  protected ActiveMQConnection createActiveMQConnection(Transport transport, JMSStatsImpl stats)
      throws Exception {
    applyCompositeTlsPolicy(transport, urlHasSslMember);
    return super.createActiveMQConnection(transport, stats);
  }

  /**
   * Composite downgrade policy (package-visible static helper): when the transport is a failover
   * composite <em>and</em> the URL has an {@code ssl} member, disable {@code updateURIs} and
   * broker-directed {@code reconnect} so no member can be replaced behind the audited TLS material.
   * {@code updateURIsURL} stays unset. Plain {@code ssl://} transports narrow to {@code null} and
   * are skipped; non-ssl composites keep their failover defaults.
   */
  static void applyCompositeTlsPolicy(Transport transport, boolean urlHasSslMember) {
    FailoverTransport failover = transport.narrow(FailoverTransport.class);
    if (failover != null && urlHasSslMember) {
      failover.setUpdateURIsSupported(false);
      failover.setReconnectSupported(false);
    }
  }

  /** Current thread's bridge TLS identity state, or {@code null} outside a connection build. */
  static TlsConnectionState currentTlsConnectionState() {
    return TLS_CONNECTION_STATE.get();
  }

  /**
   * TLS identity state of the connection being established: the broker identity {@code serverName}
   * for SNI/endpoint-identification binding, and {@code tlsActive} — whether the configured broker
   * URL actually has an {@code ssl} member (TLS in play) for this connection.
   */
  static final class TlsConnectionState {
    private final String serverName;
    private final boolean tlsActive;

    TlsConnectionState(String serverName, boolean tlsActive) {
      this.serverName = serverName;
      this.tlsActive = tlsActive;
    }

    String serverName() {
      return serverName;
    }

    boolean tlsActive() {
      return tlsActive;
    }
  }
}
