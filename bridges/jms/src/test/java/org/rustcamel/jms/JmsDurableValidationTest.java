package org.rustcamel.jms;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertInstanceOf;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;
import static org.mockito.ArgumentMatchers.any;
import static org.mockito.ArgumentMatchers.anyLong;
import static org.mockito.Mockito.inOrder;
import static org.mockito.Mockito.mock;
import static org.mockito.Mockito.never;
import static org.mockito.Mockito.verify;
import static org.mockito.Mockito.when;

import io.grpc.Status;
import io.grpc.StatusRuntimeException;
import io.grpc.stub.StreamObserver;
import jakarta.enterprise.inject.Instance;
import java.util.Map;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicReference;
import javax.jms.Connection;
import javax.jms.Destination;
import javax.jms.MessageConsumer;
import javax.jms.Session;
import javax.jms.Topic;
import jms_bridge.JmsMessage;
import jms_bridge.SubscribeRequest;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;
import org.mockito.ArgumentCaptor;
import org.mockito.InOrder;
import org.mockito.Mock;
import org.mockito.MockitoAnnotations;

/**
 * Durable topic subscriber construction and identity validation (task 1.4).
 *
 * <p>A durable identity is the pair {@code client_id} + {@code subscription_name}. An incomplete
 * pair, or any durable identity on a queue destination, must fail the subscribe permanently with
 * {@code INVALID_ARGUMENT} synchronously in {@link JmsConsumer#subscribe} — before the owner thread
 * exists and before any broker connection is created, so no partial subscription is ever attempted.
 * A complete pair on a topic sets the client id before {@code start()} and creates the consumer
 * with {@code createDurableConsumer(topic, subscriptionName, selector, false)} (D2: {@code
 * noLocal=false}).
 */
class JmsDurableValidationTest {

  private static final String TOPIC_DESTINATION = "topic:prices";
  private static final String QUEUE_DESTINATION = "queue:orders";
  private static final String CLIENT_ID = "app-1";
  private static final String SUBSCRIPTION_NAME = "snapshots";
  private static final String SELECTOR = "priority > 5";
  private static final long AWAIT_SECONDS = 15L;
  private static final long STOP_TIMEOUT_MS = 15_000L;

  @Mock JmsClientFactory factory;
  @Mock Connection connection;
  @Mock Session session;
  @Mock MessageConsumer messageConsumer;
  @Mock Topic topic;

  private AutoCloseable mocks;

  @BeforeEach
  void setUp() throws Exception {
    mocks = MockitoAnnotations.openMocks(this);
    when(factory.createDedicatedConnection()).thenReturn(connection);
    when(connection.createSession(false, Session.CLIENT_ACKNOWLEDGE)).thenReturn(session);
    when(session.createTopic("prices")).thenReturn(topic);
    when(messageConsumer.receive(anyLong())).thenReturn(null);
  }

  @AfterEach
  void tearDown() throws Exception {
    mocks.close();
  }

  @Test
  void halfSpecifiedIdentityFailsPermanentlyBeforeOwnerThread() throws Exception {
    JmsConsumer consumer = newConsumer();
    consumer.clientId = CLIENT_ID;
    consumer.subscriptionName = "";

    StatusRuntimeException error =
        assertThrows(
            StatusRuntimeException.class,
            () ->
                consumer.subscribe(
                    TOPIC_DESTINATION,
                    "sub-durable-half",
                    new JmsConsumerBodyCapTest.RecordingObserver(),
                    new AtomicBoolean(false)));

    Status status = Status.fromThrowable(error);
    assertEquals(Status.Code.INVALID_ARGUMENT, status.getCode());
    String description = status.getDescription();
    assertTrue(description.contains("client_id"), "must name client_id: " + description);
    assertTrue(
        description.contains("subscription_name"), "must name subscription_name: " + description);
    assertFalse(description.contains(CLIENT_ID), "must not echo the identity: " + description);
    // No owner thread, hence no partial subscription and no broker connection.
    assertTrue(consumer.awaitStopped(0L), "no owner thread may be created on invalid identity");
    assertFalse(
        ownerThreadExists("jms-consumer-sub-durable-half"),
        "no jms-consumer- thread may be created on invalid identity");
    verify(factory, never()).createDedicatedConnection();
  }

  @Test
  void durableIdentityOnQueueFailsPermanentlyBeforeOwnerThread() throws Exception {
    JmsConsumer consumer = newConsumer();
    consumer.clientId = CLIENT_ID;
    consumer.subscriptionName = SUBSCRIPTION_NAME;

    StatusRuntimeException error =
        assertThrows(
            StatusRuntimeException.class,
            () ->
                consumer.subscribe(
                    QUEUE_DESTINATION,
                    "sub-durable-queue",
                    new JmsConsumerBodyCapTest.RecordingObserver(),
                    new AtomicBoolean(false)));

    Status status = Status.fromThrowable(error);
    assertEquals(Status.Code.INVALID_ARGUMENT, status.getCode());
    String description = status.getDescription();
    assertTrue(description.contains("topic"), "must state the topic requirement: " + description);
    assertTrue(description.contains("client_id"), "must name client_id: " + description);
    assertTrue(
        description.contains("subscription_name"), "must name subscription_name: " + description);
    assertFalse(
        description.contains(CLIENT_ID) || description.contains(SUBSCRIPTION_NAME),
        "must not echo the identity: " + description);
    assertTrue(consumer.awaitStopped(0L), "no owner thread may be created on invalid identity");
    assertFalse(
        ownerThreadExists("jms-consumer-sub-durable-queue"),
        "no jms-consumer- thread may be created on invalid identity");
    verify(factory, never()).createDedicatedConnection();
  }

  /**
   * The service wires the request's durable fields onto the injected consumer exactly like {@code
   * messageSelector}: the values are visible on the instance when {@code subscribe()} is entered.
   */
  @Test
  void subscribeRequestFieldsReachConsumer() {
    JmsBridgeService service = newService();
    AtomicReference<String> clientIdSeen = new AtomicReference<>();
    AtomicReference<String> subscriptionNameSeen = new AtomicReference<>();
    AtomicReference<String> selectorSeen = new AtomicReference<>();
    JmsConsumer consumer =
        new JmsConsumer() {
          @Override
          public void subscribe(
              String destination,
              String subscriptionId,
              StreamObserver<JmsMessage> observer,
              AtomicBoolean finished) {
            clientIdSeen.set(clientId);
            subscriptionNameSeen.set(subscriptionName);
            selectorSeen.set(messageSelector);
          }
        };
    @SuppressWarnings("unchecked")
    Instance<JmsConsumer> instance = mock(Instance.class);
    when(instance.get()).thenReturn(consumer);
    service.consumerFactory = instance;
    @SuppressWarnings("unchecked")
    StreamObserver<JmsMessage> observer = mock(StreamObserver.class);
    SubscribeRequest request =
        SubscribeRequest.newBuilder()
            .setDestination(TOPIC_DESTINATION)
            .setSubscriptionId("sub-durable-wiring")
            .setMessageSelector(SELECTOR)
            .setClientId(CLIENT_ID)
            .setSubscriptionName(SUBSCRIPTION_NAME)
            .build();

    service.subscribe(request, observer);

    assertEquals(CLIENT_ID, clientIdSeen.get());
    assertEquals(SUBSCRIPTION_NAME, subscriptionNameSeen.get());
    assertEquals(SELECTOR, selectorSeen.get());
    verify(observer, never()).onError(any());
  }

  /** D2 pinned: the durable branch calls {@code createDurableConsumer(..., noLocal=false)}. */
  @Test
  void durableConsumerCreatedWithNoLocalFalse() throws Exception {
    CountDownLatch created = new CountDownLatch(1);
    when(session.createDurableConsumer(topic, SUBSCRIPTION_NAME, SELECTOR, false))
        .thenAnswer(
            inv -> {
              created.countDown();
              return messageConsumer;
            });
    JmsConsumer consumer = newConsumer();
    consumer.clientId = CLIENT_ID;
    consumer.subscriptionName = SUBSCRIPTION_NAME;
    consumer.messageSelector = SELECTOR;

    consumer.subscribe(
        TOPIC_DESTINATION,
        "sub-durable-create",
        new JmsConsumerBodyCapTest.RecordingObserver(),
        new AtomicBoolean(false));

    assertTrue(
        created.await(AWAIT_SECONDS, TimeUnit.SECONDS), "the durable consumer must be created");
    // The client id is visible on the connection before start().
    InOrder order = inOrder(connection);
    order.verify(connection).setClientID(CLIENT_ID);
    order.verify(connection).start();
    verify(session).createDurableConsumer(topic, SUBSCRIPTION_NAME, SELECTOR, false);
    verify(session, never()).createConsumer(any(Destination.class));
    verify(session, never()).createConsumer(any(Destination.class), any());

    consumer.stop();
    assertTrue(consumer.awaitStopped(STOP_TIMEOUT_MS), "the owner thread must terminate");
  }

  /**
   * A request-level durable validation failure must release the registration: the service forwards
   * the consumer's {@code INVALID_ARGUMENT} status and, because {@code cleanupSubscription} ran,
   * the same {@code subscription_id} is immediately reusable for a valid (complete) identity
   * instead of being refused with {@code ALREADY_EXISTS}.
   */
  @Test
  void serviceSubscribeSurfacesInvalidArgumentWithoutStaleEntry() throws Exception {
    JmsBridgeService service = newService();
    JmsConsumer invalid = newConsumer();
    JmsConsumer valid = newConsumer();
    @SuppressWarnings("unchecked")
    Instance<JmsConsumer> instance = mock(Instance.class);
    when(instance.get()).thenReturn(invalid, valid);
    service.consumerFactory = instance;
    @SuppressWarnings("unchecked")
    StreamObserver<JmsMessage> invalidObserver = mock(StreamObserver.class);
    @SuppressWarnings("unchecked")
    StreamObserver<JmsMessage> validObserver = mock(StreamObserver.class);

    SubscribeRequest halfSpecified =
        SubscribeRequest.newBuilder()
            .setDestination(TOPIC_DESTINATION)
            .setSubscriptionId("sub-durable-service")
            .setClientId(CLIENT_ID)
            .build();
    service.subscribe(halfSpecified, invalidObserver);

    ArgumentCaptor<Throwable> error = ArgumentCaptor.forClass(Throwable.class);
    verify(invalidObserver).onError(error.capture());
    assertInstanceOf(StatusRuntimeException.class, error.getValue());
    assertEquals(Status.Code.INVALID_ARGUMENT, Status.fromThrowable(error.getValue()).getCode());
    verify(instance).destroy(invalid);
    assertTrue(invalid.awaitStopped(0L), "the rejected consumer never started an owner thread");

    // The same subscription_id with a complete identity must start a stream: a stale
    // activeConsumers entry would have been answered with ALREADY_EXISTS instead.
    CountDownLatch started = new CountDownLatch(1);
    when(session.createDurableConsumer(topic, SUBSCRIPTION_NAME, null, false))
        .thenAnswer(
            inv -> {
              started.countDown();
              return messageConsumer;
            });
    try {
      service.subscribe(
          halfSpecified.toBuilder().setSubscriptionName(SUBSCRIPTION_NAME).build(), validObserver);

      assertTrue(
          started.await(AWAIT_SECONDS, TimeUnit.SECONDS),
          "the reused subscription_id must reach the broker subscription");
      verify(validObserver, never()).onError(any());
    } finally {
      valid.stop();
      assertTrue(valid.awaitStopped(STOP_TIMEOUT_MS), "the owner thread must terminate");
    }
  }

  private JmsConsumer newConsumer() {
    JmsConsumer consumer = new JmsConsumer();
    consumer.factory = factory;
    return consumer;
  }

  private JmsBridgeService newService() {
    JmsBridgeService service = new JmsBridgeService();
    service.config = new BridgeConfig(Map.of());
    service.producer = mock(JmsProducer.class);
    service.clientFactory = mock(JmsClientFactory.class);
    return service;
  }

  private static boolean ownerThreadExists(String name) {
    for (Thread thread : Thread.getAllStackTraces().keySet()) {
      if (name.equals(thread.getName()) && thread.isAlive()) {
        return true;
      }
    }
    return false;
  }
}
