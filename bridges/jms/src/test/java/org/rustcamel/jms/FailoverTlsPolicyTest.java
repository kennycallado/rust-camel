package org.rustcamel.jms;

import static org.junit.jupiter.api.Assertions.assertDoesNotThrow;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.net.URI;
import javax.net.ssl.SSLContext;
import javax.net.ssl.SSLSocket;
import org.apache.activemq.management.JMSStatsImpl;
import org.apache.activemq.openwire.OpenWireFormat;
import org.apache.activemq.transport.Transport;
import org.apache.activemq.transport.TransportFactory;
import org.apache.activemq.transport.failover.FailoverTransport;
import org.apache.activemq.transport.tcp.SslTransport;
import org.junit.jupiter.api.DisplayName;
import org.junit.jupiter.api.Test;

/**
 * Composite downgrade policy of {@link BridgeActiveMqConnectionFactory#applyCompositeTlsPolicy} on
 * real ActiveMQ transport objects (task 5.1). Pure JVM, no broker: {@code
 * TransportFactory.compositeConnect} builds a {@link FailoverTransport} from the member list
 * without dialing anything, and {@code createActiveMQConnection(Transport, JMSStatsImpl)} is the
 * exact seam the guard runs on. A plain {@code ssl://} transport cannot be constructed by dialing
 * {@code ssl://a:1} without a broker, so it is built directly as the real {@link SslTransport}
 * object the ssl scheme produces.
 */
@DisplayName("Failover composite TLS downgrade policy")
class FailoverTlsPolicyTest {

  private static final String COMPOSITE_SSL_URL = "failover:(ssl://a:1,ssl://b:2)";

  private static FailoverTransport drivenGuard(String url, Transport transport) throws Exception {
    BridgeActiveMqConnectionFactory factory =
        new BridgeActiveMqConnectionFactory(url, null, null, null);
    var connection = factory.createActiveMQConnection(transport, new JMSStatsImpl());
    connection.close();
    return transport.narrow(FailoverTransport.class);
  }

  @Test
  void compositeSslDisablesUriUpdatesAndReconnects() throws Exception {
    Transport transport = TransportFactory.compositeConnect(new URI(COMPOSITE_SSL_URL));

    FailoverTransport failover = drivenGuard(COMPOSITE_SSL_URL, transport);

    assertNotNull(failover, "composite yields a FailoverTransport");
    assertFalse(failover.isUpdateURIsSupported(), "updateURIs disabled");
    assertFalse(failover.isReconnectSupported(), "broker-directed reconnect disabled");
  }

  @Test
  void plainSslDoesNotTouchFailoverFlags() throws Exception {
    // Real SslTransport as the ssl:// scheme produces it; narrow must find no FailoverTransport
    // so the policy is a no-op and cannot touch any failover flags.
    SSLSocket raw = (SSLSocket) SSLContext.getDefault().getSocketFactory().createSocket();
    Transport transport = new SslTransport(new OpenWireFormat(), raw);

    assertNull(transport.narrow(FailoverTransport.class), "plain ssl has no failover member");
    assertDoesNotThrow(() -> drivenGuard("ssl://a:1", transport));
  }

  @Test
  void tcpCompositeUntouched() throws Exception {
    Transport transport = TransportFactory.compositeConnect(new URI("failover:(tcp://a:1)"));

    FailoverTransport failover = drivenGuard("failover:(tcp://a:1)", transport);

    assertNotNull(failover, "composite yields a FailoverTransport");
    assertTrue(failover.isUpdateURIsSupported(), "updateURIs keeps its default");
    assertTrue(failover.isReconnectSupported(), "broker-directed reconnect keeps its default");
  }
}
