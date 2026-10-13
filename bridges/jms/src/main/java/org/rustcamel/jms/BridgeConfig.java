package org.rustcamel.jms;

import jakarta.enterprise.context.ApplicationScoped;
import java.util.Map;

/**
 * Environment-backed bridge configuration. Every accessor is a stateless read of the process
 * environment, so callers may construct a new instance directly.
 *
 * <p>The journal numeric settings are range-validated at read time. An out-of-range or non-numeric
 * value raises {@link JournalPermanentException}, the permanent configuration error that phase 4
 * maps to the startup permanent-failure signal. The error names the variable only and never echoes
 * the raw value.
 *
 * <p>Validated ranges and defaults:
 *
 * <ul>
 *   <li>{@code JMS_ATTEMPT_JOURNAL_RETENTION_MS} in {@code [60000, 31536000000]}, default {@code
 *       86400000}
 *   <li>{@code JMS_ATTEMPT_JOURNAL_LOCK_WAIT_MS} in {@code [100, 300000]}, default {@code 30000}
 *   <li>{@code JMS_ATTEMPT_JOURNAL_MAX_BYTES} in {@code [65536, 268435456]}, default {@code
 *       4194304}
 *   <li>{@code JMS_MAX_DELIVERY_ATTEMPTS} in {@code [1, 100]}, default {@code 6}
 * </ul>
 */
@ApplicationScoped
public class BridgeConfig {
  static final String JOURNAL_RETENTION_MS_ENV = "JMS_ATTEMPT_JOURNAL_RETENTION_MS";
  static final String JOURNAL_LOCK_WAIT_MS_ENV = "JMS_ATTEMPT_JOURNAL_LOCK_WAIT_MS";
  static final String JOURNAL_MAX_BYTES_ENV = "JMS_ATTEMPT_JOURNAL_MAX_BYTES";
  static final String MAX_DELIVERY_ATTEMPTS_ENV = "JMS_MAX_DELIVERY_ATTEMPTS";

  static final long JOURNAL_RETENTION_MS_MIN = 60_000L;
  static final long JOURNAL_RETENTION_MS_MAX = 31_536_000_000L;
  static final long JOURNAL_RETENTION_MS_DEFAULT = 86_400_000L;

  static final long JOURNAL_LOCK_WAIT_MS_MIN = 100L;
  static final long JOURNAL_LOCK_WAIT_MS_MAX = 300_000L;
  static final long JOURNAL_LOCK_WAIT_MS_DEFAULT = 30_000L;

  static final long JOURNAL_MAX_BYTES_MIN = 65_536L;
  static final long JOURNAL_MAX_BYTES_MAX = 268_435_456L;
  static final long JOURNAL_MAX_BYTES_DEFAULT = 4_194_304L;

  static final long MAX_DELIVERY_ATTEMPTS_MIN = 1L;
  static final long MAX_DELIVERY_ATTEMPTS_MAX = 100L;
  static final long MAX_DELIVERY_ATTEMPTS_DEFAULT = 6L;

  private final Map<String, String> env;

  /** CDI constructor: reads the process environment. */
  public BridgeConfig() {
    this(System.getenv());
  }

  /**
   * Test seam: an explicit environment, mirroring {@code JmsClientFactory.transportConfig}'s
   * explicit-values overload because {@code System.getenv()} is immutable.
   *
   * @param env the environment to read; never {@code null}
   */
  BridgeConfig(Map<String, String> env) {
    this.env = env;
  }

  public String brokerUrl() {
    return env.getOrDefault("BRIDGE_BROKER_URL", "tcp://localhost:61616");
  }

  public String brokerType() {
    return env.getOrDefault("BRIDGE_BROKER_TYPE", "activemq").toLowerCase();
  }

  public String username() {
    return env.get("BRIDGE_USERNAME");
  }

  public String password() {
    return env.get("BRIDGE_PASSWORD");
  }

  /**
   * Value of {@code JMS_ATTEMPT_JOURNAL_DIR}, or {@code null} when unset. Rust resolves the default
   * and passes the directory; Java never re-derives it. An unset directory defers the failure to a
   * queue subscribe.
   *
   * @return the configured journal root, or {@code null} when unset
   */
  public String attemptJournalDir() {
    return env.get(JournalService.JOURNAL_DIRECTORY_ENV);
  }

  /**
   * Value of {@code JMS_BRIDGE_BROKER_KEY}, or {@code null} when unset. Rust derives the key and
   * passes it; Java validates its shape at startup. An unset key defers the failure to a queue
   * subscribe.
   *
   * @return the configured broker key, or {@code null} when unset
   */
  public String brokerKey() {
    return env.get(JournalService.BROKER_KEY_ENV);
  }

  /**
   * Validated retention window for journal entries, in milliseconds.
   *
   * @return {@code JMS_ATTEMPT_JOURNAL_RETENTION_MS}, default {@code 86400000}
   * @throws JournalPermanentException when the value is non-numeric or outside {@code [60000,
   *     31536000000]}
   */
  public long journalRetentionMs() throws JournalPermanentException {
    return bounded(
        JOURNAL_RETENTION_MS_ENV,
        env.get(JOURNAL_RETENTION_MS_ENV),
        JOURNAL_RETENTION_MS_MIN,
        JOURNAL_RETENTION_MS_MAX,
        JOURNAL_RETENTION_MS_DEFAULT);
  }

  /**
   * Validated bounded wait for a busy destination lock, in milliseconds.
   *
   * @return {@code JMS_ATTEMPT_JOURNAL_LOCK_WAIT_MS}, default {@code 30000}
   * @throws JournalPermanentException when the value is non-numeric or outside {@code [100,
   *     300000]}
   */
  public long journalLockWaitMs() throws JournalPermanentException {
    return bounded(
        JOURNAL_LOCK_WAIT_MS_ENV,
        env.get(JOURNAL_LOCK_WAIT_MS_ENV),
        JOURNAL_LOCK_WAIT_MS_MIN,
        JOURNAL_LOCK_WAIT_MS_MAX,
        JOURNAL_LOCK_WAIT_MS_DEFAULT);
  }

  /**
   * Validated per-destination journal byte ceiling, in bytes.
   *
   * @return {@code JMS_ATTEMPT_JOURNAL_MAX_BYTES}, default {@code 4194304}
   * @throws JournalPermanentException when the value is non-numeric or outside {@code [65536,
   *     268435456]}
   */
  public long journalMaxBytes() throws JournalPermanentException {
    return bounded(
        JOURNAL_MAX_BYTES_ENV,
        env.get(JOURNAL_MAX_BYTES_ENV),
        JOURNAL_MAX_BYTES_MIN,
        JOURNAL_MAX_BYTES_MAX,
        JOURNAL_MAX_BYTES_DEFAULT);
  }

  /**
   * Validated maximum delivery attempts before the terminal reject-then-log policy.
   *
   * @return {@code JMS_MAX_DELIVERY_ATTEMPTS}, default {@code 6}
   * @throws JournalPermanentException when the value is non-numeric or outside {@code [1, 100]}
   */
  public long maxDeliveryAttempts() throws JournalPermanentException {
    return bounded(
        MAX_DELIVERY_ATTEMPTS_ENV,
        env.get(MAX_DELIVERY_ATTEMPTS_ENV),
        MAX_DELIVERY_ATTEMPTS_MIN,
        MAX_DELIVERY_ATTEMPTS_MAX,
        MAX_DELIVERY_ATTEMPTS_DEFAULT);
  }

  /**
   * Parses one bounded numeric setting. Pure: it reads only its arguments, so tests exercise the
   * bounds without touching the process environment.
   *
   * @param variable the environment variable name, used in the error
   * @param raw the raw value; {@code null} or blank selects the default
   * @param min the inclusive lower bound
   * @param max the inclusive upper bound
   * @param defaultValue the value used when {@code raw} is unset
   * @return the parsed value
   * @throws JournalPermanentException when {@code raw} is non-numeric or outside {@code [min, max]}
   */
  static long bounded(String variable, String raw, long min, long max, long defaultValue)
      throws JournalPermanentException {
    if (raw == null || raw.isBlank()) {
      return defaultValue;
    }
    long parsed;
    try {
      parsed = Long.parseLong(raw.trim());
    } catch (NumberFormatException ignored) {
      // No cause: the raw value must never reach a message, stack trace, or cause chain.
      throw new JournalPermanentException(
          variable + " must be an integer in [" + min + ", " + max + "]");
    }
    if (parsed < min || parsed > max) {
      throw new JournalPermanentException(variable + " must be in [" + min + ", " + max + "]");
    }
    return parsed;
  }
}
