package org.rustcamel.jms;

import java.io.Serializable;
import javax.jms.BytesMessage;
import javax.jms.Destination;
import javax.jms.MapMessage;
import javax.jms.Message;
import javax.jms.MessageConsumer;
import javax.jms.MessageListener;
import javax.jms.MessageProducer;
import javax.jms.ObjectMessage;
import javax.jms.Queue;
import javax.jms.QueueBrowser;
import javax.jms.Session;
import javax.jms.StreamMessage;
import javax.jms.TemporaryQueue;
import javax.jms.TemporaryTopic;
import javax.jms.TextMessage;
import javax.jms.Topic;
import javax.jms.TopicSubscriber;

/**
 * Minimal broker-free JMS session stub. Every operation is a no-op so an unexpected call from a
 * pooled connection never fails with a null pointer.
 */
final class StubJmsSession implements Session {

  @Override
  public BytesMessage createBytesMessage() {
    return null;
  }

  @Override
  public MapMessage createMapMessage() {
    return null;
  }

  @Override
  public Message createMessage() {
    return null;
  }

  @Override
  public ObjectMessage createObjectMessage() {
    return null;
  }

  @Override
  public ObjectMessage createObjectMessage(Serializable object) {
    return null;
  }

  @Override
  public StreamMessage createStreamMessage() {
    return null;
  }

  @Override
  public TextMessage createTextMessage() {
    return null;
  }

  @Override
  public TextMessage createTextMessage(String text) {
    return null;
  }

  @Override
  public boolean getTransacted() {
    return false;
  }

  @Override
  public int getAcknowledgeMode() {
    return 0;
  }

  @Override
  public void commit() {}

  @Override
  public void rollback() {}

  @Override
  public void close() {}

  @Override
  public void recover() {}

  @Override
  public MessageListener getMessageListener() {
    return null;
  }

  @Override
  public void setMessageListener(MessageListener listener) {}

  @Override
  public void run() {}

  @Override
  public MessageProducer createProducer(Destination destination) {
    return null;
  }

  @Override
  public MessageConsumer createConsumer(Destination destination) {
    return null;
  }

  @Override
  public MessageConsumer createConsumer(Destination destination, String messageSelector) {
    return null;
  }

  @Override
  public MessageConsumer createConsumer(
      Destination destination, String messageSelector, boolean noLocal) {
    return null;
  }

  @Override
  public MessageConsumer createSharedConsumer(Topic topic, String sharedSubscriptionName) {
    return null;
  }

  @Override
  public MessageConsumer createSharedConsumer(
      Topic topic, String sharedSubscriptionName, String messageSelector) {
    return null;
  }

  @Override
  public Queue createQueue(String queueName) {
    return null;
  }

  @Override
  public Topic createTopic(String topicName) {
    return null;
  }

  @Override
  public TopicSubscriber createDurableSubscriber(Topic topic, String name) {
    return null;
  }

  @Override
  public TopicSubscriber createDurableSubscriber(
      Topic topic, String name, String messageSelector, boolean noLocal) {
    return null;
  }

  @Override
  public MessageConsumer createDurableConsumer(Topic topic, String name) {
    return null;
  }

  @Override
  public MessageConsumer createDurableConsumer(
      Topic topic, String name, String messageSelector, boolean noLocal) {
    return null;
  }

  @Override
  public MessageConsumer createSharedDurableConsumer(Topic topic, String name) {
    return null;
  }

  @Override
  public MessageConsumer createSharedDurableConsumer(
      Topic topic, String name, String messageSelector) {
    return null;
  }

  @Override
  public QueueBrowser createBrowser(Queue queue) {
    return null;
  }

  @Override
  public QueueBrowser createBrowser(Queue queue, String messageSelector) {
    return null;
  }

  @Override
  public TemporaryQueue createTemporaryQueue() {
    return null;
  }

  @Override
  public TemporaryTopic createTemporaryTopic() {
    return null;
  }

  @Override
  public void unsubscribe(String name) {}
}
