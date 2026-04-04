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
import javax.jms.MessageListener;
import javax.jms.Session;
import javax.jms.TextMessage;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.atomic.AtomicBoolean;
import jms_bridge.JmsMessage;

@Dependent
public class JmsConsumer {
    @Inject JmsClientFactory factory;

    private volatile Connection connection;
    private volatile Session session;
    private volatile MessageConsumer consumer;
    private volatile CountDownLatch stopLatch;
    private volatile boolean running = false;
    private final AtomicBoolean resourcesClosed = new AtomicBoolean(false);

    public void subscribe(String destination, String subscriptionId, StreamObserver<JmsMessage> observer) {
        running = true;
        resourcesClosed.set(false);
        CountDownLatch latch = new CountDownLatch(1);
        stopLatch = latch;

        Thread t = new Thread(() -> {
            try {
                connection = factory.createConnection();
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

                consumer.setMessageListener(new MessageListener() {
                    @Override
                    public void onMessage(Message msg) {
                        if (!running) {
                            return;
                        }
                        try {
                            JmsMessage grpcMsg = convertMessage(msg, destination);
                            observer.onNext(grpcMsg);
                        } catch (Exception e) {
                            if (running) {
                                observer.onError(e);
                            }
                        }
                    }
                });

                latch.await();
                if (running) {
                    observer.onCompleted();
                }
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

        CountDownLatch latch = stopLatch;
        if (latch != null) {
            latch.countDown();
        }

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
