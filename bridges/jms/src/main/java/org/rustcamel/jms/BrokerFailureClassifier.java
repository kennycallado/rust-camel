package org.rustcamel.jms;

import java.net.ConnectException;
import java.net.SocketTimeoutException;
import java.security.cert.CertPathBuilderException;
import java.security.cert.CertPathValidatorException;
import java.security.cert.CertificateException;
import java.util.IdentityHashMap;
import java.util.Map;
import javax.jms.JMSSecurityException;
import javax.net.ssl.SSLException;
import javax.net.ssl.SSLHandshakeException;

/**
 * Deterministic, type-first classification of broker connection failure cause chains.
 *
 * <p>Only the resulting enum leaves the process. Message inspection is confined to the named
 * constants below, all of which are JDK-authored TLS texts or the bridge's own startup marker; the
 * JMS client-ID restriction is classified by type alone.
 */
public final class BrokerFailureClassifier {

  /** JDK hostname-mismatch text family (hostname verification has no dedicated type). */
  static final String[] JDK_HOSTNAME_TEXTS = {
    "No name matching",
    "No subject alternative names matching IP address",
    "No subject alternative DNS name matching",
    "No subject alternative names present"
  };

  /** JDK TLS alerts that indicate the broker rejected or required the client certificate. */
  static final String[] CLIENT_AUTH_ALERTS = {"bad_certificate", "certificate_required"};

  /** JDK TLS alerts that indicate a protocol or cipher negotiation failure. */
  static final String[] PROTOCOL_ALERTS = {"handshake_failure", "protocol_version"};

  /** JSSE text emitted when a TLS client speaks to a plaintext peer. */
  static final String PLAINTEXT_PEER_MESSAGE = "Unsupported or unrecognized SSL message";

  /** Bridge startup self-check marker (Task 4.1). */
  static final String UNSUPPORTED_TRANSPORT_MARKER = "UNSUPPORTED_TRANSPORT:";

  private BrokerFailureClassifier() {}

  /**
   * Classifies a failure and its cause chain.
   *
   * @param failure the thrown failure, or {@code null}
   * @return the closed classification; {@link BrokerFailureClass#OTHER} for {@code null} or an
   *     unknown chain
   */
  public static BrokerFailureClass classify(Throwable failure) {
    if (failure == null) {
      return BrokerFailureClass.OTHER;
    }

    // Type-first passes. NETWORK and PKIX failures win regardless of the provider wrapper that
    // carries them, and both CertPath roots map to TLS_TRUST (SunCertPathBuilderException is a
    // CertPathBuilderException, not a validator failure).
    if (chainHas(failure, error -> error instanceof ConnectException)
        || chainHas(failure, error -> error instanceof SocketTimeoutException)) {
      return BrokerFailureClass.NETWORK;
    }
    if (chainHas(failure, BrokerFailureClassifier::isCertPathFailure)) {
      return BrokerFailureClass.TLS_TRUST;
    }
    if (chainHas(
        failure,
        error ->
            error instanceof SSLHandshakeException
                && containsAny(error.getMessage(), CLIENT_AUTH_ALERTS))) {
      return BrokerFailureClass.TLS_CLIENT_AUTH;
    }
    if (chainHas(
        failure,
        error ->
            error instanceof CertificateException
                && !hasCertPathCause(error)
                && containsAny(error.getMessage(), JDK_HOSTNAME_TEXTS))) {
      return BrokerFailureClass.TLS_HOSTNAME;
    }
    if (chainHas(
        failure,
        error ->
            (error instanceof SSLException || error instanceof CertificateException)
                && contains(error.getMessage(), PLAINTEXT_PEER_MESSAGE))) {
      return BrokerFailureClass.TLS_PLAINTEXT_PEER;
    }
    if (chainHas(
        failure,
        error ->
            error instanceof SSLHandshakeException
                && containsAny(error.getMessage(), PROTOCOL_ALERTS))) {
      return BrokerFailureClass.TLS_PROTOCOL;
    }
    if (chainHas(
        failure,
        error -> error instanceof SSLException || error instanceof SSLHandshakeException)) {
      return BrokerFailureClass.TLS_PROTOCOL;
    }

    if (chainHas(failure, error -> error instanceof JMSSecurityException)) {
      return BrokerFailureClass.AUTH;
    }
    // Precedence: the bridge's own java.lang marker is checked before the provider's
    // javax.jms.IllegalStateException type. The two types are unrelated by inheritance, so only an
    // explicit marker on a java.lang.IllegalStateException (possibly wrapped) selects
    // UNSUPPORTED_TRANSPORT; a javax.jms.IllegalStateException is always CLIENT_ID_IN_USE.
    if (chainHas(failure, BrokerFailureClassifier::isUnsupportedTransportMarker)) {
      return BrokerFailureClass.UNSUPPORTED_TRANSPORT;
    }
    if (chainHas(failure, error -> error instanceof javax.jms.IllegalStateException)) {
      return BrokerFailureClass.CLIENT_ID_IN_USE;
    }
    return BrokerFailureClass.OTHER;
  }

  private static boolean isCertPathFailure(Throwable error) {
    return error instanceof CertPathValidatorException || error instanceof CertPathBuilderException;
  }

  private static boolean isUnsupportedTransportMarker(Throwable error) {
    return error instanceof IllegalStateException
        && startsWith(error.getMessage(), UNSUPPORTED_TRANSPORT_MARKER);
  }

  private static boolean hasCertPathCause(Throwable error) {
    return chainHas(error, BrokerFailureClassifier::isCertPathFailure);
  }

  private static boolean chainHas(Throwable start, ThrowablePredicate predicate) {
    Map<Throwable, Boolean> seen = new IdentityHashMap<>();
    Throwable current = start;
    while (current != null && seen.put(current, Boolean.TRUE) == null) {
      if (predicate.test(current)) {
        return true;
      }
      current = current.getCause();
    }
    return false;
  }

  private static boolean containsAny(String message, String[] needles) {
    if (message == null) {
      return false;
    }
    for (String needle : needles) {
      if (message.contains(needle)) {
        return true;
      }
    }
    return false;
  }

  private static boolean contains(String message, String needle) {
    return message != null && message.contains(needle);
  }

  private static boolean startsWith(String message, String prefix) {
    return message != null && message.startsWith(prefix);
  }

  @FunctionalInterface
  private interface ThrowablePredicate {
    boolean test(Throwable error);
  }
}
