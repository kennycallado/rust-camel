package org.rustcamel.cxf;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertInstanceOf;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.nio.file.Files;
import java.nio.file.Path;
import java.security.cert.CertificateExpiredException;
import org.apache.wss4j.common.ext.WSSecurityException;
import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;

/**
 * Baseline characterization of the shipped expiry rejection: an expired certificate pinned as both
 * keystore and truststore is rejected before the opt-in exists, with no reference to the new flag
 * or trust seam.
 */
class ExpiredPinnedCertBaselineCharacterizationTest {

  private static final String RSA_SHA256 = "http://www.w3.org/2001/04/xmldsig-more#rsa-sha256";
  private static final String SHA256 = "http://www.w3.org/2001/04/xmlenc#sha256";

  private static Path expiredStore;

  @BeforeAll
  static void setUpExpiredStore() throws Exception {
    expiredStore = TestKeystoreHelper.createExpiredTestKeystore();
  }

  @AfterAll
  static void tearDownExpiredStore() throws Exception {
    if (expiredStore != null) {
      Files.deleteIfExists(expiredStore);
    }
  }

  @Test
  void baselineExpiredPinnedCertIsRejectedByExistingProfile() throws Exception {
    SecurityProfile profile =
        SecurityProfile.builder("baseline-expired-pinned")
            .keystore(expiredStore.toString(), TestKeystoreHelper.KEYSTORE_PASSWORD)
            .truststore(expiredStore.toString(), TestKeystoreHelper.KEYSTORE_PASSWORD)
            .actionsIn("Signature")
            .build();

    String signed =
        ExpiredSignedEnvelopeFactory.signSoap11(
            plainSoap11(),
            expiredStore,
            TestKeystoreHelper.KEY_ALIAS,
            RSA_SHA256,
            SHA256,
            false,
            false);

    WSSecurityException ex =
        assertThrows(
            WSSecurityException.class,
            () -> new WssSecurityProcessor(profile).processInbound(signed),
            "the shipped profile must reject an expired pinned certificate");

    assertEquals(WSSecurityException.ErrorCode.FAILED_CHECK, ex.getErrorCode());
    assertInstanceOf(CertificateExpiredException.class, ex.getCause());
    assertTrue(
        ex.getMessage().contains("The provided certificate is invalid"),
        "unexpected rejection message: " + ex.getMessage());
  }

  private static String plainSoap11() {
    return """
        <soapenv:Envelope xmlns:soapenv="http://schemas.xmlsoap.org/soap/envelope/">
          <soapenv:Header/>
          <soapenv:Body><test:Hello xmlns:test="http://test.example.com">World</test:Hello></soapenv:Body>
        </soapenv:Envelope>
        """;
  }
}
