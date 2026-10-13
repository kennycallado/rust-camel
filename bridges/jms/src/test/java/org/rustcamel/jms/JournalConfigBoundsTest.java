package org.rustcamel.jms;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.util.List;
import java.util.Map;
import org.junit.jupiter.api.DisplayName;
import org.junit.jupiter.api.Test;

/**
 * Unit tests for {@link BridgeConfig} journal numeric bounds validation (jms-bridge-080 task 2.6).
 *
 * <p>{@code System.getenv()} is immutable, so the tests inject an explicit environment through the
 * package-private {@link BridgeConfig#BridgeConfig(Map)} seam — the same explicit-values pattern
 * {@code JmsClientFactory.transportConfig} uses — instead of mutating the process environment.
 *
 * <p>An out-of-range or non-numeric value must raise a permanent configuration error that names the
 * variable and never echoes the raw value.
 */
@DisplayName("BridgeConfig journal bounds validation")
class JournalConfigBoundsTest {

  private static final String RETENTION = "JMS_ATTEMPT_JOURNAL_RETENTION_MS";
  private static final String LOCK_WAIT = "JMS_ATTEMPT_JOURNAL_LOCK_WAIT_MS";
  private static final String MAX_BYTES = "JMS_ATTEMPT_JOURNAL_MAX_BYTES";
  private static final String MAX_ATTEMPTS = "JMS_MAX_DELIVERY_ATTEMPTS";

  private static final List<Field> FIELDS =
      List.of(
          new Field(RETENTION, BridgeConfig::journalRetentionMs, 60_000L, 31_536_000_000L),
          new Field(LOCK_WAIT, BridgeConfig::journalLockWaitMs, 100L, 300_000L),
          new Field(MAX_BYTES, BridgeConfig::journalMaxBytes, 65_536L, 268_435_456L),
          new Field(MAX_ATTEMPTS, BridgeConfig::maxDeliveryAttempts, 1L, 100L));

  @Test
  void defaults_match_bounds() throws Exception {
    BridgeConfig config = new BridgeConfig(Map.of());

    assertEquals(86_400_000L, config.journalRetentionMs());
    assertEquals(30_000L, config.journalLockWaitMs());
    assertEquals(4_194_304L, config.journalMaxBytes());
    assertEquals(6L, config.maxDeliveryAttempts());

    // A blank value means unset. Both inclusive bounds are accepted: 4 fields x 2 = 8 cases.
    assertEquals(6L, new BridgeConfig(Map.of(MAX_ATTEMPTS, "  ")).maxDeliveryAttempts());
    for (Field field : FIELDS) {
      for (long bound : new long[] {field.min(), field.max()}) {
        BridgeConfig bounded = new BridgeConfig(Map.of(field.variable(), Long.toString(bound)));
        assertEquals(bound, field.accessor().read(bounded), field.variable());
      }
    }
  }

  @Test
  void out_of_range_value_is_permanent() {
    // retention [60000, 31536000000]
    assertPermanent(RETENTION, "59999", BridgeConfig::journalRetentionMs);
    assertPermanent(RETENTION, "31536000001", BridgeConfig::journalRetentionMs);
    // lock wait [100, 300000]
    assertPermanent(LOCK_WAIT, "99", BridgeConfig::journalLockWaitMs);
    assertPermanent(LOCK_WAIT, "300001", BridgeConfig::journalLockWaitMs);
    // max bytes [65536, 268435456]
    assertPermanent(MAX_BYTES, "65535", BridgeConfig::journalMaxBytes);
    assertPermanent(MAX_BYTES, "268435457", BridgeConfig::journalMaxBytes);
    // max attempts [1, 100]
    assertPermanent(MAX_ATTEMPTS, "0", BridgeConfig::maxDeliveryAttempts);
    assertPermanent(MAX_ATTEMPTS, "101", BridgeConfig::maxDeliveryAttempts);
    // a non-numeric value is equally a permanent failure, and the raw value never leaks
    assertPermanent(RETENTION, "not-a-number", BridgeConfig::journalRetentionMs);

    BridgeConfig leak = new BridgeConfig(Map.of(LOCK_WAIT, "raw-probe-42"));
    JournalPermanentException error =
        assertThrows(JournalPermanentException.class, leak::journalLockWaitMs);
    assertNull(error.getCause(), "non-numeric rejection must not wrap the raw value");
    assertFalse(error.getMessage().contains("raw-probe-42"), error.getMessage());
    assertFalse(error.toString().contains("raw-probe-42"), error.toString());
  }

  /**
   * Asserts {@code accessor} rejects {@code raw} with a permanent error that names {@code
   * variable}.
   */
  private static void assertPermanent(String variable, String raw, Accessor accessor) {
    BridgeConfig config = new BridgeConfig(Map.of(variable, raw));

    JournalPermanentException error =
        assertThrows(JournalPermanentException.class, () -> accessor.read(config));

    assertTrue(
        error.getMessage().contains(variable),
        "error must name the variable: " + error.getMessage());
  }

  /** Reads one bounded numeric setting from an injected {@link BridgeConfig}. */
  @FunctionalInterface
  private interface Accessor {
    long read(BridgeConfig config) throws JournalPermanentException;
  }

  /** One bounded field: its variable, accessor, and inclusive bounds. */
  private record Field(String variable, Accessor accessor, long min, long max) {}
}
