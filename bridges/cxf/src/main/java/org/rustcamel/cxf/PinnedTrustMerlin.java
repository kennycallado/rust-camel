package org.rustcamel.cxf;

import java.io.IOException;
import java.security.KeyStore;
import java.security.KeyStoreException;
import java.security.cert.Certificate;
import java.security.cert.CertificateEncodingException;
import java.security.cert.CertificateExpiredException;
import java.security.cert.X509Certificate;
import java.util.Arrays;
import java.util.Collection;
import java.util.Enumeration;
import java.util.Properties;
import java.util.regex.Pattern;
import org.apache.wss4j.common.crypto.Merlin;
import org.apache.wss4j.common.ext.WSSecurityException;

/**
 * Merlin variant that accepts an expired certificate only when the per-profile opt-in is enabled
 * and the presented single certificate is byte-identical to an entry in the instance keystore or
 * truststore. Every other verification failure is re-thrown unchanged.
 */
final class PinnedTrustMerlin extends Merlin {

  private final boolean allowExpired;

  PinnedTrustMerlin(Properties cryptoProperties, boolean allowExpired) throws WSSecurityException {
    // Merlin's 3-arg constructor is exactly no-arg initialization plus loadProperties, but its
    // IOException cannot be caught around an explicit super(...) invocation. Delegating through
    // loadProperties keeps the same construction semantics and lets the IOException be mapped.
    super();
    this.allowExpired = allowExpired;
    try {
      loadProperties(cryptoProperties, PinnedTrustMerlin.class.getClassLoader(), null);
    } catch (IOException e) {
      throw new WSSecurityException(WSSecurityException.ErrorCode.FAILURE, e);
    }
  }

  @Override
  protected void verifyTrust(
      X509Certificate[] certs, boolean enableRevocation, Collection<Pattern> subjectCertConstraints)
      throws WSSecurityException {
    try {
      super.verifyTrust(certs, enableRevocation, subjectCertConstraints);
    } catch (WSSecurityException e) {
      if (!acceptsExpiredPinnedCertificate(e, certs, enableRevocation)) {
        throw e;
      }
    }
  }

  private boolean acceptsExpiredPinnedCertificate(
      WSSecurityException failure, X509Certificate[] certs, boolean enableRevocation) {
    if (!allowExpired || enableRevocation || certs == null || certs.length != 1) {
      return false;
    }
    if (!(failure.getCause() instanceof CertificateExpiredException)) {
      return false;
    }
    return isPinned(certs[0]);
  }

  private boolean isPinned(X509Certificate certificate) {
    try {
      return containsCertificate(getKeyStore(), certificate)
          || containsCertificate(getTrustStore(), certificate);
    } catch (KeyStoreException | CertificateEncodingException e) {
      // A store scan failure is not a reason to relax trust: fall through to the original
      // verification failure from Merlin.
      return false;
    }
  }

  private static boolean containsCertificate(KeyStore store, X509Certificate certificate)
      throws KeyStoreException, CertificateEncodingException {
    if (store == null) {
      return false;
    }
    Enumeration<String> aliases = store.aliases();
    while (aliases.hasMoreElements()) {
      Certificate entry = store.getCertificate(aliases.nextElement());
      if (entry != null && Arrays.equals(certificate.getEncoded(), entry.getEncoded())) {
        return true;
      }
    }
    return false;
  }
}
