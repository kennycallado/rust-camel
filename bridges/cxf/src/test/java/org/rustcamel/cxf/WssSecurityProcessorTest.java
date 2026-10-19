package org.rustcamel.cxf;

import static org.junit.jupiter.api.Assertions.*;
import static org.mockito.ArgumentMatchers.any;
import static org.mockito.Mockito.mock;
import static org.mockito.Mockito.when;

import cxf_bridge.SignerIdentity;
import java.math.BigInteger;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.security.KeyStore;
import java.security.MessageDigest;
import java.security.cert.X509Certificate;
import java.util.ArrayList;
import java.util.List;
import javax.xml.parsers.DocumentBuilderFactory;
import org.apache.wss4j.common.ext.WSSecurityException;
import org.apache.wss4j.dom.WSConstants;
import org.apache.wss4j.dom.WSDataRef;
import org.apache.wss4j.dom.engine.WSSecurityEngine;
import org.apache.wss4j.dom.engine.WSSecurityEngineResult;
import org.apache.wss4j.dom.handler.RequestData;
import org.apache.wss4j.dom.handler.WSHandlerResult;
import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;
import org.w3c.dom.Document;
import org.w3c.dom.Element;

class WssSecurityProcessorTest {

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

  private WssSecurityProcessor withProfile(SecurityProfile profile) {
    return new WssSecurityProcessor(profile);
  }

  // --- canSignOutbound ---

  @Test
  void canSignOutbound_returnsFalse_whenKeystorePathIsNull() {
    SecurityProfile profile = SecurityProfile.builder("test").build();
    assertFalse(withProfile(profile).canSignOutbound());
  }

  @Test
  void canSignOutbound_returnsFalse_whenKeystorePathIsEmpty() {
    SecurityProfile profile = SecurityProfile.builder("test").keystore("", "pass").build();
    assertFalse(withProfile(profile).canSignOutbound());
  }

  @Test
  void canSignOutbound_returnsFalse_whenKeystorePathIsBlank() {
    SecurityProfile profile = SecurityProfile.builder("test").keystore("   ", "pass").build();
    assertFalse(withProfile(profile).canSignOutbound());
  }

  @Test
  void canSignOutbound_returnsTrue_whenKeystorePathIsSet() {
    SecurityProfile profile =
        SecurityProfile.builder("test").keystore("/path/to/keystore.jks", "pass").build();
    assertTrue(withProfile(profile).canSignOutbound());
  }

  // --- canVerifyInbound ---

  @Test
  void canVerifyInbound_returnsFalse_whenNeitherKeystoreNorTruststoreSet() {
    SecurityProfile profile = SecurityProfile.builder("test").build();
    assertFalse(withProfile(profile).canVerifyInbound());
  }

  @Test
  void canVerifyInbound_returnsTrue_whenTruststorePathIsSet() {
    SecurityProfile profile =
        SecurityProfile.builder("test").truststore("/path/to/truststore.jks", "pass").build();
    assertTrue(withProfile(profile).canVerifyInbound());
  }

  @Test
  void canVerifyInbound_returnsTrue_whenKeystorePathIsSet() {
    SecurityProfile profile =
        SecurityProfile.builder("test").keystore("/path/to/keystore.jks", "pass").build();
    assertTrue(withProfile(profile).canVerifyInbound());
  }

  // --- processOutbound / processInbound passthrough when disabled ---

  @Test
  void processOutbound_returnsInput_whenDisabled() throws Exception {
    SecurityProfile profile = SecurityProfile.builder("test").build();
    String input =
        "<soap:Envelope xmlns:soap=\"http://schemas.xmlsoap.org/soap/envelope/\"><soap:Body><test/></soap:Body></soap:Envelope>";
    assertEquals(input, withProfile(profile).processOutbound(input));
  }

  @Test
  void processInbound_returnsInput_whenDisabled() throws Exception {
    SecurityProfile profile = SecurityProfile.builder("test").build();
    String input =
        "<soap:Envelope xmlns:soap=\"http://schemas.xmlsoap.org/soap/envelope/\"><soap:Body><test/></soap:Body></soap:Envelope>";
    WssSecurityProcessor.VerifiedInboundMessage verified =
        withProfile(profile).processInbound(input);
    assertEquals(input, verified.xml());
    assertTrue(verified.signer().isEmpty(), "A disabled processor exports no signer");
  }

  @Test
  void processOutbound_returnsNull_whenInputIsNull() throws Exception {
    SecurityProfile profile = SecurityProfile.builder("test").build();
    assertNull(withProfile(profile).processOutbound(null));
  }

  @Test
  void processInbound_returnsNull_whenInputIsNull() throws Exception {
    SecurityProfile profile = SecurityProfile.builder("test").build();
    WssSecurityProcessor.VerifiedInboundMessage verified =
        withProfile(profile).processInbound(null);
    assertNull(verified.xml());
    assertTrue(verified.signer().isEmpty());
  }

  @Test
  void processOutbound_returnsBlank_whenInputIsBlank() throws Exception {
    SecurityProfile profile = SecurityProfile.builder("test").build();
    assertEquals("", withProfile(profile).processOutbound(""));
  }

  @Test
  void processInbound_returnsBlank_whenInputIsBlank() throws Exception {
    SecurityProfile profile = SecurityProfile.builder("test").build();
    WssSecurityProcessor.VerifiedInboundMessage verified = withProfile(profile).processInbound("");
    assertEquals("", verified.xml());
    assertTrue(verified.signer().isEmpty());
  }

  // --- Signer descriptor export (real WSS verification) ---

  @Test
  void signedDescriptorHasFourLowercaseFields() throws Exception {
    SecurityProfile profile =
        SecurityProfile.builder("test")
            .keystore(keystorePath.toString(), "changeit")
            .truststore(keystorePath.toString(), "changeit")
            .sigUser(TestKeystoreHelper.KEY_ALIAS, "changeit")
            .encUser(TestKeystoreHelper.KEY_ALIAS)
            .actionsOut("Signature")
            .actionsIn("Signature")
            .signatureAlgorithm("http://www.w3.org/2001/04/xmldsig-more#rsa-sha256")
            .signatureDigestAlgorithm("http://www.w3.org/2001/04/xmlenc#sha256")
            .build();

    String signed = new WssSecurityProcessor(profile).processOutbound(plainSoap11());
    WssSecurityProcessor.VerifiedInboundMessage verified =
        new WssSecurityProcessor(profile).processInbound(signed);

    assertTrue(verified.signer().isPresent(), "A verified Body signature must export a signer");
    SignerIdentity signer = verified.signer().get();
    X509Certificate cert = aliceCertificate();

    assertEquals(
        cert.getSubjectX500Principal().getName(javax.security.auth.x500.X500Principal.RFC2253),
        signer.getSubject(),
        "subject must be the RFC 2253 distinguished name");
    assertEquals(
        cert.getIssuerX500Principal().getName(javax.security.auth.x500.X500Principal.RFC2253),
        signer.getIssuer(),
        "issuer must be the RFC 2253 distinguished name");
    assertEquals(
        cert.getSerialNumber().toString(16),
        signer.getSerial(),
        "serial must be the lowercase-hex certificate serial");
    assertTrue(signer.getSerial().matches("[0-9a-f]+"), "serial must be lowercase hex");
    assertEquals(
        sha256Hex(cert.getEncoded()),
        signer.getFingerprintSha256(),
        "fingerprint must be the known SHA-256 of the DER certificate");
    assertTrue(
        signer.getFingerprintSha256().matches("[0-9a-f]{64}"),
        "fingerprint must be 64 lowercase hex characters");
  }

  // --- Body-coverage scoping of SIGN results ---

  @Test
  void distinctBodySignersRejected() throws Exception {
    X509Certificate certA = mockCertificate(derBytes("A"));
    X509Certificate certB = mockCertificate(derBytes("B"));

    List<WSSecurityEngineResult> distinctBodySigners =
        List.of(signResult(certA, bodyRef()), signResult(certB, bodyRef()));
    WssSecurityProcessor processor = processorReturning("Signature", distinctBodySigners);

    WSSecurityException ex =
        assertThrows(
            WSSecurityException.class,
            () -> processor.processInbound(soap11WithBodyId()),
            "Two distinct Body-covering certificates must be rejected as ambiguous");
    assertTrue(
        ex.getMessage().contains("ambiguous"),
        "The ambiguity failure must name the condition: " + ex.getMessage());

    // Budget contract: a 9th SIGN result must be rejected without inspecting its certificate
    // or references — results are never silently dropped. Table rows: eight Body-covering
    // duplicates of certA plus a distinct ninth certB, and eight header-only results plus one
    // Body-covering ninth. Both name the budget in the failure, not missing Body coverage.
    List<WSSecurityEngineResult> ninthDistinctBody = new ArrayList<>();
    for (int i = 0; i < 8; i++) {
      ninthDistinctBody.add(signResult(certA, bodyRef()));
    }
    ninthDistinctBody.add(signResult(certB, bodyRef()));

    List<WSSecurityEngineResult> ninthBodyAfterHeaderOnly = new ArrayList<>();
    for (int i = 0; i < 8; i++) {
      ninthBodyAfterHeaderOnly.add(signResult(certA, headerRef()));
    }
    ninthBodyAfterHeaderOnly.add(signResult(certB, bodyRef()));

    for (List<WSSecurityEngineResult> overBudget :
        List.of(ninthDistinctBody, ninthBodyAfterHeaderOnly)) {
      WssSecurityProcessor bounded = processorReturning("Signature", overBudget);
      WSSecurityException boundEx =
          assertThrows(
              WSSecurityException.class,
              () -> bounded.processInbound(soap11WithBodyId()),
              "A 9th SIGN result must be rejected, never silently dropped");
      assertTrue(
          boundEx.getMessage().contains("budget"),
          "The over-budget failure must name the budget: " + boundEx.getMessage());
    }

    // Budget preflight must run before ANY coverage or metadata pass reads references or
    // certificates. With a Timestamp+Signature profile the timestamp-coverage pass would
    // otherwise walk the 9th result's references first; the tripwire result fails the test if
    // any key other than the action tag is ever fetched from it.
    List<WSSecurityEngineResult> ninthTripwire = new ArrayList<>();
    ninthTripwire.add(new WSSecurityEngineResult(WSConstants.TS));
    for (int i = 0; i < 8; i++) {
      ninthTripwire.add(signResult(certA, bodyRef()));
    }
    ninthTripwire.add(tripwireSignResult(certB, bodyRef(), timestampRef()));
    WssSecurityProcessor timed = processorReturning("Timestamp Signature", ninthTripwire);
    WSSecurityException preflightEx =
        assertThrows(
            WSSecurityException.class,
            () -> timed.processInbound(soap11WithBodyAndTimestampIds()),
            "The budget must reject before timestamp coverage touches the 9th result");
    assertTrue(
        preflightEx.getMessage().contains("budget"),
        "The over-budget failure must name the budget: " + preflightEx.getMessage());
  }

  @Test
  void sameFingerprintIsOneSigner() throws Exception {
    // Two SIGN results whose certificates share one fingerprint count as one signer.
    X509Certificate first = mockCertificate(derBytes("A"));
    X509Certificate duplicate = mockCertificate(derBytes("A"));

    List<WSSecurityEngineResult> results =
        List.of(signResult(first, bodyRef()), signResult(duplicate, bodyRef()));
    WssSecurityProcessor processor = processorReturning("Signature", results);

    WssSecurityProcessor.VerifiedInboundMessage verified =
        processor.processInbound(soap11WithBodyId());

    assertTrue(verified.signer().isPresent(), "One shared fingerprint must be accepted");
    assertEquals(
        sha256Hex(derBytes("A")),
        verified.signer().get().getFingerprintSha256(),
        "The exported signer must carry the shared fingerprint");
  }

  @Test
  void mixedBodyAndNonBodySignaturesYieldBodySigner() throws Exception {
    X509Certificate bodyCert = mockCertificate(derBytes("A"));
    X509Certificate headerCert = mockCertificate(derBytes("B"));

    // The header-only result carries a non-Body protected element that masquerades as the
    // Body through its wsu:Id; it must not count toward Body coverage or ambiguity.
    List<WSSecurityEngineResult> results =
        List.of(signResult(headerCert, headerRef()), signResult(bodyCert, bodyRef()));
    WssSecurityProcessor processor = processorReturning("Signature", results);

    WssSecurityProcessor.VerifiedInboundMessage verified =
        processor.processInbound(soap11WithBodyId());

    assertTrue(verified.signer().isPresent(), "The Body-covering certificate yields the signer");
    assertEquals(
        sha256Hex(derBytes("A")),
        verified.signer().get().getFingerprintSha256(),
        "Only the Body-covering certificate may be exported");
  }

  @Test
  void signatureRequiredWithoutBodyCoverageRejected() throws Exception {
    X509Certificate cert = mockCertificate(derBytes("A"));
    List<WSSecurityEngineResult> results =
        List.of(new WSSecurityEngineResult(WSConstants.TS), signResult(cert, timestampRef()));
    WssSecurityProcessor processor = processorReturning("Timestamp Signature", results);

    WSSecurityException ex =
        assertThrows(
            WSSecurityException.class,
            () -> processor.processInbound(soap11WithBodyAndTimestampIds()),
            "A timestamp-only signature must be rejected when Signature is required");
    assertEquals(
        "Required Signature does not cover the SOAP Body",
        ex.getMessage(),
        "The Body-coverage failure must carry its distinct detail text");
  }

  @Test
  void unsignedProfileDispatchesWithoutSigner() throws Exception {
    // Profile without a Signature action: an unsigned request proceeds with no signer.
    List<WSSecurityEngineResult> results = List.of(new WSSecurityEngineResult(WSConstants.ENCR));
    WssSecurityProcessor processor = processorReturning("Encrypt", results);

    WssSecurityProcessor.VerifiedInboundMessage verified =
        processor.processInbound(soap11WithBodyId());

    assertTrue(
        verified.xml().contains("Hello") && verified.xml().contains("World"),
        "The unsigned request must dispatch with its Body intact");
    assertTrue(verified.signer().isEmpty(), "An unsigned request must export no signer");

    // Signer policy is scoped to Signature-required profiles: verified Body-signing results
    // under a non-Signature profile dispatch through normal WSS with no signer exported.
    X509Certificate certA = mockCertificate(derBytes("A"));
    X509Certificate certB = mockCertificate(derBytes("B"));
    List<WSSecurityEngineResult> twoBodySigners =
        List.of(
            new WSSecurityEngineResult(WSConstants.ENCR),
            signResult(certA, bodyRef()),
            signResult(certB, bodyRef()));
    WssSecurityProcessor noSignerPolicy = processorReturning("Encrypt", twoBodySigners);
    WssSecurityProcessor.VerifiedInboundMessage verifiedTwo =
        noSignerPolicy.processInbound(soap11WithBodyId());
    assertTrue(
        verifiedTwo.signer().isEmpty(),
        "A non-Signature profile must export no signer metadata even for Body signers");
    assertTrue(verifiedTwo.xml().contains("Hello"), "The request must still dispatch");

    // An excessive SIGN count is no policy trigger either: without a required Signature the
    // message stays on the normal WSS path and the 9th SIGN result is never inspected.
    List<WSSecurityEngineResult> overBudget = new ArrayList<>();
    overBudget.add(new WSSecurityEngineResult(WSConstants.ENCR));
    for (int i = 0; i < 8; i++) {
      overBudget.add(signResult(certA, bodyRef()));
    }
    overBudget.add(signResult(certB, bodyRef()));
    WssSecurityProcessor unbounded = processorReturning("Encrypt", overBudget);
    WssSecurityProcessor.VerifiedInboundMessage verifiedNine =
        unbounded.processInbound(soap11WithBodyId());
    assertTrue(
        verifiedNine.signer().isEmpty(),
        "No Signature requirement means no signer policy, budget or otherwise");
    assertTrue(verifiedNine.xml().contains("Hello"), "The request must still dispatch");
  }

  @Test
  void oversizedCertificateRejectedNotTruncated() throws Exception {
    byte[] oversizedDer = new byte[65537];
    X509Certificate oversized = mockCertificate(oversizedDer);
    List<WSSecurityEngineResult> results = List.of(signResult(oversized, bodyRef()));
    WssSecurityProcessor processor = processorReturning("Signature", results);

    WSSecurityException ex =
        assertThrows(
            WSSecurityException.class,
            () -> processor.processInbound(soap11WithBodyId()),
            "A certificate whose DER exceeds 64 KiB must be rejected, never truncated");
    assertTrue(
        ex.getMessage().contains("64 KiB"),
        "The oversize failure must name the cap: " + ex.getMessage());
  }

  // --- Fixtures ---

  /** Builds a processor whose engine returns the given verified results. */
  private static WssSecurityProcessor processorReturning(
      String actionsIn, List<WSSecurityEngineResult> results) throws Exception {
    SecurityProfile profile =
        SecurityProfile.builder("test")
            .keystore(keystorePath.toString(), "changeit")
            .truststore(keystorePath.toString(), "changeit")
            .sigUser(TestKeystoreHelper.KEY_ALIAS, "changeit")
            .encUser(TestKeystoreHelper.KEY_ALIAS)
            .actionsOut("Signature")
            .actionsIn(actionsIn)
            .build();
    WSSecurityEngine engine = mock(WSSecurityEngine.class);
    WSHandlerResult handlerResult = mock(WSHandlerResult.class);
    when(handlerResult.getResults()).thenReturn(results);
    when(engine.processSecurityHeader(any(org.w3c.dom.Document.class), any(RequestData.class)))
        .thenReturn(handlerResult);
    return new WssSecurityProcessor(profile, () -> engine);
  }

  /** One validated {@code SIGN} engine result over the given data references. */
  private static WSSecurityEngineResult signResult(X509Certificate cert, WSDataRef... refs) {
    WSSecurityEngineResult result = new WSSecurityEngineResult(WSConstants.SIGN, List.of(refs));
    result.put(WSSecurityEngineResult.TAG_X509_CERTIFICATE, cert);
    return result;
  }

  /**
   * One {@code SIGN} result whose map access allows only the action tag: fetching any reference or
   * certificate key trips an assertion. Pins that the SIGN budget preflight rejects the request
   * before any coverage or metadata pass inspects this result.
   */
  private static WSSecurityEngineResult tripwireSignResult(
      X509Certificate cert, WSDataRef... refs) {
    WSSecurityEngineResult result =
        new WSSecurityEngineResult(WSConstants.SIGN, List.of(refs)) {
          @Override
          public Object get(Object key) {
            if (!WSSecurityEngineResult.TAG_ACTION.equals(key)) {
              throw new AssertionError("SIGN result inspected before the budget preflight: " + key);
            }
            return super.get(key);
          }
        };
    result.put(WSSecurityEngineResult.TAG_X509_CERTIFICATE, cert);
    return result;
  }

  /** A reference that covers the Body by its wsu:Id (the id-target rule). */
  private static WSDataRef bodyRef() {
    WSDataRef ref = new WSDataRef();
    ref.setWsuId("Body-1");
    return ref;
  }

  /** A reference that covers only the Timestamp. */
  private static WSDataRef timestampRef() {
    WSDataRef ref = new WSDataRef();
    ref.setWsuId("Ts-1");
    return ref;
  }

  /**
   * A header-only reference that masquerades as Body coverage: a non-Body protected element whose
   * data reference carries the Body's wsu:Id. The known element must win, so this reference never
   * covers the Body.
   */
  private static WSDataRef headerRef() throws Exception {
    WSDataRef ref = new WSDataRef();
    Document detached = DocumentBuilderFactory.newInstance().newDocumentBuilder().newDocument();
    Element masquerade =
        detached.createElementNS("http://schemas.xmlsoap.org/soap/envelope/", "Body");
    ref.setProtectedElement(masquerade);
    ref.setWsuId("Body-1");
    return ref;
  }

  /** Distinct DER payloads whose digests act as distinct certificate fingerprints. */
  private static byte[] derBytes(String tag) {
    return ("unit-der-" + tag).getBytes(StandardCharsets.UTF_8);
  }

  /** A certificate stub whose DER encoding is the supplied bytes. */
  private static X509Certificate mockCertificate(byte[] der) throws Exception {
    X509Certificate cert = mock(X509Certificate.class);
    when(cert.getEncoded()).thenReturn(der);
    when(cert.getSubjectX500Principal())
        .thenReturn(new javax.security.auth.x500.X500Principal("CN=unit-subject"));
    when(cert.getIssuerX500Principal())
        .thenReturn(new javax.security.auth.x500.X500Principal("CN=unit-issuer"));
    when(cert.getSerialNumber()).thenReturn(BigInteger.valueOf(0xabcL));
    return cert;
  }

  /** The real self-signed certificate stored in the test keystore. */
  private static X509Certificate aliceCertificate() throws Exception {
    KeyStore keyStore = KeyStore.getInstance("JKS");
    try (var in = Files.newInputStream(keystorePath)) {
      keyStore.load(in, TestKeystoreHelper.KEYSTORE_PASSWORD.toCharArray());
    }
    return (X509Certificate) keyStore.getCertificate(TestKeystoreHelper.KEY_ALIAS);
  }

  private static String sha256Hex(byte[] der) throws Exception {
    byte[] digest = MessageDigest.getInstance("SHA-256").digest(der);
    StringBuilder hex = new StringBuilder(digest.length * 2);
    for (byte b : digest) {
      hex.append(Character.forDigit((b >> 4) & 0xf, 16)).append(Character.forDigit(b & 0xf, 16));
    }
    return hex.toString();
  }

  /** Plain SOAP 1.1 envelope used as the real signing input. */
  private static String plainSoap11() {
    return """
        <soapenv:Envelope xmlns:soapenv="http://schemas.xmlsoap.org/soap/envelope/">
          <soapenv:Header/>
          <soapenv:Body><test:Hello xmlns:test="http://test.example.com">World</test:Hello></soapenv:Body>
        </soapenv:Envelope>
        """;
  }

  /** SOAP 1.1 envelope whose Body carries a wsu:Id for reference matching. */
  private static String soap11WithBodyId() {
    return """
        <soapenv:Envelope xmlns:soapenv="http://schemas.xmlsoap.org/soap/envelope/" xmlns:wsu="http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-wssecurity-utility-1.0.xsd">
          <soapenv:Header/>
          <soapenv:Body wsu:Id="Body-1"><test:Hello xmlns:test="http://test.example.com">World</test:Hello></soapenv:Body>
        </soapenv:Envelope>
        """;
  }

  /** SOAP 1.1 envelope carrying both a wsu:Timestamp and a wsu:Id Body. */
  private static String soap11WithBodyAndTimestampIds() {
    return """
        <soapenv:Envelope xmlns:soapenv="http://schemas.xmlsoap.org/soap/envelope/" xmlns:wsu="http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-wssecurity-utility-1.0.xsd">
          <soapenv:Header>
            <wsu:Timestamp wsu:Id="Ts-1">
              <wsu:Created>2026-10-13T08:00:00Z</wsu:Created>
              <wsu:Expires>2126-10-13T08:05:00Z</wsu:Expires>
            </wsu:Timestamp>
          </soapenv:Header>
          <soapenv:Body wsu:Id="Body-1"><test:Hello xmlns:test="http://test.example.com">World</test:Hello></soapenv:Body>
        </soapenv:Envelope>
        """;
  }
}
