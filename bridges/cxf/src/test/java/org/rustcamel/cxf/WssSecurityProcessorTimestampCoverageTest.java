package org.rustcamel.cxf;

import static org.junit.jupiter.api.Assertions.*;

import java.io.ByteArrayInputStream;
import java.lang.reflect.InvocationTargetException;
import java.lang.reflect.Method;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.time.Instant;
import java.time.format.DateTimeFormatter;
import java.time.temporal.ChronoUnit;
import java.util.List;
import javax.xml.parsers.DocumentBuilderFactory;
import javax.xml.transform.dom.DOMSource;
import org.apache.wss4j.common.ext.WSSecurityException;
import org.apache.wss4j.dom.WSConstants;
import org.apache.wss4j.dom.WSDataRef;
import org.apache.wss4j.dom.engine.WSSecurityEngineResult;
import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;
import org.w3c.dom.Document;
import org.w3c.dom.Element;
import org.w3c.dom.NodeList;

/**
 * Branch tests for the timestamp-signature coverage check of WssSecurityProcessor. Each branch of
 * verifyTimestampSignatureCoverage rejects a distinct attack shape: an unsigned Timestamp sitting
 * next to a valid Body-only signature, an injected Timestamp shadowing the signed one in document
 * order, and a Timestamp result whose element is gone by check time (defense in depth). The
 * injected and stripped shapes never reach the check on the wire — the WSS4J engine rejects
 * multi-Timestamp headers itself (BSP:R3227) and element-less messages yield no TS result — so
 * those branches are pinned by direct invocation of the private check. Every rejection asserts its
 * distinct operator-facing message text: since bd rc-sggo9 the coverage throw sites carry their
 * detail verbatim on WssRequirementException, so the texts are part of the pinned behavior.
 */
class WssSecurityProcessorTimestampCoverageTest {

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

  // --- unsigned Timestamp: present, fresh, outside every verified reference ---

  @Test
  void processInbound_rejectsUnsignedTimestamp_nextToBodyOnlySignature() throws Exception {
    // Inbound requires Timestamp + Signature, so the coverage check runs.
    WssSecurityProcessor processor = createProcessor("Signature", "Timestamp Signature");

    // Outbound action list has no Timestamp, so the signature covers the Body only.
    String signed = processor.processOutbound(soap11Envelope());
    Document doc = parseNamespaced(signed);
    Element securityHeader = soleElement(doc, WSConstants.WSSE_NS, "Security");
    securityHeader.appendChild(freshTimestamp(doc, "UnsignedTimestamp-1"));
    String crafted = SoapEnvelopeHelper.sourceToString(new DOMSource(doc));

    // Control: with Signature alone required, the same crafted message verifies. The signature
    // is cryptographically sound and the Timestamp passes its freshness window, so the rejection
    // below is caused by the unsigned Timestamp failing the coverage check alone.
    WssSecurityProcessor signatureOnlyVerifier = createProcessor("Signature", "Signature");
    assertDoesNotThrow(() -> signatureOnlyVerifier.processInbound(crafted));

    WSSecurityException unsignedEx =
        assertThrows(
            WSSecurityException.class,
            () -> processor.processInbound(crafted),
            "An unsigned Timestamp next to a Body-only signature must be rejected");
    assertEquals(
        "wsu:Timestamp is not covered by the message signature",
        unsignedEx.getMessage(),
        "The unsigned-Timestamp failure must carry its distinct detail text");
  }

  // --- injected Timestamp: wire shape rejected by the engine, branch pinned directly ---

  /**
   * On the wire, an attacker Timestamp added ahead of the signed one is rejected by the WSS4J
   * engine itself: BSP rule R3227 forbids more than one Timestamp per Security header. The control
   * proves the unmodified single-Timestamp message verifies, so the rejection is caused by the
   * injected element alone.
   */
  @Test
  void processInbound_rejectsInjectedTimestamp_atEngineLevel() throws Exception {
    WssSecurityProcessor processor = createProcessor("Timestamp Signature", "Timestamp Signature");

    String signed = processor.processOutbound(soap11Envelope());
    Document doc = parseNamespaced(signed);
    Element securityHeader = soleElement(doc, WSConstants.WSSE_NS, "Security");
    Element attackerTimestamp = freshTimestamp(doc, "InjectedTimestamp-attacker");
    securityHeader.insertBefore(attackerTimestamp, securityHeader.getFirstChild());
    String crafted = SoapEnvelopeHelper.sourceToString(new DOMSource(doc));

    WssSecurityProcessor sibling = createProcessor("Timestamp Signature", "Timestamp Signature");
    assertDoesNotThrow(() -> sibling.processInbound(signed));

    WSSecurityException ex =
        assertThrows(
            WSSecurityException.class,
            () -> processor.processInbound(crafted),
            "A Security header with two Timestamps must be rejected");
    assertTrue(
        ex.getMessage().contains("R3227"),
        "Engine-level rejection must cite the multi-Timestamp BSP rule, got: " + ex.getMessage());
  }

  /**
   * The coverage check's own answer to the injected shape: the first Timestamp in document order —
   * the one an attacker would surface — must be a covered one, so a shadowing element fails even
   * though a different, covered Timestamp exists in the same message.
   */
  @Test
  void verifyTimestampSignatureCoverage_rejectsInjectedTimestamp_shadowingSignedTimestamp()
      throws Exception {
    WssSecurityProcessor processor =
        new WssSecurityProcessor(SecurityProfile.builder("coverage").build());

    Document doc = parseNamespaced(soap11Envelope());
    doc.getDocumentElement().appendChild(freshTimestamp(doc, "InjectedTimestamp-attacker"));
    doc.getDocumentElement().appendChild(freshTimestamp(doc, "InjectedTimestamp-signed"));

    WSDataRef dataRef = new WSDataRef();
    dataRef.setWsuId("InjectedTimestamp-signed");
    List<WSSecurityEngineResult> signatureResult =
        List.of(new WSSecurityEngineResult(WSConstants.SIGN, List.of(dataRef)));

    WSSecurityException shadowingEx =
        assertThrows(
            WSSecurityException.class,
            () -> invokeCoverageCheck(processor, doc, signatureResult),
            "A Timestamp injected ahead of the signed one must fail the coverage check");
    assertEquals(
        "wsu:Timestamp is not covered by the message signature",
        shadowingEx.getMessage(),
        "The shadowing-Timestamp failure must carry its distinct detail text");
  }

  // --- stripped Timestamp: engine result present, element absent (defense in depth) ---

  /**
   * The element-absent branch cannot be reached through processInbound with a wire message: a
   * message whose Timestamp was stripped yields no TS engine result, so enforceRequiredActions
   * rejects it first. The branch guards the case where a TS result exists but the element is gone
   * by check time, so it is pinned here by invoking the private check directly with an
   * engine-shaped results list.
   */
  @Test
  void verifyTimestampSignatureCoverage_rejectsStrippedTimestamp_elementAbsent() throws Exception {
    WssSecurityProcessor processor =
        new WssSecurityProcessor(SecurityProfile.builder("coverage").build());

    Document doc = parseNamespaced(soap11Envelope());
    List<WSSecurityEngineResult> timestampResult =
        List.of(new WSSecurityEngineResult(WSConstants.TS));

    WSSecurityException strippedEx =
        assertThrows(
            WSSecurityException.class,
            () -> invokeCoverageCheck(processor, doc, timestampResult),
            "A missing wsu:Timestamp element must be rejected even when a TS result exists");
    assertEquals(
        "Required wsu:Timestamp element not found in message",
        strippedEx.getMessage(),
        "The stripped-Timestamp failure must carry its distinct detail text");
  }

  /** Happy-path control: the check returns normally when a signed reference covers the element. */
  @Test
  void verifyTimestampSignatureCoverage_acceptsCoveredTimestamp() throws Exception {
    WssSecurityProcessor processor =
        new WssSecurityProcessor(SecurityProfile.builder("coverage").build());

    Document doc = parseNamespaced(soap11Envelope());
    Element timestamp = freshTimestamp(doc, "CoveredTimestamp-1");
    doc.getDocumentElement().appendChild(timestamp);

    // Id-equality return: the reference names the Timestamp's wsu:Id.
    WSDataRef idRef = new WSDataRef();
    idRef.setWsuId("CoveredTimestamp-1");
    List<WSSecurityEngineResult> idRefResult =
        List.of(new WSSecurityEngineResult(WSConstants.SIGN, List.of(idRef)));
    assertDoesNotThrow(() -> invokeCoverageCheck(processor, doc, idRefResult));

    // Element-identity return: the reference carries no wsu:Id but protects the element itself.
    WSDataRef elementRef = new WSDataRef();
    elementRef.setProtectedElement(timestamp);
    List<WSSecurityEngineResult> elementRefResult =
        List.of(new WSSecurityEngineResult(WSConstants.SIGN, List.of(elementRef)));
    assertDoesNotThrow(() -> invokeCoverageCheck(processor, doc, elementRefResult));
  }

  // --- Helpers ---

  /** Creates a WssSecurityProcessor with the given outbound/inbound action strings. */
  private static WssSecurityProcessor createProcessor(String actionsOut, String actionsIn) {
    SecurityProfile profile =
        SecurityProfile.builder("test")
            .keystore(keystorePath.toString(), "changeit")
            .truststore(keystorePath.toString(), "changeit")
            .sigUser("alice", "changeit")
            .encUser("alice")
            .actionsOut(actionsOut)
            .actionsIn(actionsIn)
            .build();
    return new WssSecurityProcessor(profile);
  }

  /** SOAP 1.1 envelope used as the signing input. */
  private static String soap11Envelope() {
    return """
        <soapenv:Envelope xmlns:soapenv="http://schemas.xmlsoap.org/soap/envelope/">
          <soapenv:Header/>
          <soapenv:Body><test:Hello xmlns:test="http://test.example.com">World</test:Hello></soapenv:Body>
        </soapenv:Envelope>
        """;
  }

  /**
   * Parses XML through the suite's canonical secure, namespace-aware DocumentBuilderFactory so
   * header manipulation can look elements up by NS.
   */
  private static Document parseNamespaced(String xml) throws Exception {
    DocumentBuilderFactory dbf =
        SoapEnvelopeHelper.configureSecure(DocumentBuilderFactory.newInstance());
    return dbf.newDocumentBuilder()
        .parse(new ByteArrayInputStream(xml.getBytes(StandardCharsets.UTF_8)));
  }

  private static Element soleElement(Document doc, String ns, String localName) {
    NodeList nodes = doc.getElementsByTagNameNS(ns, localName);
    assertEquals(1, nodes.getLength(), "Expected exactly one <" + localName + "> element");
    return (Element) nodes.item(0);
  }

  /** Builds a wsu:Timestamp with a fresh Created/Expires window and the given wsu:Id. */
  private static Element freshTimestamp(Document doc, String wsuId) {
    Element timestamp = doc.createElementNS(WSConstants.WSU_NS, "wsu:Timestamp");
    timestamp.setAttributeNS(WSConstants.WSU_NS, "wsu:Id", wsuId);
    DateTimeFormatter fmt = DateTimeFormatter.ISO_INSTANT;
    Instant now = Instant.now();
    appendWsuChild(doc, timestamp, "Created", fmt.format(now.truncatedTo(ChronoUnit.MILLIS)));
    appendWsuChild(
        doc, timestamp, "Expires", fmt.format(now.plusSeconds(300).truncatedTo(ChronoUnit.MILLIS)));
    return timestamp;
  }

  private static void appendWsuChild(Document doc, Element parent, String localName, String text) {
    Element child = doc.createElementNS(WSConstants.WSU_NS, "wsu:" + localName);
    child.setTextContent(text);
    parent.appendChild(child);
  }

  /**
   * Invokes the private verifyTimestampSignatureCoverage method. WSSecurityException causes are
   * rethrown as-is so callers can assert on them directly.
   */
  private static void invokeCoverageCheck(
      WssSecurityProcessor processor, Document doc, List<WSSecurityEngineResult> results)
      throws Exception {
    Method check =
        WssSecurityProcessor.class.getDeclaredMethod(
            "verifyTimestampSignatureCoverage", Document.class, List.class);
    check.setAccessible(true);
    try {
      check.invoke(processor, doc, results);
    } catch (InvocationTargetException e) {
      if (e.getCause() instanceof WSSecurityException wss) {
        throw wss;
      }
      throw e;
    }
  }
}
