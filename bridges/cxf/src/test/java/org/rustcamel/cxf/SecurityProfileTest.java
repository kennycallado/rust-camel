package org.rustcamel.cxf;

import static org.junit.jupiter.api.Assertions.*;

import java.util.Map;
import java.util.Properties;
import org.apache.cxf.ws.security.wss4j.WSS4JOutInterceptor;
import org.junit.jupiter.api.DisplayName;
import org.junit.jupiter.api.Test;

@DisplayName("SecurityProfile")
class SecurityProfileTest {

  @Test
  @DisplayName("builder creates minimal profile with name only, all defaults")
  void builderCreatesMinimalProfile() {
    SecurityProfile p = SecurityProfile.builder("minimal").build();
    assertEquals("minimal", p.name());
    assertNull(p.wsdlPath());
    assertNull(p.serviceName());
    assertNull(p.portName());
    assertNull(p.address());
    assertNull(p.keystorePath());
    assertNull(p.keystorePassword());
    assertNull(p.truststorePath());
    assertNull(p.truststorePassword());
    assertNull(p.securityActionsOut());
    assertNull(p.securityActionsIn());
    assertNull(p.signatureAlgorithm());
    assertNull(p.signatureDigestAlgorithm());
    assertNull(p.signatureC14nAlgorithm());
    assertNull(p.signatureParts());
  }

  @Test
  @DisplayName("builder creates full profile with all fields set")
  void builderCreatesFullProfile() {
    SecurityProfile p =
        SecurityProfile.builder("full")
            .wsdlPath("/wsdl")
            .serviceName("Svc")
            .portName("Port")
            .address("http://host:8080")
            .keystore("/ks", "kspass")
            .truststore("/ts", "tspass")
            .sigUser("siguser", "sigpass")
            .encUser("encuser")
            .actionsOut("Signature Encrypt")
            .actionsIn("Signature")
            .signatureAlgorithm("http://www.w3.org/2001/04/xmldsig-more#rsa-sha256")
            .signatureDigestAlgorithm("http://www.w3.org/2001/04/xmlenc#sha256")
            .signatureC14nAlgorithm("http://www.w3.org/2001/10/xml-exc-c14n#")
            .signatureParts(
                "{}{http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-wssecurity-utility-1.0.xsd}Body")
            .build();

    assertEquals("full", p.name());
    assertEquals("/wsdl", p.wsdlPath());
    assertEquals("Svc", p.serviceName());
    assertEquals("Port", p.portName());
    assertEquals("http://host:8080", p.address());
    assertEquals("/ks", p.keystorePath());
    assertEquals("kspass", p.keystorePassword());
    assertEquals("/ts", p.truststorePath());
    assertEquals("tspass", p.truststorePassword());
    assertEquals("siguser", p.sigUsername());
    assertEquals("sigpass", p.sigPassword());
    assertEquals("encuser", p.encUsername());
    assertEquals("Signature Encrypt", p.securityActionsOut());
    assertEquals("Signature", p.securityActionsIn());
    assertEquals("http://www.w3.org/2001/04/xmldsig-more#rsa-sha256", p.signatureAlgorithm());
    assertEquals("http://www.w3.org/2001/04/xmlenc#sha256", p.signatureDigestAlgorithm());
    assertEquals("http://www.w3.org/2001/10/xml-exc-c14n#", p.signatureC14nAlgorithm());
    assertNotNull(p.signatureParts());
  }

  @Test
  @DisplayName("hasSecurity returns true when keystore is set")
  void hasSecurityReturnsTrueWhenKeystoreSet() {
    SecurityProfile p = SecurityProfile.builder("test").keystore("/ks.jks", "pass").build();
    assertTrue(p.hasSecurity());
  }

  @Test
  @DisplayName("hasSecurity returns false when no keystore or truststore")
  void hasSecurityReturnsFalseWhenNoKeystore() {
    SecurityProfile p = SecurityProfile.builder("test").build();
    assertFalse(p.hasSecurity());
  }

  @Test
  @DisplayName("hasSecurity returns true when only truststore set")
  void hasSecurityReturnsTrueWhenTruststoreSet() {
    SecurityProfile p = SecurityProfile.builder("test").truststore("/ts.jks", "pass").build();
    assertTrue(p.hasSecurity());
  }

  @Test
  @DisplayName("canSignOutbound returns true when keystore set")
  void canSignOutboundReturnsTrueWhenKeystoreSet() {
    SecurityProfile p = SecurityProfile.builder("test").keystore("/ks.jks", "pass").build();
    assertTrue(p.canSignOutbound());
  }

  @Test
  @DisplayName("canSignOutbound returns false when no keystore")
  void canSignOutboundReturnsFalseWhenNoKeystore() {
    SecurityProfile p = SecurityProfile.builder("test").build();
    assertFalse(p.canSignOutbound());
  }

  @Test
  @DisplayName("canVerifyInbound returns true when actionsIn set")
  void canVerifyInboundReturnsTrueWhenActionsInSet() {
    SecurityProfile p =
        SecurityProfile.builder("test").keystore("/ks.jks", "pass").actionsIn("Signature").build();
    assertTrue(p.canVerifyInbound());
  }

  @Test
  @DisplayName("canVerifyInbound returns true when truststore set")
  void canVerifyInboundReturnsTrueWhenTruststoreSet() {
    SecurityProfile p = SecurityProfile.builder("test").truststore("/ts.jks", "pass").build();
    assertTrue(p.canVerifyInbound());
  }

  @Test
  @DisplayName("canVerifyInbound returns false when no stores")
  void canVerifyInboundReturnsFalseWhenNoStores() {
    SecurityProfile p = SecurityProfile.builder("test").build();
    assertFalse(p.canVerifyInbound());
  }

  @Test
  @DisplayName("createCryptoProperties returns correct map with path and password")
  void createCryptoPropertiesReturnsCorrectMap() {
    Properties props = SecurityProfile.createCryptoProperties("/ks.jks", "pass");
    assertEquals(
        "org.apache.wss4j.common.crypto.Merlin", props.get("org.apache.wss4j.crypto.provider"));
    assertEquals("JKS", props.get("org.apache.wss4j.crypto.merlin.keystore.type"));
    assertEquals("/ks.jks", props.get("org.apache.wss4j.crypto.merlin.keystore.file"));
    assertEquals("pass", props.get("org.apache.wss4j.crypto.merlin.keystore.password"));
  }

  @Test
  @DisplayName("createCryptoProperties returns empty properties when path is null")
  void createCryptoPropertiesReturnsEmptyWhenPathNull() {
    Properties props = SecurityProfile.createCryptoProperties(null, null);
    assertTrue(props.isEmpty());
  }

  @Test
  @DisplayName("createCryptoProperties returns empty properties when path is blank")
  void createCryptoPropertiesReturnsEmptyWhenPathBlank() {
    Properties props = SecurityProfile.createCryptoProperties("  ", "pass");
    assertTrue(props.isEmpty());
  }

  @Test
  @DisplayName("builder defaults: sigUsername=clientkey, encUsername=serverkey, actions=Signature")
  void builderDefaults() {
    SecurityProfile p = SecurityProfile.builder("test").build();
    assertEquals("clientkey", p.sigUsername(), "sigUsername defaults to 'clientkey'");
    assertEquals("serverkey", p.encUsername(), "encUsername defaults to 'serverkey'");
    assertEquals("Signature", p.resolveActionsOut(), "actionsOut defaults to 'Signature'");
    assertEquals("Signature", p.resolveActionsIn(), "actionsIn defaults to 'Signature'");
  }

  @Test
  @DisplayName("builder overrides sigUsername when explicitly set")
  void builderOverridesSigUsername() {
    SecurityProfile p = SecurityProfile.builder("test").sigUser("custom", "pass").build();
    assertEquals("custom", p.sigUsername());
    assertEquals("pass", p.sigPassword());
  }

  @Test
  @DisplayName("builder overrides encUsername when explicitly set")
  void builderOverridesEncUsername() {
    SecurityProfile p = SecurityProfile.builder("test").encUser("customenc").build();
    assertEquals("customenc", p.encUsername());
  }

  @Test
  @DisplayName("resolveActionsOut returns explicit value when set")
  void resolveActionsOutReturnsExplicit() {
    SecurityProfile p = SecurityProfile.builder("test").actionsOut("Signature Encrypt").build();
    assertEquals("Signature Encrypt", p.resolveActionsOut());
  }

  @Test
  @DisplayName("resolveActionsIn returns explicit value when set")
  void resolveActionsInReturnsExplicit() {
    SecurityProfile p = SecurityProfile.builder("test").actionsIn("Encrypt Signature").build();
    assertEquals("Encrypt Signature", p.resolveActionsIn());
  }

  @Test
  @DisplayName("getSignatureCrypto caches instance (call twice, same instance)")
  void getSignatureCryptoCachesInstance() throws Exception {
    // Need a real keystore for CryptoFactory
    java.nio.file.Path ksPath = TestKeystoreHelper.createTestKeystore();
    try {
      SecurityProfile p =
          SecurityProfile.builder("test").keystore(ksPath.toString(), "changeit").build();
      var crypto1 = p.getSignatureCrypto();
      var crypto2 = p.getSignatureCrypto();
      assertSame(crypto1, crypto2, "getSignatureCrypto should return cached instance");
    } finally {
      java.nio.file.Files.deleteIfExists(ksPath);
    }
  }

  @Test
  @DisplayName("getVerificationCrypto caches instance (call twice, same instance)")
  void getVerificationCryptoCachesInstance() throws Exception {
    java.nio.file.Path ksPath = TestKeystoreHelper.createTestKeystore();
    try {
      SecurityProfile p =
          SecurityProfile.builder("test").keystore(ksPath.toString(), "changeit").build();
      var crypto1 = p.getVerificationCrypto();
      var crypto2 = p.getVerificationCrypto();
      assertSame(crypto1, crypto2, "getVerificationCrypto should return cached instance");
    } finally {
      java.nio.file.Files.deleteIfExists(ksPath);
    }
  }

  @Test
  @DisplayName("getVerificationCrypto uses truststore when set, keystore otherwise")
  void getVerificationCryptoPrefersTruststore() throws Exception {
    java.nio.file.Path ksPath = TestKeystoreHelper.createTestKeystore();
    java.nio.file.Path tsPath = TestKeystoreHelper.createTestKeystore();
    try {
      SecurityProfile p =
          SecurityProfile.builder("test")
              .keystore(ksPath.toString(), "changeit")
              .truststore(tsPath.toString(), "changeit")
              .build();
      // Should not throw — truststore is used for verification
      var crypto = p.getVerificationCrypto();
      assertNotNull(crypto);
    } finally {
      java.nio.file.Files.deleteIfExists(ksPath);
      java.nio.file.Files.deleteIfExists(tsPath);
    }
  }

  @Test
  @DisplayName("signature knob set while out-actions omit Signature is rejected")
  void knobWithoutSignatureActionIsRejected() {
    assertBuildRejectedNaming(
        SecurityProfile.builder("no-action")
            .keystore("/ks.jks", "pass")
            .actionsOut("Encrypt")
            .signatureAlgorithm("http://www.w3.org/2001/04/xmldsig-more#rsa-sha384"),
        "SIGNATURE_ALGORITHM");
    assertBuildRejectedNaming(
        SecurityProfile.builder("no-action")
            .keystore("/ks.jks", "pass")
            .actionsOut("Encrypt")
            .signatureDigestAlgorithm("http://www.w3.org/2001/04/xmlenc#sha384"),
        "SIGNATURE_DIGEST_ALGORITHM");
    assertBuildRejectedNaming(
        SecurityProfile.builder("no-action")
            .keystore("/ks.jks", "pass")
            .actionsOut("Encrypt")
            .signatureC14nAlgorithm("http://www.w3.org/2001/10/xml-exc-c14n#"),
        "SIGNATURE_C14N_ALGORITHM");
    assertBuildRejectedNaming(
        SecurityProfile.builder("no-action")
            .keystore("/ks.jks", "pass")
            .actionsOut("Encrypt")
            .signatureParts("Body"),
        "SIGNATURE_PARTS");
  }

  @Test
  @DisplayName("signature knob set without a signing keystore is rejected")
  void knobWithoutKeystoreIsRejected() {
    assertBuildRejectedNaming(
        SecurityProfile.builder("no-keystore")
            .actionsOut("Signature")
            .signatureDigestAlgorithm("http://www.w3.org/2001/04/xmlenc#sha384"),
        "SIGNATURE_DIGEST_ALGORITHM");
  }

  @Test
  @DisplayName("malformed SIGNATURE_PARTS segments are rejected")
  void malformedPartsSegmentIsRejected() throws Exception {
    java.nio.file.Path ksPath = TestKeystoreHelper.createTestKeystore();
    try {
      String[] malformed = {
        "{}{http://x}",
        "{Bogus}{http://x}Body",
        "{Content}{http://x}",
        "{Content}http://x}Body",
        "{Content}{http://x}}Body",
        "{Content}{http://{x}Body",
      };
      for (String parts : malformed) {
        assertBuildRejectedNaming(
            SecurityProfile.builder("bad-parts")
                .keystore(ksPath.toString(), "changeit")
                .actionsOut("Signature")
                .signatureParts(parts),
            "SIGNATURE_PARTS");
      }
    } finally {
      java.nio.file.Files.deleteIfExists(ksPath);
    }
  }

  @Test
  @DisplayName("validateSignaturePartsSyntax direct edge matrix")
  void validateSignaturePartsSyntaxDirectMatrix() {
    String[] invalid = {
      "Body;;Other", "Body;", "{Content}{http://x}}Body", "{Content}{http://{x}Body",
    };
    for (String parts : invalid) {
      assertThrows(
          IllegalArgumentException.class,
          () -> SecurityProfile.validateSignaturePartsSyntax(parts),
          "expected rejection of '" + parts + "'");
    }
    String[] valid = {
      "Body",
      "{Content}{http://x}Body",
      "{Element}{}Timestamp",
      "{}{}Timestamp",
      "Body;{}{http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-wssecurity-utility-1.0.xsd}Timestamp",
    };
    for (String parts : valid) {
      assertDoesNotThrow(
          () -> SecurityProfile.validateSignaturePartsSyntax(parts),
          "expected acceptance of '" + parts + "'");
    }
  }

  @Test
  @DisplayName("signature algorithm that is not an absolute URI is rejected")
  void nonUriAlgorithmIsRejected() throws Exception {
    java.nio.file.Path ksPath = TestKeystoreHelper.createTestKeystore();
    try {
      assertBuildRejectedNaming(
          SecurityProfile.builder("bad-algo")
              .keystore(ksPath.toString(), "changeit")
              .actionsOut("Signature")
              .signatureAlgorithm("not a uri"),
          "SIGNATURE_ALGORITHM");
    } finally {
      java.nio.file.Files.deleteIfExists(ksPath);
    }
  }

  @Test
  @DisplayName("well-formed SIGNATURE_PARTS forms are accepted and preserved verbatim")
  void validPartsFormsAreAccepted() throws Exception {
    java.nio.file.Path ksPath = TestKeystoreHelper.createTestKeystore();
    try {
      String[] valid = {
        "Body",
        "{Content}{http://schemas.xmlsoap.org/soap/envelope/}Body",
        "{}{http://x}Body",
        "{}{}Timestamp",
        "Body;{}{http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-wssecurity-utility-1.0.xsd}Timestamp",
      };
      for (String parts : valid) {
        SecurityProfile p =
            SecurityProfile.builder("good-parts")
                .keystore(ksPath.toString(), "changeit")
                .actionsOut("Signature")
                .signatureParts(parts)
                .build();
        assertEquals(parts, p.signatureParts());
      }
    } finally {
      java.nio.file.Files.deleteIfExists(ksPath);
    }
  }

  @Test
  @DisplayName("producer out-interceptor carries configured knobs under literal WSS4J keys")
  void producerInterceptorAppliesSignatureKnobs() throws Exception {
    java.nio.file.Path ksPath = TestKeystoreHelper.createTestKeystore();
    try {
      String algo = "http://www.w3.org/2001/04/xmldsig-more#rsa-sha384";
      String digest = "http://www.w3.org/2001/04/xmlenc#sha384";
      String c14n = "http://www.w3.org/2001/10/xml-exc-c14n#";
      String parts = "{Content}{http://x}Body";
      SecurityProfile p =
          SecurityProfile.builder("producer")
              .keystore(ksPath.toString(), "changeit")
              .actionsOut("Signature")
              .signatureAlgorithm(algo)
              .signatureDigestAlgorithm(digest)
              .signatureC14nAlgorithm(c14n)
              .signatureParts(parts)
              .build();
      var interceptor = assertInstanceOf(WSS4JOutInterceptor.class, p.createOutInterceptor());
      Map<String, Object> props = interceptor.getProperties();
      assertEquals(algo, props.get("signatureAlgorithm"));
      assertEquals(digest, props.get("signatureDigestAlgorithm"));
      // wss4j 4.0.1: SIG_C14N_ALGO's runtime value is "signatureC14nAlgorithm", not the
      // legacy long form "signatureCanonicalizationAlgorithm"
      assertEquals(c14n, props.get("signatureC14nAlgorithm"));
      assertEquals(parts, props.get("signatureParts"));
    } finally {
      java.nio.file.Files.deleteIfExists(ksPath);
    }
  }

  @Test
  @DisplayName("producer out-interceptor omits signature knob keys when no knobs are set")
  void producerInterceptorOmitsUnsetKnobs() throws Exception {
    java.nio.file.Path ksPath = TestKeystoreHelper.createTestKeystore();
    try {
      SecurityProfile p =
          SecurityProfile.builder("producer")
              .keystore(ksPath.toString(), "changeit")
              .actionsOut("Signature")
              .build();
      var interceptor = assertInstanceOf(WSS4JOutInterceptor.class, p.createOutInterceptor());
      Map<String, Object> props = interceptor.getProperties();
      assertFalse(props.containsKey("signatureAlgorithm"));
      assertFalse(props.containsKey("signatureDigestAlgorithm"));
      assertFalse(props.containsKey("signatureC14nAlgorithm"));
      assertFalse(props.containsKey("signatureParts"));
    } finally {
      java.nio.file.Files.deleteIfExists(ksPath);
    }
  }

  private static void assertBuildRejectedNaming(SecurityProfile.Builder builder, String envVar) {
    IllegalArgumentException ex =
        assertThrows(
            IllegalArgumentException.class, builder::build, "expected build() to reject " + envVar);
    assertTrue(
        ex.getMessage().contains(envVar),
        "expected message naming " + envVar + " but was: " + ex.getMessage());
  }
}
