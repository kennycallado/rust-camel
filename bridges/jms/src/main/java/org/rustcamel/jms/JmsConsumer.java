package org.rustcamel.jms;

import com.google.protobuf.ByteString;
import io.grpc.stub.StreamObserver;
import jakarta.enterprise.context.Dependent;
import jakarta.inject.Inject;
import javax.jms.BytesMessage;
import javax.jms.Connection;
import javax.jms.Destination;
import javax.jms.JMSException;
import javax.jms.Message;
import javax.jms.MessageConsumer;
import javax.jms.Session;
import javax.jms.TextMessage;
import java.util.concurrent.atomic.AtomicBoolean;
import jms_bridge.JmsMessage;
import org.jboss.logging.Logger;

@Dependent
public class JmsConsumer {
    private static final Logger LOG = Logger.getLogger(JmsConsumer.class);
    @Inject JmsClientFactory factory;

    private volatile Connection connection;
    private volatile Session session;
    private volatile MessageConsumer consumer;
    private volatile boolean running = false;
    private final AtomicBoolean resourcesClosed = new AtomicBoolean(false);

    /**
     * Subscribe to a JMS destination and forward messages to the gRPC stream.
     *
     * Uses synchronous polling (receive with timeout) instead of async
     * MessageListener. The previous MessageListener approach caused the JMS
     * delivery thread to block on gRPC's responseObserver.onNext() — which
     * can stall when Vert.x back-pressures the stream. With AUTO_ACKNOWLEDGE
     * the broker won't dispatch the next message until onMessage returns,
     * so the consumer silently stopped after the first message.
     *
     * Polling on a dedicated thread avoids this: the thread owns both the
     * JMS receive and the gRPC write, so there is no cross-thread blocking.
     */
    public void subscribe(String destination, String subscriptionId, StreamObserver<JmsMessage> observer) {
        running = true;
        resourcesClosed.set(false);

        Thread t = new Thread(() -> {
            try {
                connection = factory.createDedicatedConnection();
            } catch (Exception e) {
                if (running) observer.onError(e);
                else observer.onCompleted();
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
                        observer.onNext(grpcMsg);
                    } catch (Exception e) {
                        LOG.error("Error forwarding message: " + e.getMessage(), e);
                        if (running) {
                            observer.onError(e);
                            return;
                        }
                    }
                }

                observer.onCompleted();
            } catch (Exception e) {
                if (running) observer.onError(e);
                else observer.onCompleted();
            } finally {
                closeResources();
            }
        }, "jms-consumer-" + subscriptionId);
        t.setDaemon(true);
        t.start();
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

        if (msg instanceof BytesMessage bm) {
            byte[] buf = new byte[(int) bm.getBodyLength()];
            bm.readBytes(buf);
            b.setBody(ByteString.copyFrom(buf));
        } else if (msg instanceof TextMessage tm) {
            b.setBody(ByteString.copyFromUtf8(tm.getText() != null ? tm.getText() : ""));
            b.setContentType("text/plain");
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
