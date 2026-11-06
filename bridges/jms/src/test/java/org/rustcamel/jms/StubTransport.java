package org.rustcamel.jms;

import java.net.URI;
import java.security.cert.X509Certificate;
import java.util.List;
import java.util.concurrent.CopyOnWriteArrayList;
import org.apache.activemq.transport.FutureResponse;
import org.apache.activemq.transport.ResponseCallback;
import org.apache.activemq.transport.Transport;
import org.apache.activemq.transport.TransportListener;
import org.apache.activemq.wireformat.WireFormat;

/**
 * Minimal ActiveMQ transport stub. The ActiveMQ Classic connection constructor only calls {@link
 * #isFaultTolerant()} and {@link #setTransportListener(TransportListener)}; {@code close()} ends in
 * {@link #stop()}, which is recorded so tests can count physical connection shutdowns.
 */
final class StubTransport implements Transport {

  private volatile boolean stopped;
  private final List<Long> stopTimes = new CopyOnWriteArrayList<>();
  private volatile TransportListener listener;

  public boolean isStopped() {
    return stopped;
  }

  public List<Long> stopTimes() {
    return stopTimes;
  }

  public TransportListener listener() {
    return listener;
  }

  @Override
  public void start() {}

  @Override
  public void stop() {
    stopTimes.add(System.nanoTime());
    stopped = true;
  }

  @Override
  public void oneway(Object command) {}

  @Override
  public FutureResponse asyncRequest(Object command, ResponseCallback responseCallback) {
    return null;
  }

  @Override
  public Object request(Object command) {
    return null;
  }

  @Override
  public Object request(Object command, int timeout) {
    return null;
  }

  @Override
  public TransportListener getTransportListener() {
    return listener;
  }

  @Override
  public void setTransportListener(TransportListener commandListener) {
    this.listener = commandListener;
  }

  @Override
  public <T> T narrow(Class<T> target) {
    return null;
  }

  @Override
  public String getRemoteAddress() {
    return "stub://evictor-verifier";
  }

  @Override
  public boolean isFaultTolerant() {
    return false;
  }

  @Override
  public boolean isDisposed() {
    return isStopped();
  }

  @Override
  public boolean isConnected() {
    return !isStopped();
  }

  @Override
  public boolean isReconnectSupported() {
    return false;
  }

  @Override
  public boolean isUpdateURIsSupported() {
    return false;
  }

  @Override
  public void reconnect(URI uri) {}

  @Override
  public void updateURIs(boolean rebalance, URI[] uris) {}

  @Override
  public int getReceiveCounter() {
    return 0;
  }

  @Override
  public X509Certificate[] getPeerCertificates() {
    return null;
  }

  @Override
  public void setPeerCertificates(X509Certificate[] certificates) {}

  @Override
  public WireFormat getWireFormat() {
    return null;
  }
}
