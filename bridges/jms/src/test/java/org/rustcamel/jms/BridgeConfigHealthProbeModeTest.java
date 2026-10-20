package org.rustcamel.jms;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

import io.quarkus.runtime.StartupEvent;
import jakarta.enterprise.event.Observes;
import java.lang.reflect.Method;
import java.lang.reflect.Parameter;
import java.util.Map;
import org.junit.jupiter.api.Test;

/**
 * Unit tests for {@link BridgeConfig} broker-probe mode parsing and startup validation.
 *
 * <p>The mode reaches the bridge as {@code JMS_HEALTH_BROKER_PROBE_MODE}. Unset or blank selects
 * {@code reuse}; {@code skip} disables broker I/O for health probes. An unsupported value is a
 * permanent configuration error that names the variable and never echoes the raw value. Startup
 * validation must run eagerly, before the first RPC can observe a lazily initialized bean.
 */
class BridgeConfigHealthProbeModeTest {

  @Test
  void defaults_to_reuse() throws Exception {
    BridgeConfig config = new BridgeConfig(Map.of());

    assertEquals(BridgeConfig.BrokerProbeMode.REUSE, config.brokerProbeMode());
  }

  @Test
  void parses_skip() throws Exception {
    BridgeConfig config =
        new BridgeConfig(Map.of(BridgeConfig.HEALTH_BROKER_PROBE_MODE_ENV, "skip"));

    assertEquals(BridgeConfig.BrokerProbeMode.SKIP, config.brokerProbeMode());
  }

  @Test
  void rejects_unknown_mode_without_echoing_raw_value() {
    BridgeConfig config =
        new BridgeConfig(Map.of(BridgeConfig.HEALTH_BROKER_PROBE_MODE_ENV, "probe-v2"));

    JournalPermanentException error =
        assertThrows(JournalPermanentException.class, config::brokerProbeMode);

    assertTrue(
        error.getMessage().contains(BridgeConfig.HEALTH_BROKER_PROBE_MODE_ENV), error.getMessage());
    assertFalse(error.getMessage().contains("probe-v2"), error.getMessage());
    assertFalse(error.toString().contains("probe-v2"), error.toString());
  }

  @Test
  void startup_validation_rejects_unknown_mode() {
    BridgeConfig config =
        new BridgeConfig(Map.of(BridgeConfig.HEALTH_BROKER_PROBE_MODE_ENV, "later"));

    IllegalStateException error =
        assertThrows(IllegalStateException.class, config::validateStartup);

    assertTrue(
        error.getMessage().contains(BridgeConfig.HEALTH_BROKER_PROBE_MODE_ENV), error.getMessage());
    assertFalse(error.getMessage().contains("later"), error.getMessage());
  }

  @Test
  void startup_observer_validates_before_rpc() throws Exception {
    BridgeConfig config =
        new BridgeConfig(Map.of(BridgeConfig.HEALTH_BROKER_PROBE_MODE_ENV, "invalid-mode"));

    IllegalStateException error =
        assertThrows(IllegalStateException.class, () -> config.onStartup(new StartupEvent()));

    assertTrue(
        error.getMessage().contains(BridgeConfig.HEALTH_BROKER_PROBE_MODE_ENV), error.getMessage());
    assertFalse(error.getMessage().contains("invalid-mode"), error.getMessage());

    Method observer = BridgeConfig.class.getDeclaredMethod("onStartup", StartupEvent.class);
    Parameter[] parameters = observer.getParameters();
    assertEquals(1, parameters.length);
    assertTrue(
        parameters[0].isAnnotationPresent(Observes.class),
        "onStartup must observe StartupEvent with Jakarta @Observes so Quarkus invokes it eagerly");
  }
}
