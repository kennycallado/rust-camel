package org.rustcamel.jms;

/**
 * Generation-scoped broker connectivity probe.
 *
 * <p>{@link #invalidate(long)} is non-blocking and performs no provider I/O on the caller thread:
 * it atomically detaches the retained diagnostic and hands provider cleanup to an owned worker.
 */
public interface BrokerProbe {
  /**
   * Verify broker connectivity for {@code generation}.
   *
   * @param generation the currently expected probe generation
   * @return a credential-safe probe outcome
   * @throws Exception only for deferred diagnostic creation failures
   */
  BrokerProbeResult probe(long generation) throws Exception;

  /**
   * Advance the probe generation and retire the currently retained diagnostic.
   *
   * <p>Non-blocking. Provider I/O is never performed on the caller thread. Generations are
   * monotonic: a value less than or equal to the current generation is ignored.
   *
   * @param generation the new generation
   */
  void invalidate(long generation);

  /** Terminal teardown: detach the retained diagnostic once and bound provider cleanup. */
  void close();

  /**
   * Classification of the last connect-path failure, or {@code null} when none was recorded.
   *
   * <p>Probes without diagnostics keep the default: no failure to report.
   */
  default BrokerFailureClass lastFailureClass() {
    return null;
  }
}
