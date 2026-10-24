package org.rustcamel.jms;

/**
 * Credential-safe classification of the last broker connection failure.
 *
 * <p>The wire contract carries only this enum; the remote broker text never leaves the bridge
 * process.
 */
public enum BrokerFailureClass {
  NETWORK,
  TLS_TRUST,
  TLS_HOSTNAME,
  TLS_CLIENT_AUTH,
  TLS_PROTOCOL,
  TLS_PLAINTEXT_PEER,
  UNSUPPORTED_TRANSPORT,
  AUTH,
  CLIENT_ID_IN_USE,
  OTHER;

  /** Maps this classification onto the generated health proto enum. */
  public jms_bridge.BrokerFailureClass toProto() {
    return switch (this) {
      case NETWORK -> jms_bridge.BrokerFailureClass.NETWORK;
      case TLS_TRUST -> jms_bridge.BrokerFailureClass.TLS_TRUST;
      case TLS_HOSTNAME -> jms_bridge.BrokerFailureClass.TLS_HOSTNAME;
      case TLS_CLIENT_AUTH -> jms_bridge.BrokerFailureClass.TLS_CLIENT_AUTH;
      case TLS_PROTOCOL -> jms_bridge.BrokerFailureClass.TLS_PROTOCOL;
      case TLS_PLAINTEXT_PEER -> jms_bridge.BrokerFailureClass.TLS_PLAINTEXT_PEER;
      case UNSUPPORTED_TRANSPORT -> jms_bridge.BrokerFailureClass.UNSUPPORTED_TRANSPORT;
      case AUTH -> jms_bridge.BrokerFailureClass.AUTH;
      case CLIENT_ID_IN_USE -> jms_bridge.BrokerFailureClass.CLIENT_ID_IN_USE;
      case OTHER -> jms_bridge.BrokerFailureClass.OTHER;
    };
  }

  /**
   * Null-tolerant proto mapping: no recorded failure is {@code BROKER_FAILURE_CLASS_UNSPECIFIED}.
   */
  public static jms_bridge.BrokerFailureClass toProto(BrokerFailureClass failureClass) {
    return failureClass == null
        ? jms_bridge.BrokerFailureClass.BROKER_FAILURE_CLASS_UNSPECIFIED
        : failureClass.toProto();
  }
}
