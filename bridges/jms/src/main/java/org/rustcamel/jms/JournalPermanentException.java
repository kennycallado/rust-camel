package org.rustcamel.jms;

/**
 * A permanent journal failure. Unlike a transient transport error, this signals the journal cannot
 * be trusted or recovered without operator action, so the bridge must fail closed rather than
 * retry.
 *
 * <p>Introduced by jms-bridge-080 task 2.1 as the checked error type for {@link JournalService}.
 * Phase 4 maps it to a route-only {@code FAILED_PRECONDITION} (or a startup permanent failure).
 */
public class JournalPermanentException extends Exception {

  /**
   * Creates the exception with a human-readable reason.
   *
   * @param message the reason, safe to log (never body bytes or header values)
   */
  public JournalPermanentException(String message) {
    super(message);
  }

  /**
   * Creates the exception with a human-readable reason and an underlying cause.
   *
   * @param message the reason, safe to log (never body bytes or header values)
   * @param cause the underlying failure
   */
  public JournalPermanentException(String message, Throwable cause) {
    super(message, cause);
  }
}
