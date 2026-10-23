package org.rustcamel.jms;

import static org.junit.jupiter.api.Assertions.assertEquals;

import java.util.Map;
import org.apache.activemq.ActiveMQConnectionFactory;
import org.apache.activemq.pool.PooledConnectionFactory;
import org.junit.jupiter.api.DisplayName;
import org.junit.jupiter.api.Test;
import org.messaginghub.pooled.jms.JmsPoolConnectionFactory;

/**
 * Unit tests for the pool knob wiring in {@link JmsClientFactory}. The package-private builders are
 * the testability seam: they mirror {@code createFactory()}'s pool setup without a broker.
 */
@DisplayName("JmsClientFactory pool knobs")
class JmsClientFactoryPoolKnobsTest {

  private static BridgeConfig config() {
    return new BridgeConfig(
        Map.of(
            BridgeConfig.POOL_IDLE_TIMEOUT_MS_ENV,
            "60000",
            BridgeConfig.POOL_EXPIRATION_CHECK_MS_ENV,
            "5000"));
  }

  @Test
  void activeMqPoolReceivesKnobs() {
    ActiveMQConnectionFactory cf = new ActiveMQConnectionFactory("tcp://broker:61616");
    PooledConnectionFactory pool = JmsClientFactory.buildPooledActiveMqFactory(cf, config());
    try {
      assertEquals(60000, pool.getIdleTimeout());
      assertEquals(5000L, pool.getTimeBetweenExpirationCheckMillis());
    } finally {
      pool.stop();
    }
  }

  @Test
  void artemisPoolReceivesKnobs() {
    var cf = JmsClientFactory.buildArtemisFactory("tcp://broker:61616", null, null);
    JmsPoolConnectionFactory pool = JmsClientFactory.buildPooledArtemisFactory(cf, config());
    try {
      assertEquals(60000, pool.getConnectionIdleTimeout());
      assertEquals(5000L, pool.getConnectionCheckInterval());
    } finally {
      pool.stop();
    }
  }
}
