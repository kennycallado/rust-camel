package org.rustcamel.jms;

import jakarta.enterprise.context.ApplicationScoped;
import jakarta.inject.Inject;
import jakarta.annotation.PreDestroy;
import javax.jms.Connection;
import javax.jms.ConnectionFactory;
import java.net.URI;
import java.util.HashMap;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.atomic.AtomicBoolean;
import org.apache.activemq.ActiveMQConnectionFactory;
import org.apache.activemq.openwire.OpenWireFormatFactory;
import org.apache.activemq.pool.PooledConnectionFactory;
import org.apache.activemq.transport.TransportFactory;
import org.apache.activemq.transport.tcp.TcpTransportFactory;
import org.apache.activemq.util.FactoryFinder;
import org.apache.activemq.artemis.api.core.TransportConfiguration;
import org.apache.activemq.artemis.core.remoting.impl.netty.NettyConnectorFactory;
import org.apache.activemq.artemis.core.remoting.impl.netty.TransportConstants;
import org.messaginghub.pooled.jms.JmsPoolConnectionFactory;

@ApplicationScoped
public class JmsClientFactory {
    @Inject BridgeConfig config;

    private static final AtomicBoolean NATIVE_INIT_DONE = new AtomicBoolean(false);

    private volatile ConnectionFactory factory;
    // Raw (non-pooled) factory kept for long-lived consumer connections.
    // The pool silently recycles connections after idle timeout, breaking
    // MessageListeners without any error. Consumers use this directly.
    private volatile ConnectionFactory rawFactory;

    public ConnectionFactory get() {
        if (factory == null) {
            synchronized (this) {
                if (factory == null) {
                    factory = createFactory();
                }
            }
        }
        return factory;
    }

    /**
     * Health check using a bare (non-pooled) connection factory.
     * This avoids commons-pool2 reflection issues in GraalVM native images.
     */
    public void checkHealth() throws Exception {
        initNativeImageWorkarounds();

        String type = config.brokerType();
        String url = config.brokerUrl();
        String user = config.username();
        String pass = config.password();

        switch (type) {
            case "activemq": {
                ActiveMQConnectionFactory cf = buildActiveMqFactory(url, user, pass);
                try (Connection c = cf.createConnection()) {
                    c.start();
                }
                break;
            }
            case "artemis": {
                // Use the ServerLocator directly with createSessionFactory(tc)
                // to bypass waitForTopology(). The no-arg createSessionFactory()
                // always waits for a CLUSTER_TOPOLOGY packet from the broker,
                // which never arrives in GraalVM native image (Netty callback
                // not processed), causing an infinite hang / timeout.
                var cf = buildArtemisFactory(url, user, pass);
                var locator = cf.getServerLocator();
                var tc = locator.getStaticTransportConfigurations()[0];
                try (var csf = locator.createSessionFactory(tc)) {
                    // Successfully created a session factory → broker is reachable
                } finally {
                    cf.close();
                }
                break;
            }
            default:
                throw new IllegalArgumentException(
                    "Unsupported broker_type: '" + type + "'. Valid values: activemq, artemis"
                );
        }
    }

    /**
     * Creates a JMS connection from the pool.
     *
     * Credentials are configured on the underlying factory (ActiveMQConnectionFactory
     * for Artemis, ActiveMQConnectionFactory for Classic) via setUser/setPassword or
     * setUserName/setPassword. The pool propagates them automatically when it creates
     * new physical connections — no need to pass them here.
     *
     * Passing credentials to JmsPoolConnectionFactory.createConnection(user, pass)
     * creates a separate pool bucket keyed by (user, pass), which can cause pool
     * exhaustion and deadlocks under concurrent access in GraalVM native image.
     */
    public Connection createConnection() throws javax.jms.JMSException {
        return get().createConnection();
    }

    /**
     * Creates a dedicated (non-pooled) connection for long-lived consumers.
     * The pool silently recycles idle connections, breaking MessageListeners.
     */
    public Connection createDedicatedConnection() throws javax.jms.JMSException {
        if (rawFactory == null) {
            get(); // ensure createFactory() has run and rawFactory is set
        }
        return rawFactory.createConnection();
    }

    public synchronized void reset() {
        if (factory instanceof PooledConnectionFactory pool) {
            try { pool.stop(); } catch (Exception ignored) {}
        } else if (factory instanceof JmsPoolConnectionFactory pool) {
            try { pool.stop(); } catch (Exception ignored) {}
        }
        factory = null;
    }

    private ConnectionFactory createFactory() {
        String type = config.brokerType();
        String url = config.brokerUrl();
        String user = config.username();
        String pass = config.password();

        switch (type) {
            case "activemq": {
                ActiveMQConnectionFactory cf = buildActiveMqFactory(url, user, pass);
                rawFactory = cf;
                PooledConnectionFactory pool = new PooledConnectionFactory(cf);
                pool.setMaxConnections(5);
                pool.start();
                return pool;
            }
            case "artemis": {
                var cf = buildArtemisFactory(url, user, pass);
                rawFactory = cf;
                JmsPoolConnectionFactory pool = new JmsPoolConnectionFactory();
                pool.setConnectionFactory(cf);
                pool.setMaxConnections(5);
                pool.start();
                return pool;
            }
            default: {
                throw new IllegalArgumentException(
                    "Unsupported broker_type: '" + type + "'. Valid values: activemq, artemis"
                );
            }
        }
    }

    private ActiveMQConnectionFactory buildActiveMqFactory(String url, String user, String pass) {
        initNativeImageWorkarounds();
        ActiveMQConnectionFactory cf = new ActiveMQConnectionFactory(url);
        if (user != null) cf.setUserName(user);
        if (pass != null) cf.setPassword(pass);
        return cf;
    }

    /**
     * Builds an Artemis connection factory by constructing TransportConfiguration
     * directly, bypassing URI parsing and BeanSupport.
     *
     * BeanSupport uses commons-beanutils which triggers Class.forName() chains
     * that fail in GraalVM native image. By constructing the transport config
     * manually from the URL, we eliminate that dependency.
     *
     * Key native-image considerations:
     * - useEpoll/useKQueue forced to false (Epoll not supported in SubstrateVM)
     * - reconnectAttempts set to allow retries on transient failures
     */
    private static org.apache.activemq.artemis.jms.client.ActiveMQConnectionFactory
            buildArtemisFactory(String url, String user, String pass) {
        URI uri = URI.create(url);
        String host = uri.getHost() != null ? uri.getHost() : "localhost";
        int port = uri.getPort() > 0 ? uri.getPort() : 61616;

        Map<String, Object> params = new HashMap<>();
        params.put(TransportConstants.HOST_PROP_NAME, host);
        params.put(TransportConstants.PORT_PROP_NAME, port);
        // Force NIO transport — Epoll/KQueue don't work in GraalVM native image
        params.put(TransportConstants.USE_EPOLL_PROP_NAME, false);
        params.put(TransportConstants.USE_KQUEUE_PROP_NAME, false);
        // Bound connection and call timeouts so health checks never block indefinitely.
        // GraalVM native image Netty initialization can stall under mandatory auth
        // without these — causing Rust's wait_for_health to time out.
        // Artemis expects handshake-timeout in milliseconds. Using `5` here
        // means 5ms (not 5s) and causes connection setup to fail repeatedly.
        params.put(TransportConstants.HANDSHAKE_TIMEOUT, 5_000);          // ms (int)
        params.put(TransportConstants.NETTY_CONNECT_TIMEOUT, 5_000);       // ms (int)

        TransportConfiguration tc = new TransportConfiguration(
            NettyConnectorFactory.class.getName(), params);

        var cf = new org.apache.activemq.artemis.jms.client.ActiveMQConnectionFactory(false, tc);
        cf.setReconnectAttempts(3);
        cf.setRetryInterval(1000);
        // Disable consumer-side pre-fetching. By default Artemis buffers up to
        // 1 MiB of messages on the client. With 0 the broker pushes one message
        // at a time, which avoids the scenario where the consumer's internal
        // buffer stalls in GraalVM native image and receive() never returns
        // new messages even though they exist on the broker.
        cf.setConsumerWindowSize(0);
        // Set TTL on the factory itself (not in TransportConstants where
        // it is silently ignored for client-side connectors).
        cf.setConnectionTTL(300_000);    // 5 min
        // Disable producer-side flow control. Artemis Core protocol assigns a
        // limited credit window (producerWindowSize, default ~64 KiB) to each
        // producer. When credits are exhausted the send() call blocks waiting
        // for the broker to grant more. In GraalVM native image + Netty NIO
        // the credit-grant callback can stall, causing send() to block
        // indefinitely after a few messages. Setting -1 disables the credit
        // mechanism entirely — the producer never waits for credits.
        cf.setProducerWindowSize(-1);
        if (user != null) cf.setUser(user);
        if (pass != null) cf.setPassword(pass);
        return cf;
    }

    /**
     * One-time workarounds for GraalVM native image.
     *
     * ActiveMQ Classic uses FactoryFinder (a custom service-loader) that reads
     * META-INF/services/... files at runtime via ClassLoader.getResourceAsStream(),
     * then does Class.forName(name).getConstructor().newInstance().
     *
     * In native image this chain is fragile: resource loading may fail silently,
     * and reflective constructor access needs explicit registration. We replace
     * the entire FactoryFinder.ObjectFactory with one that knows about all
     * ActiveMQ service classes and instantiates them directly — zero reflection,
     * zero resource loading.
     */
    private static void initNativeImageWorkarounds() {
        if (!NATIVE_INIT_DONE.compareAndSet(false, true)) return;

        // Register TCP transport factory eagerly (bypass TRANSPORT_FACTORY_FINDER)
        TransportFactory.registerTransportFactory("tcp", new TcpTransportFactory());

        // Replace FactoryFinder's ObjectFactory with a native-safe version
        final FactoryFinder.ObjectFactory originalFactory = FactoryFinder.getObjectFactory();
        final Map<String, java.util.function.Supplier<Object>> knownServices = new ConcurrentHashMap<>();

        // Wire format factories
        knownServices.put("META-INF/services/org/apache/activemq/wireformat/default",
            OpenWireFormatFactory::new);

        // Transport factories
        knownServices.put("META-INF/services/org/apache/activemq/transport/tcp",
            TcpTransportFactory::new);
        knownServices.put("META-INF/services/org/apache/activemq/transport/failover",
            org.apache.activemq.transport.failover.FailoverTransportFactory::new);
        knownServices.put("META-INF/services/org/apache/activemq/transport/nio",
            org.apache.activemq.transport.nio.NIOTransportFactory::new);

        FactoryFinder.setObjectFactory(path -> {
            java.util.function.Supplier<Object> supplier = knownServices.get(path);
            if (supplier != null) {
                return supplier.get();
            }
            return originalFactory.create(path);
        });
    }

    @PreDestroy
    public void close() {
        reset();
    }
}
