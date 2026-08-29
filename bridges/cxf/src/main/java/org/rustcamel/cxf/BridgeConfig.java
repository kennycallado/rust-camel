package org.rustcamel.cxf;

import jakarta.enterprise.context.ApplicationScoped;
import org.eclipse.microprofile.config.inject.ConfigProperty;

@ApplicationScoped
public class BridgeConfig {
  private static final String MAX_BODY_BYTES_ENV = "CXF_MAX_BODY_BYTES";
  private static final long DEFAULT_MAX_BODY_BYTES = 16L * 1024 * 1024;

  /** Above the 16 MiB default cap, below the 18 MiB Rust gRPC decode limit. */
  private static final long MAX_BODY_BYTES_CEILING = 17L * 1024 * 1024;

  private static final String MAX_DISPATCHES_ENV = "CXF_MAX_DISPATCHES";
  private static final int DEFAULT_MAX_DISPATCHES = 64;
  private static final int MAX_DISPATCHES_CEILING = 1024;

  @ConfigProperty(name = "cxf.address")
  java.util.Optional<String> address;

  @ConfigProperty(name = "cxf.connection.timeout.ms", defaultValue = "30000")
  int connectionTimeoutMs;

  @ConfigProperty(name = "cxf.max.concurrent.requests", defaultValue = "100")
  int maxConcurrentRequests;

  @ConfigProperty(name = "cxf.consumer.timeout.ms", defaultValue = "60000")
  int consumerTimeoutMs;

  public String address() {
    return address.orElse(null);
  }

  public int connectionTimeoutMs() {
    return connectionTimeoutMs;
  }

  public int maxConcurrentRequests() {
    return maxConcurrentRequests;
  }

  public int consumerTimeoutMs() {
    return consumerTimeoutMs;
  }

  /**
   * Reads the body cap from {@code CXF_MAX_BODY_BYTES} in bytes — shared by the listener request
   * cap and the producer response cap. Fails loud on a malformed or non-positive value so a typo
   * never silently disables the cap.
   */
  static long parseMaxBodyBytes() {
    String raw = System.getenv(MAX_BODY_BYTES_ENV);
    if (raw == null || raw.isBlank()) {
      return DEFAULT_MAX_BODY_BYTES;
    }
    return parseMaxBodyBytes(raw);
  }

  /**
   * Parses one {@code CXF_MAX_BODY_BYTES} value, failing loud with the env name on malformed,
   * non-positive, or above-ceiling input. Null and blank fall back to the default.
   */
  static long parseMaxBodyBytes(String raw) {
    if (raw == null || raw.isBlank()) {
      return DEFAULT_MAX_BODY_BYTES;
    }
    try {
      long parsed = Long.parseLong(raw.trim());
      if (parsed <= 0) {
        throw new IllegalStateException(
            MAX_BODY_BYTES_ENV + " must be a positive byte count: " + raw);
      }
      if (parsed > MAX_BODY_BYTES_CEILING) {
        throw new IllegalStateException(
            MAX_BODY_BYTES_ENV
                + " exceeds its "
                + MAX_BODY_BYTES_CEILING
                + "-byte ceiling: "
                + parsed
                + "; caps above 17 MiB invert the decode-limit ordering (cap <= 17 MiB ceiling"
                + " < 18 MiB Rust decode limit), bodies pass this Java cap only to fail at the"
                + " 18 MiB Rust gRPC decode limit");
      }
      return parsed;
    } catch (NumberFormatException e) {
      throw new IllegalStateException(MAX_BODY_BYTES_ENV + " invalid: " + raw, e);
    }
  }

  /**
   * Reads the Dispatch cache bound from {@code CXF_MAX_DISPATCHES}. Fails loud on a malformed,
   * non-positive, or over-ceiling value so a typo never silently unbounds the cache.
   */
  static int parseMaxDispatches() {
    String raw = System.getenv(MAX_DISPATCHES_ENV);
    if (raw == null || raw.isBlank()) {
      return DEFAULT_MAX_DISPATCHES;
    }
    return parseMaxDispatches(raw);
  }

  /**
   * Parses one {@code CXF_MAX_DISPATCHES} value, failing loud with the env name on malformed,
   * non-positive, or above-ceiling input. Null and blank fall back to the default.
   */
  static int parseMaxDispatches(String raw) {
    if (raw == null || raw.isBlank()) {
      return DEFAULT_MAX_DISPATCHES;
    }
    try {
      int parsed = Integer.parseInt(raw.trim());
      if (parsed <= 0) {
        throw new IllegalStateException(
            MAX_DISPATCHES_ENV + " must be a positive dispatch count: " + raw);
      }
      if (parsed > MAX_DISPATCHES_CEILING) {
        throw new IllegalStateException(
            MAX_DISPATCHES_ENV
                + " exceeds its "
                + MAX_DISPATCHES_CEILING
                + "-dispatch ceiling: "
                + parsed);
      }
      return parsed;
    } catch (NumberFormatException e) {
      throw new IllegalStateException(MAX_DISPATCHES_ENV + " invalid: " + raw, e);
    }
  }
}
