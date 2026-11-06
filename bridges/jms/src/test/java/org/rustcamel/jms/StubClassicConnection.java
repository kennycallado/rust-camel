package org.rustcamel.jms;

import org.apache.activemq.ActiveMQConnection;
import org.apache.activemq.management.JMSStatsImpl;
import org.apache.activemq.util.IdGenerator;

/**
 * Real ActiveMQ Classic connection wired to a {@link StubTransport}, so lifecycle behaviour (close
 * / stop) can be observed without a broker.
 */
final class StubClassicConnection extends ActiveMQConnection {

  private final StubTransport transport;

  StubClassicConnection(StubTransport transport) throws Exception {
    super(transport, new IdGenerator(), new IdGenerator(), new JMSStatsImpl());
    this.transport = transport;
  }

  StubTransport stubTransport() {
    return transport;
  }
}
