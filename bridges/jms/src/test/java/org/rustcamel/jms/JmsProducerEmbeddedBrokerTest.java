package org.rustcamel.jms;

import static org.junit.jupiter.api.Assertions.assertArrayEquals;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;

import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;
import java.util.Map;
import java.util.concurrent.atomic.AtomicInteger;
import javax.jms.BytesMessage;
import javax.jms.Connection;
import javax.jms.JMSException;
import javax.jms.Message;
import javax.jms.MessageConsumer;
import javax.jms.Session;
import org.apache.activemq.broker.BrokerService;
import org.apache.activemq.transport.vm.VMTransportFactory;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.DisplayName;
import org.junit.jupiter.api.Test;

/**
 * Outbound property forwarding against a real embedded ActiveMQ Classic broker (task 1.3).
 *
 * <p>Pins the pre-change behaviour the invalid-property policy must preserve: a legal-name write
 * reaches the broker unchanged and the send reports the broker-assigned message id. It also pins
 * the documented Classic-versus-Artemis legality difference: Classic tolerates a hyphenated
 * property name, so the same name that Artemis rejects (pinned by the CI-owned Rust row) sends
 * here.
 *
 * <p>Every test gets its own broker ({@code vm://producerTest-N}) so queues and connections never
 * leak between methods; connections are created through the production {@link JmsClientFactory}
 * seam, so the real {@code setStringProperty} path runs on a real broker.
 */
@DisplayName("JMS producer property forwarding on an embedded broker")
class JmsProducerEmbeddedBrokerTest {

  private static final long RECEIVE_TIMEOUT_MS = 5_000L;
  private static final AtomicInteger BROKER_SEQUENCE = new AtomicInteger();

  private BrokerService broker;
  private JmsClientFactory clientFactory;
  private final List<Connection> connections = new ArrayList<>();

  @BeforeEach
  void startBroker() throws Exception {
    // The production factory installs a native-image-safe FactoryFinder object factory that knows
    // only the broker schemes the bridge supports (tcp/ssl/failover/nio) and falls back to
    // ActiveMQ's default factory for anything else — and in 5.19.x that default's single-argument
    // create(String) is unsupported. The vm transport is test-only, so register it through the
    // package-visible known-services seam the production class exposes for tests.
    JmsClientFactory.knownServices()
        .put("META-INF/services/org/apache/activemq/transport/vm", VMTransportFactory::new);

    String brokerName = "producerTest-" + BROKER_SEQUENCE.incrementAndGet();
    broker = new BrokerService();
    broker.setBrokerName(brokerName);
    broker.setPersistent(false);
    broker.setUseJmx(false);
    broker.start();
    broker.waitUntilStarted();

    String brokerUrl = "vm://" + brokerName + "?broker.persistent=false&broker.useJmx=false";
    clientFactory = new JmsClientFactory();
    clientFactory.config =
        new BridgeConfig(Map.of("BRIDGE_BROKER_TYPE", "activemq", "BRIDGE_BROKER_URL", brokerUrl));
  }

  @AfterEach
  void stopBroker() throws Exception {
    for (Connection connection : connections) {
      try {
        connection.close();
      } catch (Exception ignored) {
      }
    }
    connections.clear();
    if (clientFactory != null) {
      clientFactory.close();
    }
    if (broker != null) {
      broker.stop();
      broker.waitUntilStopped();
    }
  }

  /** Legal names survive the round trip unchanged and the send reports the JMS message id. */
  @Test
  void legal_name_forwards_byte_identically_on_classic() throws Exception {
    JmsProducer producer = new JmsProducer();
    producer.factory = clientFactory;

    Connection consumerConnection = clientFactory.createDedicatedConnection();
    connections.add(consumerConnection);
    consumerConnection.start();
    Session consumerSession = consumerConnection.createSession(false, Session.AUTO_ACKNOWLEDGE);
    MessageConsumer consumer =
        consumerSession.createConsumer(consumerSession.createQueue("embedded-legal"));

    String messageId =
        producer.send(
            "queue:embedded-legal",
            "hello".getBytes(StandardCharsets.UTF_8),
            Map.of("traceId", "t-123", "orderId", "o-45"),
            null);

    assertNotNull(messageId, "the send must report the broker-assigned JMS message id");
    Message received = consumer.receive(RECEIVE_TIMEOUT_MS);
    assertNotNull(received, "the consumer must receive the forwarded message");
    assertEquals(
        "t-123",
        received.getStringProperty("traceId"),
        "traceId must be broker-visible with the forwarded value");
    assertEquals(
        "o-45",
        received.getStringProperty("orderId"),
        "orderId must be broker-visible with the forwarded value");
    assertArrayEquals(
        "hello".getBytes(StandardCharsets.UTF_8),
        body(received),
        "the body must be byte-identical");
  }

  /**
   * Classic tolerates hyphenated property names: the JMS identifier rule excludes {@code -} but the
   * Classic client marshals it, so a send carrying {@code x-crash-proof} succeeds. Artemis rejects
   * the same name; the CI-owned Rust row pins that half of the difference.
   */
  @Test
  void hyphenated_name_tolerated_on_classic_pin() throws Exception {
    JmsProducer producer = new JmsProducer();
    producer.factory = clientFactory;

    String messageId =
        producer.send(
            "queue:embedded-hyphen",
            "hello".getBytes(StandardCharsets.UTF_8),
            Map.of("x-crash-proof", "cv"),
            null);

    assertNotNull(
        messageId,
        "Classic must tolerate a hyphenated property name and report the broker-assigned message id");
  }

  private static byte[] body(Message message) throws JMSException {
    BytesMessage bytesMessage = (BytesMessage) message;
    byte[] buffer = new byte[(int) bytesMessage.getBodyLength()];
    int read = bytesMessage.readBytes(buffer);
    return Arrays.copyOf(buffer, read);
  }
}
