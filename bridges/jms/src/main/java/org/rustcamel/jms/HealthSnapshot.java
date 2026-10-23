package org.rustcamel.jms;

import jms_bridge.HealthResponse;
import jms_bridge.HealthStatus;

/**
 * Immutable generation-scoped broker-health status snapshot.
 *
 * <p>The explicit {@link Status} decides freshness; {@code checkedAtMillis} is never used to infer
 * unknown. A verified snapshot taken at clock {@code 0} is therefore still valid and connected.
 */
public record HealthSnapshot(
    long generation,
    Status status,
    boolean healthy,
    boolean brokerConnected,
    String message,
    long checkedAtMillis,
    long staleAgeMillis) {

  /** Normalizes the stale age: only STALE carries a numeric age, everything else is zero. */
  public HealthSnapshot {
    if (status != Status.STALE) {
      staleAgeMillis = 0L;
    }
  }

  /** Explicit broker-status classification carried by a snapshot. */
  public enum Status {
    UNKNOWN,
    SKIPPED,
    STALE,
    UP,
    DOWN
  }

  /** Maps this snapshot onto the bridge health RPC response contract. */
  public HealthResponse toResponse() {
    return HealthResponse.newBuilder()
        .setHealthy(healthy)
        .setBrokerConnected(brokerConnected)
        .setMessage(message)
        .setStatus(toProtoStatus())
        .setAgeMs(status == Status.STALE ? staleAgeMillis : 0L)
        .build();
  }

  private HealthStatus toProtoStatus() {
    return switch (status) {
      case UNKNOWN -> HealthStatus.UNKNOWN;
      case SKIPPED -> HealthStatus.SKIPPED;
      case STALE -> HealthStatus.STALE;
      case UP -> HealthStatus.UP;
      case DOWN -> HealthStatus.DOWN;
    };
  }

  /** Not-yet-checked status: live but unverified. */
  public static HealthSnapshot unknown(long generation, long now) {
    return new HealthSnapshot(
        generation, Status.UNKNOWN, true, false, "broker status not yet checked", now, 0L);
  }

  /** Probe-skipped status: live but unverified. */
  public static HealthSnapshot skipped(long generation, long now) {
    return new HealthSnapshot(
        generation,
        Status.SKIPPED,
        true,
        false,
        "broker probe skipped; connectivity unverified",
        now,
        0L);
  }

  /** Expired status: live but unverified, reporting the largest observed age. */
  public static HealthSnapshot stale(long generation, long ageMillis, long now) {
    return new HealthSnapshot(
        generation,
        Status.STALE,
        true,
        false,
        "broker status stale; age " + ageMillis + " ms",
        now,
        ageMillis);
  }

  /** Verified connected status. */
  public static HealthSnapshot up(long generation, long now) {
    return new HealthSnapshot(generation, Status.UP, true, true, "ok", now, 0L);
  }

  /** Verified disconnected status carrying a credential-safe reason. */
  public static HealthSnapshot down(long generation, String reason, long now) {
    return new HealthSnapshot(generation, Status.DOWN, false, false, reason, now, 0L);
  }
}
