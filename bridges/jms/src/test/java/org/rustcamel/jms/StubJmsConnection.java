package org.rustcamel.jms;

import java.util.Collections;
import java.util.Enumeration;
import java.util.List;
import java.util.concurrent.CopyOnWriteArrayList;
import java.util.concurrent.atomic.AtomicBoolean;
import javax.jms.Connection;
import javax.jms.ConnectionConsumer;
import javax.jms.ConnectionMetaData;
import javax.jms.Destination;
import javax.jms.ExceptionListener;
import javax.jms.ServerSessionPool;
import javax.jms.Session;
import javax.jms.Topic;

/**
 * Broker-free JMS connection stub that records close invocations so tests can observe when a pooled
 * connection is actually closed.
 */
final class StubJmsConnection implements Connection {

  private final List<Long> closeTimes = new CopyOnWriteArrayList<>();
  private final AtomicBoolean closed = new AtomicBoolean(false);

  private static final ConnectionMetaData CONNECTION_META_DATA = new StubConnectionMetaData();

  public List<Long> closeTimes() {
    return closeTimes;
  }

  public boolean isClosed() {
    return closed.get();
  }

  @Override
  public Session createSession(boolean transacted, int acknowledgeMode) {
    return new StubJmsSession();
  }

  @Override
  public Session createSession(int sessionMode) {
    return new StubJmsSession();
  }

  @Override
  public Session createSession() {
    return new StubJmsSession();
  }

  @Override
  public String getClientID() {
    return null;
  }

  @Override
  public void setClientID(String clientID) {}

  @Override
  public ConnectionMetaData getMetaData() {
    return CONNECTION_META_DATA;
  }

  @Override
  public ExceptionListener getExceptionListener() {
    return null;
  }

  @Override
  public void setExceptionListener(ExceptionListener listener) {}

  @Override
  public void start() {}

  @Override
  public void stop() {}

  @Override
  public void close() {
    closeTimes.add(System.nanoTime());
    closed.set(true);
  }

  @Override
  public ConnectionConsumer createConnectionConsumer(
      Destination destination,
      String messageSelector,
      ServerSessionPool sessionPool,
      int maxMessages) {
    return null;
  }

  @Override
  public ConnectionConsumer createSharedConnectionConsumer(
      Topic topic,
      String subscriptionName,
      String messageSelector,
      ServerSessionPool sessionPool,
      int maxMessages) {
    return null;
  }

  @Override
  public ConnectionConsumer createDurableConnectionConsumer(
      Topic topic,
      String subscriptionName,
      String messageSelector,
      ServerSessionPool sessionPool,
      int maxMessages) {
    return null;
  }

  @Override
  public ConnectionConsumer createSharedDurableConnectionConsumer(
      Topic topic,
      String subscriptionName,
      String messageSelector,
      ServerSessionPool sessionPool,
      int maxMessages) {
    return null;
  }

  /**
   * Minimal JMS 2.0 metadata. The messaginghub pool reads {@code getJMSMajorVersion()} while
   * building a {@code PooledConnection}; a null metadata aborts every physical connection creation.
   */
  private static final class StubConnectionMetaData implements ConnectionMetaData {

    @Override
    public String getJMSVersion() {
      return "2.0";
    }

    @Override
    public int getJMSMajorVersion() {
      return 2;
    }

    @Override
    public int getJMSMinorVersion() {
      return 0;
    }

    @Override
    public String getJMSProviderName() {
      return "rust-camel-stub";
    }

    @Override
    public String getProviderVersion() {
      return "2.0";
    }

    @Override
    public int getProviderMajorVersion() {
      return 2;
    }

    @Override
    public int getProviderMinorVersion() {
      return 0;
    }

    @Override
    public Enumeration<String> getJMSXPropertyNames() {
      return Collections.emptyEnumeration();
    }
  }
}
