package org.rustcamel.cxf;

import static org.junit.jupiter.api.Assertions.*;

import java.io.ByteArrayInputStream;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.security.KeyStore;
import java.security.MessageDigest;
import java.security.Provider;
import java.security.Security;
import java.security.cert.X509Certificate;
import java.util.ArrayList;
import java.util.List;
import javax.crypto.KeyGenerator;
import javax.crypto.SecretKey;
import javax.xml.parsers.DocumentBuilderFactory;
import javax.xml.transform.dom.DOMSource;
import org.apache.jcp.xml.dsig.internal.dom.XMLDSigRI;
import org.apache.wss4j.common.WSEncryptionPart;
import org.apache.wss4j.common.crypto.Crypto;
import org.apache.wss4j.common.crypto.CryptoFactory;
import org.apache.wss4j.common.ext.WSSecurityException;
import org.apache.wss4j.dom.WSConstants;
import org.apache.wss4j.dom.engine.WSSConfig;
import org.apache.wss4j.dom.engine.WSSecurityEngine;
import org.apache.wss4j.dom.engine.WSSecurityEngineResult;
import org.apache.wss4j.dom.handler.RequestData;
import org.apache.wss4j.dom.handler.WSHandlerResult;
import org.apache.wss4j.dom.message.WSSecEncrypt;
import org.apache.wss4j.dom.message.WSSecHeader;
import org.apache.wss4j.dom.message.WSSecSignature;
import org.apache.wss4j.dom.message.WSSecTimestamp;
import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.parallel.ResourceLock;
import org.w3c.dom.Document;
import org.w3c.dom.Element;
import org.w3c.dom.Node;
import org.w3c.dom.NodeList;

/**
 * Inbound SHA-1 behaviour of the production consumer path. The production constructor pins WSS4J's
 * signature processor to Santuario's {@link XMLDSigRI} and scopes verification to the profile's
 * inbound algorithm suite, so the default profile rejects legacy algorithms while an explicit
 * ALLOWED_* opt-in accepts them — including when the provider is absent from {@code
 * java.security.Security} as in a GraalVM native image.
 */
@ResourceLock("java.security.Security")
class WssSecurityProcessorLegacySha1Test {

  private static final String RSA_SHA1 = "http://www.w3.org/2000/09/xmldsig#rsa-sha1";
  private static final String SHA1 = "http://www.w3.org/2000/09/xmldsig#sha1";
  private static final String RSA_SHA256 = "http://www.w3.org/2001/04/xmldsig-more#rsa-sha256";
  private static final String SHA256 = "http://www.w3.org/2001/04/xmlenc#sha256";
  private static final String EXCLUSIVE_C14N = "http://www.w3.org/2001/10/xml-exc-c14n#";
  private static final String ECDSA_SHA256 = "http://www.w3.org/2001/04/xmldsig-more#ecdsa-sha256";
  private static final String XMLDSIG_PROVIDER = "ApacheXMLDSig";
  private static final String DS_NS = "http://www.w3.org/2000/09/xmldsig#";

  private static Path keystorePath;
  private static Path ecKeystorePath;

  @BeforeAll
  static void setUpKeystore() throws Exception {
    keystorePath = TestKeystoreHelper.createTestKeystore();
    ecKeystorePath = TestKeystoreHelper.createTestEcKeystore();
  }

  @AfterAll
  static void tearDownKeystore() throws Exception {
    if (keystorePath != null) {
      Files.deleteIfExists(keystorePath);
    }
    if (ecKeystorePath != null) {
      Files.deleteIfExists(ecKeystorePath);
    }
  }

  @Test
  void inboundRejectsRsaSha1ByDefault() throws Exception {
    String signed = new WssSecurityProcessor(legacySigningProfile()).processOutbound(plainSoap11());

    WSSecurityException ex =
        assertThrows(
            WSSecurityException.class,
            () -> new WssSecurityProcessor(verifyProfile()).processInbound(signed),
            "The default inbound allow-list must reject an rsa-sha1+sha1 signature");

    assertEquals(
        WSSecurityException.ErrorCode.INVALID_SECURITY,
        ex.getErrorCode(),
        "The rejection must come from the profile algorithm-suite validator");
  }

  @Test
  void inboundAcceptsRsaSha1WhenOptedIn() throws Exception {
    String signed = new WssSecurityProcessor(legacySigningProfile()).processOutbound(plainSoap11());

    WssSecurityProcessor.VerifiedInboundMessage verified =
        new WssSecurityProcessor(optedInVerifyProfile()).processInbound(signed);

    assertNotNull(verified, "An opted-in legacy signature must verify");
    assertTrue(
        verified.signer().isPresent(),
        "The verified legacy signature must export the Body-covering signer");
  }

  @Test
  void inboundModernUnchanged() throws Exception {
    String signed = new WssSecurityProcessor(modernSigningProfile()).processOutbound(plainSoap11());

    WssSecurityProcessor.VerifiedInboundMessage verified =
        new WssSecurityProcessor(verifyProfile()).processInbound(signed);

    assertNotNull(verified, "The default profile must keep accepting rsa-sha256+sha256");
    assertTrue(verified.signer().isPresent(), "The modern signature must export its signer");
  }

  @Test
  void inboundSha1WithoutRegisteredProvider() throws Exception {
    // Signing first initializes WSSConfig, so its one-time provider registration cannot fire
    // inside the verification call below and re-register the provider under test.
    String signed = new WssSecurityProcessor(legacySigningProfile()).processOutbound(plainSoap11());

    Provider original = Security.getProvider(XMLDSIG_PROVIDER);
    int originalPosition = original == null ? -1 : providerPosition(XMLDSIG_PROVIDER);
    Security.removeProvider(XMLDSIG_PROVIDER);
    try {
      assertNull(
          Security.getProvider(XMLDSIG_PROVIDER),
          "The provider must be absent for the duration of the verification");
      WssSecurityProcessor verifier = new WssSecurityProcessor(optedInVerifyProfile());
      assertNull(
          Security.getProvider(XMLDSIG_PROVIDER),
          "The provider must still be absent when the inbound call starts");

      WssSecurityProcessor.VerifiedInboundMessage verified = verifier.processInbound(signed);

      assertNull(
          Security.getProvider(XMLDSIG_PROVIDER),
          "The pinned engine must verify without re-registering the provider");
      assertTrue(
          verified.signer().isPresent(),
          "The pinned Santuario processor must verify SHA-1 with no registered provider");
    } finally {
      if (original != null) {
        Security.insertProviderAt(new XMLDSigRI(), originalPosition + 1);
      }
    }
  }

  @Test
  void consumerSignerIdentity_preservedForLegacyOptIn() throws Exception {
    String signed = new WssSecurityProcessor(legacySigningProfile()).processOutbound(plainSoap11());

    WssSecurityProcessor.VerifiedInboundMessage verified =
        new WssSecurityProcessor(optedInVerifyProfile()).processInbound(signed);

    assertTrue(
        verified.signer().isPresent(),
        "The opted-in legacy signature must still export the consumer signer identity");
    assertEquals(
        sha256Hex(aliceCertificate().getEncoded()),
        verified.signer().get().getFingerprintSha256(),
        "The exported signer must carry alice's certificate fingerprint");
  }

  @Test
  void inboundThreeReferencePartnerShape_verifiesWhenOptedIn() throws Exception {
    // wse-php partner shape: one signature over BinarySecurityToken + Timestamp + Body, all
    // three references carrying the SHA-1 digest.
    String legacyMessage =
        signMessage(
            plainSoap11(), keystorePath, TestKeystoreHelper.KEY_ALIAS, RSA_SHA1, SHA1, true);

    Document fixture = parseXml(legacyMessage);
    assertEquals(3, referenceCount(fixture), "the fixture must carry exactly three references");
    assertEquals(
        "http://www.w3.org/2000/09/xmldsig#sha1",
        digestAlgorithmOf(fixture, "Timestamp"),
        "the Timestamp reference must carry the sha1 digest");
    assertEquals(
        "http://www.w3.org/2000/09/xmldsig#sha1",
        digestAlgorithmOf(fixture, "Body"),
        "the Body reference must carry the sha1 digest");
    assertEquals(
        "http://www.w3.org/2000/09/xmldsig#sha1",
        digestAlgorithmOf(fixture, "BinarySecurityToken"),
        "the BinarySecurityToken reference must carry the sha1 digest");
    assertEquals(
        RSA_SHA1, algorithmOf(fixture, "SignatureMethod"), "the signature method must be rsa-sha1");
    assertEquals(
        EXCLUSIVE_C14N,
        algorithmOf(fixture, "CanonicalizationMethod"),
        "the partner shape uses exclusive c14n");

    SecurityProfile profile = optedInVerifyProfile();
    List<WSSecurityEngineResult> results = processCapturing(profile, legacyMessage);
    assertEquals(
        3,
        dataReferenceCount(results),
        "the verified signature must cover all three partner-shape references");

    WssSecurityProcessor.VerifiedInboundMessage verified =
        new WssSecurityProcessor(profile).processInbound(legacyMessage);

    assertNotNull(verified, "the three-reference legacy message must verify when opted in");
    assertTrue(
        verified.signer().isPresent(),
        "the partner-shape signature must still export its Body-covering signer");
  }

  @Test
  void inboundThreeReferenceTimestampCoverageNotRequired_whenTimestampNotInActions()
      throws Exception {
    String modernMessage =
        signMessage(
            plainSoap11(), keystorePath, TestKeystoreHelper.KEY_ALIAS, RSA_SHA256, SHA256, true);

    Document fixture = parseXml(modernMessage);
    assertEquals(3, referenceCount(fixture), "the fixture must carry exactly three references");
    assertEquals(
        1, timestampCount(fixture), "the message must carry the signed wsu:Timestamp element");

    // In-actions are "Signature" only: the timestamp-coverage check must not run even though the
    // message carries a signed Timestamp.
    WssSecurityProcessor.VerifiedInboundMessage verified =
        new WssSecurityProcessor(verifyProfile()).processInbound(modernMessage);

    assertNotNull(verified, "a signed Timestamp outside actions.in must not be required");
    assertTrue(verified.signer().isPresent(), "the modern signature must export its signer");
  }

  @Test
  void inboundEcdsaSha256_rejectedByDefault_thenAcceptedWhenAllowListed() throws Exception {
    String ecdsaMessage =
        signMessage(
            plainSoap11(),
            ecKeystorePath,
            TestKeystoreHelper.EC_KEY_ALIAS,
            ECDSA_SHA256,
            SHA256,
            false);

    WSSecurityException ex =
        assertThrows(
            WSSecurityException.class,
            () -> new WssSecurityProcessor(ecVerifyProfile()).processInbound(ecdsaMessage),
            "ecdsa-sha256 is outside the default inbound signature allow-list");
    assertEquals(
        WSSecurityException.ErrorCode.INVALID_SECURITY,
        ex.getErrorCode(),
        "the rejection must come from the profile algorithm-suite validator");

    WssSecurityProcessor.VerifiedInboundMessage verified =
        new WssSecurityProcessor(ecAllowedVerifyProfile()).processInbound(ecdsaMessage);

    assertNotNull(verified, "an allow-listed ecdsa-sha256 signature must verify");
    assertTrue(verified.signer().isPresent(), "the EC signature must export its signer");
    assertEquals(
        sha256Hex(ecCertificate().getEncoded()),
        verified.signer().get().getFingerprintSha256(),
        "the exported signer must carry the EC certificate fingerprint");
  }

  @Test
  void inboundSignedAndEncrypted_bothHeaderOrders_defaultProfileUnchanged() throws Exception {
    SecurityProfile profile = verifyAndDecryptProfile();

    String signedThenEncrypted = signThenEncrypt(plainSoap11(), RSA_SHA256, SHA256);
    List<String> signedThenEncryptedHeader = securityHeaderChildNames(signedThenEncrypted);
    assertTrue(
        signedThenEncryptedHeader.indexOf("EncryptedKey")
            < signedThenEncryptedHeader.indexOf("Signature"),
        "sign-then-encrypt prepends the EncryptedKey: " + signedThenEncryptedHeader);

    WssSecurityProcessor.VerifiedInboundMessage first =
        new WssSecurityProcessor(profile).processInbound(signedThenEncrypted);
    assertTrue(first.signer().isPresent(), "EncryptedKey-before-Signature must verify and sign");
    assertTrue(
        first.xml().contains("World"), "EncryptedKey-before-Signature must decrypt the Body");

    String encryptedThenSigned = encryptThenSign(plainSoap11(), RSA_SHA256, SHA256);
    List<String> encryptedThenSignedHeader = securityHeaderChildNames(encryptedThenSigned);
    assertTrue(
        encryptedThenSignedHeader.indexOf("Signature")
            < encryptedThenSignedHeader.indexOf("EncryptedKey"),
        "encrypt-then-sign prepends the Signature: " + encryptedThenSignedHeader);

    WssSecurityProcessor.VerifiedInboundMessage second =
        new WssSecurityProcessor(profile).processInbound(encryptedThenSigned);
    assertTrue(second.signer().isPresent(), "Signature-before-EncryptedKey must verify and sign");
    assertTrue(
        second.xml().contains("World"), "Signature-before-EncryptedKey must decrypt the Body");
  }

  @Test
  void encryptOnlyConsumer_optedIn_acceptsSignedEncryptedLegacy() throws Exception {
    String signed =
        signMessage(
            plainSoap11(), keystorePath, TestKeystoreHelper.KEY_ALIAS, RSA_SHA1, SHA1, true);
    String signedAndEncrypted = encryptDocument(signed);

    SecurityProfile profile =
        SecurityProfile.builder("encrypt-only-legacy")
            .keystore(keystorePath.toString(), TestKeystoreHelper.KEYSTORE_PASSWORD)
            .truststore(keystorePath.toString(), TestKeystoreHelper.KEYSTORE_PASSWORD)
            .encUser(TestKeystoreHelper.KEY_ALIAS)
            .actionsIn("Encrypt")
            .allowedSignatureAlgorithms(RSA_SHA1)
            .allowedDigestAlgorithms(SHA1)
            .build();

    WssSecurityProcessor.VerifiedInboundMessage verified =
        new WssSecurityProcessor(profile).processInbound(signedAndEncrypted);

    assertNotNull(verified, "an Encrypt-only consumer must still verify the legacy signature");
    assertTrue(verified.xml().contains("World"), "the legacy message must decrypt to its Body");
    assertTrue(
        verified.signer().isEmpty(), "Signature is not among actions.in, so no signer is exported");
  }

  // --- Fixtures ---

  /**
   * Legacy signing profile: rsa-sha1 + sha1 + exclusive c14n, configured via the SIGNATURE_* knobs.
   */
  private static SecurityProfile legacySigningProfile() {
    return signingProfile("legacy-signing", RSA_SHA1, SHA1);
  }

  private static SecurityProfile modernSigningProfile() {
    return signingProfile("modern-signing", RSA_SHA256, SHA256);
  }

  private static SecurityProfile signingProfile(
      String name, String signatureAlgorithm, String digestAlgorithm) {
    return SecurityProfile.builder(name)
        .keystore(keystorePath.toString(), TestKeystoreHelper.KEYSTORE_PASSWORD)
        .sigUser(TestKeystoreHelper.KEY_ALIAS, TestKeystoreHelper.KEYSTORE_PASSWORD)
        .actionsOut("Signature")
        .signatureAlgorithm(signatureAlgorithm)
        .signatureDigestAlgorithm(digestAlgorithm)
        .signatureC14nAlgorithm(EXCLUSIVE_C14N)
        .build();
  }

  /** Verify-only profile on the secure default allow-lists. */
  private static SecurityProfile verifyProfile() {
    return SecurityProfile.builder("verify-default")
        .keystore(keystorePath.toString(), TestKeystoreHelper.KEYSTORE_PASSWORD)
        .truststore(keystorePath.toString(), TestKeystoreHelper.KEYSTORE_PASSWORD)
        .actionsIn("Signature")
        .build();
  }

  /** Verify profile that explicitly opts in to rsa-sha1 and sha1. */
  private static SecurityProfile optedInVerifyProfile() {
    return SecurityProfile.builder("verify-opted-in")
        .keystore(keystorePath.toString(), TestKeystoreHelper.KEYSTORE_PASSWORD)
        .truststore(keystorePath.toString(), TestKeystoreHelper.KEYSTORE_PASSWORD)
        .actionsIn("Signature")
        .allowedSignatureAlgorithms(RSA_SHA1)
        .allowedDigestAlgorithms(SHA1)
        .build();
  }

  /** Verify-only profile over the EC test keystore, on the secure default allow-lists. */
  private static SecurityProfile ecVerifyProfile() {
    return SecurityProfile.builder("verify-ec-default")
        .keystore(ecKeystorePath.toString(), TestKeystoreHelper.KEYSTORE_PASSWORD)
        .truststore(ecKeystorePath.toString(), TestKeystoreHelper.KEYSTORE_PASSWORD)
        .actionsIn("Signature")
        .build();
  }

  /** EC verify profile that opts in to the ecdsa-sha256 signature method. */
  private static SecurityProfile ecAllowedVerifyProfile() {
    return SecurityProfile.builder("verify-ec-allowed")
        .keystore(ecKeystorePath.toString(), TestKeystoreHelper.KEYSTORE_PASSWORD)
        .truststore(ecKeystorePath.toString(), TestKeystoreHelper.KEYSTORE_PASSWORD)
        .actionsIn("Signature")
        .allowedSignatureAlgorithms(ECDSA_SHA256)
        .build();
  }

  /** Default-algorithm profile that requires and accepts both a signature and encryption. */
  private static SecurityProfile verifyAndDecryptProfile() {
    return SecurityProfile.builder("verify-decrypt-default")
        .keystore(keystorePath.toString(), TestKeystoreHelper.KEYSTORE_PASSWORD)
        .truststore(keystorePath.toString(), TestKeystoreHelper.KEYSTORE_PASSWORD)
        .encUser(TestKeystoreHelper.KEY_ALIAS)
        .actionsIn("Signature Encrypt")
        .build();
  }

  /**
   * Runs the production inbound path through an engine that records the {@link WSHandlerResult}, so
   * the test can assert the verified reference count without duplicating the coverage logic.
   */
  private static List<WSSecurityEngineResult> processCapturing(
      SecurityProfile profile, String message) throws Exception {
    CapturingEngine engine = new CapturingEngine();
    WSSConfig config = WSSConfig.getNewInstance();
    config.setProcessor(
        WSConstants.SIGNATURE, new ProfileSignatureProcessor(profile.inboundAlgorithmSuite()));
    engine.setWssConfig(config);

    new WssSecurityProcessor(profile, () -> engine).processInbound(message);
    assertNotNull(engine.captured, "the engine must have processed the security header");
    return engine.captured.getResults();
  }

  /** WSS4J engine that records the handler result of the production processing call. */
  private static final class CapturingEngine extends WSSecurityEngine {
    private WSHandlerResult captured;

    @Override
    public WSHandlerResult processSecurityHeader(Document doc, RequestData requestData)
        throws WSSecurityException {
      captured = super.processSecurityHeader(doc, requestData);
      return captured;
    }
  }

  /** Zero-based position of a registered provider, or -1 when absent. */
  private static int providerPosition(String name) {
    Provider[] providers = Security.getProviders();
    for (int i = 0; i < providers.length; i++) {
      if (name.equals(providers[i].getName())) return i;
    }
    return -1;
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

  /** The real EC certificate stored in the EC test keystore. */
  private static X509Certificate ecCertificate() throws Exception {
    KeyStore keyStore = KeyStore.getInstance("JKS");
    try (var in = Files.newInputStream(ecKeystorePath)) {
      keyStore.load(in, TestKeystoreHelper.KEYSTORE_PASSWORD.toCharArray());
    }
    return (X509Certificate) keyStore.getCertificate(TestKeystoreHelper.EC_KEY_ALIAS);
  }

  // --- Direct WSS4J fixture drivers ---
  //
  // These mirror the consumer path's WSSecHeader/WSSecTimestamp/WSSecSignature call pattern so the
  // tests exercise real WSS4J wire messages instead of production's own output.
  //
  // WSSecSignature materializes the BST only after prepare(); a BinarySecurityToken part cannot be
  // resolved before the element is in the security header, so the driver prepends the BST between
  // prepare() and addReferencesToSign().

  private static Crypto cryptoFor(Path storePath) throws Exception {
    return CryptoFactory.getInstance(
        SecurityProfile.createCryptoProperties(
            storePath.toString(), TestKeystoreHelper.KEYSTORE_PASSWORD));
  }

  private static String signMessage(
      String soapXml,
      Path storePath,
      String alias,
      String signatureAlgorithm,
      String digestAlgorithm,
      boolean threeReferences)
      throws Exception {
    Document doc = parseXml(soapXml);
    WSSecHeader secHeader = new WSSecHeader(doc);
    secHeader.insertSecurityHeader();

    if (threeReferences) {
      new WSSecTimestamp(secHeader).build();
    }

    Crypto crypto = cryptoFor(storePath);
    List<WSEncryptionPart> parts = new ArrayList<>();
    parts.add(new WSEncryptionPart("Body", doc.getDocumentElement().getNamespaceURI(), "Content"));
    if (threeReferences) {
      parts.add(new WSEncryptionPart("Timestamp", WSConstants.WSU_NS, ""));
      parts.add(new WSEncryptionPart("BinarySecurityToken", WSConstants.WSSE_NS, ""));
    }

    signParts(secHeader, crypto, alias, signatureAlgorithm, digestAlgorithm, parts);

    return serialize(doc);
  }

  /**
   * Shared WSSecSignature recipe: BST direct reference, exclusive c14n, and the given parts. The
   * BST is materialized between prepare() and addReferencesToSign() because a BinarySecurityToken
   * part cannot be resolved before the element is in the header.
   */
  private static void signParts(
      WSSecHeader secHeader,
      Crypto crypto,
      String alias,
      String signatureAlgorithm,
      String digestAlgorithm,
      List<WSEncryptionPart> parts)
      throws Exception {
    WSSecSignature sign = new WSSecSignature(secHeader);
    sign.setUserInfo(alias, TestKeystoreHelper.KEYSTORE_PASSWORD);
    sign.setKeyIdentifierType(WSConstants.BST_DIRECT_REFERENCE);
    sign.setSignatureAlgorithm(signatureAlgorithm);
    sign.setDigestAlgo(digestAlgorithm);
    sign.setSigCanonicalization(EXCLUSIVE_C14N);
    sign.getParts().addAll(parts);

    sign.prepare(crypto);
    sign.prependBSTElementToHeader();
    sign.computeSignature(sign.addReferencesToSign(sign.getParts()));
  }

  /** Signs first, then encrypts the Body (mirrors {@code processOutbound} action order). */
  private static String signThenEncrypt(
      String soapXml, String signatureAlgorithm, String digestAlgorithm) throws Exception {
    Document doc = parseXml(soapXml);
    WSSecHeader secHeader = new WSSecHeader(doc);
    secHeader.insertSecurityHeader();
    Crypto crypto = cryptoFor(keystorePath);

    signBody(doc, secHeader, crypto, signatureAlgorithm, digestAlgorithm);
    encryptBody(doc, secHeader, crypto);

    return serialize(doc);
  }

  /** Encrypts the Body first, then signs the ciphertext (the opposite security-header order). */
  private static String encryptThenSign(
      String soapXml, String signatureAlgorithm, String digestAlgorithm) throws Exception {
    Document doc = parseXml(soapXml);
    WSSecHeader secHeader = new WSSecHeader(doc);
    secHeader.insertSecurityHeader();
    Crypto crypto = cryptoFor(keystorePath);

    encryptBody(doc, secHeader, crypto);
    signBody(doc, secHeader, crypto, signatureAlgorithm, digestAlgorithm);

    return serialize(doc);
  }

  private static void signBody(
      Document doc,
      WSSecHeader secHeader,
      Crypto crypto,
      String signatureAlgorithm,
      String digestAlgorithm)
      throws Exception {
    signParts(
        secHeader,
        crypto,
        TestKeystoreHelper.KEY_ALIAS,
        signatureAlgorithm,
        digestAlgorithm,
        List.of(
            new WSEncryptionPart("Body", doc.getDocumentElement().getNamespaceURI(), "Content")));
  }

  /** Encrypts the Body content to the alice certificate with AES-128 + rsa-oaep-mgf1p. */
  private static void encryptBody(Document doc, WSSecHeader secHeader, Crypto crypto)
      throws Exception {
    WSSecEncrypt encrypt = new WSSecEncrypt(secHeader);
    encrypt.setUserInfo(TestKeystoreHelper.KEY_ALIAS);
    encrypt.setKeyIdentifierType(WSConstants.X509_KEY_IDENTIFIER);
    encrypt.setSymmetricEncAlgorithm(WSConstants.AES_128);
    encrypt.setKeyEncAlgo(WSConstants.KEYTRANSPORT_RSAOAEP);
    encrypt.build(crypto, aes128Key());
  }

  /** Parses an already signed envelope and encrypts its Body (for the Encrypt-only test). */
  private static String encryptDocument(String signedSoapXml) throws Exception {
    Document doc = parseXml(signedSoapXml);
    WSSecHeader secHeader = new WSSecHeader(doc);
    secHeader.setSecurityHeaderElement(
        (Element) doc.getElementsByTagNameNS(WSConstants.WSSE_NS, "Security").item(0));
    encryptBody(doc, secHeader, cryptoFor(keystorePath));
    return serialize(doc);
  }

  private static SecretKey aes128Key() throws Exception {
    KeyGenerator keyGenerator = KeyGenerator.getInstance("AES");
    keyGenerator.init(128);
    return keyGenerator.generateKey();
  }

  private static Document parseXml(String xml) throws Exception {
    DocumentBuilderFactory factory = DocumentBuilderFactory.newInstance();
    factory.setNamespaceAware(true);
    return factory
        .newDocumentBuilder()
        .parse(new ByteArrayInputStream(xml.getBytes(StandardCharsets.UTF_8)));
  }

  private static String serialize(Document doc) throws Exception {
    return SoapEnvelopeHelper.sourceToString(new DOMSource(doc));
  }

  private static int referenceCount(Document doc) {
    return doc.getElementsByTagNameNS(DS_NS, "Reference").getLength();
  }

  private static int timestampCount(Document doc) {
    return doc.getElementsByTagNameNS(WSConstants.WSU_NS, "Timestamp").getLength();
  }

  /** DigestMethod URI of the single reference whose URI targets the given wsu:Id element. */
  private static String digestAlgorithmOf(Document doc, String signedElementLocalName) {
    NodeList signedElements = doc.getElementsByTagNameNS("*", signedElementLocalName);
    String wsuId = ((Element) signedElements.item(0)).getAttributeNS(WSConstants.WSU_NS, "Id");
    NodeList references = doc.getElementsByTagNameNS(DS_NS, "Reference");
    for (int i = 0; i < references.getLength(); i++) {
      Element reference = (Element) references.item(i);
      if (("#" + wsuId).equals(reference.getAttribute("URI"))) {
        NodeList digests = reference.getElementsByTagNameNS(DS_NS, "DigestMethod");
        return ((Element) digests.item(0)).getAttribute("Algorithm");
      }
    }
    fail("no reference targets the " + signedElementLocalName + " element");
    return null;
  }

  /** Algorithm attribute of the first ds:{@code localName} element. */
  private static String algorithmOf(Document doc, String localName) {
    NodeList nodes = doc.getElementsByTagNameNS(DS_NS, localName);
    return ((Element) nodes.item(0)).getAttribute("Algorithm");
  }

  private static int dataReferenceCount(List<WSSecurityEngineResult> results) {
    int references = 0;
    for (WSSecurityEngineResult result : results) {
      Integer action = (Integer) result.get(WSSecurityEngineResult.TAG_ACTION);
      if (action == null || action != WSConstants.SIGN) continue;
      Object refs = result.get(WSSecurityEngineResult.TAG_DATA_REF_URIS);
      if (refs instanceof List<?> list) {
        references += list.size();
      }
    }
    return references;
  }

  /** Local names of the direct element children of the wsse:Security header, in document order. */
  private static List<String> securityHeaderChildNames(String soapXml) throws Exception {
    Document doc = parseXml(soapXml);
    Element security =
        (Element) doc.getElementsByTagNameNS(WSConstants.WSSE_NS, "Security").item(0);
    List<String> names = new ArrayList<>();
    for (Node node = security.getFirstChild(); node != null; node = node.getNextSibling()) {
      if (node.getNodeType() == Node.ELEMENT_NODE) {
        names.add(node.getLocalName());
      }
    }
    return names;
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
}
