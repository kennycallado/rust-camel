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
import java.security.Provider;
import java.security.Security;
import java.util.Map;
import javax.xml.parsers.DocumentBuilderFactory;
import javax.xml.transform.dom.DOMSource;
import org.apache.cxf.binding.soap.Soap11;
import org.apache.cxf.binding.soap.SoapMessage;
import org.apache.cxf.interceptor.Fault;
import org.apache.cxf.message.Exchange;
import org.apache.cxf.message.ExchangeImpl;
import org.apache.cxf.message.MessageImpl;
import org.apache.cxf.ws.security.wss4j.WSS4JInInterceptor;
import org.apache.jcp.xml.dsig.internal.dom.XMLDSigRI;
import org.apache.wss4j.common.WSEncryptionPart;
import org.apache.wss4j.common.crypto.Crypto;
import org.apache.wss4j.common.crypto.CryptoFactory;
import org.apache.wss4j.common.ext.WSSecurityException;
import org.apache.wss4j.dom.WSConstants;
import org.apache.wss4j.dom.message.WSSecHeader;
import org.apache.wss4j.dom.message.WSSecSignature;
import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.parallel.ResourceLock;
import org.w3c.dom.Document;

/**
 * Producer response-verification path: the profile's {@link WSS4JInInterceptor} must carry the same
 * pinned {@link ProfileSignatureProcessor} and per-profile algorithm suite as the consumer path.
 * The integration test drives {@code handleMessage} on a CXF {@link SoapMessage} directly, without
 * a {@code PhaseInterceptorChain}, so a lost PROCESSOR_MAP entry fails here.
 */
@ResourceLock("java.security.Security")
class SecurityProfileInInterceptorTest {

  private static final String XMLDSIG_PROVIDER = "ApacheXMLDSig";
  private static final String RSA_SHA1 = "http://www.w3.org/2000/09/xmldsig#rsa-sha1";
  private static final String SHA1 = "http://www.w3.org/2000/09/xmldsig#sha1";
  private static final String EXCLUSIVE_C14N = "http://www.w3.org/2001/10/xml-exc-c14n#";

  private static Path keystorePath;

  @BeforeAll
  static void setUpKeystore() throws Exception {
    keystorePath = TestKeystoreHelper.createTestKeystore();
  }

  @AfterAll
  static void tearDownKeystore() throws Exception {
    if (keystorePath != null) {
      Files.deleteIfExists(keystorePath);
    }
  }

  @Test
  void inInterceptor_carriesProcessorMapEntryForSignature() {
    WSS4JInInterceptor interceptor =
        assertInstanceOf(WSS4JInInterceptor.class, verifyProfile().createInInterceptor());

    Object rawProcessorMap = interceptor.getProperties().get(WSS4JInInterceptor.PROCESSOR_MAP);
    Map<?, ?> processorMap =
        assertInstanceOf(
            Map.class,
            rawProcessorMap,
            "the producer in-interceptor must carry a PROCESSOR_MAP entry");
    assertInstanceOf(
        ProfileSignatureProcessor.class,
        processorMap.get(WSConstants.SIGNATURE),
        "the Signature action must be handled by the profile-scoped wrapper");
  }

  @Test
  void inInterceptor_nullWithoutVerificationMaterial() {
    SecurityProfile profile = SecurityProfile.builder("no-verification-material").build();

    assertNull(
        profile.createInInterceptor(),
        "without a truststore or keystore the producer in-interceptor stays null");
  }

  @Test
  void inInterceptorWrapper_enforcesSameAllowList() throws Exception {
    String legacySigned = legacySignedSoap();

    Fault rejection =
        assertThrows(
            Fault.class,
            () -> invokeInInterceptor(verifyProfile(), legacySigned),
            "the default allow-list must reject an rsa-sha1+sha1 response signature");

    WSSecurityException securityException = findCause(rejection, WSSecurityException.class);
    assertNotNull(
        securityException, "the fault must wrap a WSSecurityException: " + causeChain(rejection));
    assertEquals(
        WSSecurityException.ErrorCode.INVALID_SECURITY,
        securityException.getErrorCode(),
        "the rejection must come from the profile algorithm-suite validator");

    assertDoesNotThrow(
        () -> invokeInInterceptor(optedInVerifyProfile(), legacySigned),
        "the opted-in profile must verify the same legacy response signature");
  }

  @Test
  void inInterceptorWrapper_enforcesSameAllowList_withoutRegisteredProvider() throws Exception {
    String legacySigned = legacySignedSoap();

    Provider original = Security.getProvider(XMLDSIG_PROVIDER);
    int originalPosition = original == null ? -1 : providerPosition(XMLDSIG_PROVIDER);
    Security.removeProvider(XMLDSIG_PROVIDER);
    try {
      assertNull(
          Security.getProvider(XMLDSIG_PROVIDER),
          "The provider must be absent for the duration of the verification");
      assertDoesNotThrow(
          () -> invokeInInterceptor(optedInVerifyProfile(), legacySigned),
          "the opted-in producer in-interceptor must verify without a registered provider");
      assertNull(
          Security.getProvider(XMLDSIG_PROVIDER),
          "the pinned engine must not re-register the provider during verification");
    } finally {
      if (original != null) {
        Security.insertProviderAt(new XMLDSigRI(), originalPosition + 1);
      }
    }
  }

  /** Drives the profile's in-interceptor directly on a CXF SOAP message. */
  private static void invokeInInterceptor(SecurityProfile profile, String signedSoapXml)
      throws Exception {
    SOAPMessage saaj =
        MessageFactory.newInstance(SOAPConstants.SOAP_1_1_PROTOCOL)
            .createMessage(
                new MimeHeaders(),
                new ByteArrayInputStream(signedSoapXml.getBytes(StandardCharsets.UTF_8)));

    SoapMessage message = new SoapMessage(new MessageImpl());
    Exchange exchange = new ExchangeImpl();
    exchange.setInMessage(message);
    message.setExchange(exchange);
    message.setVersion(Soap11.getInstance());
    message.setContent(SOAPMessage.class, saaj);

    WSS4JInInterceptor interceptor =
        assertInstanceOf(WSS4JInInterceptor.class, profile.createInInterceptor());
    interceptor.handleMessage(message);
  }

  /** Verify-only profile on the secure default allow-lists. */
  private static SecurityProfile verifyProfile() {
    return SecurityProfile.builder("verify-default")
        .truststore(keystorePath.toString(), TestKeystoreHelper.KEYSTORE_PASSWORD)
        .actionsIn("Signature")
        .build();
  }

  /** Verify profile that explicitly opts in to rsa-sha1 and sha1. */
  private static SecurityProfile optedInVerifyProfile() {
    return SecurityProfile.builder("verify-opted-in")
        .truststore(keystorePath.toString(), TestKeystoreHelper.KEYSTORE_PASSWORD)
        .actionsIn("Signature")
        .allowedSignatureAlgorithms(RSA_SHA1)
        .allowedDigestAlgorithms(SHA1)
        .build();
  }

  // --- Legacy fixture ---

  private static String legacySignedSoap() throws Exception {
    Document doc = parseXml(plainSoap11());
    WSSecHeader secHeader = new WSSecHeader(doc);
    secHeader.insertSecurityHeader();

    Crypto crypto =
        CryptoFactory.getInstance(
            SecurityProfile.createCryptoProperties(
                keystorePath.toString(), TestKeystoreHelper.KEYSTORE_PASSWORD));

    WSSecSignature sign = new WSSecSignature(secHeader);
    sign.setUserInfo(TestKeystoreHelper.KEY_ALIAS, TestKeystoreHelper.KEYSTORE_PASSWORD);
    sign.setKeyIdentifierType(WSConstants.BST_DIRECT_REFERENCE);
    sign.setSignatureAlgorithm(RSA_SHA1);
    sign.setDigestAlgo(SHA1);
    sign.setSigCanonicalization(EXCLUSIVE_C14N);
    sign.getParts()
        .add(new WSEncryptionPart("Body", doc.getDocumentElement().getNamespaceURI(), "Content"));

    sign.prepare(crypto);
    sign.prependBSTElementToHeader();
    sign.computeSignature(sign.addReferencesToSign(sign.getParts()));

    return SoapEnvelopeHelper.sourceToString(new DOMSource(doc));
  }

  private static Document parseXml(String xml) throws Exception {
    DocumentBuilderFactory factory = DocumentBuilderFactory.newInstance();
    factory.setNamespaceAware(true);
    return factory
        .newDocumentBuilder()
        .parse(new ByteArrayInputStream(xml.getBytes(StandardCharsets.UTF_8)));
  }

  private static String plainSoap11() {
    return """
        <soapenv:Envelope xmlns:soapenv="http://schemas.xmlsoap.org/soap/envelope/">
          <soapenv:Header/>
          <soapenv:Body><test:Hello xmlns:test="http://test.example.com">World</test:Hello></soapenv:Body>
        </soapenv:Envelope>
        """;
  }

  private static int providerPosition(String name) {
    Provider[] providers = Security.getProviders();
    for (int i = 0; i < providers.length; i++) {
      if (name.equals(providers[i].getName())) return i;
    }
    return -1;
  }

  private static <T extends Throwable> T findCause(Throwable thrown, Class<T> type) {
    for (Throwable current = thrown; current != null; current = current.getCause()) {
      if (type.isInstance(current)) {
        return type.cast(current);
      }
    }
    return null;
  }

  private static String causeChain(Throwable thrown) {
    StringBuilder chain = new StringBuilder();
    for (Throwable current = thrown; current != null; current = current.getCause()) {
      if (chain.length() > 0) chain.append(" -> ");
      chain.append(current.getClass().getName()).append(": ").append(current.getMessage());
    }
    return chain.toString();
  }
}
