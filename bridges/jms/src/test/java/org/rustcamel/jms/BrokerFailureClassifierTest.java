package org.rustcamel.jms;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertInstanceOf;
import static org.junit.jupiter.api.Assertions.assertSame;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.net.ConnectException;
import java.net.SocketException;
import java.security.cert.CertPathBuilderException;
import java.security.cert.CertPathValidatorException;
import java.security.cert.CertificateException;
import java.util.ArrayList;
import java.util.List;
import javax.jms.JMSSecurityException;
import javax.net.ssl.SSLException;
import javax.net.ssl.SSLHandshakeException;
import jms_bridge.HealthResponse;
import org.apache.activemq.util.IOExceptionSupport;
import org.junit.jupiter.api.Test;

/** Cause-chain classification and health-response capability wiring. */
class BrokerFailureClassifierTest {

  @Test
  void pkixCauseClassifiesTlsTrust() {
    assertEquals(
        BrokerFailureClass.TLS_TRUST,
        BrokerFailureClassifier.classify(handshakeWith(new CertPathValidatorException("pkix"))));
    assertEquals(
        BrokerFailureClass.TLS_TRUST,
        BrokerFailureClassifier.classify(handshakeWith(new CertPathBuilderException("pkix"))));
  }

  @Test
  void hostnameTextsClassifyTlsHostname() {
    List<String> texts =
        List.of(
            "No name matching mq.internal found",
            "No subject alternative names matching IP address 10.0.0.5 found",
            "No subject alternative DNS name matching mq.internal found.",
            "No subject alternative names present");
    for (String text : texts) {
      assertEquals(
          BrokerFailureClass.TLS_HOSTNAME,
          BrokerFailureClassifier.classify(handshakeWith(new CertificateException(text))),
          text);
    }
  }

  @Test
  void connectExceptionClassifiesNetwork() {
    assertEquals(
        BrokerFailureClass.NETWORK,
        BrokerFailureClassifier.classify(new ConnectException("Connection refused")));
  }

  @Test
  void plaintextPortClassifiesPlaintextPeer() {
    assertEquals(
        BrokerFailureClass.TLS_PLAINTEXT_PEER,
        BrokerFailureClassifier.classify(
            new SSLException("Unsupported or unrecognized SSL message")));
    SSLHandshakeException wrapped = new SSLHandshakeException("handshake failed");
    wrapped.initCause(new SSLException("Unsupported or unrecognized SSL message"));
    assertEquals(
        BrokerFailureClass.TLS_PLAINTEXT_PEER,
        BrokerFailureClassifier.classify(wrapped),
        "the plaintext text must classify through a wrapper chain too");
  }

  @Test
  void socketResetClassifiesOtherNotTls() {
    // Windows RST during a plaintext-peer exchange surfaces as SocketException ("Connection
    // reset") through JSSE, thrown as-is during the handshake. Mapping a reset to TLS would make
    // the classifier lie about load balancers, firewalls and broker crashes — the deliberate
    // refusal is pinned here, bare and through the ActiveMQ transport wrap.
    assertEquals(
        BrokerFailureClass.OTHER,
        BrokerFailureClassifier.classify(new SocketException("Connection reset")));
    java.io.IOException wrapped =
        IOExceptionSupport.create(new SocketException("Connection reset"));
    assertEquals(
        BrokerFailureClass.OTHER,
        BrokerFailureClassifier.classify(wrapped),
        "a reset wrapped by the transport stays OTHER");
  }

  @Test
  void alertMessageClassifiesTlsProtocol() {
    assertEquals(
        BrokerFailureClass.TLS_PROTOCOL,
        BrokerFailureClassifier.classify(
            new SSLHandshakeException("Received fatal alert: handshake_failure")));
  }

  @Test
  void clientAuthAlertsClassifyTlsClientAuth() {
    assertEquals(
        BrokerFailureClass.TLS_CLIENT_AUTH,
        BrokerFailureClassifier.classify(
            new SSLHandshakeException("Received fatal alert: bad_certificate")));
    assertEquals(
        BrokerFailureClass.TLS_CLIENT_AUTH,
        BrokerFailureClassifier.classify(
            new SSLHandshakeException("Received fatal alert: certificate_required")));
  }

  @Test
  void unsupportedTransportMarkerClassifiesUnsupported() {
    IllegalStateException marker =
        new IllegalStateException("UNSUPPORTED_TRANSPORT: no transport factory for scheme qls");
    assertEquals(
        BrokerFailureClass.UNSUPPORTED_TRANSPORT, BrokerFailureClassifier.classify(marker));
    assertEquals(
        BrokerFailureClass.UNSUPPORTED_TRANSPORT,
        BrokerFailureClassifier.classify(new RuntimeException("wrapped", marker)));
  }

  @Test
  void securityExceptionClassifiesAuth() {
    assertEquals(
        BrokerFailureClass.AUTH,
        BrokerFailureClassifier.classify(new JMSSecurityException("bad credentials")));
  }

  @Test
  void clientExceptionByTypeNotText() {
    String brokerText = "clientID bridge-1 is already connected from somewhere";
    assertEquals(
        BrokerFailureClass.CLIENT_ID_IN_USE,
        BrokerFailureClassifier.classify(new javax.jms.IllegalStateException(brokerText)));
    assertEquals(
        BrokerFailureClass.OTHER,
        BrokerFailureClassifier.classify(new RuntimeException(brokerText)),
        "exception text alone must not select the class");
  }

  @Test
  void unknownClassifiesOther() {
    assertEquals(
        BrokerFailureClass.OTHER,
        BrokerFailureClassifier.classify(new RuntimeException("broker said whatever")));
  }

  @Test
  void classifierReturnsEnumOnly() {
    List<Throwable> cases = new ArrayList<>();
    cases.add(new ConnectException("Connection refused"));
    cases.add(handshakeWith(new CertPathValidatorException("pkix")));
    cases.add(handshakeWith(new CertificateException("No name matching mq.internal found")));
    cases.add(new SSLException("Unsupported or unrecognized SSL message"));
    cases.add(new SSLHandshakeException("Received fatal alert: bad_certificate"));
    cases.add(new SSLHandshakeException("Received fatal alert: handshake_failure"));
    cases.add(
        new IllegalStateException("UNSUPPORTED_TRANSPORT: no transport factory for scheme qls"));
    cases.add(new JMSSecurityException("bad credentials"));
    cases.add(new javax.jms.IllegalStateException("client id in use"));
    cases.add(new RuntimeException("broker said whatever"));

    for (Throwable failure : cases) {
      BrokerFailureClass classified = BrokerFailureClassifier.classify(failure);
      assertInstanceOf(BrokerFailureClass.class, classified);
      assertEquals(classified.name(), classified.toString(), "enum must not carry message text");
    }
  }

  @Test
  void probeReportsCapabilityAndClass() {
    HealthResponse noFailure = JmsBridgeService.healthResponse(HealthSnapshot.up(1L, 0L), null);
    assertTrue(noFailure.getCapabilitiesList().contains(JmsBridgeService.CAP_BROKER_TLS_V1));
    assertEquals(
        jms_bridge.BrokerFailureClass.BROKER_FAILURE_CLASS_UNSPECIFIED,
        noFailure.getFailureClass());

    JmsBrokerProbe probe =
        new JmsBrokerProbe(
            new BridgeConfig(
                java.util.Map.of(
                    "BRIDGE_BROKER_TYPE",
                    "activemq",
                    "BRIDGE_BROKER_URL",
                    "ssl://broker.internal:61617")),
            (type, url, username, password) -> {
              SSLHandshakeException failure = new SSLHandshakeException("handshake failed");
              failure.initCause(new CertPathValidatorException("pkix"));
              throw failure;
            },
            () -> {},
            () -> {},
            () -> {},
            () -> {});
    try {
      assertFalse(probe.probe(1L).connected());
      assertSame(BrokerFailureClass.TLS_TRUST, probe.lastFailureClass());

      HealthResponse response =
          JmsBridgeService.healthResponse(
              HealthSnapshot.down(1L, "broker probe failed", 0L), probe.lastFailureClass());

      assertTrue(response.getCapabilitiesList().contains(JmsBridgeService.CAP_BROKER_TLS_V1));
      assertEquals(jms_bridge.BrokerFailureClass.TLS_TRUST, response.getFailureClass());
    } finally {
      probe.close();
    }
  }

  private static SSLHandshakeException handshakeWith(Throwable cause) {
    SSLHandshakeException failure = new SSLHandshakeException("handshake failed");
    failure.initCause(cause);
    return failure;
  }
}
