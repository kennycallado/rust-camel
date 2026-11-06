package org.rustcamel.jms;

import java.util.function.Consumer;
import javax.jms.Connection;
import javax.jms.JMSException;
import org.apache.activemq.ActiveMQConnectionFactory;

/**
 * ActiveMQ Classic connection factory that hands out real {@link StubClassicConnection} instances
 * backed by a fresh stub transport each time, so every physical connection close is independent. An
 * optional creation listener records the instances the pool creates; the default listener is a
 * no-op, so behavior without a listener is unchanged.
 */
final class StubClassicConnectionFactory extends ActiveMQConnectionFactory {

  private volatile Consumer<StubClassicConnection> creationListener = connection -> {};

  StubClassicConnectionFactory() {
    super("tcp://localhost:1");
  }

  void setCreationListener(Consumer<StubClassicConnection> listener) {
    this.creationListener = listener == null ? connection -> {} : listener;
  }

  @Override
  public Connection createConnection() throws JMSException {
    return newStubConnection();
  }

  @Override
  public Connection createConnection(String userName, String password) throws JMSException {
    return newStubConnection();
  }

  private StubClassicConnection newStubConnection() throws JMSException {
    try {
      StubClassicConnection connection = new StubClassicConnection(new StubTransport());
      creationListener.accept(connection);
      return connection;
    } catch (Exception e) {
      JMSException failure = new JMSException("failed to create stub classic connection");
      failure.initCause(e);
      throw failure;
    }
  }
}
