package org.rustcamel.cxf;

import static org.junit.jupiter.api.Assertions.assertArrayEquals;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.nio.file.Files;
import java.nio.file.Path;
import java.security.KeyStore;
import java.security.MessageDigest;
import java.security.cert.X509Certificate;
import java.util.Enumeration;
import javax.security.auth.x500.X500Principal;
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
            // WSS4J secure validation (default-on) forbids RSA-SHA1; sign with SHA-256.
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
            // WSS4J secure validation (default-on) forbids RSA-SHA1; sign with SHA-256.
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
            // WSS4J secure validation (default-on) forbids RSA-SHA1; sign with SHA-256.
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
