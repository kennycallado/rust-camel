package org.rustcamel.cxf;

import static org.junit.jupiter.api.Assertions.assertArrayEquals;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertInstanceOf;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;
import static org.mockito.ArgumentMatchers.any;
import static org.mockito.ArgumentMatchers.anyBoolean;
import static org.mockito.ArgumentMatchers.anyCollection;
import static org.mockito.Mockito.doAnswer;
import static org.mockito.Mockito.doReturn;
import static org.mockito.Mockito.spy;

import java.io.ByteArrayInputStream;
import java.nio.file.Files;
import java.nio.file.Path;
import java.security.cert.Certificate;
import java.security.cert.CertificateExpiredException;
import java.security.cert.CertificateFactory;
import java.security.cert.CertificateNotYetValidException;
import java.security.cert.X509Certificate;
import java.util.Base64;
import java.util.List;
import java.util.concurrent.atomic.AtomicInteger;
import org.apache.wss4j.common.crypto.Crypto;
import org.apache.wss4j.common.ext.WSSecurityException;
import org.apache.wss4j.dom.WSConstants;
import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;
import org.w3c.dom.Document;
import org.w3c.dom.Element;
import org.w3c.dom.NodeList;

/**
 * Proves the opt-in relaxes expiry only: not-yet-valid, chain, and issuer-only presentations still
 * reject, and the SHA-1 allow-list composes independently with the expiry opt-in.
 */
class ExpiredPinnedCertGateTest {

  private static final String RSA_SHA256 = "http://www.w3.org/2001/04/xmldsig-more#rsa-sha256";
  private static final String SHA256 = "http://www.w3.org/2001/04/xmlenc#sha256";
  private static final String RSA_SHA1 = "http://www.w3.org/2000/09/xmldsig#rsa-sha1";
  private static final String SHA1 = "http://www.w3.org/2000/09/xmldsig#sha1";
  private static final String INVALID_CERT_MESSAGE = "The provided certificate is invalid";
  private static final String PKIPATH_VALUE_TYPE = WSConstants.X509TOKEN_NS + "#X509PKIPathv1";

  @TempDir static Path tempDir;

  private static Path futureStore;
  private static Path sha1Store;
  private static Path chainLeafStore;
  private static Path caTruststore;

  @BeforeAll
  static void setUpStores() throws Exception {
    futureStore = TestKeystoreHelper.createFutureTestKeystore();
    sha1Store = TestKeystoreHelper.createExpiredSha1TestKeystore();
    chainLeafStore = TestKeystoreHelper.createExpiredChainKeystore(tempDir);
    caTruststore = TestKeystoreHelper.createTruststoreWithCertificate(chainLeafStore, "ca");
  }

  @AfterAll
  static void tearDownStores() throws Exception {
    for (Path store : new Path[] {futureStore, sha1Store, chainLeafStore, caTruststore}) {
      if (store != null) {
        Files.deleteIfExists(store);
      }
    }
  }

  @Test
  void not_yet_valid_cert_rejected() throws Exception {
    SecurityProfile profile =
        SecurityProfile.builder("gate-future")
            .keystore(futureStore.toString(), TestKeystoreHelper.KEYSTORE_PASSWORD)
            .truststore(futureStore.toString(), TestKeystoreHelper.KEYSTORE_PASSWORD)
            .actionsIn("Signature")
            .allowExpiredPinnedCertificate(true)
            .build();

    String signed =
        ExpiredSignedEnvelopeFactory.signSoap11(
            plainSoap11(),
            futureStore,
            TestKeystoreHelper.KEY_ALIAS,
            RSA_SHA256,
            SHA256,
            false,
            false);

    WSSecurityException ex =
        assertThrows(
            WSSecurityException.class,
            () -> new WssSecurityProcessor(profile).processInbound(signed),
            "the opt-in must not accept a not-yet-valid certificate");

    assertEquals(WSSecurityException.ErrorCode.FAILED_CHECK, ex.getErrorCode());
    assertInstanceOf(CertificateNotYetValidException.class, ex.getCause());
    assertTrue(
        ex.getMessage().contains(INVALID_CERT_MESSAGE),
        "unexpected rejection message: " + ex.getMessage());
  }

  @Test
  void chain_with_expired_leaf_rejected() throws Exception {
    SecurityProfile profile = chainProfile("gate-chain");

    String signed =
        ExpiredSignedEnvelopeFactory.signSoap11(
            plainSoap11(), chainLeafStore, "leaf", RSA_SHA256, SHA256, false, true);

    Document doc = SoapEnvelopeHelper.parseResponse(signed);
    NodeList tokens = doc.getElementsByTagNameNS(WSConstants.WSSE_NS, "BinarySecurityToken");
    assertEquals(
        1,
        tokens.getLength(),
        "a chain signature must carry one BinarySecurityToken; the X509_KEY_IDENTIFIER branch"
            + " serializes certs[0] only");
    Element token = (Element) tokens.item(0);
    assertEquals(
        PKIPATH_VALUE_TYPE,
        token.getAttribute("ValueType"),
        "the chain token must be an X509PKIPathv1, not a single X509v3 certificate");
    String tokenId = token.getAttributeNS(WSConstants.WSU_NS, "Id");
    assertFalse(tokenId.isBlank(), "the PKIPath token must carry a wsu:Id");

    NodeList references = doc.getElementsByTagNameNS(WSConstants.WSSE_NS, "Reference");
    assertEquals(1, references.getLength(), "the signature must carry one direct BST reference");
    Element reference = (Element) references.item(0);
    assertEquals("#" + tokenId, reference.getAttribute("URI"), "the STR must point at the BST");
    assertEquals(PKIPATH_VALUE_TYPE, reference.getAttribute("ValueType"), "STR token type");

    X509Certificate[] expected = TestKeystoreHelper.certificateChainFrom(chainLeafStore, "leaf");
    assertEquals(2, expected.length);
    byte[] pkiPath = Base64.getMimeDecoder().decode(token.getTextContent().trim());
    List<? extends Certificate> decoded =
        CertificateFactory.getInstance("X.509")
            .generateCertPath(new ByteArrayInputStream(pkiPath))
            .getCertificates();
    assertEquals(2, decoded.size(), "the DER PkiPath must carry leaf and CA");
    assertArrayEquals(expected[0].getEncoded(), decoded.get(0).getEncoded(), "leaf must be first");
    assertArrayEquals(expected[1].getEncoded(), decoded.get(1).getEncoded(), "CA must be second");

    Crypto realCrypto = profile.getVerificationCrypto();
    Crypto cryptoSpy = spy(realCrypto);
    AtomicInteger trustChecks = new AtomicInteger();
    doAnswer(
            invocation -> {
              X509Certificate[] presented = invocation.getArgument(0);
              assertEquals(
                  2,
                  presented.length,
                  "trust verification must receive the full leaf-plus-CA chain");
              assertArrayEquals(expected[0].getEncoded(), presented[0].getEncoded(), "leaf");
              assertArrayEquals(expected[1].getEncoded(), presented[1].getEncoded(), "CA");
              trustChecks.incrementAndGet();
              return invocation.callRealMethod();
            })
        .when(cryptoSpy)
        .verifyTrust(any(X509Certificate[].class), anyBoolean(), anyCollection(), anyCollection());

    SecurityProfile observedProfile = spy(profile);
    doReturn(cryptoSpy).when(observedProfile).getVerificationCrypto();

    assertThrows(
        WSSecurityException.class,
        () -> new WssSecurityProcessor(observedProfile).processInbound(signed),
        "the opt-in must not accept a chain whose leaf is expired");
    assertEquals(
        1,
        trustChecks.get(),
        "the inbound engine must invoke the observed production trust verification exactly once");
  }

  @Test
  void pinned_issuer_does_not_authorize_expired_leaf() throws Exception {
    SecurityProfile profile = chainProfile("gate-issuer-only");

    String signed =
        ExpiredSignedEnvelopeFactory.signSoap11(
            plainSoap11(), chainLeafStore, "leaf", RSA_SHA256, SHA256, false, false);

    assertThrows(
        WSSecurityException.class,
        () -> new WssSecurityProcessor(profile).processInbound(signed),
        "a pinned issuer must not authorize the presented expired leaf");
  }

  @Test
  void sha1_and_expiry_opt_ins_compose() throws Exception {
    String signed =
        ExpiredSignedEnvelopeFactory.signSoap11(
            plainSoap11(), sha1Store, TestKeystoreHelper.KEY_ALIAS, RSA_SHA1, SHA1, false, false);

    SecurityProfile bothOptIns = sha1Profile("gate-sha1-and-expiry", true, true);
    WssSecurityProcessor.VerifiedInboundMessage verified =
        new WssSecurityProcessor(bothOptIns).processInbound(signed);

    assertTrue(
        verified.signer().isPresent(),
        "both opt-ins must accept the partner signature and export its signer");

    SecurityProfile expiryOnly = sha1Profile("gate-sha1-off", false, true);
    WSSecurityException algorithmRejection =
        assertThrows(
            WSSecurityException.class,
            () -> new WssSecurityProcessor(expiryOnly).processInbound(signed),
            "the secure default allow-lists must reject the SHA-1 signature");

    assertEquals(
        WSSecurityException.ErrorCode.INVALID_SECURITY,
        algorithmRejection.getErrorCode(),
        "the rejection must come from the algorithm allow-list");
    assertFalse(
        algorithmRejection.getCause() instanceof CertificateExpiredException,
        "the algorithm rejection must not be the expiry failure");
    assertFalse(
        algorithmRejection.getMessage().contains(INVALID_CERT_MESSAGE),
        "the algorithm rejection must not render the invalidCert message: "
            + algorithmRejection.getMessage());

    SecurityProfile sha1Only = sha1Profile("gate-expiry-off", true, false);
    WSSecurityException expiryRejection =
        assertThrows(
            WSSecurityException.class,
            () -> new WssSecurityProcessor(sha1Only).processInbound(signed),
            "with the expiry opt-in off the pinned expired certificate must be rejected");

    assertEquals(WSSecurityException.ErrorCode.FAILED_CHECK, expiryRejection.getErrorCode());
    assertInstanceOf(CertificateExpiredException.class, expiryRejection.getCause());
    assertTrue(
        expiryRejection.getMessage().contains(INVALID_CERT_MESSAGE),
        "unexpected rejection message: " + expiryRejection.getMessage());
  }

  private static SecurityProfile chainProfile(String name) {
    return SecurityProfile.builder(name)
        .keystore(chainLeafStore.toString(), TestKeystoreHelper.KEYSTORE_PASSWORD)
        .truststore(caTruststore.toString(), TestKeystoreHelper.KEYSTORE_PASSWORD)
        .actionsIn("Signature")
        .allowExpiredPinnedCertificate(true)
        .build();
  }

  private static SecurityProfile sha1Profile(String name, boolean allowSha1, boolean allowExpired) {
    SecurityProfile.Builder builder =
        SecurityProfile.builder(name)
            .keystore(sha1Store.toString(), TestKeystoreHelper.KEYSTORE_PASSWORD)
            .truststore(sha1Store.toString(), TestKeystoreHelper.KEYSTORE_PASSWORD)
            .actionsIn("Signature");
    if (allowSha1) {
      builder.allowedSignatureAlgorithms(RSA_SHA1).allowedDigestAlgorithms(SHA1);
    }
    if (allowExpired) {
      builder.allowExpiredPinnedCertificate(true);
    }
    return builder.build();
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
