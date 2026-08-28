package org.rustcamel.cxf;

import static org.junit.jupiter.api.Assertions.*;

import jakarta.xml.soap.MessageFactory;
import jakarta.xml.soap.MimeHeaders;
import jakarta.xml.soap.SOAPConstants;
import jakarta.xml.soap.SOAPMessage;
import java.io.ByteArrayInputStream;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.HashSet;
import java.util.Set;
import javax.xml.transform.dom.DOMSource;
import org.apache.cxf.binding.soap.Soap11;
import org.apache.cxf.binding.soap.SoapMessage;
import org.apache.cxf.binding.soap.saaj.SAAJOutInterceptor;
import org.apache.cxf.bus.managers.PhaseManagerImpl;
import org.apache.cxf.interceptor.Interceptor;
import org.apache.cxf.message.Exchange;
import org.apache.cxf.message.ExchangeImpl;
import org.apache.cxf.message.Message;
import org.apache.cxf.message.MessageImpl;
import org.apache.cxf.phase.PhaseInterceptorChain;
import org.apache.wss4j.dom.WSConstants;
import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;
import org.w3c.dom.Document;
import org.w3c.dom.Element;
import org.w3c.dom.NodeList;

/**
 * Wire-level proof for the producer out-interceptor path: the profile's {@code WSS4JOutInterceptor}
 * runs inside a real {@code PhaseInterceptorChain} (with {@code SAAJOutInterceptor}) over a SAAJ
 * message, and assertions target the mutated OUTPUT document — never interceptor properties.
 */
class SecurityProfileWireTest {

  private static final String SOAP11_NS = "http://schemas.xmlsoap.org/soap/envelope/";
  private static final String DS_NS = "http://www.w3.org/2000/09/xmldsig#";
  private static final String SPLIT_KEY_PASSWORD = "split-key-pass";

  private static Path keystorePath;
  private static Path splitKeystorePath;

  @BeforeAll
  static void setUp() throws Exception {
    keystorePath = TestKeystoreHelper.createTestKeystore();
    splitKeystorePath =
        TestKeystoreHelper.createTestKeystore(
            TestKeystoreHelper.KEYSTORE_PASSWORD, SPLIT_KEY_PASSWORD);
  }

  @AfterAll
  static void tearDown() throws Exception {
    if (keystorePath != null) {
      Files.deleteIfExists(keystorePath);
    }
    if (splitKeystorePath != null) {
      Files.deleteIfExists(splitKeystorePath);
    }
  }

  @Test
  void signedMessageCarriesCoveredTimestamp() throws Exception {
    SecurityProfile profile = wireProfileBuilder().actionsOut("Signature Timestamp").build();

    Document doc = runOutboundChain(profile);

    assertEquals(
        1,
        doc.getElementsByTagNameNS(WSConstants.WSU_NS, "Timestamp").getLength(),
        "Timestamp action must emit a wsu:Timestamp on the wire");
    assertCoversBodyAndTimestamp(doc);
  }

  @Test
  void explicitBodyOnlyPartsWire() throws Exception {
    SecurityProfile profile =
        wireProfileBuilder().actionsOut("Signature Timestamp").signatureParts("Body").build();

    Document doc = runOutboundChain(profile);

    assertEquals(
        1,
        doc.getElementsByTagNameNS(WSConstants.WSU_NS, "Timestamp").getLength(),
        "Timestamp action must still emit the element when parts exclude it");
    Set<String> referenceIds = signedReferenceIds(doc);
    assertEquals(
        Set.of(requireWsuId(soleElement(doc, SOAP11_NS, "Body"))),
        referenceIds,
        "Explicit SIGNATURE_PARTS must be honored verbatim on the wire");
    Element timestamp = soleElement(doc, WSConstants.WSU_NS, "Timestamp");
    assertFalse(
        referenceIds.contains(timestamp.getAttributeNS(WSConstants.WSU_NS, "Id")),
        "The emitted Timestamp must not be referenced under Body-only parts");
  }

  @Test
  void timestampFreeWireUnchanged() throws Exception {
    SecurityProfile profile = wireProfileBuilder().actionsOut("Signature").build();

    Document doc = runOutboundChain(profile);

    assertEquals(
        0,
        doc.getElementsByTagNameNS(WSConstants.WSU_NS, "Timestamp").getLength(),
        "No Timestamp action — no wsu:Timestamp may appear");
    assertEquals(
        Set.of(requireWsuId(soleElement(doc, SOAP11_NS, "Body"))),
        signedReferenceIds(doc),
        "Coverage without a Timestamp action must stay Body-only");
  }

  @Test
  void sigPasswordHonoredOnWire() throws Exception {
    SecurityProfile profile =
        SecurityProfile.builder("wire")
            .keystore(splitKeystorePath.toString(), TestKeystoreHelper.KEYSTORE_PASSWORD)
            .sigUser(TestKeystoreHelper.KEY_ALIAS, SPLIT_KEY_PASSWORD)
            .actionsOut("Signature Timestamp")
            .build();

    Document doc = runOutboundChain(profile);

    assertEquals(
        1, doc.getElementsByTagNameNS(DS_NS, "Signature").getLength(), "Signature must be present");
    assertFalse(
        soleElement(doc, DS_NS, "SignatureValue").getTextContent().isBlank(),
        "SignatureValue must carry bytes");

    // Signing at all proves the callback supplied sigPassword: the keystore password cannot
    // unlock this key. The round-trip below additionally proves the signature is verifiable.
    SecurityProfile verifyProfile =
        SecurityProfile.builder("wire-verify")
            .keystore(splitKeystorePath.toString(), TestKeystoreHelper.KEYSTORE_PASSWORD)
            .truststore(splitKeystorePath.toString(), TestKeystoreHelper.KEYSTORE_PASSWORD)
            .sigUser(TestKeystoreHelper.KEY_ALIAS, SPLIT_KEY_PASSWORD)
            .actionsOut("Signature Timestamp")
            .actionsIn("Timestamp Signature")
            .build();
    String verified =
        new WssSecurityProcessor(verifyProfile)
            .processInbound(SoapEnvelopeHelper.sourceToString(new DOMSource(doc)));
    assertTrue(
        verified.contains("Hello") && verified.contains("World"),
        "Verified envelope must retain the body content");
  }

  // --- Fixtures ---

  /** SOAP 1.1 envelope pushed through the outbound chain. */
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

  /** Producer-path signing profile over the standard fixture keystore. */
  private SecurityProfile.Builder wireProfileBuilder() {
    return SecurityProfile.builder("wire")
        .keystore(keystorePath.toString(), TestKeystoreHelper.KEYSTORE_PASSWORD)
        .sigUser(TestKeystoreHelper.KEY_ALIAS, TestKeystoreHelper.KEYSTORE_PASSWORD);
  }

  // --- Outbound chain scaffolding ---

  /**
   * Runs the profile's real out-interceptor chain in-process: a {@code PhaseInterceptorChain} with
   * {@code SAAJOutInterceptor} plus the profile's {@code WSS4JOutInterceptor} over a {@code
   * SoapMessage} backed by a parsed SAAJ document. A direct {@code handleMessage} call would not
   * sign anything — the interceptor only queues its POST_PROTOCOL ending into the live chain.
   *
   * @return the mutated SAAJ SOAP document
   */
  private static Document runOutboundChain(SecurityProfile profile) throws Exception {
    Interceptor<? extends Message> outInterceptor = profile.createOutInterceptor();
    assertNotNull(outInterceptor, "Security-enabled profile must produce an out-interceptor");

    SOAPMessage saaj =
        MessageFactory.newInstance(SOAPConstants.SOAP_1_1_PROTOCOL)
            .createMessage(
                new MimeHeaders(),
                new ByteArrayInputStream(soap11Envelope().getBytes(StandardCharsets.UTF_8)));

    SoapMessage message = new SoapMessage(new MessageImpl());
    Exchange exchange = new ExchangeImpl();
    exchange.setOutMessage(message);
    message.setExchange(exchange);
    message.setVersion(Soap11.getInstance());
    message.put(Message.REQUESTOR_ROLE, Boolean.TRUE);
    message.setContent(SOAPMessage.class, saaj);

    PhaseInterceptorChain chain = new PhaseInterceptorChain(new PhaseManagerImpl().getOutPhases());
    chain.add(new SAAJOutInterceptor());
    chain.add(outInterceptor);
    message.setInterceptorChain(chain);

    assertTrue(chain.doIntercept(message), "Outbound chain must complete without abort");
    return saaj.getSOAPPart();
  }

  // --- Reference-collection helpers (from WssSecurityProcessorIntegrationTest) ---

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
}
