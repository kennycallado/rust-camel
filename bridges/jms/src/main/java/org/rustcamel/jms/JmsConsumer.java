package org.rustcamel.jms;

import com.google.protobuf.ByteString;
import io.grpc.stub.StreamObserver;
import jakarta.enterprise.context.Dependent;
import jakarta.inject.Inject;
import java.util.concurrent.atomic.AtomicBoolean;
import javax.jms.BytesMessage;
import javax.jms.Connection;
import javax.jms.Destination;
import javax.jms.JMSException;
import javax.jms.Message;
import javax.jms.MessageConsumer;
import javax.jms.Session;
import javax.jms.TextMessage;
import jms_bridge.JmsMessage;
import org.jboss.logging.Logger;

@Dependent
public class JmsConsumer {
  private static final Logger LOG = Logger.getLogger(JmsConsumer.class);
  private static final String MAX_BODY_BYTES_ENV = "JMS_MAX_BODY_BYTES";
  private static final long DEFAULT_MAX_BODY_BYTES = 16L * 1024 * 1024;

  /**
   * Hard ceiling for {@code JMS_MAX_BODY_BYTES}: the Rust IPC decode limit is 20 MiB, so a
   * configured cap above 19 MiB would let bodies pass this gate only to fail decode on the Rust
   * side. The headroom absorbs IPC framing overhead.
   */
  static final long MAX_BODY_BYTES_CEILING = 19L * 1024 * 1024;

  @Inject JmsClientFactory factory;

  private volatile Connection connection;
  private volatile Session session;
  private volatile MessageConsumer consumer;
  private volatile boolean running = false;
  private final AtomicBoolean resourcesClosed = new AtomicBoolean(false);

  /**
   * Test seam: pins the body cap deterministically. {@code -1} (production) resolves the cap from
   * {@link #maxBodyBytes()} per message.
   */
  long pinnedMaxBodyBytes = -1;

  private long resolveMaxBodyBytes() {
    return pinnedMaxBodyBytes > 0 ? pinnedMaxBodyBytes : maxBodyBytes();
  }

  /**
   * Reads the consumer body cap from {@code JMS_MAX_BODY_BYTES} in bytes. Fails loud on a malformed
   * or non-positive value so a typo never silently disables the cap.
   */
  static long maxBodyBytes() {
    String raw = System.getenv(MAX_BODY_BYTES_ENV);
    if (raw == null || raw.isBlank()) {
      return DEFAULT_MAX_BODY_BYTES;
    }
    return parseCap(raw);
  }

  /**
   * Parses one {@code JMS_MAX_BODY_BYTES} value, failing loud with the env name on malformed,
   * non-positive, or above-ceiling input.
   */
  static long parseCap(String raw) {
    try {
      long parsed = Long.parseLong(raw.trim());
      if (parsed <= 0) {
        throw new IllegalStateException(
            MAX_BODY_BYTES_ENV + " must be a positive byte count: " + raw);
      }
      if (parsed > MAX_BODY_BYTES_CEILING) {
        throw new IllegalStateException(
            MAX_BODY_BYTES_ENV
                + " exceeds its "
                + MAX_BODY_BYTES_CEILING
                + "-byte ceiling: "
                + parsed
                + "; caps above 19 MiB invert the decode-limit ordering, bodies pass this"
                + " Java cap only to fail at the 20 MiB Rust IPC limit");
      }
      return parsed;
    } catch (NumberFormatException e) {
      throw new IllegalStateException(MAX_BODY_BYTES_ENV + " invalid: " + raw, e);
    }
  }

  /**
   * Subscribe to a JMS destination and forward messages to the gRPC stream.
   *
   * <p>Uses synchronous polling (receive with timeout) instead of async MessageListener. The
   * previous MessageListener approach caused the JMS delivery thread to block on gRPC's
   * responseObserver.onNext() — which can stall when Vert.x back-pressures the stream. With
   * AUTO_ACKNOWLEDGE the broker won't dispatch the next message until onMessage returns, so the
   * consumer silently stopped after the first message.
   *
   * <p>Polling on a dedicated thread avoids this: the thread owns both the JMS receive and the gRPC
   * write, so there is no cross-thread blocking.
   *
   * @param finished shared flag indicating the stream has terminated. Set to {@code true} by
   *     exactly one of: gRPC cancel handler, consumer error, or normal completion. Uses
   *     compareAndSet to prevent the TOCTOU race that could call observer methods on a cancelled
   *     stream, which tears down the shared H2 connection in Quarkus.
   */
  public void subscribe(
      String destination,
      String subscriptionId,
      StreamObserver<JmsMessage> observer,
      AtomicBoolean finished) {
    // Startup validation (ADR-0033): a malformed JMS_MAX_BODY_BYTES must fail loud before any
    // broker connection is created.
    maxBodyBytes();

    running = true;
    resourcesClosed.set(false);

    Thread t =
        new Thread(
            () -> {
              try {
                connection = factory.createDedicatedConnection();
              } catch (Exception e) {
                if (running && finished.compareAndSet(false, true)) {
                  safeOnError(observer, e);
                }
                return;
              }

              try {
                connection.start();
                session = connection.createSession(false, Session.AUTO_ACKNOWLEDGE);
                Destination dest = JmsProducer.parseDestination(session, destination);
                consumer = session.createConsumer(dest);

                while (running) {
                  Message msg = consumer.receive(1000);
                  if (msg == null) {
                    continue;
                  }
                  LOG.debug("Received JMS message on " + destination);
                  try {
                    JmsMessage grpcMsg = convertMessage(msg, destination);
                    if (!finished.get()) {
                      try {
                        observer.onNext(grpcMsg);
                      } catch (Exception ignored) {
                      }
                    }
                  } catch (Exception e) {
                    LOG.error("Error forwarding message: " + e.getMessage(), e);
                    if (running && finished.compareAndSet(false, true)) {
                      safeOnError(observer, e);
                      return;
                    }
                  }
                }

                if (finished.compareAndSet(false, true)) {
                  try {
                    observer.onCompleted();
                  } catch (Exception ignored) {
                  }
                }
              } catch (Exception e) {
                if (running && finished.compareAndSet(false, true)) {
                  safeOnError(observer, e);
                }
              } finally {
                closeResources();
              }
            },
            "jms-consumer-" + subscriptionId);
    t.setDaemon(true);
    t.start();
  }

  private static void safeOnError(StreamObserver<JmsMessage> observer, Throwable t) {
    try {
      observer.onError(t);
    } catch (Exception ignored) {
    }
  }

  public void stop() {
    running = false;
    closeResources();
  }

  private void closeResources() {
    if (!resourcesClosed.compareAndSet(false, true)) return;
    try {
      if (consumer != null) {
        consumer.close();
        consumer = null;
      }
    } catch (Exception ignored) {
    }
    try {
      if (session != null) {
        session.close();
        session = null;
      }
    } catch (Exception ignored) {
    }
    try {
      if (connection != null) {
        connection.close();
        connection = null;
      }
    } catch (Exception ignored) {
    }
  }

  private JmsMessage convertMessage(Message msg, String destination) throws JMSException {
    JmsMessage.Builder b = JmsMessage.newBuilder();
    b.setMessageId(msg.getJMSMessageID() != null ? msg.getJMSMessageID() : "");
    b.setCorrelationId(msg.getJMSCorrelationID() != null ? msg.getJMSCorrelationID() : "");
    b.setTimestamp(msg.getJMSTimestamp());
    b.setDestination(destination);

    // Content-type fidelity (rc-kzti): an explicit "ContentType" property wins over the
    // TextMessage default. Defensive read: well-behaved providers return null for an absent
    // property; a misbehaving provider that throws (checked or unchecked) must not kill
    // forwarding — same posture as the header-enumeration loop below.
    String contentTypeProp = null;
    try {
      contentTypeProp = msg.getStringProperty("ContentType");
    } catch (Exception ignored) {
    }

    if (msg instanceof BytesMessage bm) {
      long len = bm.getBodyLength();
      long cap = resolveMaxBodyBytes();
      if (len < 0 || len > cap) {
        String diagnostic =
            MAX_BODY_BYTES_ENV
                + ": rejecting message body of "
                + len
                + " bytes (cap "
                + cap
                + " bytes); message not forwarded";
        // Handler-contract boundary (ADR-0012): the consumer logs at warn and forwards the error
        // outcome; the route owns the operational signal.
        LOG.warn(diagnostic);
        throw new JMSException(diagnostic);
      }
      byte[] buf = new byte[(int) len];
      bm.readBytes(buf);
      b.setBody(ByteString.copyFrom(buf));
    } else if (msg instanceof TextMessage tm) {
      // TextMessage exposes no pre-read length, so the cap lands after getText(): the string is
      // materialized by the JMS client either way, but oversized text never reaches the protobuf
      // body or the stream.
      String text = tm.getText();
      int len = text != null ? text.length() : 0;
      long cap = resolveMaxBodyBytes();
      if (len > cap) {
        String diagnostic =
            MAX_BODY_BYTES_ENV
                + ": rejecting message body of "
                + len
                + " chars (cap "
                + cap
                + " bytes); message not forwarded";
        // Handler-contract boundary (ADR-0012): the consumer logs at warn and forwards the error
        // outcome; the route owns the operational signal.
        LOG.warn(diagnostic);
        throw new JMSException(diagnostic);
      }
      b.setBody(ByteString.copyFromUtf8(text != null ? text : ""));
      // Empty means "no real content type"; whitespace-only values are preserved as-is.
      b.setContentType(
          contentTypeProp != null && !contentTypeProp.isEmpty() ? contentTypeProp : "text/plain");
    }

    java.util.Enumeration<?> names = msg.getPropertyNames();
    while (names.hasMoreElements()) {
      String name = names.nextElement().toString();
      try {
        b.putHeaders(name, String.valueOf(msg.getObjectProperty(name)));
      } catch (Exception ignored) {
      }
    }
    return b.build();
  }
}
