package org.rustcamel.jms;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.util.List;
import java.util.Map;
import java.util.function.LongSupplier;
import org.junit.jupiter.api.Test;

/**
 * Unit tests for {@link BridgeConfig} commit-time pool knob parsing.
 *
 * <p>Both pool knobs share the same range and default. They reach the bridge as {@code
 * JMS_POOL_IDLE_TIMEOUT_MS} and {@code JMS_POOL_EXPIRATION_CHECK_MS}; each test exercises both
 * variables through their accessor.
 */
class BridgeConfigPoolKnobsTest {

  private record Knob(String variable, LongSupplier accessor) {}

  private static Knob single(String variable, String raw) {
    return variable.equals(BridgeConfig.POOL_IDLE_TIMEOUT_MS_ENV)
        ? new Knob(variable, new BridgeConfig(Map.of(variable, raw))::poolIdleTimeoutMs)
        : new Knob(variable, new BridgeConfig(Map.of(variable, raw))::poolExpirationCheckMs);
  }

  private static List<Knob> eachSingle(String raw) {
    return List.of(
        single(BridgeConfig.POOL_IDLE_TIMEOUT_MS_ENV, raw),
        single(BridgeConfig.POOL_EXPIRATION_CHECK_MS_ENV, raw));
  }

  private static void assertRejected(Knob knob, String raw) {
    IllegalArgumentException error =
        assertThrows(IllegalArgumentException.class, knob.accessor()::getAsLong);
    String message = error.getMessage();
    assertTrue(message.contains(knob.variable()), message);
    assertTrue(message.contains("1000"), message);
    assertTrue(message.contains("86400000"), message);
    assertTrue(!message.contains(raw), message);
  }

  @Test
  void poolKnobEndpointsAccepted() {
    for (String variable :
        List.of(BridgeConfig.POOL_IDLE_TIMEOUT_MS_ENV, BridgeConfig.POOL_EXPIRATION_CHECK_MS_ENV)) {
      for (long endpoint : new long[] {1_000L, 86_400_000L}) {
        Knob knob = single(variable, Long.toString(endpoint));
        assertEquals(endpoint, knob.accessor().getAsLong(), variable);
      }
    }
  }

  @Test
  void poolKnobUnsetOrBlankSelectsDefault() {
    BridgeConfig unset = new BridgeConfig(Map.of());
    assertEquals(30_000L, unset.poolIdleTimeoutMs());
    assertEquals(30_000L, unset.poolExpirationCheckMs());

    for (Knob knob : eachSingle(" ")) {
      assertEquals(30_000L, knob.accessor().getAsLong(), knob.variable());
    }
  }

  @Test
  void poolKnobBelowMinRejected() {
    for (Knob knob : eachSingle("999")) {
      assertRejected(knob, "999");
    }
  }

  @Test
  void poolKnobAboveMaxRejected() {
    for (Knob knob : eachSingle("86400001")) {
      assertRejected(knob, "86400001");
    }
  }

  @Test
  void poolKnobNonNumericRejected() {
    for (Knob knob : eachSingle("abc")) {
      assertRejected(knob, "abc");
    }
  }
}
