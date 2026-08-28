package org.rustcamel.cxf;

import static org.junit.jupiter.api.Assertions.*;

import jakarta.xml.soap.MessageFactory;
import jakarta.xml.soap.MimeHeaders;
import jakarta.xml.soap.SOAPConstants;
import jakarta.xml.soap.SOAPMessage;
import java.io.ByteArrayInputStream;
import java.io.ByteArrayOutputStream;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import javax.xml.transform.Transformer;
import javax.xml.transform.TransformerFactory;
import javax.xml.transform.dom.DOMSource;
import javax.xml.transform.stream.StreamResult;
import org.apache.cxf.binding.soap.Soap11;
import org.apache.cxf.binding.soap.SoapMessage;
import org.apache.cxf.binding.soap.saaj.SAAJInInterceptor;
import org.apache.cxf.binding.soap.saaj.SAAJOutInterceptor;
import org.apache.cxf.bus.managers.PhaseManagerImpl;
import org.apache.cxf.interceptor.Interceptor;
import org.apache.cxf.message.Exchange;
import org.apache.cxf.message.ExchangeImpl;
import org.apache.cxf.message.Message;
import org.apache.cxf.message.MessageImpl;
import org.apache.cxf.phase.PhaseInterceptorChain;
import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;
import org.w3c.dom.Document;
import org.w3c.dom.Element;
import org.w3c.dom.NodeList;

/**
 * Wire-level proof for the consumer in-interceptor path: the profile's {@code WSS4JInInterceptor}
 * runs inside a real {@code PhaseInterceptorChain} (with {@code SAAJInInterceptor}) over a SAAJ
 * message, and assertions target the processed OUTPUT document — never interceptor properties. Each
 * case produces a secured document on an outbound chain first, then feeds it back through the
 * matching profile's inbound chain.
 */
class SecurityProfileInWireTest {

  private static final String SOAP11_NS = "http://schemas.xmlsoap.org/soap/envelope/";
  private static final String TEST_NS = "http://test.example.com";

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
  void signedResponseVerifiesInbound() throws Exception {
    SecurityProfile signer =
        SecurityProfile.builder("in-signer")
            .keystore(keystorePath.toString(), TestKeystoreHelper.KEYSTORE_PASSWORD)
            .sigUser(TestKeystoreHelper.KEY_ALIAS, TestKeystoreHelper.KEYSTORE_PASSWORD)
            .actionsOut("Signature")
            .build();
    // The signer's own keystore doubles as the verifier's truststore: no dedicated truststore
    // fixture exists and the signer cert must be a trust anchor for verification anyway.
    SecurityProfile verifier =
        SecurityProfile.builder("in-verifier")
            .truststore(keystorePath.toString(), TestKeystoreHelper.KEYSTORE_PASSWORD)
            .actionsIn("Signature")
            .build();

    Document secured = runOutboundChain(signer);
    Document verified = runInboundChain(verifier, secured);

    Element hello = soleElement(verified, TEST_NS, "Hello");
    assertEquals("World", hello.getTextContent(), "Verified envelope must retain the body content");
  }

  @Test
  void encryptedResponseDecryptsInbound() throws Exception {
    SecurityProfile encryptor =
        SecurityProfile.builder("in-encryptor")
            .keystore(keystorePath.toString(), TestKeystoreHelper.KEYSTORE_PASSWORD)
            .encUser(TestKeystoreHelper.KEY_ALIAS)
            .actionsOut("Encrypt")
            .build();
    // Decryptor keystore must be the default same-password fixture: the inbound PW_CALLBACK_REF
    // supplies only the store password, so a split-password key would spuriously fail (fenced
    // non-goal — callback semantics are out of scope).
    SecurityProfile decryptor =
        SecurityProfile.builder("in-decryptor")
            .keystore(keystorePath.toString(), TestKeystoreHelper.KEYSTORE_PASSWORD)
            .actionsIn("Encrypt")
            .build();

    Document encrypted = runOutboundChain(encryptor);
    Document decrypted = runInboundChain(decryptor, encrypted);

    Element hello = soleElement(decrypted, TEST_NS, "Hello");
    assertEquals(
        "World",
        hello.getTextContent(),
        "Decrypted envelope must round-trip the original body content");
    assertEquals(
        0,
        decrypted
            .getElementsByTagNameNS("http://www.w3.org/2001/04/xmlenc#", "EncryptedData")
            .getLength(),
        "Decrypted envelope must contain no remaining xenc:EncryptedData elements");
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

  // --- Outbound chain scaffolding (from SecurityProfileWireTest) ---

  /**
   * Runs the profile's real out-interceptor chain in-process: a {@code PhaseInterceptorChain} with
   * {@code SAAJOutInterceptor} plus the profile's {@code WSS4JOutInterceptor} over a {@code
   * SoapMessage} backed by a parsed SAAJ document. A direct {@code handleMessage} call would not
   * secure anything — the interceptor only queues its POST_PROTOCOL ending into the live chain.
   *
   * @return the secured SAAJ SOAP document
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

  // --- Inbound chain scaffolding ---

  /**
   * Runs the profile's real in-interceptor chain in-process: the secured document is serialized
   * back to wire-form XML, re-parsed as a fresh SAAJ message, and pushed through a {@code
   * PhaseInterceptorChain} (in phases) with {@code SAAJInInterceptor} before the profile's {@code
   * WSS4JInInterceptor}. A failed verification or decryption aborts the chain, so a completed chain
   * is itself the pass signal.
   *
   * @return the processed SAAJ SOAP document
   */
  private static Document runInboundChain(SecurityProfile profile, Document secured)
      throws Exception {
    Interceptor<? extends Message> inInterceptor = profile.createInInterceptor();
    assertNotNull(inInterceptor, "Security-enabled profile must produce an in-interceptor");

    SOAPMessage saaj =
        MessageFactory.newInstance(SOAPConstants.SOAP_1_1_PROTOCOL)
            .createMessage(
                new MimeHeaders(),
                new ByteArrayInputStream(toWireXml(secured).getBytes(StandardCharsets.UTF_8)));

    SoapMessage message = new SoapMessage(new MessageImpl());
    Exchange exchange = new ExchangeImpl();
    exchange.setInMessage(message);
    message.setExchange(exchange);
    message.setVersion(Soap11.getInstance());
    message.setContent(SOAPMessage.class, saaj);

    PhaseInterceptorChain chain = new PhaseInterceptorChain(new PhaseManagerImpl().getInPhases());
    chain.add(new SAAJInInterceptor());
    chain.add(inInterceptor);
    message.setInterceptorChain(chain);

    assertTrue(chain.doIntercept(message), "Inbound chain must complete without abort");
    return saaj.getSOAPPart();
  }

  /** Serializes the secured document back to wire-form XML for the inbound leg. */
  private static String toWireXml(Document doc) throws Exception {
    Transformer transformer = TransformerFactory.newInstance().newTransformer();
    ByteArrayOutputStream out = new ByteArrayOutputStream();
    transformer.transform(new DOMSource(doc), new StreamResult(out));
    return out.toString(StandardCharsets.UTF_8);
  }

  private static Element soleElement(Document doc, String ns, String localName) {
    NodeList nodes = doc.getElementsByTagNameNS(ns, localName);
    assertEquals(1, nodes.getLength(), "Expected exactly one <" + localName + "> element");
    return (Element) nodes.item(0);
  }
}
