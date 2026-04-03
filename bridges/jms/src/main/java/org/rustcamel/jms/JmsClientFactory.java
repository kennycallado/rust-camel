package org.rustcamel.jms;

import jakarta.enterprise.context.ApplicationScoped;
import jakarta.inject.Inject;
import jakarta.annotation.PreDestroy;
import javax.jms.ConnectionFactory;
import org.apache.activemq.ActiveMQConnectionFactory;
import org.apache.activemq.pool.PooledConnectionFactory;
import org.messaginghub.pooled.jms.JmsPoolConnectionFactory;

@ApplicationScoped
public class JmsClientFactory {
    @Inject BridgeConfig config;

    private volatile ConnectionFactory factory;

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

    private ConnectionFactory createFactory() {
        String type = config.brokerType();
        String url = config.brokerUrl();
        String user = config.username();
        String pass = config.password();

        switch (type) {
            case "activemq": {
                ActiveMQConnectionFactory cf = new ActiveMQConnectionFactory(url);
                if (user != null) cf.setUserName(user);
                if (pass != null) cf.setPassword(pass);
                PooledConnectionFactory pool = new PooledConnectionFactory(cf);
                pool.setMaxConnections(5);
                pool.start();
                return pool;
            }
            case "artemis": {
                org.apache.activemq.artemis.jms.client.ActiveMQConnectionFactory cf =
                    new org.apache.activemq.artemis.jms.client.ActiveMQConnectionFactory(url);
                if (user != null) cf.setUser(user);
                if (pass != null) cf.setPassword(pass);
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

    @PreDestroy
    public void close() {
        if (factory instanceof PooledConnectionFactory pool) {
            pool.stop();
        } else if (factory instanceof JmsPoolConnectionFactory pool) {
            try { pool.stop(); } catch (Exception ignored) {}
        }
    }
}
