package org.rustcamel.cxf;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertInstanceOf;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.nio.file.Files;
import java.nio.file.Path;
import java.security.MessageDigest;
import java.security.cert.CertificateExpiredException;
import java.security.cert.X509Certificate;
import org.apache.wss4j.common.ext.WSSecurityException;
import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;

/**
 * Engine-boundary acceptance and rejection of an expired certificate that is exactly pinned in the
 * profile's verification truststore. The flag never relaxes any other validity gate.
 */
class ExpiredPinnedCertAcceptanceTest {

  private static final String RSA_SHA256 = "http://www.w3.org/2001/04/xmldsig-more#rsa-sha256";
  private static final String SHA256 = "http://www.w3.org/2001/04/xmlenc#sha256";
  private static final String INVALID_CERT_MESSAGE = "The provided certificate is invalid";

  private static Path expiredStore;
  private static Path expiredTruststore;
  private static Path sameDnDifferentKeyStore;

  @BeforeAll
  static void setUpStores() throws Exception {
    expiredStore = TestKeystoreHelper.createExpiredTestKeystore();
    expiredTruststore =
        TestKeystoreHelper.createTruststoreWithCertificate(
            expiredStore, TestKeystoreHelper.KEY_ALIAS);
    sameDnDifferentKeyStore = TestKeystoreHelper.createExpiredSameDnDifferentKeyKeystore();
  }

  @AfterAll
  static void tearDownStores() throws Exception {
    for (Path store : new Path[] {expiredStore, expiredTruststore, sameDnDifferentKeyStore}) {
      if (store != null) {
        Files.deleteIfExists(store);
      }
    }
  }

  @Test
  void expired_pinned_cert_accepted_when_opted_in() throws Exception {
    SecurityProfile profile = expiredProfile("expired-pinned-acceptance", true);

    WssSecurityProcessor.VerifiedInboundMessage verified =
        new WssSecurityProcessor(profile).processInbound(signWith(expiredStore));

    assertTrue(verified.signer().isPresent(), "an accepted pinned signature must export a signer");
    X509Certificate expired =
        TestKeystoreHelper.certificateFrom(expiredStore, TestKeystoreHelper.KEY_ALIAS);
    assertEquals(
        sha256Hex(expired.getEncoded()),
        verified.signer().get().getFingerprintSha256(),
        "the exported signer must be the exact pinned expired certificate");
  }

  @Test
  void default_off_engine_rejection_is_rendered() throws Exception {
    SecurityProfile profile = expiredProfile("expired-pinned-default-off", false);

    WSSecurityException ex =
        assertThrows(
            WSSecurityException.class,
            () -> new WssSecurityProcessor(profile).processInbound(signWith(expiredStore)));

    assertEquals(WSSecurityException.ErrorCode.FAILED_CHECK, ex.getErrorCode());
    assertInstanceOf(CertificateExpiredException.class, ex.getCause());
    assertTrue(
        ex.getMessage().contains(INVALID_CERT_MESSAGE),
        "unexpected rejection message: " + ex.getMessage());
  }

  @Test
  void expired_cert_rejected_when_not_pinned() throws Exception {
    SecurityProfile profile = expiredProfile("expired-pinned-not-matching", true);

    assertThrows(
        WSSecurityException.class,
        () -> new WssSecurityProcessor(profile).processInbound(signWith(sameDnDifferentKeyStore)),
        "a same-DN certificate with different key material must not match the pin");
  }

  private static SecurityProfile expiredProfile(String name, boolean optIn) {
    SecurityProfile.Builder builder =
        SecurityProfile.builder(name)
            .keystore(expiredStore.toString(), TestKeystoreHelper.KEYSTORE_PASSWORD)
            .truststore(expiredTruststore.toString(), TestKeystoreHelper.KEYSTORE_PASSWORD);
    if (optIn) {
      builder.allowExpiredPinnedCertificate(true);
    }
    return builder.build();
  }

  private static String signWith(Path keystore) throws Exception {
    return ExpiredSignedEnvelopeFactory.signSoap11(
        plainSoap11(), keystore, TestKeystoreHelper.KEY_ALIAS, RSA_SHA256, SHA256, false, false);
  }

  private static String plainSoap11() {
    return """
        <soapenv:Envelope xmlns:soapenv="http://schemas.xmlsoap.org/soap/envelope/">
          <soapenv:Header/>
          <soapenv:Body><test:Hello xmlns:test="http://test.example.com">World</test:Hello></soapenv:Body>
        </soapenv:Envelope>
        """;
  }

  private static String sha256Hex(byte[] der) throws Exception {
    byte[] digest = MessageDigest.getInstance("SHA-256").digest(der);
    StringBuilder hex = new StringBuilder(digest.length * 2);
    for (byte b : digest) {
      hex.append(Character.forDigit((b >> 4) & 0xf, 16)).append(Character.forDigit(b & 0xf, 16));
    }
    return hex.toString();
  }
}
