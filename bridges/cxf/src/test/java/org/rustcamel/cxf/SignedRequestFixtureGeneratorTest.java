package org.rustcamel.cxf;

import static org.junit.jupiter.api.Assertions.assertArrayEquals;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.nio.file.Files;
import java.nio.file.Path;
import java.security.KeyStore;
import java.security.MessageDigest;
import java.security.cert.X509Certificate;
import java.util.ArrayList;
import java.util.Enumeration;
import java.util.List;
import java.util.Locale;
import javax.security.auth.x500.X500Principal;
import javax.xml.transform.dom.DOMSource;
import org.apache.wss4j.common.WSEncryptionPart;
import org.apache.wss4j.common.crypto.Crypto;
import org.apache.wss4j.common.crypto.CryptoFactory;
import org.apache.wss4j.dom.WSConstants;
import org.apache.wss4j.dom.message.WSSecHeader;
import org.apache.wss4j.dom.message.WSSecSignature;
import org.apache.wss4j.dom.message.WSSecTimestamp;
import org.junit.jupiter.api.Assumptions;
import org.junit.jupiter.api.Test;
import org.w3c.dom.Document;
import org.w3c.dom.Element;
import org.w3c.dom.Node;
import org.w3c.dom.NodeList;

/**
 * Test-support generator for the consumer WSS integration fixture ({@code
 * crates/camel-test/tests/fixtures/cxf-wss-signed-request.xml}).
 *
 * <p>Skipped during normal test runs. To (re)generate the fixture, run this test with {@code
 * CXF_SIGNED_FIXTURE_JKS} pointing at the JKS fixture and {@code CXF_SIGNED_FIXTURE_OUT} at the
 * destination path, for example via the capped Gradle container:
 *
 * <pre>
 *   docker run ... \
 *     -e CXF_SIGNED_FIXTURE_JKS=/fixtures/cxf-wss-test-keystore.jks \
 *     -e CXF_SIGNED_FIXTURE_OUT=/fixtures/cxf-wss-signed-request.xml \
 *     -v .../tests/fixtures:/fixtures \
 *     ... ./gradlew test --tests "org.rustcamel.cxf.SignedRequestFixtureGeneratorTest"
 * </pre>
 *
 * <p>The emitted request is a SOAP 1.2 Body signature with <b>no</b> {@code wsu:Timestamp}. The
 * consumer profile requires {@code Signature} only (not {@code Timestamp}), so no freshness window
 * applies and the committed fixture never expires.
 */
class SignedRequestFixtureGeneratorTest {

  private static final String SOAP12_REQUEST =
      "<soapenv:Envelope xmlns:soapenv=\"http://www.w3.org/2003/05/soap-envelope\""
          + " xmlns:hel=\"http://example.com/hello\""
          + " xmlns:s11=\"http://schemas.xmlsoap.org/soap/envelope/\">"
          + "<soapenv:Header><s11:Body>"
          + "<evil:operation xmlns:evil=\"urn:evil\">header-body</evil:operation>"
          + "</s11:Body></soapenv:Header>"
          + "<soapenv:Body>"
          + "<hel:sayHelloRequest><name>signed</name></hel:sayHelloRequest>"
          + "</soapenv:Body></soapenv:Envelope>";

  private static final String HELLO_NS = "http://example.com/hello";
  private static final String EVIL_NS = "urn:evil";
  private static final String SOAP12_NS = "http://www.w3.org/2003/05/soap-envelope";
  private static final String SOAP11_NS = "http://schemas.xmlsoap.org/soap/envelope/";
  private static final String RSA_SHA1 = "http://www.w3.org/2000/09/xmldsig#rsa-sha1";
  private static final String SHA1 = "http://www.w3.org/2000/09/xmldsig#sha1";
  private static final String RSA_SHA256 = "http://www.w3.org/2001/04/xmldsig-more#rsa-sha256";
  private static final String SHA256 = "http://www.w3.org/2001/04/xmlenc#sha256";
  private static final String EXCLUSIVE_C14N = "http://www.w3.org/2001/10/xml-exc-c14n#";
  private static final String DS_NS = "http://www.w3.org/2000/09/xmldsig#";

  /**
   * SOAP 1.2 {@code sayHello} request with no Headers; the app header is inserted by the factory.
   */
  private static final String SOAP12_HEADERLESS_REQUEST =
      "<soapenv:Envelope xmlns:soapenv=\""
          + SOAP12_NS
          + "\" xmlns:hel=\""
          + HELLO_NS
          + "\"><soapenv:Body><hel:sayHelloRequest><name>signed</name></hel:sayHelloRequest>"
          + "</soapenv:Body></soapenv:Envelope>";

  /**
   * The application header carried UNSIGNED by the Rust fixture (T9 asserts {@code signed=false}).
   */
  private static final String HEADER_XML =
      "<rh:requestHeaderType xmlns:rh=\"http://tempuri.org/commonSchema\">"
          + "<rh:user>alice</rh:user><rh:messageId>m-1</rh:messageId>"
          + "<rh:organisation>28071</rh:organisation></rh:requestHeaderType>";

  private static final String APP_HEADER_NS = "http://tempuri.org/commonSchema";
  private static final String APP_HEADER_QNAME = "{" + APP_HEADER_NS + "}requestHeaderType";

  @Test
  void generate() throws Exception {
    String jks = System.getenv("CXF_SIGNED_FIXTURE_JKS");
    String out = System.getenv("CXF_SIGNED_FIXTURE_OUT");
    Assumptions.assumeTrue(
        jks != null && out != null,
        "set CXF_SIGNED_FIXTURE_JKS and CXF_SIGNED_FIXTURE_OUT to regenerate the signed fixture");

    Path keystore = Path.of(jks);
    byte[] keystoreBefore = Files.readAllBytes(keystore);

    SecurityProfile profile =
        SecurityProfile.builder("esap")
            .keystore(jks, "changeit")
            .sigUser("alice", "changeit")
            // JVM-side WSS4J does not forbid RSA-SHA1 (Santuario allows it); these fixtures sign
            // SHA-256 because the bridge's DEFAULT inbound allow-list rejects SHA-1 unless a
            // profile opts in (mission 370 / bd rc-cln4q).
            .signatureAlgorithm("http://www.w3.org/2001/04/xmldsig-more#rsa-sha256")
            .signatureDigestAlgorithm("http://www.w3.org/2001/04/xmlenc#sha256")
            .build();
    String signed = new WssSecurityProcessor(profile).processOutbound(SOAP12_REQUEST);

    // Prove the emitted fixture before committing it: the same trusted keystore verifies the
    // signature over the SOAP Body, the direct Body child is the WSDL input element, the decoy
    // Header Body survives, and no Timestamp leaks in.
    verifySignedRequest(keystore, signed);

    Files.writeString(Path.of(out), signed);

    assertArrayEquals(
        keystoreBefore,
        Files.readAllBytes(keystore),
        "generating the fixture must not mutate the signing keystore");
  }

  /**
   * Test-support generator for the header-bearing Rust fixture ({@code
   * crates/camel-test/tests/fixtures/cxf-wss-signed-request-with-header.xml}).
   *
   * <p>Skipped during normal test runs. To (re)generate the fixture, set {@code
   * CXF_SIGNED_FIXTURE_JKS} to the checked-in JKS and {@code CXF_SIGNED_FIXTURE_WITH_HEADER_OUT} to
   * the destination path:
   *
   * <pre>
   *   CXF_SIGNED_FIXTURE_JKS=.../cxf-wss-test-keystore.jks \
   *   CXF_SIGNED_FIXTURE_WITH_HEADER_OUT=.../cxf-wss-signed-request-with-header.xml \
   *     ./gradlew test --tests "org.rustcamel.cxf.SignedRequestFixtureGeneratorTest"
   * </pre>
   *
   * <p>The request is a SOAP 1.2 Body signature with <b>no</b> {@code wsu:Timestamp}. The
   * application header is carried but deliberately <b>not</b> covered by the signature, so the
   * native test proves {@code signed=false} for a header outside the signed parts.
   */
  @Test
  void generateWithHeader() throws Exception {
    String jks = System.getenv("CXF_SIGNED_FIXTURE_JKS");
    String out = System.getenv("CXF_SIGNED_FIXTURE_WITH_HEADER_OUT");
    Assumptions.assumeTrue(
        jks != null && out != null,
        "set CXF_SIGNED_FIXTURE_JKS and CXF_SIGNED_FIXTURE_WITH_HEADER_OUT to regenerate the "
            + "header-bearing signed fixture");

    Path keystore = Path.of(jks);
    byte[] keystoreBefore = Files.readAllBytes(keystore);

    String signed =
        SignedSoapHeaderEnvelopeFactory.signSoap11(
            SOAP12_HEADERLESS_REQUEST,
            keystore,
            TestKeystoreHelper.KEY_ALIAS,
            RSA_SHA256,
            SHA256,
            HEADER_XML,
            false);

    // Prove the emitted fixture before committing it with the production inbound path: the same
    // trusted keystore verifies the Body signature, exactly one allow-listed header is exported,
    // and that header is NOT covered by the signature (the fixture's contract for T9).
    SecurityProfile verifier =
        SecurityProfile.builder("esap")
            .keystore(jks, TestKeystoreHelper.KEYSTORE_PASSWORD)
            .sigUser(TestKeystoreHelper.KEY_ALIAS, TestKeystoreHelper.KEYSTORE_PASSWORD)
            .exposedSoapHeaders(List.of(APP_HEADER_QNAME))
            .build();
    WssSecurityProcessor.VerifiedInboundMessage verified =
        new WssSecurityProcessor(verifier).processInbound(signed);

    assertNotNull(verified, "the generated header fixture must verify inbound");
    assertTrue(
        verified.signer().isPresent(),
        "the generated header fixture must export a Body-covering signer");
    assertEquals(1, verified.soapHeaders().size(), "the generator must export exactly one header");
    assertEquals(
        "requestHeaderType",
        verified.soapHeaders().get(0).getLocalName(),
        "the exported header must be the allow-listed application header");
    assertFalse(
        verified.soapHeaders().get(0).getSigned(),
        "the fixture header is outside the signed parts and must stay unsigned");

    Document doc = SoapEnvelopeHelper.parseResponse(signed);
    assertEquals(
        0,
        doc.getElementsByTagNameNS("*", "Timestamp").getLength(),
        "no wsu:Timestamp may appear in the fixture");

    Files.writeString(Path.of(out), signed);

    assertArrayEquals(
        keystoreBefore,
        Files.readAllBytes(keystore),
        "generating the fixture must not mutate the signing keystore");
  }

  /**
   * Test-support generator for the long-subject-DN signer fixtures used by the Rust-side descriptor
   * bound tests ({@code crates/camel-test/tests/fixtures/cxf-wss-longdn-keystore.jks} and {@code
   * cxf-wss-longdn-signed-request.xml}).
   *
   * <p>Skipped during normal test runs. To (re)generate the fixtures, run this test with {@code
   * CXF_LONGDN_FIXTURE_DIR} pointing at {@code crates/camel-test/tests/fixtures}, for example via
   * the capped Gradle container:
   *
   * <pre>
   *   docker run ... -e CXF_LONGDN_FIXTURE_DIR=/fixtures -v .../tests/fixtures:/fixtures \
   *     ... ./gradlew test --tests "org.rustcamel.cxf.SignedRequestFixtureGeneratorTest"
   * </pre>
   *
   * <p>The generated certificate subject is a 4097-character {@code CN} (an RFC 2253 DN of more
   * than 4096 UTF-8 bytes) while the DER certificate stays far below the 64 KiB cap, so the Java
   * side verifies the Body signature and exports the descriptor and the Rust bounds reject it. The
   * request is a SOAP 1.2 Body signature with <b>no</b> {@code wsu:Timestamp}, so the committed
   * fixture never expires. When {@code CXF_SIGNED_FIXTURE_JKS} is also set, the exported signer
   * descriptor of that fixture's certificate is printed too (the known expected values asserted by
   * the native signer-metadata test).
   */
  @Test
  void generateLongDnFixture() throws Exception {
    String dir = System.getenv("CXF_LONGDN_FIXTURE_DIR");
    Assumptions.assumeTrue(
        dir != null, "set CXF_LONGDN_FIXTURE_DIR to regenerate the long-DN signer fixtures");

    // 4097 ASCII bytes: Java accepts the DN and verifies the signature; the
    // RFC 2253 subject exceeds the Rust 4096-byte bound and must be rejected.
    String dname = "CN=" + "a".repeat(4097) + ", O=RustCamel Test";
    Path jks = Path.of(dir, "cxf-wss-longdn-keystore.jks");
    TestKeystoreHelper.createTestKeystoreAt(
        jks, TestKeystoreHelper.KEYSTORE_PASSWORD, TestKeystoreHelper.KEYSTORE_PASSWORD, dname);

    SecurityProfile profile =
        SecurityProfile.builder("esap")
            .keystore(jks.toString(), TestKeystoreHelper.KEYSTORE_PASSWORD)
            .sigUser(TestKeystoreHelper.KEY_ALIAS, TestKeystoreHelper.KEYSTORE_PASSWORD)
            // JVM-side WSS4J does not forbid RSA-SHA1 (Santuario allows it); these fixtures sign
            // SHA-256 because the bridge's DEFAULT inbound allow-list rejects SHA-1 unless a
            // profile opts in (mission 370 / bd rc-cln4q).
            .signatureAlgorithm("http://www.w3.org/2001/04/xmldsig-more#rsa-sha256")
            .signatureDigestAlgorithm("http://www.w3.org/2001/04/xmlenc#sha256")
            .build();
    String signed = new WssSecurityProcessor(profile).processOutbound(SOAP12_REQUEST);
    Files.writeString(Path.of(dir, "cxf-wss-longdn-signed-request.xml"), signed);

    dumpDescriptor("longdn", jks);
    String aliceJks = System.getenv("CXF_SIGNED_FIXTURE_JKS");
    if (aliceJks != null) {
      dumpDescriptor("alice", Path.of(aliceJks));
    }
  }

  /**
   * Re-signs the corrected static long-DN request with the EXISTING long-DN keystore, without
   * regenerating its certificate. Gated by {@code CXF_LONGDN_REUSE_FIXTURE_DIR}; skipped silently
   * when unset (the same opt-in skip pattern as the other generators). The keystore bytes must be
   * identical before and after: only {@code cxf-wss-longdn-signed-request.xml} is written.
   */
  @Test
  void regenerateLongDnWithExistingKeystore() throws Exception {
    String dir = System.getenv("CXF_LONGDN_REUSE_FIXTURE_DIR");
    Assumptions.assumeTrue(
        dir != null,
        "set CXF_LONGDN_REUSE_FIXTURE_DIR to re-sign the long-DN fixture with its existing keystore");

    Path jks = Path.of(dir, "cxf-wss-longdn-keystore.jks");
    if (!Files.exists(jks)) {
      throw new IllegalStateException(
          "the existing long-DN keystore is required for re-signing: " + jks);
    }
    byte[] keystoreBefore = Files.readAllBytes(jks);

    SecurityProfile profile =
        SecurityProfile.builder("esap")
            .keystore(jks.toString(), TestKeystoreHelper.KEYSTORE_PASSWORD)
            .sigUser(TestKeystoreHelper.KEY_ALIAS, TestKeystoreHelper.KEYSTORE_PASSWORD)
            // JVM-side WSS4J does not forbid RSA-SHA1 (Santuario allows it); these fixtures sign
            // SHA-256 because the bridge's DEFAULT inbound allow-list rejects SHA-1 unless a
            // profile opts in (mission 370 / bd rc-cln4q).
            .signatureAlgorithm("http://www.w3.org/2001/04/xmldsig-more#rsa-sha256")
            .signatureDigestAlgorithm("http://www.w3.org/2001/04/xmlenc#sha256")
            .build();
    String signed = new WssSecurityProcessor(profile).processOutbound(SOAP12_REQUEST);

    // Prove the re-signed fixture before committing it (same guarantees as `generate`).
    verifySignedRequest(jks, signed);

    Files.writeString(Path.of(dir, "cxf-wss-longdn-signed-request.xml"), signed);

    byte[] keystoreAfter = Files.readAllBytes(jks);
    assertArrayEquals(
        keystoreBefore, keystoreAfter, "re-signing must not mutate the existing long-DN keystore");
  }

  /**
   * Generates the legacy partner shape fresh in memory and round-trips it through the production
   * inbound path with the SHA-1 opt-in. No fixture file is committed: the shape carries a signed
   * {@code wsu:Timestamp} that expires after the WSS4J TTL (~300 s), so a static file would be a
   * time bomb. The generator runs in-test, so the TTL is irrelevant here (mission 370 / bd
   * rc-cln4q).
   */
  @Test
  void generateLegacySha1Fixture_roundTripsAgainstOptedInProfile() throws Exception {
    Path keystore = TestKeystoreHelper.createTestKeystore();

    String signed =
        generateLegacySha1Fixture(
            keystore, TestKeystoreHelper.KEY_ALIAS, TestKeystoreHelper.KEYSTORE_PASSWORD);

    Document doc = SoapEnvelopeHelper.parseResponse(signed);
    assertEquals(
        3,
        doc.getElementsByTagNameNS(DS_NS, "Reference").getLength(),
        "the partner shape must carry exactly three references");
    assertEquals(
        RSA_SHA1,
        algorithmOf(doc, "SignatureMethod"),
        "the generated signature method must be rsa-sha1");
    assertEquals(SHA1, digestAlgorithmOf(doc, "Timestamp"), "the Timestamp digest must be sha1");
    assertEquals(SHA1, digestAlgorithmOf(doc, "Body"), "the Body digest must be sha1");
    assertEquals(
        SHA1,
        digestAlgorithmOf(doc, "BinarySecurityToken"),
        "the BinarySecurityToken digest must be sha1");

    SecurityProfile optedIn =
        SecurityProfile.builder("legacy-fixture-opted-in")
            .keystore(keystore.toString(), TestKeystoreHelper.KEYSTORE_PASSWORD)
            .truststore(keystore.toString(), TestKeystoreHelper.KEYSTORE_PASSWORD)
            .actionsIn("Signature")
            .allowedSignatureAlgorithms(RSA_SHA1)
            .allowedDigestAlgorithms(SHA1)
            .build();

    WssSecurityProcessor.VerifiedInboundMessage verified =
        new WssSecurityProcessor(optedIn).processInbound(signed);

    assertNotNull(verified, "the freshly generated legacy fixture must verify when opted in");
    assertTrue(
        verified.signer().isPresent(),
        "the opted-in verification must export the Body-covering signer");
  }

  /**
   * Guards this file's own prose: no comment may keep the old claim that the JVM-side stack forbids
   * the legacy signature algorithm. The real reason the fixtures sign SHA-256 is the bridge's
   * opt-in inbound policy, not JVM-side crypto (mission 370 / bd rc-cln4q).
   */
  @Test
  void commentsAreTruthful() throws Exception {
    Path source = locateOwnSource();
    List<String> offenders = new ArrayList<>();
    boolean truthfulReasonDocumented = false;
    for (String line : Files.readAllLines(source)) {
      String text = line.strip();
      boolean comment = text.startsWith("//") || text.startsWith("*") || text.startsWith("/*");
      if (!comment) {
        continue;
      }
      String lower = text.toLowerCase(Locale.ROOT);
      if (lower.contains("forbids rsa-sha1") || lower.contains("forbidden rsa-sha1")) {
        offenders.add(text);
      }
      if (lower.contains("default inbound allow-list")) {
        truthfulReasonDocumented = true;
      }
    }
    assertTrue(
        offenders.isEmpty(),
        "comments must not claim the JVM-side stack forbids the legacy algorithm: " + offenders);
    assertTrue(
        truthfulReasonDocumented,
        "the comments must document the real reason: the bridge default inbound allow-list");
  }

  /**
   * Resolves this test's own source file from the Gradle project dir or the repository root. Used
   * only by {@link #commentsAreTruthful()}.
   */
  private static Path locateOwnSource() {
    String relative = "src/test/java/org/rustcamel/cxf/SignedRequestFixtureGeneratorTest.java";
    for (Path candidate : List.of(Path.of(relative), Path.of("bridges/cxf", relative))) {
      if (Files.exists(candidate)) {
        return candidate;
      }
    }
    throw new IllegalStateException(
        "cannot locate SignedRequestFixtureGeneratorTest.java from "
            + Path.of("").toAbsolutePath());
  }

  /**
   * Generates the wse-php partner shape fully in memory: rsa-sha1 + sha1 over BinarySecurityToken,
   * Timestamp, and Body with exclusive c14n. Test/repro generator only — no fixture file is
   * committed, because a signed Timestamp expires after the WSS4J TTL (~300 s). The JVM allows
   * SHA-1; only the bridge's default inbound allow-list rejects it unless a profile opts in
   * (mission 370 / bd rc-cln4q).
   */
  static String generateLegacySha1Fixture(Path keystore, String alias, String password)
      throws Exception {
    Document doc = SoapEnvelopeHelper.parseResponse(SOAP12_REQUEST);
    WSSecHeader secHeader = new WSSecHeader(doc);
    secHeader.insertSecurityHeader();
    new WSSecTimestamp(secHeader).build();

    Crypto crypto =
        CryptoFactory.getInstance(
            SecurityProfile.createCryptoProperties(keystore.toString(), password));

    WSSecSignature sign = new WSSecSignature(secHeader);
    sign.setUserInfo(alias, password);
    sign.setKeyIdentifierType(WSConstants.BST_DIRECT_REFERENCE);
    sign.setSignatureAlgorithm(RSA_SHA1);
    sign.setDigestAlgo(SHA1);
    sign.setSigCanonicalization(EXCLUSIVE_C14N);
    // The envelope root NS is the SOAP NS — never hardcode SOAP 1.1 here, or SOAP 1.2 envelopes
    // fail part resolution at sign time (the same expression as WssSecurityProcessor's outbound
    // Body part).
    sign.getParts()
        .add(new WSEncryptionPart("Body", doc.getDocumentElement().getNamespaceURI(), "Content"));
    sign.getParts().add(new WSEncryptionPart("Timestamp", WSConstants.WSU_NS, ""));
    sign.getParts().add(new WSEncryptionPart("BinarySecurityToken", WSConstants.WSSE_NS, ""));

    // WSSecSignature materializes the BST only after prepare(); a BinarySecurityToken part cannot
    // be resolved before the element is in the security header.
    sign.prepare(crypto);
    sign.prependBSTElementToHeader();
    sign.computeSignature(sign.addReferencesToSign(sign.getParts()));

    return SoapEnvelopeHelper.sourceToString(new DOMSource(doc));
  }

  /** Algorithm attribute of the first ds:{@code localName} element. */
  private static String algorithmOf(Document doc, String localName) {
    NodeList nodes = doc.getElementsByTagNameNS(DS_NS, localName);
    assertTrue(nodes.getLength() > 0, "missing ds:" + localName);
    return ((Element) nodes.item(0)).getAttribute("Algorithm");
  }

  /**
   * DigestMethod URI of the single reference whose URI targets the given wsu:Id element. Scans all
   * elements with the local name because this fixture also carries a decoy Header {@code s11:Body}:
   * only the signed element has a {@code wsu:Id} and a matching reference.
   */
  private static String digestAlgorithmOf(Document doc, String signedElementLocalName) {
    NodeList signedElements = doc.getElementsByTagNameNS("*", signedElementLocalName);
    assertTrue(signedElements.getLength() > 0, "missing " + signedElementLocalName);
    NodeList references = doc.getElementsByTagNameNS(DS_NS, "Reference");
    for (int e = 0; e < signedElements.getLength(); e++) {
      Element signedElement = (Element) signedElements.item(e);
      String wsuId = signedElement.getAttributeNS(WSConstants.WSU_NS, "Id");
      if (wsuId.isEmpty()) {
        continue;
      }
      for (int i = 0; i < references.getLength(); i++) {
        Element reference = (Element) references.item(i);
        if (("#" + wsuId).equals(reference.getAttribute("URI"))) {
          NodeList digests = reference.getElementsByTagNameNS(DS_NS, "DigestMethod");
          assertTrue(digests.getLength() > 0, "missing DigestMethod for " + signedElementLocalName);
          return ((Element) digests.item(0)).getAttribute("Algorithm");
        }
      }
    }
    throw new AssertionError("no reference targets the " + signedElementLocalName + " element");
  }

  /**
   * Proves a freshly signed fixture with the production WSS path and namespace-aware DOM checks:
   * the certificate in {@code keystore} trusts the signature, a Body-covering signer is exported,
   * the direct SOAP Body child is {@code {http://example.com/hello}sayHelloRequest}, the decoy
   * {@code s11:Body}/{@code {urn:evil}operation} survives in the Header, and no Timestamp exists.
   * Reuses {@link WssSecurityProcessor#processInbound} and {@link SoapEnvelopeHelper}; adds no new
   * crypto.
   */
  private static void verifySignedRequest(Path keystore, String signed) throws Exception {
    SecurityProfile verifier =
        SecurityProfile.builder("esap")
            .keystore(keystore.toString(), TestKeystoreHelper.KEYSTORE_PASSWORD)
            .sigUser(TestKeystoreHelper.KEY_ALIAS, TestKeystoreHelper.KEYSTORE_PASSWORD)
            .build();
    WssSecurityProcessor.VerifiedInboundMessage verified =
        new WssSecurityProcessor(verifier).processInbound(signed);
    assertTrue(
        verified.signer().isPresent(),
        "inbound WSS verification must succeed and export a Body-covering signer");

    Document doc = SoapEnvelopeHelper.parseResponse(signed);
    Element body = SoapEnvelopeHelper.directBodyElement(doc);
    assertNotNull(body, "the signed request must expose a direct SOAP Body child");
    Element payload = firstElementChild(body);
    assertNotNull(payload, "the direct SOAP Body must carry the operation payload");
    assertEquals("sayHelloRequest", payload.getLocalName(), "direct SOAP Body child local name");
    assertEquals(HELLO_NS, payload.getNamespaceURI(), "direct SOAP Body child namespace");

    Element header = firstElementChildNamed(doc.getDocumentElement(), SOAP12_NS, "Header");
    assertNotNull(header, "the signed request must retain its SOAP Header");
    assertEquals(SOAP12_NS, header.getNamespaceURI(), "SOAP Header namespace");
    Element decoy = firstElementChildNamed(header, SOAP11_NS, "Body");
    assertNotNull(decoy, "the decoy s11:Body must be retained in the SOAP Header");
    assertEquals(SOAP11_NS, decoy.getNamespaceURI(), "decoy Body namespace");
    Element evil = firstElementChild(decoy);
    assertNotNull(evil, "the decoy Header Body must retain its payload");
    assertEquals("operation", evil.getLocalName(), "decoy operation local name");
    assertEquals(EVIL_NS, evil.getNamespaceURI(), "decoy operation namespace");

    NodeList timestamps = doc.getElementsByTagNameNS("*", "Timestamp");
    assertEquals(0, timestamps.getLength(), "no wsu:Timestamp may appear in the fixture");
  }

  private static Element firstElementChild(Element parent) {
    for (Node node = parent.getFirstChild(); node != null; node = node.getNextSibling()) {
      if (node instanceof Element element) {
        return element;
      }
    }
    return null;
  }

  private static Element firstElementChildNamed(
      Element parent, String namespaceUri, String localName) {
    for (Node node = parent.getFirstChild(); node != null; node = node.getNextSibling()) {
      if (node instanceof Element element
          && localName.equals(element.getLocalName())
          && namespaceUri.equals(element.getNamespaceURI())) {
        return element;
      }
    }
    return null;
  }

  /** Prints the exact descriptor fields the bridge exports for one keystore certificate. */
  private static void dumpDescriptor(String label, Path jks) throws Exception {
    KeyStore keyStore = KeyStore.getInstance("JKS");
    try (var in = Files.newInputStream(jks)) {
      keyStore.load(in, TestKeystoreHelper.KEYSTORE_PASSWORD.toCharArray());
    }
    for (Enumeration<String> aliases = keyStore.aliases(); aliases.hasMoreElements(); ) {
      String alias = aliases.nextElement();
      java.security.cert.Certificate cert = keyStore.getCertificate(alias);
      if (!(cert instanceof X509Certificate x509)) {
        continue;
      }
      byte[] der = x509.getEncoded();
      MessageDigest sha = MessageDigest.getInstance("SHA-256");
      StringBuilder fingerprint = new StringBuilder();
      for (byte b : sha.digest(der)) {
        fingerprint.append(Character.forDigit((b >> 4) & 0xf, 16));
        fingerprint.append(Character.forDigit(b & 0xf, 16));
      }
      System.out.println("[" + label + "] alias=" + alias);
      System.out.println(
          "["
              + label
              + "] subject="
              + x509.getSubjectX500Principal().getName(X500Principal.RFC2253));
      System.out.println(
          "[" + label + "] issuer=" + x509.getIssuerX500Principal().getName(X500Principal.RFC2253));
      System.out.println("[" + label + "] serial=" + x509.getSerialNumber().toString(16));
      System.out.println("[" + label + "] fingerprint=" + fingerprint);
      System.out.println("[" + label + "] der_len=" + der.length);
    }
  }
}
