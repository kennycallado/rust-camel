package org.rustcamel.cxf;

import static org.junit.jupiter.api.Assertions.*;
import static org.mockito.Mockito.*;

import java.io.ByteArrayInputStream;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.time.Instant;
import java.time.format.DateTimeFormatter;
import java.time.temporal.ChronoUnit;
import java.util.HashSet;
import java.util.Set;
import javax.xml.parsers.DocumentBuilderFactory;
import org.apache.wss4j.common.ext.WSSecurityException;
import org.apache.wss4j.dom.WSConstants;
import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;
import org.w3c.dom.Document;
import org.w3c.dom.Element;
import org.w3c.dom.NodeList;

/**
 * Integration tests for WssSecurityProcessor using a real JKS keystore. Exercises the actual WSS4J
 * signing and verification code path — no mocks for crypto.
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
    assertTrue(signed.contains("SignatureValue"), "Signed envelope should contain SignatureValue");
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
    SecurityProfile profile = SecurityProfile.builder("test").build();
    WssSecurityProcessor processor = new WssSecurityProcessor(profile);

    String soapXml = "<soap:Envelope><soap:Body><test/></soap:Envelope>";

    assertEquals(soapXml, processor.processOutbound(soapXml));
    assertEquals(soapXml, processor.processInbound(soapXml));
    assertFalse(processor.canSignOutbound());
    assertFalse(processor.canVerifyInbound());
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
    WssSecurityProcessor processor =
        createProcessorWithActions(
            keystorePath, "changeit", "alice", "changeit", "alice", "Signature", "Signature");

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
    WssSecurityProcessor processor =
        createProcessorWithActions(
            keystorePath, "changeit", "alice", "changeit", "alice", "Signature", "Signature");

    String soapXml =
        """
        <soapenv:Envelope xmlns:soapenv="http://schemas.xmlsoap.org/soap/envelope/">
          <soapenv:Header/>
          <soapenv:Body><test:Hello xmlns:test="http://test.example.com">World</test:Hello></soapenv:Body>
        </soapenv:Envelope>
        """;

    String signed = processor.processOutbound(soapXml);

    assertTrue(signed.contains("SignatureValue"), "Should contain digital signature");
    assertFalse(
        signed.contains("EncryptedData"), "Should not encrypt when action is Signature only");
  }

  @Test
  void signEncrypt_roundTrip() throws Exception {
    WssSecurityProcessor encProcessor =
        createProcessorWithActions(
            keystorePath,
            "changeit",
            "alice",
            "changeit",
            "alice",
            "Signature Encrypt",
            "Signature Encrypt");

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
    WssSecurityProcessor withKs =
        createProcessorWithActions(
            keystorePath, "changeit", "alice", "changeit", "alice", "Signature", "Signature");
    assertTrue(withKs.canSignOutbound(), "Should be able to sign with keystore");

    // Without keystore → canSignOutbound = false
    SecurityProfile noKs = SecurityProfile.builder("test").build();
    WssSecurityProcessor withoutKs = new WssSecurityProcessor(noKs);
    assertFalse(withoutKs.canSignOutbound(), "Should not be able to sign without keystore");
  }

  @Test
  void capabilities_verifyInbound_requires_truststore_or_keystore() {
    // With truststore → canVerifyInbound = true
    SecurityProfile withTs =
        SecurityProfile.builder("test").truststore(keystorePath.toString(), "changeit").build();
    WssSecurityProcessor withTruststore = new WssSecurityProcessor(withTs);
    assertTrue(withTruststore.canVerifyInbound(), "Should verify with truststore");

    // With keystore only → canVerifyInbound = true
    SecurityProfile withKsOnly =
        SecurityProfile.builder("test").keystore(keystorePath.toString(), "changeit").build();
    WssSecurityProcessor withKeystoreOnly = new WssSecurityProcessor(withKsOnly);
    assertTrue(withKeystoreOnly.canVerifyInbound(), "Should verify with keystore only");

    // With neither → canVerifyInbound = false
    SecurityProfile withNeither = SecurityProfile.builder("test").build();
    WssSecurityProcessor withNeitherStore = new WssSecurityProcessor(withNeither);
    assertFalse(withNeitherStore.canVerifyInbound(), "Should not verify without any store");
  }

  @Test
  void replayedFreshSignedMessageRejectedAtProcessorLevel() throws Exception {
    WssSecurityProcessor processor =
        createProcessorWithActions(
            keystorePath,
            "changeit",
            "alice",
            "changeit",
            "alice",
            "Timestamp Signature",
            "Timestamp Signature");

    String plainEnvelope =
        """
        <soapenv:Envelope xmlns:soapenv="http://schemas.xmlsoap.org/soap/envelope/">
          <soapenv:Header/>
          <soapenv:Body><test:Hello xmlns:test="http://test.example.com">World</test:Hello></soapenv:Body>
        </soapenv:Envelope>
        """;

    String signed = processor.processOutbound(plainEnvelope);

    // First delivery of the identical bytes must verify normally
    assertDoesNotThrow(() -> processor.processInbound(signed));

    // Replay: delivering the identical signed bytes again must hit the replay cache
    assertThrows(
        WSSecurityException.class,
        () -> processor.processInbound(signed),
        "Replayed signed message must be rejected at processor level");
  }

  @Test
  void timestampRewriteCannotMintFreshCacheKey() throws Exception {
    WssSecurityProcessor processor =
        createProcessorWithActions(
            keystorePath,
            "changeit",
            "alice",
            "changeit",
            "alice",
            "Timestamp Signature",
            "Timestamp Signature");

    String plainEnvelope =
        """
        <soapenv:Envelope xmlns:soapenv="http://schemas.xmlsoap.org/soap/envelope/">
          <soapenv:Header/>
          <soapenv:Body><test:Hello xmlns:test="http://test.example.com">World</test:Hello></soapenv:Body>
        </soapenv:Envelope>
        """;

    String signed = processor.processOutbound(plainEnvelope);

    // Action A: the original signed bytes verify normally
    assertDoesNotThrow(() -> processor.processInbound(signed));

    // Action B: rewrite Created/Expires to fresh values — breaks the signature over the Timestamp.
    // The rewritten unsigned timestamp must NOT be processed as fresh.
    String rewritten = rewriteTimestampFresh(signed);
    assertThrows(
        WSSecurityException.class,
        () -> processor.processInbound(rewritten),
        "Rewritten unsigned timestamp must fail signature validation");

    // Action C: re-deliver the original bytes again — replay cache hit
    assertThrows(
        WSSecurityException.class,
        () -> processor.processInbound(signed),
        "Original bytes must hit the replay cache after first delivery");
  }

  @Test
  void soap12OutboundInboundRoundtrip() throws Exception {
    WssSecurityProcessor processor =
        createProcessorWithActions(
            keystorePath,
            "changeit",
            "alice",
            "changeit",
            "alice",
            "Timestamp Signature",
            "Timestamp Signature");

    String soap12Xml =
        """
        <soapenv:Envelope xmlns:soapenv="http://www.w3.org/2003/05/soap-envelope">
          <soapenv:Header/>
          <soapenv:Body><test:Hello xmlns:test="http://test.example.com">World</test:Hello></soapenv:Body>
        </soapenv:Envelope>
        """;

    String signed = processor.processOutbound(soap12Xml);

    // The signature must cover exactly the SOAP 1.2 Body and the wsu:Timestamp — the Body
    // reference carries the envelope's own namespace, not a hardcoded SOAP 1.1 one.
    Document signedDoc = parseNamespaced(signed);
    Set<String> coveredIds = new HashSet<>();
    coveredIds.add(requireWsuId(soleElement(signedDoc, SOAP12_NS, "Body")));
    coveredIds.add(requireWsuId(soleElement(signedDoc, WSConstants.WSU_NS, "Timestamp")));

    NodeList refs = signedDoc.getElementsByTagName("ds:Reference");
    assertEquals(2, refs.getLength(), "Signature should carry exactly two references");
    for (int i = 0; i < refs.getLength(); i++) {
      String uri = ((Element) refs.item(i)).getAttribute("URI");
      assertTrue(uri.startsWith("#"), "Reference URI must be a local id");
      coveredIds.remove(uri.substring(1));
    }
    assertTrue(coveredIds.isEmpty(), "Signature must cover the 1.2 Body and Timestamp");

    // One inbound pass verifies cleanly (a second would trip the replay cache).
    String verified = processor.processInbound(signed);
    assertTrue(
        verified.contains("Hello") && verified.contains("World"),
        "Verified envelope should still contain original body content");
  }

  // --- XML helpers for signature-coverage assertions ---

  private static final String SOAP12_NS = "http://www.w3.org/2003/05/soap-envelope";

  /** Parses XML with namespace awareness so coverage assertions can look parts up by NS. */
  private static Document parseNamespaced(String xml) throws Exception {
    DocumentBuilderFactory dbf = DocumentBuilderFactory.newInstance();
    dbf.setNamespaceAware(true);
    return dbf.newDocumentBuilder()
        .parse(new ByteArrayInputStream(xml.getBytes(StandardCharsets.UTF_8)));
  }

  private static Element soleElement(Document doc, String ns, String localName) {
    NodeList nodes = doc.getElementsByTagNameNS(ns, localName);
    assertEquals(1, nodes.getLength(), "Expected exactly one <" + localName + "> element");
    return (Element) nodes.item(0);
  }

  private static String requireWsuId(Element element) {
    String id = element.getAttributeNS(WSConstants.WSU_NS, "Id");
    assertFalse(id.isBlank(), "Signed part must carry a wsu:Id");
    return id;
  }

  // --- Timestamp rewrite helpers ---

  /** Replaces the Created/Expires text of the wsu:Timestamp with fresh values. */
  private static String rewriteTimestampFresh(String xml) {
    Instant now = Instant.now();
    DateTimeFormatter fmt = DateTimeFormatter.ISO_INSTANT;
    xml = replaceTimestampNodeText(xml, "Created", fmt.format(now.truncatedTo(ChronoUnit.MILLIS)));
    return replaceTimestampNodeText(
        xml, "Expires", fmt.format(now.plusSeconds(300).truncatedTo(ChronoUnit.MILLIS)));
  }

  private static String replaceTimestampNodeText(String xml, String localName, String newValue) {
    String open = "<wsu:" + localName + ">";
    int valueStart = xml.indexOf(open);
    assertTrue(valueStart >= 0, "Expected element <wsu:" + localName + "> in envelope");
    valueStart += open.length();
    int valueEnd = xml.indexOf("</wsu:" + localName + ">", valueStart);
    assertTrue(valueEnd > valueStart, "Expected closing </wsu:" + localName + ">");
    return xml.substring(0, valueStart) + newValue + xml.substring(valueEnd);
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
    SecurityProfile profile =
        SecurityProfile.builder("test")
            .keystore(ksPath.toString(), ksPass)
            .truststore(ksPath.toString(), ksPass)
            .sigUser(sigUser, sigPass)
            .encUser(encUser)
            .actionsOut(actionsOut)
            .actionsIn(actionsIn)
            .build();
    return new WssSecurityProcessor(profile);
  }

  private WssSecurityProcessor createProcessor() {
    SecurityProfile profile =
        SecurityProfile.builder("test")
            .keystore(keystorePath.toString(), "changeit")
            .sigUser("alice", "changeit")
            .encUser("serverkey")
            .build();
    return new WssSecurityProcessor(profile);
  }
}
