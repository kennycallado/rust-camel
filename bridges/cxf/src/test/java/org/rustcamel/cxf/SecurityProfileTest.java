package org.rustcamel.cxf;

import static org.junit.jupiter.api.Assertions.*;

import java.util.Properties;
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
}
