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

  // --- Consumer-path signature knob application ---

  private static final String SOAP11_NS = "http://schemas.xmlsoap.org/soap/envelope/";
  private static final String DS_NS = "http://www.w3.org/2000/09/xmldsig#";
  // sha-384 lives in the xmldsig-more namespace — the xmlenc namespace has no sha384 digest
  // registration in Santuario, so the xmlenc#sha384 form would fail digest resolution.
  private static final String SHA384_DIGEST_URI = "http://www.w3.org/2001/04/xmldsig-more#sha384";
  private static final String RSA_SHA384_URI = "http://www.w3.org/2001/04/xmldsig-more#rsa-sha384";
  // exc-c14n WITHOUT comments is WSS4J's default c14n — using it here could not go RED.
  // The WithComments variant is a distinct non-default URI, so a dropped knob call fails.
  private static final String EXC_C14N_URI = "http://www.w3.org/2001/10/xml-exc-c14n#";
  private static final String EXC_C14N_WITH_COMMENTS_URI =
      "http://www.w3.org/2001/10/xml-exc-c14n#WithComments";

  /** SOAP 1.1 envelope signed by the consumer-path knob tests. */
  private static String soap11Envelope() {
    return """
        <soapenv:Envelope xmlns:soapenv="http://schemas.xmlsoap.org/soap/envelope/">
          <soapenv:Header/>
          <soapenv:Body>
            <test:Hello xmlns:test="http://test.example.com">World</test:Hello>
          </soapenv:Body>
        </soapenv:Envelope>
        """;
  }

  /** Consumer signed-response profile: keystore plus "Signature Timestamp" out-actions. */
  private SecurityProfile.Builder consumerSigningProfileBuilder() {
    return SecurityProfile.builder("test")
        .keystore(keystorePath.toString(), "changeit")
        .truststore(keystorePath.toString(), "changeit")
        .sigUser("alice", "changeit")
        .encUser("alice")
        .actionsOut("Signature Timestamp")
        .actionsIn("Signature");
  }

  /** Collects the URI attribute of every ds:Reference, stripping the leading '#'. */
  private static Set<String> signedReferenceIds(Document doc) {
    Set<String> ids = new HashSet<>();
    NodeList refs = doc.getElementsByTagNameNS(DS_NS, "Reference");
    for (int i = 0; i < refs.getLength(); i++) {
      Element ref = (Element) refs.item(i);
      ids.add(ref.getAttribute("URI").replaceFirst("^#", ""));
    }
    return ids;
  }

  /** Asserts the signature references cover both the SOAP 1.1 Body and the Timestamp. */
  private static void assertCoversBodyAndTimestamp(Document doc) {
    Set<String> signedIds = signedReferenceIds(doc);
    assertTrue(
        signedIds.contains(requireWsuId(soleElement(doc, SOAP11_NS, "Body"))),
        "Signature must reference the envelope Body");
    assertTrue(
        signedIds.contains(requireWsuId(soleElement(doc, WSConstants.WSU_NS, "Timestamp"))),
        "Signature must reference the Timestamp");
  }

  @Test
  void consumerAppliesDigestAlgorithm() throws Exception {
    WssSecurityProcessor processor =
        new WssSecurityProcessor(
            consumerSigningProfileBuilder().signatureDigestAlgorithm(SHA384_DIGEST_URI).build());

    Document doc = parseNamespaced(processor.processOutbound(soap11Envelope()));

    NodeList digestMethods = doc.getElementsByTagNameNS(DS_NS, "DigestMethod");
    assertTrue(digestMethods.getLength() > 0, "Signature must carry DigestMethod elements");
    for (int i = 0; i < digestMethods.getLength(); i++) {
      assertEquals(
          SHA384_DIGEST_URI,
          ((Element) digestMethods.item(i)).getAttribute("Algorithm"),
          "Every DigestMethod must use the configured digest");
    }
    assertCoversBodyAndTimestamp(doc);
  }

  @Test
  void consumerAppliesSignatureAlgorithm() throws Exception {
    WssSecurityProcessor processor =
        new WssSecurityProcessor(
            consumerSigningProfileBuilder().signatureAlgorithm(RSA_SHA384_URI).build());

    Document doc = parseNamespaced(processor.processOutbound(soap11Envelope()));

    assertEquals(
        RSA_SHA384_URI,
        soleElement(doc, DS_NS, "SignatureMethod").getAttribute("Algorithm"),
        "SignatureMethod must use the configured signature algorithm");
  }

  @Test
  void consumerAppliesCanonicalizationAlgorithm() throws Exception {
    WssSecurityProcessor processor =
        new WssSecurityProcessor(
            consumerSigningProfileBuilder()
                .signatureC14nAlgorithm(EXC_C14N_WITH_COMMENTS_URI)
                .build());

    Document doc = parseNamespaced(processor.processOutbound(soap11Envelope()));

    assertEquals(
        EXC_C14N_WITH_COMMENTS_URI,
        soleElement(doc, DS_NS, "CanonicalizationMethod").getAttribute("Algorithm"),
        "CanonicalizationMethod must use the configured c14n algorithm");
  }

  @Test
  void consumerDefaultsUnchangedWithoutKnobs() throws Exception {
    WssSecurityProcessor processor =
        new WssSecurityProcessor(consumerSigningProfileBuilder().build());

    Document doc = parseNamespaced(processor.processOutbound(soap11Envelope()));

    assertEquals(
        WSConstants.RSA_SHA1,
        soleElement(doc, DS_NS, "SignatureMethod").getAttribute("Algorithm"),
        "No knobs configured — the pre-change default signature algorithm must stay rsa-sha1");

    NodeList digestMethods = doc.getElementsByTagNameNS(DS_NS, "DigestMethod");
    assertTrue(digestMethods.getLength() > 0, "Signature must carry DigestMethod elements");
    for (int i = 0; i < digestMethods.getLength(); i++) {
      assertEquals(
          WSConstants.SHA1,
          ((Element) digestMethods.item(i)).getAttribute("Algorithm"),
          "No knobs configured — the pre-change default digest must stay sha-1");
    }

    assertEquals(
        EXC_C14N_URI,
        soleElement(doc, DS_NS, "CanonicalizationMethod").getAttribute("Algorithm"),
        "No knobs configured — the pre-change default c14n must stay exclusive c14n");
    assertCoversBodyAndTimestamp(doc);
  }

  @Test
  void consumerRefusesPartsProfile() {
    // "Body" is valid PARTS syntax with a keystore, a truststore, and a Signature action — the
    // builder accepts it; refusal happens on the consumer path because coverage is path-fixed.
    // The truststore is required: inbound Signature needs verification anchors at build time.
    SecurityProfile profile =
        SecurityProfile.builder("test")
            .keystore(keystorePath.toString(), "changeit")
            .truststore(keystorePath.toString(), "changeit")
            .sigUser("alice", "changeit")
            .encUser("alice")
            .actionsOut("Signature")
            .actionsIn("Signature")
            .signatureParts("Body")
            .build();

    IllegalStateException ex =
        assertThrows(IllegalStateException.class, () -> new WssSecurityProcessor(profile));
    assertTrue(ex.getMessage().contains("SIGNATURE_PARTS"), "Message must name SIGNATURE_PARTS");
    assertTrue(ex.getMessage().contains("Body+Timestamp"), "Message must name Body+Timestamp");
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
