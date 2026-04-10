package org.rustcamel.jms;

import io.quarkus.runtime.annotations.RegisterForReflection;

/**
 * Registers classes for GraalVM native-image reflection.
 *
 * <p>Uses {@code classNames} (String-based) instead of {@code targets} (class-literal) to avoid
 * build-time class loading which can pull in classes with static Random fields (e.g. RandomUtil)
 * that cause "Random in image heap" build failures.
 *
 * <p>The classes listed here are loaded via Class.forName() at runtime by ActiveMQ Classic
 * (OpenWire protocol), Artemis JMS client, and connection pooling libraries.
 */
@RegisterForReflection(
    classNames = {
      // --- commons-pool2 (connection pooling) ---
      "org.apache.commons.pool2.impl.DefaultEvictionPolicy",

      // --- ActiveMQ Classic: OpenWire protocol ---
      "org.apache.activemq.openwire.OpenWireFormatFactory",
      "org.apache.activemq.openwire.OpenWireFormat",
      // Transport factories (loaded via META-INF/services/org/apache/activemq/transport/*)
      "org.apache.activemq.transport.tcp.TcpTransportFactory",
      "org.apache.activemq.transport.failover.FailoverTransportFactory",
      "org.apache.activemq.transport.nio.NIOTransportFactory",
      // Marshaller factories (one per OpenWire version — broker negotiates version at connect)
      "org.apache.activemq.openwire.v1.MarshallerFactory",
      "org.apache.activemq.openwire.v9.MarshallerFactory",
      "org.apache.activemq.openwire.v10.MarshallerFactory",
      "org.apache.activemq.openwire.v11.MarshallerFactory",
      "org.apache.activemq.openwire.v12.MarshallerFactory",

      // --- ActiveMQ Classic: v12 marshallers (all loaded dynamically by MarshallerFactory) ---
      "org.apache.activemq.openwire.v12.ActiveMQBlobMessageMarshaller",
      "org.apache.activemq.openwire.v12.ActiveMQBytesMessageMarshaller",
      "org.apache.activemq.openwire.v12.ActiveMQDestinationMarshaller",
      "org.apache.activemq.openwire.v12.ActiveMQMapMessageMarshaller",
      "org.apache.activemq.openwire.v12.ActiveMQMessageMarshaller",
      "org.apache.activemq.openwire.v12.ActiveMQObjectMessageMarshaller",
      "org.apache.activemq.openwire.v12.ActiveMQQueueMarshaller",
      "org.apache.activemq.openwire.v12.ActiveMQStreamMessageMarshaller",
      "org.apache.activemq.openwire.v12.ActiveMQTempDestinationMarshaller",
      "org.apache.activemq.openwire.v12.ActiveMQTempQueueMarshaller",
      "org.apache.activemq.openwire.v12.ActiveMQTempTopicMarshaller",
      "org.apache.activemq.openwire.v12.ActiveMQTextMessageMarshaller",
      "org.apache.activemq.openwire.v12.ActiveMQTopicMarshaller",
      "org.apache.activemq.openwire.v12.BaseCommandMarshaller",
      "org.apache.activemq.openwire.v12.BaseDataStreamMarshaller",
      "org.apache.activemq.openwire.v12.BrokerIdMarshaller",
      "org.apache.activemq.openwire.v12.BrokerInfoMarshaller",
      "org.apache.activemq.openwire.v12.BrokerSubscriptionInfoMarshaller",
      "org.apache.activemq.openwire.v12.ConnectionControlMarshaller",
      "org.apache.activemq.openwire.v12.ConnectionErrorMarshaller",
      "org.apache.activemq.openwire.v12.ConnectionIdMarshaller",
      "org.apache.activemq.openwire.v12.ConnectionInfoMarshaller",
      "org.apache.activemq.openwire.v12.ConsumerControlMarshaller",
      "org.apache.activemq.openwire.v12.ConsumerIdMarshaller",
      "org.apache.activemq.openwire.v12.ConsumerInfoMarshaller",
      "org.apache.activemq.openwire.v12.ControlCommandMarshaller",
      "org.apache.activemq.openwire.v12.DataArrayResponseMarshaller",
      "org.apache.activemq.openwire.v12.DataResponseMarshaller",
      "org.apache.activemq.openwire.v12.DestinationInfoMarshaller",
      "org.apache.activemq.openwire.v12.DiscoveryEventMarshaller",
      "org.apache.activemq.openwire.v12.ExceptionResponseMarshaller",
      "org.apache.activemq.openwire.v12.FlushCommandMarshaller",
      "org.apache.activemq.openwire.v12.IntegerResponseMarshaller",
      "org.apache.activemq.openwire.v12.JournalQueueAckMarshaller",
      "org.apache.activemq.openwire.v12.JournalTopicAckMarshaller",
      "org.apache.activemq.openwire.v12.JournalTraceMarshaller",
      "org.apache.activemq.openwire.v12.JournalTransactionMarshaller",
      "org.apache.activemq.openwire.v12.KeepAliveInfoMarshaller",
      "org.apache.activemq.openwire.v12.LastPartialCommandMarshaller",
      "org.apache.activemq.openwire.v12.LocalTransactionIdMarshaller",
      "org.apache.activemq.openwire.v12.MessageAckMarshaller",
      "org.apache.activemq.openwire.v12.MessageDispatchMarshaller",
      "org.apache.activemq.openwire.v12.MessageDispatchNotificationMarshaller",
      "org.apache.activemq.openwire.v12.MessageIdMarshaller",
      "org.apache.activemq.openwire.v12.MessageMarshaller",
      "org.apache.activemq.openwire.v12.MessagePullMarshaller",
      "org.apache.activemq.openwire.v12.NetworkBridgeFilterMarshaller",
      "org.apache.activemq.openwire.v12.PartialCommandMarshaller",
      "org.apache.activemq.openwire.v12.ProducerAckMarshaller",
      "org.apache.activemq.openwire.v12.ProducerIdMarshaller",
      "org.apache.activemq.openwire.v12.ProducerInfoMarshaller",
      "org.apache.activemq.openwire.v12.RemoveInfoMarshaller",
      "org.apache.activemq.openwire.v12.RemoveSubscriptionInfoMarshaller",
      "org.apache.activemq.openwire.v12.ReplayCommandMarshaller",
      "org.apache.activemq.openwire.v12.ResponseMarshaller",
      "org.apache.activemq.openwire.v12.SessionIdMarshaller",
      "org.apache.activemq.openwire.v12.SessionInfoMarshaller",
      "org.apache.activemq.openwire.v12.ShutdownInfoMarshaller",
      "org.apache.activemq.openwire.v12.SubscriptionInfoMarshaller",
      "org.apache.activemq.openwire.v12.TransactionIdMarshaller",
      "org.apache.activemq.openwire.v12.TransactionInfoMarshaller",
      "org.apache.activemq.openwire.v12.WireFormatInfoMarshaller",
      "org.apache.activemq.openwire.v12.XATransactionIdMarshaller",

      // --- Artemis: shaded commons-logging (loaded via LogFactory.getFactory() / Class.forName)
      // ---
      "org.apache.activemq.artemis.shaded.org.apache.commons.logging.LogFactory",
      "org.apache.activemq.artemis.shaded.org.apache.commons.logging.impl.LogFactoryImpl",
      "org.apache.activemq.artemis.shaded.org.apache.commons.logging.impl.Jdk14Logger",
      "org.apache.activemq.artemis.shaded.org.apache.commons.logging.impl.Jdk13LumberjackLogger",
      "org.apache.activemq.artemis.shaded.org.apache.commons.logging.impl.SimpleLog",
      "org.apache.activemq.artemis.shaded.org.apache.commons.logging.impl.NoOpLog",
      "org.apache.activemq.artemis.shaded.org.apache.commons.logging.impl.Slf4jLogFactory",

      // --- Artemis: load-balancing policies (loaded via Class.forName in ServerLocatorImpl) ---
      "org.apache.activemq.artemis.api.core.client.loadbalance.RoundRobinConnectionLoadBalancingPolicy",
      "org.apache.activemq.artemis.api.core.client.loadbalance.RandomConnectionLoadBalancingPolicy",
      "org.apache.activemq.artemis.api.core.client.loadbalance.RandomStickyConnectionLoadBalancingPolicy",
      "org.apache.activemq.artemis.api.core.client.loadbalance.FirstElementConnectionLoadBalancingPolicy",

      // --- Artemis: connector factory (loaded via Class.forName in TransportConfigurationUtil) ---
      "org.apache.activemq.artemis.core.remoting.impl.netty.NettyConnectorFactory",

      // --- Artemis: protocol manager factory ---
      "org.apache.activemq.artemis.core.protocol.core.impl.ActiveMQClientProtocolManagerFactory",

      // --- Artemis: URI schema parsers (loaded via ServiceLoader-like patterns) ---
      "org.apache.activemq.artemis.uri.schema.serverLocator.TCPServerLocatorSchema",
      "org.apache.activemq.artemis.uri.schema.serverLocator.InVMServerLocatorSchema",
      "org.apache.activemq.artemis.uri.schema.serverLocator.UDPServerLocatorSchema",

      // --- Artemis: JGroups logging (loaded dynamically, shaded) ---
      // NOTE: Log4J2LogImpl is intentionally EXCLUDED — it references log4j-core
      // classes not on the classpath (we only have log4j-api), causing build failure.
      "org.apache.activemq.artemis.shaded.org.jgroups.logging.Slf4jLogImpl",
      "org.apache.activemq.artemis.shaded.org.jgroups.logging.JDKLogImpl"
    })
public class NativeReflectionConfig {}
