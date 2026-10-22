package org.rustcamel.cxf;

import static org.junit.jupiter.api.Assertions.*;

import java.io.ByteArrayInputStream;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.time.Instant;
import java.util.List;
import javax.xml.parsers.DocumentBuilderFactory;
import javax.xml.transform.dom.DOMSource;
import org.apache.wss4j.common.WSEncryptionPart;
import org.apache.wss4j.common.ext.WSSecurityException;
import org.apache.wss4j.dom.WSConstants;
import org.apache.wss4j.dom.message.WSSecHeader;
import org.apache.wss4j.dom.message.WSSecSignature;
import org.apache.wss4j.dom.message.WSSecTimestamp;
import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;
import org.w3c.dom.Document;

/**
 * Consumer-path tests for the per-profile WS-Security Timestamp freshness window (ADR-0088). Signed
 * envelopes carry an explicit {@code Created} so the default 300 s TTL / 60 s future tolerance and
 * the profile-raised values can be told apart on the same wire shape. Every default-versus-raised
 * comparison uses a new processor so the replay cache never couples the two calls.
 */
class WssSecurityProcessorTimestampWindowTest {

  private static final String RSA_SHA256 = "http://www.w3.org/2001/04/xmldsig-more#rsa-sha256";
  private static final String SHA256 = "http://www.w3.org/2001/04/xmlenc#sha256";

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
  void oldCreated_rejectedByDefault_acceptedWithRaisedTtl() throws Exception {
    String xml = signedEnvelope(Instant.now().minusSeconds(600), 3600);

    WSSecurityException ex =
        assertThrows(
            WSSecurityException.class,
            () -> new WssSecurityProcessor(profile(null, null)).processInbound(xml),
            "The default 300 s TTL must reject a 600 s old Created");
    assertTrue(ex.getMessage().contains("out of range"), ex.getMessage());

    assertDoesNotThrow(
        () -> new WssSecurityProcessor(profile(900, null)).processInbound(xml),
        "A 900 s TTL must accept a 600 s old Created");
  }

  @Test
  void futureCreated_rejectedByDefault_acceptedWithRaisedTolerance() throws Exception {
    String xml = signedEnvelope(Instant.now().plusSeconds(120), 600);

    WSSecurityException ex =
        assertThrows(
            WSSecurityException.class,
            () -> new WssSecurityProcessor(profile(null, null)).processInbound(xml),
            "The default 60 s future tolerance must reject a Created 120 s ahead");
    assertTrue(ex.getMessage().contains("out of range"), ex.getMessage());

    assertDoesNotThrow(
        () -> new WssSecurityProcessor(profile(null, 300)).processInbound(xml),
        "A 300 s future tolerance must accept a Created 120 s ahead");
  }

  @Test
  void fixtureWithMultiYearWindow_accepted() throws Exception {
    Instant created = Instant.now().minusSeconds(9L * 365 * 24 * 3600);
    String xml = signedEnvelope(created, 315_360_000);

    assertDoesNotThrow(
        () -> new WssSecurityProcessor(profile(315_360_000, null)).processInbound(xml),
        "A ten-year TTL must accept a fixture Created nine years in the past");
  }

  @Test
  void expiredExpires_stillRejected_withRaisedTtl() throws Exception {
    String xml = signedEnvelope(Instant.now().minusSeconds(600), 300);

    WSSecurityException ex =
        assertThrows(
            WSSecurityException.class,
            () -> new WssSecurityProcessor(profile(900, null)).processInbound(xml),
            "A raised TTL must not resurrect an already expired Expires");
    assertTrue(ex.getMessage().contains("expired"), ex.getMessage());
  }

  @Test
  void replay_stillRejected_withRaisedTtl() throws Exception {
    String xml = signedEnvelope(Instant.now().minusSeconds(600), 3600);
    WssSecurityProcessor processor = new WssSecurityProcessor(profile(900, null));

    assertDoesNotThrow(() -> processor.processInbound(xml), "The first delivery must be accepted");

    WSSecurityException ex =
        assertThrows(
            WSSecurityException.class,
            () -> processor.processInbound(xml),
            "The identical second delivery must be rejected as a replay");
    assertTrue(ex.getMessage().contains("replay"), ex.getMessage());
  }

  // --- Helpers ---

  private static SecurityProfile profile(Integer ttl, Integer future) {
    return SecurityProfile.builder("window")
        .keystore(keystorePath.toString(), "changeit")
        .truststore(keystorePath.toString(), "changeit")
        .sigUser("alice", "changeit")
        .actionsIn("Timestamp Signature")
        .timestampTtlSeconds(ttl)
        .timestampFutureToleranceSeconds(future)
        .build();
  }

  private static String signedEnvelope(Instant created, int expiresAfterCreatedSeconds)
      throws Exception {
    DocumentBuilderFactory dbf =
        SoapEnvelopeHelper.configureSecure(DocumentBuilderFactory.newInstance());
    Document doc =
        dbf.newDocumentBuilder()
            .parse(new ByteArrayInputStream(soap11Envelope().getBytes(StandardCharsets.UTF_8)));

    WSSecHeader secHeader = new WSSecHeader(doc);
    secHeader.insertSecurityHeader();

    WSSecTimestamp timestamp = new WSSecTimestamp(secHeader);
    timestamp.setWsTimeSource(() -> created);
    timestamp.setTimeToLive(expiresAfterCreatedSeconds);
    timestamp.build();

    WSSecSignature sign = new WSSecSignature(secHeader);
    sign.setUserInfo("alice", "changeit");
    sign.setKeyIdentifierType(WSConstants.BST_DIRECT_REFERENCE);
    List<WSEncryptionPart> parts = sign.getParts();
    parts.add(new WSEncryptionPart("Body", doc.getDocumentElement().getNamespaceURI(), "Content"));
    parts.add(new WSEncryptionPart("Timestamp", WSConstants.WSU_NS, ""));
    sign.setSignatureAlgorithm(RSA_SHA256);
    sign.setDigestAlgo(SHA256);
    sign.build(profile(null, null).getSignatureCrypto());

    return SoapEnvelopeHelper.sourceToString(new DOMSource(doc));
  }

  /** SOAP 1.1 envelope used by {@code WssSecurityProcessorTimestampCoverageTest}. */
  private static String soap11Envelope() {
    return """
        <soapenv:Envelope xmlns:soapenv="http://schemas.xmlsoap.org/soap/envelope/">
          <soapenv:Header/>
          <soapenv:Body><test:Hello xmlns:test="http://test.example.com">World</test:Hello></soapenv:Body>
        </soapenv:Envelope>
        """;
  }
}
