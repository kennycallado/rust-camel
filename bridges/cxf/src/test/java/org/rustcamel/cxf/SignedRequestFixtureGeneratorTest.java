package org.rustcamel.cxf;

import java.nio.file.Files;
import java.nio.file.Path;
import java.security.KeyStore;
import java.security.MessageDigest;
import java.security.cert.X509Certificate;
import java.util.Enumeration;
import javax.security.auth.x500.X500Principal;
import org.junit.jupiter.api.Assumptions;
import org.junit.jupiter.api.Test;

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
          + "<hel:sayHello><name>signed</name></hel:sayHello>"
          + "</soapenv:Body></soapenv:Envelope>";

  @Test
  void generate() throws Exception {
    String jks = System.getenv("CXF_SIGNED_FIXTURE_JKS");
    String out = System.getenv("CXF_SIGNED_FIXTURE_OUT");
    Assumptions.assumeTrue(
        jks != null && out != null,
        "set CXF_SIGNED_FIXTURE_JKS and CXF_SIGNED_FIXTURE_OUT to regenerate the signed fixture");

    SecurityProfile profile =
        SecurityProfile.builder("esap")
            .keystore(jks, "changeit")
            .sigUser("alice", "changeit")
            // WSS4J secure validation (default-on) forbids RSA-SHA1; sign with SHA-256.
            .signatureAlgorithm("http://www.w3.org/2001/04/xmldsig-more#rsa-sha256")
            .signatureDigestAlgorithm("http://www.w3.org/2001/04/xmlenc#sha256")
            .build();
    String signed = new WssSecurityProcessor(profile).processOutbound(SOAP12_REQUEST);
    Files.writeString(Path.of(out), signed);
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
