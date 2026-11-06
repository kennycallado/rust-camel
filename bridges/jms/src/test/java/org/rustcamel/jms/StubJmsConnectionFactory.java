package org.rustcamel.jms;

import java.util.function.Supplier;
import javax.jms.Connection;
import javax.jms.ConnectionFactory;
import javax.jms.JMSContext;

/** Connection factory stub delegating to a supplied connection instance. */
final class StubJmsConnectionFactory implements ConnectionFactory {

  private final Supplier<Connection> connectionSupplier;

  StubJmsConnectionFactory(Supplier<Connection> connectionSupplier) {
    this.connectionSupplier = connectionSupplier;
  }

  @Override
  public Connection createConnection() {
    return connectionSupplier.get();
  }

  @Override
  public Connection createConnection(String userName, String password) {
    return connectionSupplier.get();
  }

  @Override
  public JMSContext createContext() {
    return null;
  }

  @Override
  public JMSContext createContext(String userName, String password) {
    return null;
  }

  @Override
  public JMSContext createContext(String userName, String password, int sessionMode) {
    return null;
  }

  @Override
  public JMSContext createContext(int sessionMode) {
    return null;
  }
}
