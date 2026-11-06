package org.rustcamel.jms;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertInstanceOf;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertNull;

import java.util.ArrayList;
import java.util.HashSet;
import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.concurrent.atomic.AtomicInteger;
import javax.jms.Connection;
import javax.jms.InvalidClientIDException;
import javax.jms.JMSException;
import javax.jms.Message;
import javax.jms.MessageConsumer;
import javax.jms.MessageProducer;
import javax.jms.Session;
import javax.jms.TextMessage;
import javax.jms.Topic;
import org.apache.activemq.ActiveMQConnectionFactory;
import org.apache.activemq.broker.BrokerService;
import org.apache.activemq.transport.vm.VMTransportFactory;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.DisplayName;
import org.junit.jupiter.api.Test;

/**
 * Durable topic subscriber behavior against a real embedded ActiveMQ Classic broker (task 1.5).
 *
 * <p>Every test gets its own broker ({@code vm://durableTest-N}) so durable subscriptions, topics
 * and client ids never leak between tests. Connections are created through the production {@link
 * JmsClientFactory} seam — the same {@code buildActiveMqFactory} path the bridge uses, including
 * its unconditional client-ID prefix — so the tests pin the real factory, not a test double.
 *
 * <p>The broker is constructed programmatically ({@link BrokerService}) and reached over {@code
 * vm://}: the URL is the transport the production factory must resolve, while the programmatic
 * service gives the test a deterministic lifecycle ({@code stop()} in {@link #stopBroker()})
 * without relying on autostart side effects.
 */
@DisplayName("JMS durable topic subscribers on an embedded broker")
class JmsDurableSubscriberTest {

  private static final String CLIENT_ID = "app-1";
  private static final String SUBSCRIPTION_NAME = "snapshots";
  private static final String TOPIC_NAME = "prices";
  private static final String SELECTOR = "priority > 5";
  private static final long RECEIVE_TIMEOUT_MS = 5_000L;
  private static final long QUIET_TIMEOUT_MS = 500L;
  private static final AtomicInteger BROKER_SEQUENCE = new AtomicInteger();

  private BrokerService broker;
  private String brokerUrl;
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

    String brokerName = "durableTest-" + BROKER_SEQUENCE.incrementAndGet();
    broker = new BrokerService();
    broker.setBrokerName(brokerName);
    broker.setPersistent(false);
    broker.setUseJmx(false);
    broker.start();
    broker.waitUntilStarted();

    brokerUrl = "vm://" + brokerName + "?broker.persistent=false&broker.useJmx=false";
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

  /**
   * D4: an explicitly configured client id wins over the factory's generated prefix, and the
   * production factory always supplies a prefix so a durable identity never falls back to
   * per-process generated state.
   */
  @Test
  void explicitClientIdOverridesPrefix() throws Exception {
    ActiveMQConnectionFactory factory =
        JmsClientFactory.buildActiveMqFactory(brokerUrl, null, null);
    assertNotNull(
        factory.getClientIDPrefix(),
        "the production factory must always set a client ID prefix (setClientIDPrefix is"
            + " unconditional)");

    Connection connection = dedicatedConnection();
    connection.setClientID(CLIENT_ID);
    connection.start();

    assertEquals(
        CLIENT_ID,
        connection.getClientID(),
        "the explicit client id must win over the generated prefix");
    Session session = connection.createSession(false, Session.CLIENT_ACKNOWLEDGE);
    MessageConsumer consumer =
        session.createDurableConsumer(topic(session), SUBSCRIPTION_NAME, null, false);
    assertNotNull(consumer, "the durable consumer must be created on the topic");
  }

  /** The durable subscription survives a disconnect: nothing published while away is lost. */
  @Test
  void durableSubscriptionSurvivesDisconnect() throws Exception {
    Connection consumerConnection = dedicatedConnection();
    consumerConnection.setClientID(CLIENT_ID);
    consumerConnection.start();
    MessageConsumer first = durableConsumer(consumerConnection, null);

    Connection producerConnection = dedicatedConnection();
    producerConnection.start();
    publish(producerConnection, "before-disconnect");
    Message live = first.receive(RECEIVE_TIMEOUT_MS);
    assertEquals(
        "before-disconnect",
        text(live),
        "the attached durable consumer must receive the live message");
    live.acknowledge();

    first.close();
    consumerConnection.close();
    publish(producerConnection, "offline-1", "offline-2");

    Connection reconnected = dedicatedConnection();
    reconnected.setClientID(CLIENT_ID);
    reconnected.start();
    MessageConsumer second = durableConsumer(reconnected, null);

    Set<String> received = new HashSet<>();
    for (int i = 0; i < 2; i++) {
      String body = text(second.receive(RECEIVE_TIMEOUT_MS));
      assertNotNull(body, "offline message " + (i + 1) + " must be delivered after reconnect");
      received.add(body);
    }
    assertEquals(
        Set.of("offline-1", "offline-2"),
        received,
        "every message published while disconnected must be delivered exactly once");
    assertNull(second.receive(QUIET_TIMEOUT_MS), "no extra message may remain queued");
  }

  /** A duplicate client id is rejected by type and classifies as {@code CLIENT_ID_IN_USE}. */
  @Test
  void collidingClientIdsFailLoudAsClientIdInUse() throws Exception {
    Connection first = dedicatedConnection();
    first.setClientID(CLIENT_ID);
    first.start();

    Connection second = dedicatedConnection();
    // ActiveMQ sends the ConnectionInfo when the client id is applied, so the broker's rejection
    // surfaces here rather than at start().
    Throwable failure = null;
    try {
      second.setClientID(CLIENT_ID);
      second.start();
    } catch (JMSException rejected) {
      failure = rejected;
    }

    assertNotNull(failure, "the broker must reject the duplicate client id");
    assertInstanceOf(
        InvalidClientIDException.class,
        failure,
        "the broker must reject a duplicate client id with InvalidClientIDException");
    assertEquals(
        BrokerFailureClass.CLIENT_ID_IN_USE,
        BrokerFailureClassifier.classify(failure),
        "the duplicate-id rejection must classify as CLIENT_ID_IN_USE");
    assertEquals(CLIENT_ID, first.getClientID(), "the first connection keeps the identity");
  }

  /**
   * Baseline for the capability gate: a plain (non-durable) subscriber misses everything published
   * while it is disconnected — exactly the silent downgrade a durable route must never accept.
   */
  @Test
  void nonDurableSubscriptionMissesOfflineMessages() throws Exception {
    Connection consumerConnection = dedicatedConnection();
    consumerConnection.start();
    Session session = consumerConnection.createSession(false, Session.CLIENT_ACKNOWLEDGE);
    MessageConsumer first = session.createConsumer(topic(session));

    Connection producerConnection = dedicatedConnection();
    producerConnection.start();
    publish(producerConnection, "attached");
    assertEquals(
        "attached",
        text(first.receive(RECEIVE_TIMEOUT_MS)),
        "the attached non-durable subscriber must receive the live message");

    consumerConnection.close();
    publish(producerConnection, "offline-1", "offline-2");

    Connection reconnected = dedicatedConnection();
    reconnected.start();
    Session reconnectedSession = reconnected.createSession(false, Session.CLIENT_ACKNOWLEDGE);
    MessageConsumer second = reconnectedSession.createConsumer(topic(reconnectedSession));
    assertNull(
        second.receive(QUIET_TIMEOUT_MS),
        "a non-durable subscriber must not receive messages published while disconnected");
  }

  /** A durable subscription applies its selector: only matching messages are delivered. */
  @Test
  void durableSubscriptionWithSelectorReceivesMatchingOnly() throws Exception {
    Connection consumerConnection = dedicatedConnection();
    consumerConnection.setClientID(CLIENT_ID);
    consumerConnection.start();
    MessageConsumer consumer = durableConsumer(consumerConnection, SELECTOR);

    Connection producerConnection = dedicatedConnection();
    producerConnection.start();
    Session producerSession = producerConnection.createSession(false, Session.AUTO_ACKNOWLEDGE);
    MessageProducer producer = producerSession.createProducer(topic(producerSession));
    TextMessage matching = producerSession.createTextMessage("matching");
    matching.setIntProperty("priority", 10);
    producer.send(matching);
    TextMessage nonMatching = producerSession.createTextMessage("non-matching");
    nonMatching.setIntProperty("priority", 1);
    producer.send(nonMatching);

    assertEquals(
        "matching",
        text(consumer.receive(RECEIVE_TIMEOUT_MS)),
        "the durable consumer must apply its selector");
    assertNull(
        consumer.receive(QUIET_TIMEOUT_MS),
        "the non-matching message must be filtered by the broker");
  }

  private Connection dedicatedConnection() throws JMSException {
    Connection connection = clientFactory.createDedicatedConnection();
    connections.add(connection);
    return connection;
  }

  private static MessageConsumer durableConsumer(Connection connection, String selector)
      throws JMSException {
    Session session = connection.createSession(false, Session.CLIENT_ACKNOWLEDGE);
    return session.createDurableConsumer(topic(session), SUBSCRIPTION_NAME, selector, false);
  }

  private static void publish(Connection connection, String... bodies) throws JMSException {
    Session session = connection.createSession(false, Session.AUTO_ACKNOWLEDGE);
    try {
      MessageProducer producer = session.createProducer(topic(session));
      for (String body : bodies) {
        producer.send(session.createTextMessage(body));
      }
    } finally {
      session.close();
    }
  }

  private static Topic topic(Session session) throws JMSException {
    return (Topic) JmsProducer.parseDestination(session, "topic:" + TOPIC_NAME);
  }

  private static String text(Message message) throws JMSException {
    return message == null ? null : ((TextMessage) message).getText();
  }
}
