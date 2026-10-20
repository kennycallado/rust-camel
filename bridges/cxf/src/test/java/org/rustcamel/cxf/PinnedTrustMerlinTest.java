package org.rustcamel.cxf;

import static org.junit.jupiter.api.Assertions.assertDoesNotThrow;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertInstanceOf;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.io.InputStream;
import java.lang.reflect.Method;
import java.nio.file.Files;
import java.nio.file.Path;
import java.security.KeyStore;
import java.security.cert.CertificateExpiredException;
import java.security.cert.CertificateNotYetValidException;
import java.security.cert.X509Certificate;
import java.util.Arrays;
import java.util.Collections;
import java.util.Properties;
import org.apache.wss4j.common.crypto.Merlin;
import org.apache.wss4j.common.crypto.WSProviderConfig;
import org.apache.wss4j.common.ext.WSSecurityException;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

/** Direct-Crypto rejection matrix for the per-profile expired-pinned-certificate opt-in. */
class PinnedTrustMerlinTest {

  private static final String INVALID_CERT_MESSAGE = "The provided certificate is invalid";

  @TempDir static Path tempDir;

  private static Path validStore;
  private static Path expiredStore;
  private static Path futureStore;
  private static Path expiredTruststore;
  private static Path sameDnStore;
  private static Path chainLeafStore;
  private static Path caTruststore;

  @BeforeAll
  static void setUpStores() throws Exception {
    // Direct Crypto use bypasses the CXF/WSS4J engine bootstrap that installs the WSS4J message
    // bundle. Install only the bundles; JCE provider registration stays disabled.
    WSProviderConfig.init(false, false, false);
    validStore = TestKeystoreHelper.createTestKeystore();
    expiredStore = TestKeystoreHelper.createExpiredTestKeystore();
    futureStore = TestKeystoreHelper.createFutureTestKeystore();
    chainLeafStore = TestKeystoreHelper.createExpiredChainKeystore(tempDir);
    expiredTruststore =
        TestKeystoreHelper.createTruststoreWithCertificate(
            expiredStore, TestKeystoreHelper.KEY_ALIAS);
    sameDnStore = TestKeystoreHelper.createExpiredSameDnDifferentKeyKeystore();
    caTruststore = TestKeystoreHelper.createTruststoreWithCertificate(chainLeafStore, "ca");
  }

  @Test
  void expiredPinnedCertAcceptedWhenPinnedInKeystore() throws Exception {
    KeyStore keystore = load(expiredStore);
    X509Certificate expired = certificate(keystore, TestKeystoreHelper.KEY_ALIAS);

    PinnedTrustMerlin merlin = merlin(true, keystore, null);

    assertDoesNotThrow(
        () -> merlin.verifyTrust(new X509Certificate[] {expired}, false, Collections.emptyList()));
  }

  @Test
  void expiredPinnedCertAcceptedWhenPinnedInTruststore() throws Exception {
    KeyStore keystore = load(validStore);
    KeyStore truststore = load(expiredTruststore);
    X509Certificate expired = certificate(truststore, "pinned");

    PinnedTrustMerlin merlin = merlin(true, keystore, truststore);

    assertDoesNotThrow(
        () -> merlin.verifyTrust(new X509Certificate[] {expired}, false, Collections.emptyList()));
  }

  @Test
  void notYetValidCertRejectedByMerlin() throws Exception {
    KeyStore keystore = load(futureStore);
    X509Certificate future = certificate(keystore, TestKeystoreHelper.KEY_ALIAS);

    PinnedTrustMerlin merlin = merlin(true, keystore, null);

    WSSecurityException ex =
        assertThrows(
            WSSecurityException.class,
            () ->
                merlin.verifyTrust(new X509Certificate[] {future}, false, Collections.emptyList()));

    assertEquals(WSSecurityException.ErrorCode.FAILED_CHECK, ex.getErrorCode());
    assertInstanceOf(CertificateNotYetValidException.class, ex.getCause());
    assertTrue(
        ex.getMessage().contains(INVALID_CERT_MESSAGE),
        "unexpected rejection message: " + ex.getMessage());
  }

  @Test
  void chainWithExpiredLeafRejectedByMerlin() throws Exception {
    X509Certificate[] chain = TestKeystoreHelper.certificateChainFrom(chainLeafStore, "leaf");
    KeyStore truststore = load(caTruststore);

    PinnedTrustMerlin merlin = merlin(true, null, truststore);

    assertThrows(
        WSSecurityException.class, () -> merlin.verifyTrust(chain, false, Collections.emptyList()));
  }

  @Test
  void pinnedIssuerDoesNotAuthorizeExpiredLeafByMerlin() throws Exception {
    X509Certificate expiredLeaf =
        TestKeystoreHelper.certificateChainFrom(chainLeafStore, "leaf")[0];
    KeyStore truststore = load(caTruststore);

    PinnedTrustMerlin merlin = merlin(true, null, truststore);

    assertThrows(
        WSSecurityException.class,
        () ->
            merlin.verifyTrust(
                new X509Certificate[] {expiredLeaf}, false, Collections.emptyList()));
  }

  @Test
  void revocation_enabled_disables_the_opt_in() throws Exception {
    KeyStore keystore = load(expiredStore);
    X509Certificate expired = certificate(keystore, TestKeystoreHelper.KEY_ALIAS);

    PinnedTrustMerlin merlin = merlin(true, keystore, null);

    assertThrows(
        WSSecurityException.class,
        () -> merlin.verifyTrust(new X509Certificate[] {expired}, true, Collections.emptyList()));
    assertDoesNotThrow(
        () -> merlin.verifyTrust(new X509Certificate[] {expired}, false, Collections.emptyList()));
  }

  @Test
  void unrelated_failure_is_rethrown_unchanged() throws Exception {
    KeyStore keystore = load(validStore);
    KeyStore truststore = load(caTruststore);
    X509Certificate valid = certificate(keystore, TestKeystoreHelper.KEY_ALIAS);

    PinnedTrustMerlin pinned = merlin(true, null, truststore);
    Merlin stock = new Merlin();
    stock.setTrustStore(truststore);

    WSSecurityException pinnedFailure =
        assertThrows(
            WSSecurityException.class,
            () ->
                pinned.verifyTrust(new X509Certificate[] {valid}, false, Collections.emptyList()));
    WSSecurityException stockFailure =
        assertThrows(
            WSSecurityException.class,
            () ->
                stock.verifyTrust(
                    new X509Certificate[] {valid},
                    false,
                    Collections.emptyList(),
                    Collections.emptyList()));

    assertEquals(stockFailure.getErrorCode(), pinnedFailure.getErrorCode());
    assertEquals(stockFailure.getMessage(), pinnedFailure.getMessage());
  }

  @Test
  void acceptancePredicate_acceptsSinglePinnedExpiredCertificate() throws Exception {
    KeyStore keystore = load(expiredStore);
    X509Certificate pinned = certificate(keystore, TestKeystoreHelper.KEY_ALIAS);

    assertTrue(
        acceptsExpiredPinned(
            merlin(true, keystore, null), expiredFailure(), new X509Certificate[] {pinned}, false),
        "positive control: the sole pinned certificate plus a direct expiry cause must be accepted");
  }

  @Test
  void acceptancePredicate_rejectsDifferentDer() throws Exception {
    KeyStore keystore = load(expiredStore);
    X509Certificate pinned = certificate(keystore, TestKeystoreHelper.KEY_ALIAS);
    X509Certificate sameDn = certificate(load(sameDnStore), TestKeystoreHelper.KEY_ALIAS);

    assertEquals(pinned.getSubjectX500Principal(), sameDn.getSubjectX500Principal());
    assertFalse(
        Arrays.equals(pinned.getEncoded(), sameDn.getEncoded()),
        "the negative certificate must have a different DER while keeping the same subject DN");

    PinnedTrustMerlin merlin = merlin(true, keystore, null);
    assertTrue(
        acceptsExpiredPinned(merlin, expiredFailure(), new X509Certificate[] {pinned}, false),
        "positive control: the exact pinned certificate is accepted");
    assertFalse(
        acceptsExpiredPinned(merlin, expiredFailure(), new X509Certificate[] {sameDn}, false),
        "a same-DN certificate with different DER must not be treated as pinned");
  }

  @Test
  void acceptancePredicate_rejectsMultipleCertificates() throws Exception {
    KeyStore keystore = load(expiredStore);
    X509Certificate pinned = certificate(keystore, TestKeystoreHelper.KEY_ALIAS);
    X509Certificate ca = certificate(load(chainLeafStore), "ca");

    PinnedTrustMerlin merlin = merlin(true, keystore, null);
    assertTrue(
        acceptsExpiredPinned(merlin, expiredFailure(), new X509Certificate[] {pinned}, false),
        "positive control: the single pinned certificate is accepted");
    assertFalse(
        acceptsExpiredPinned(merlin, expiredFailure(), new X509Certificate[] {pinned, ca}, false),
        "the opt-in must cover a single presented certificate only, even when certs[0] is pinned");
  }

  @Test
  void acceptancePredicate_rejectsRevocationEnabled() throws Exception {
    KeyStore keystore = load(expiredStore);
    X509Certificate pinned = certificate(keystore, TestKeystoreHelper.KEY_ALIAS);

    PinnedTrustMerlin merlin = merlin(true, keystore, null);
    assertTrue(
        acceptsExpiredPinned(merlin, expiredFailure(), new X509Certificate[] {pinned}, false),
        "positive control: revocation disabled accepts the pinned certificate");
    assertFalse(
        acceptsExpiredPinned(merlin, expiredFailure(), new X509Certificate[] {pinned}, true),
        "enabling revocation must disable the expiry opt-in for the same certificate");
  }

  @Test
  void acceptancePredicate_rejectsOptInDisabled() throws Exception {
    KeyStore keystore = load(expiredStore);
    X509Certificate pinned = certificate(keystore, TestKeystoreHelper.KEY_ALIAS);

    assertTrue(
        acceptsExpiredPinned(
            merlin(true, keystore, null), expiredFailure(), new X509Certificate[] {pinned}, false),
        "positive control: the same certificate is accepted when the opt-in is enabled");
    assertFalse(
        acceptsExpiredPinned(
            merlin(false, keystore, null), expiredFailure(), new X509Certificate[] {pinned}, false),
        "the opt-in must remain disabled unless explicitly enabled");
  }

  @Test
  void acceptancePredicate_rejectsNonExpiryCause() throws Exception {
    KeyStore keystore = load(expiredStore);
    X509Certificate pinned = certificate(keystore, TestKeystoreHelper.KEY_ALIAS);
    WSSecurityException notYetValid =
        new WSSecurityException(
            WSSecurityException.ErrorCode.FAILED_CHECK,
            new CertificateNotYetValidException("controlled not-yet-valid"));

    PinnedTrustMerlin merlin = merlin(true, keystore, null);
    assertTrue(
        acceptsExpiredPinned(merlin, expiredFailure(), new X509Certificate[] {pinned}, false),
        "positive control: a direct expiry cause is accepted");
    assertFalse(
        acceptsExpiredPinned(merlin, notYetValid, new X509Certificate[] {pinned}, false),
        "only CertificateExpiredException may relax trust");
  }

  @Test
  void acceptancePredicate_rejectsNestedExpiryCause() throws Exception {
    KeyStore keystore = load(expiredStore);
    X509Certificate pinned = certificate(keystore, TestKeystoreHelper.KEY_ALIAS);
    WSSecurityException nestedExpiry =
        new WSSecurityException(
            WSSecurityException.ErrorCode.FAILED_CHECK,
            new IllegalStateException(
                "wrapped", new CertificateExpiredException("nested controlled expiry")));

    PinnedTrustMerlin merlin = merlin(true, keystore, null);
    assertTrue(
        acceptsExpiredPinned(merlin, expiredFailure(), new X509Certificate[] {pinned}, false),
        "positive control: a direct expiry cause is accepted");
    assertFalse(
        acceptsExpiredPinned(merlin, nestedExpiry, new X509Certificate[] {pinned}, false),
        "an expiry cause nested below the direct cause must not relax trust");
  }

  private static WSSecurityException expiredFailure() {
    return new WSSecurityException(
        WSSecurityException.ErrorCode.FAILED_CHECK,
        new CertificateExpiredException("controlled expiry"));
  }

  private static boolean acceptsExpiredPinned(
      PinnedTrustMerlin merlin,
      WSSecurityException failure,
      X509Certificate[] certs,
      boolean enableRevocation)
      throws Exception {
    Method predicate =
        PinnedTrustMerlin.class.getDeclaredMethod(
            "acceptsExpiredPinnedCertificate",
            WSSecurityException.class,
            X509Certificate[].class,
            boolean.class);
    predicate.setAccessible(true);
    return (boolean) predicate.invoke(merlin, failure, certs, enableRevocation);
  }

  private static PinnedTrustMerlin merlin(
      boolean allowExpired, KeyStore keystore, KeyStore truststore) throws Exception {
    PinnedTrustMerlin merlin = new PinnedTrustMerlin(new Properties(), allowExpired);
    merlin.setKeyStore(keystore);
    merlin.setTrustStore(truststore);
    return merlin;
  }

  private static KeyStore load(Path path) throws Exception {
    KeyStore store = KeyStore.getInstance("JKS");
    try (InputStream in = Files.newInputStream(path)) {
      store.load(in, TestKeystoreHelper.KEYSTORE_PASSWORD.toCharArray());
    }
    return store;
  }

  private static X509Certificate certificate(KeyStore store, String alias) throws Exception {
    return (X509Certificate) store.getCertificate(alias);
  }
}
