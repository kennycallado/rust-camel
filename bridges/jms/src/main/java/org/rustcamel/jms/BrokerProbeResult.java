package org.rustcamel.jms;

/** Outcome of one credential-safe broker diagnostic probe. */
public record BrokerProbeResult(boolean connected, String message) {
  /** A successful probe. */
  public static BrokerProbeResult up() {
    return new BrokerProbeResult(true, "ok");
  }

  /** A failed probe carrying a credential-safe reason. */
  public static BrokerProbeResult down(String reason) {
    return new BrokerProbeResult(false, reason);
  }
}
