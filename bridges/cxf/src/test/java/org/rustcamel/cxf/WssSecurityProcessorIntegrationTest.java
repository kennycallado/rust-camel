package org.rustcamel.cxf;

import static org.junit.jupiter.api.Assertions.*;
import static org.mockito.Mockito.*;

import java.nio.file.Files;
import java.nio.file.Path;
import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;

/**
 * Integration tests for WssSecurityProcessor using a real JKS keystore.
 * Exercises the actual WSS4J signing and verification code path — no mocks for crypto.
 */
class WssSecurityProcessorIntegrationTest {

  private static Path keystorePath;

  @BeforeAll
  static void setUp() throws Exception {
    keystorePath = TestKeystoreHelper.createTestKeystore();
  }

  @AfterAll
  static void tearDown() throws Exception {
    if (keystorePath != null) {
      Files.deleteIfExists(keystorePath);
    }
  }

  @Test
  void signAndVerify_roundTrip() throws Exception {
    WssSecurityProcessor processor = createProcessor();

    String soapXml =
        """
        <soapenv:Envelope xmlns:soapenv="http://schemas.xmlsoap.org/soap/envelope/">
          <soapenv:Header/>
          <soapenv:Body>
            <test:Hello xmlns:test="http://test.example.com">World</test:Hello>
          </soapenv:Body>
        </soapenv:Envelope>
        """;

    // Sign the outbound message
    String signed = processor.processOutbound(soapXml);

    // Verify the signed envelope contains WSS Security header elements
    assertTrue(
        signed.contains("wsse:Security") || signed.contains("Security"),
        "Signed envelope should contain WSS Security header");
    assertTrue(
        signed.contains("BinarySecurityToken") || signed.contains("X509"),
        "Signed envelope should contain X509 certificate reference");
    assertTrue(
        signed.contains("SignatureValue"), "Signed envelope should contain SignatureValue");
    assertTrue(
        signed.contains("SignatureMethod"), "Signed envelope should contain SignatureMethod");

    // Verify it round-trips — inbound verification succeeds
    String verified = processor.processInbound(signed);

    // After verification, Security header may be stripped — body content must remain
    assertTrue(
        verified.contains("Hello") && verified.contains("World"),
        "Verified envelope should still contain original body content");
  }

  @Test
  void tampered_message_failsVerification() throws Exception {
    WssSecurityProcessor processor = createProcessor();

    String soapXml =
        """
        <soapenv:Envelope xmlns:soapenv="http://schemas.xmlsoap.org/soap/envelope/">
          <soapenv:Header/>
          <soapenv:Body>
            <test:Hello xmlns:test="http://test.example.com">World</test:Hello>
          </soapenv:Body>
        </soapenv:Envelope>
        """;

    String signed = processor.processOutbound(soapXml);

    // Tamper with the body content — this breaks the signature
    String tampered = signed.replace("World", "Evil");

    // Verification should fail with an exception
    assertThrows(
        Exception.class,
        () -> processor.processInbound(tampered),
        "Tampered message should fail signature verification");
  }

  @Test
  void tampered_signature_value_failsVerification() throws Exception {
    WssSecurityProcessor processor = createProcessor();

    String soapXml =
        """
        <soapenv:Envelope xmlns:soapenv="http://schemas.xmlsoap.org/soap/envelope/">
          <soapenv:Header/>
          <soapenv:Body>
            <test:Hello xmlns:test="http://test.example.com">World</test:Hello>
          </soapenv:Body>
        </soapenv:Envelope>
        """;

    String signed = processor.processOutbound(soapXml);

    // Tamper with the signature value itself
    String tampered = signed.replace("<ds:SignatureValue>", "<ds:SignatureValue>AAAA");

    assertThrows(
        Exception.class,
        () -> processor.processInbound(tampered),
        "Tampered signature should fail verification");
  }

  @Test
  void disabled_processor_passesThrough() throws Exception {
    BridgeConfig config = mock(BridgeConfig.class);
    when(config.keystorePath()).thenReturn(null);
    when(config.truststorePath()).thenReturn(null);

    WssSecurityProcessor processor = new WssSecurityProcessor(config);

    String soapXml = "<soap:Envelope><soap:Body><test/></soap:Envelope>";

    assertEquals(soapXml, processor.processOutbound(soapXml));
    assertEquals(soapXml, processor.processInbound(soapXml));
    assertFalse(processor.isEnabled());
  }

  @Test
  void null_input_returnsNull() throws Exception {
    WssSecurityProcessor processor = createProcessor();

    assertNull(processor.processOutbound(null));
    assertNull(processor.processInbound(null));
  }

  @Test
  void blank_input_returnsBlank() throws Exception {
    WssSecurityProcessor processor = createProcessor();

    assertEquals("", processor.processOutbound(""));
    assertEquals("", processor.processInbound(""));
  }

  @Test
  void processInbound_rejectsUnsignedMessage_whenSignatureRequired() throws Exception {
    // Processor with keystore (canVerifyInbound=true) and Signature required
    WssSecurityProcessor processor = createProcessorWithActions(
        keystorePath, "changeit", "alice", "changeit", "alice",
        "Signature", "Signature");

    String plainSoap =
        """
        <soapenv:Envelope xmlns:soapenv="http://schemas.xmlsoap.org/soap/envelope/">
          <soapenv:Header/>
          <soapenv:Body><test:Hello xmlns:test="http://test.example.com">World</test:Hello></soapenv:Body>
        </soapenv:Envelope>
        """;

    assertThrows(
        Exception.class,
        () -> processor.processInbound(plainSoap),
        "Should reject unsigned message when Signature action is required");
  }

  @Test
  void processOutbound_signatureOnly_whenActionsIsSignature() throws Exception {
    WssSecurityProcessor processor = createProcessorWithActions(
        keystorePath, "changeit", "alice", "changeit", "alice",
        "Signature", "Signature");

    String soapXml =
        """
        <soapenv:Envelope xmlns:soapenv="http://schemas.xmlsoap.org/soap/envelope/">
          <soapenv:Header/>
          <soapenv:Body><test:Hello xmlns:test="http://test.example.com">World</test:Hello></soapenv:Body>
        </soapenv:Envelope>
        """;

    String signed = processor.processOutbound(soapXml);

    assertTrue(
        signed.contains("SignatureValue"), "Should contain digital signature");
    assertFalse(
        signed.contains("EncryptedData"),
        "Should not encrypt when action is Signature only");
  }

  @Test
  void signEncrypt_roundTrip() throws Exception {
    WssSecurityProcessor encProcessor = createProcessorWithActions(
        keystorePath, "changeit", "alice", "changeit", "alice",
        "Signature Encrypt", "Signature Encrypt");

    String soapXml =
        """
        <soapenv:Envelope xmlns:soapenv="http://schemas.xmlsoap.org/soap/envelope/">
          <soapenv:Header/>
          <soapenv:Body><test:Hello xmlns:test="http://test.example.com">World</test:Hello></soapenv:Body>
        </soapenv:Envelope>
        """;

    String processed = encProcessor.processOutbound(soapXml);
    assertTrue(processed.contains("EncryptedData"), "Should be encrypted");
    assertTrue(processed.contains("SignatureValue"), "Should also be signed");

    String decrypted = encProcessor.processInbound(processed);
    assertTrue(
        decrypted.contains("Hello") && decrypted.contains("World"),
        "Decrypted message should contain original body");
  }

  @Test
  void capabilities_signOutbound_requires_keystore() {
    // With keystore → canSignOutbound = true
    WssSecurityProcessor withKs = createProcessorWithActions(
        keystorePath, "changeit", "alice", "changeit", "alice",
        "Signature", "Signature");
    assertTrue(withKs.canSignOutbound(), "Should be able to sign with keystore");

    // Without keystore → canSignOutbound = false
    BridgeConfig noKs = mock(BridgeConfig.class);
    when(noKs.keystorePath()).thenReturn(null);
    WssSecurityProcessor withoutKs = new WssSecurityProcessor(noKs);
    assertFalse(withoutKs.canSignOutbound(), "Should not be able to sign without keystore");
  }

  @Test
  void capabilities_verifyInbound_requires_truststore_or_keystore() {
    // With truststore → canVerifyInbound = true
    BridgeConfig withTs = mock(BridgeConfig.class);
    when(withTs.truststorePath()).thenReturn(keystorePath.toString());
    when(withTs.truststorePassword()).thenReturn("changeit");
    when(withTs.keystorePath()).thenReturn(null);
    WssSecurityProcessor withTruststore = new WssSecurityProcessor(withTs);
    assertTrue(withTruststore.canVerifyInbound(), "Should verify with truststore");

    // With keystore only → canVerifyInbound = true
    BridgeConfig withKsOnly = mock(BridgeConfig.class);
    when(withKsOnly.keystorePath()).thenReturn(keystorePath.toString());
    when(withKsOnly.keystorePassword()).thenReturn("changeit");
    when(withKsOnly.truststorePath()).thenReturn(null);
    WssSecurityProcessor withKeystoreOnly = new WssSecurityProcessor(withKsOnly);
    assertTrue(withKeystoreOnly.canVerifyInbound(), "Should verify with keystore only");

    // With neither → canVerifyInbound = false
    BridgeConfig withNeither = mock(BridgeConfig.class);
    when(withNeither.keystorePath()).thenReturn(null);
    when(withNeither.truststorePath()).thenReturn(null);
    WssSecurityProcessor withNeitherStore = new WssSecurityProcessor(withNeither);
    assertFalse(withNeitherStore.canVerifyInbound(), "Should not verify without any store");
  }

  // --- Helper ---

  /** Creates a WssSecurityProcessor with specific action strings for testing. */
  private WssSecurityProcessor createProcessorWithActions(
      Path ksPath,
      String ksPass,
      String sigUser,
      String sigPass,
      String encUser,
      String actionsOut,
      String actionsIn) {
    BridgeConfig config = mock(BridgeConfig.class);
    when(config.keystorePath()).thenReturn(ksPath.toString());
    when(config.keystorePassword()).thenReturn(ksPass);
    when(config.truststorePath()).thenReturn(ksPath.toString());
    when(config.truststorePassword()).thenReturn(ksPass);
    when(config.sigUsername()).thenReturn(sigUser);
    when(config.sigPassword()).thenReturn(sigPass);
    when(config.encUsername()).thenReturn(encUser);
    when(config.securityActionsOut()).thenReturn(actionsOut);
    when(config.securityActionsIn()).thenReturn(actionsIn);

    SecurityConfig secConfig = mock(SecurityConfig.class);
    when(secConfig.resolveActionsOut()).thenReturn(actionsOut);
    when(secConfig.resolveActionsIn()).thenReturn(actionsIn);

    return new WssSecurityProcessor(config, secConfig);
  }

  private WssSecurityProcessor createProcessor() {
    BridgeConfig config = mock(BridgeConfig.class);
    when(config.keystorePath()).thenReturn(keystorePath.toString());
    when(config.keystorePassword()).thenReturn("changeit");
    when(config.sigUsername()).thenReturn("alice");
    when(config.truststorePath()).thenReturn(null);
    when(config.truststorePassword()).thenReturn(null);
    when(config.encUsername()).thenReturn("serverkey");
    when(config.securityActionsOut()).thenReturn("");
    when(config.securityActionsIn()).thenReturn("");

    SecurityConfig secConfig = mock(SecurityConfig.class);
    when(secConfig.resolveActionsOut()).thenReturn("Signature");
    when(secConfig.resolveActionsIn()).thenReturn("Signature");

    WssSecurityProcessor processor = new WssSecurityProcessor(config);
    processor.securityConfig = secConfig;
    return processor;
  }
}
