package org.rustcamel.jms;

import java.io.ByteArrayInputStream;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.security.GeneralSecurityException;
import java.security.KeyFactory;
import java.security.KeyStore;
import java.security.NoSuchAlgorithmException;
import java.security.PrivateKey;
import java.security.SecureRandom;
import java.security.cert.Certificate;
import java.security.cert.CertificateFactory;
import java.security.spec.InvalidKeySpecException;
import java.security.spec.PKCS8EncodedKeySpec;
import java.util.ArrayList;
import java.util.Base64;
import java.util.List;
import javax.net.ssl.KeyManager;
import javax.net.ssl.KeyManagerFactory;
import javax.net.ssl.TrustManager;
import javax.net.ssl.TrustManagerFactory;

/**
 * Run-time construction of broker TLS key/trust managers from PEM material (task 5.1).
 *
 * <p>Every builder runs at call time and returns fresh arrays — nothing is cached in static state,
 * so GraalVM native image never folds TLS material into the image heap. File reads use {@link
 * Files#readAllBytes} and the bytes are never logged or embedded in exception messages; failures
 * name only the file path and the format requirement.
 *
 * <p>Expected material:
 *
 * <ul>
 *   <li>CA bundle: PEM {@code CERTIFICATE} blocks (one or many) — {@link #buildTrustManagers}.
 *   <li>Client identity: PEM {@code CERTIFICATE} chain plus an <em>unencrypted</em> PKCS#8 key
 *       ({@code -----BEGIN PRIVATE KEY-----}) — {@link #buildKeyManagers}. Encrypted PKCS#8 ({@code
 *       -----BEGIN ENCRYPTED PRIVATE KEY-----}) and legacy PKCS#1 armors are rejected: the bridge
 *       contract is unencrypted PKCS#8 on disk, protected by filesystem permissions.
 * </ul>
 *
 * <p>The test fixtures under {@code src/test/resources/tls-fixtures} were generated once (task 5.1,
 * reused verbatim by task 7.1 — task 7.1 only re-issued {@code server-other-name} under a second CA
 * for the wrong-CA negative) with OpenSSL 3 as follows — the CA signing keys are throwaway and are
 * not committed:
 *
 * <pre>{@code
 * openssl req -x509 -newkey rsa:2048 -nodes -keyout ca.key -out test-ca.pem -days 3650 \
 *   -subj "/CN=rust-camel jms bridge test CA" \
 *   -addext "basicConstraints=critical,CA:TRUE" \
 *   -addext "keyUsage=critical,keyCertSign,cRLSign"
 *
 * # extfile per leaf, e.g. ext-dns:
 * #   basicConstraints=critical,CA:FALSE
 * #   keyUsage=critical,digitalSignature,keyEncipherment
 * #   extendedKeyUsage=serverAuth
 * #   subjectAltName=DNS:mq.internal.example
 * openssl req -newkey rsa:2048 -nodes -keyout dns.key -out dns.csr -subj "/CN=mq.internal.example"
 * openssl x509 -req -in dns.csr -CA test-ca.pem -CAkey ca.key -set_serial 0x1001 \
 *   -out server-dns-san.pem -days 3650 -extfile ext-dns
 * openssl pkcs8 -topk8 -nocrypt -in dns.key -out server-dns-san.key
 *
 * # server-ip-san: same with subjectAltName=IP:127.0.0.1, CN=127.0.0.1, serial 0x1002
 * # client-cert/client-key-pkcs8: extendedKeyUsage=clientAuth, serial 0x1004
 *
 * # server-other-name is signed by a SECOND CA so the wrong-CA negative has a
 * # hermetic "distrusted issuer" (client pins test-ca.pem against it):
 * openssl req -x509 -newkey rsa:2048 -nodes -keyout other-ca.key -out other-ca.pem -days 3650 \
 *   -subj "/CN=rust-camel jms bridge other test CA" \
 *   -addext "basicConstraints=critical,CA:TRUE" \
 *   -addext "keyUsage=critical,keyCertSign,cRLSign"
 * # ext-other: same leaf extensions with subjectAltName=DNS:other.example
 * openssl req -newkey rsa:2048 -nodes -keyout othername.key -out othername.csr -subj "/CN=other.example"
 * openssl x509 -req -in othername.csr -CA other-ca.pem -CAkey other-ca.key -set_serial 0x1003 \
 *   -out server-other-name.pem -days 3650 -extfile ext-other
 * openssl pkcs8 -topk8 -nocrypt -in othername.key -out server-other-name.key
 *
 * openssl pkcs8 -topk8 -in client.key -out client-key-encrypted.pem -passout pass:test-only
 * }</pre>
 */
public final class BrokerTlsMaterial {

  private static final String PKCS8_REQUIREMENT =
      "bridge TLS client key must be an unencrypted PKCS#8 PEM private key "
          + "(-----BEGIN PRIVATE KEY-----): ";
  private static final String THROWAWAY_ALIAS = "bridge-tls-throwaway";
  private static final char[] THROWAWAY_PASSWORD = "bridge-tls-throwaway".toCharArray();

  private BrokerTlsMaterial() {}

  /**
   * Trust managers pinned to the given PEM CA bundle. A {@code null} path yields the platform
   * default trust store ({@code TrustManagerFactory.init(null)}), never a pinned empty store.
   */
  public static TrustManager[] buildTrustManagers(String caPemPath) {
    if (caPemPath == null) {
      try {
        TrustManagerFactory platform = trustManagerFactory();
        platform.init((KeyStore) null);
        return platform.getTrustManagers();
      } catch (GeneralSecurityException e) {
        throw new IllegalStateException("cannot build platform default trust managers", e);
      }
    }
    List<PemBlock> blocks = parsePem(readPem(caPemPath, "bridge TLS CA bundle"));
    CertificateFactory x509 = certificateFactory();
    KeyStore store = newInMemoryKeyStore();
    int accepted = 0;
    try {
      for (PemBlock block : blocks) {
        if (!"CERTIFICATE".equals(block.type())) {
          continue;
        }
        store.setCertificateEntry(
            THROWAWAY_ALIAS + "-" + accepted, decodeCertificate(x509, block, caPemPath));
        accepted++;
      }
      if (accepted == 0) {
        throw new IllegalStateException(
            "bridge TLS CA bundle contains no X.509 CERTIFICATE PEM block: " + caPemPath);
      }
      TrustManagerFactory tmf = trustManagerFactory();
      tmf.init(store);
      return tmf.getTrustManagers();
    } catch (GeneralSecurityException e) {
      throw new IllegalStateException(
          "cannot build trust managers from PEM CA bundle: " + caPemPath, e);
    }
  }

  /**
   * Key managers for a PEM client certificate chain plus an unencrypted PKCS#8 private key. The
   * in-memory keystore uses a throwaway alias and password — the material lives only in the
   * returned managers.
   */
  public static KeyManager[] buildKeyManagers(String certPemPath, String keyPemPath) {
    CertificateFactory x509 = certificateFactory();
    List<Certificate> chain = new ArrayList<>();
    for (PemBlock block : parsePem(readPem(certPemPath, "bridge TLS client certificate"))) {
      if ("CERTIFICATE".equals(block.type())) {
        chain.add(decodeCertificate(x509, block, certPemPath));
      }
    }
    if (chain.isEmpty()) {
      throw new IllegalStateException(
          "bridge TLS client certificate contains no X.509 CERTIFICATE PEM block: " + certPemPath);
    }
    PrivateKey key = readPkcs8PrivateKey(keyPemPath);
    KeyStore store = newInMemoryKeyStore();
    try {
      store.setKeyEntry(
          THROWAWAY_ALIAS, key, THROWAWAY_PASSWORD, chain.toArray(new Certificate[0]));
      KeyManagerFactory kmf = keyManagerFactory();
      kmf.init(store, THROWAWAY_PASSWORD);
      return kmf.getKeyManagers();
    } catch (GeneralSecurityException e) {
      throw new IllegalStateException(
          "cannot build key managers from PEM client key: " + keyPemPath, e);
    }
  }

  /** Fresh {@link SecureRandom} per factory build; never shared or cached across factories. */
  public static SecureRandom newSecureRandom() {
    return new SecureRandom();
  }

  /**
   * Unencrypted PKCS#8 key parse: RSA first, falling back to EC when RSA construction throws {@link
   * InvalidKeySpecException}. An {@code ENCRYPTED PRIVATE KEY} armor (or anything that fails both
   * key factories) is rejected with a message naming the PKCS#8 requirement — never key contents.
   */
  private static PrivateKey readPkcs8PrivateKey(String keyPemPath) {
    List<PemBlock> blocks = parsePem(readPem(keyPemPath, "bridge TLS client key"));
    for (PemBlock block : blocks) {
      if (!block.type().contains("PRIVATE KEY")) {
        continue;
      }
      if ("ENCRYPTED PRIVATE KEY".equals(block.type())) {
        throw new IllegalStateException(PKCS8_REQUIREMENT + keyPemPath);
      }
      PKCS8EncodedKeySpec spec = new PKCS8EncodedKeySpec(block.der());
      try {
        try {
          return KeyFactory.getInstance("RSA").generatePrivate(spec);
        } catch (InvalidKeySpecException rsaFailure) {
          return KeyFactory.getInstance("EC").generatePrivate(spec);
        }
      } catch (InvalidKeySpecException | NoSuchAlgorithmException e) {
        throw new IllegalStateException(PKCS8_REQUIREMENT + keyPemPath, e);
      }
    }
    throw new IllegalStateException(PKCS8_REQUIREMENT + keyPemPath);
  }

  private static byte[] readPem(String path, String what) {
    try {
      return Files.readAllBytes(Path.of(path));
    } catch (IOException e) {
      throw new IllegalStateException("cannot read " + what + " PEM file: " + path, e);
    }
  }

  private static Certificate decodeCertificate(
      CertificateFactory x509, PemBlock block, String path) {
    try {
      return x509.generateCertificate(new ByteArrayInputStream(block.der()));
    } catch (GeneralSecurityException e) {
      throw new IllegalStateException("cannot parse X.509 CERTIFICATE PEM block in: " + path, e);
    }
  }

  private static TrustManagerFactory trustManagerFactory() {
    try {
      return TrustManagerFactory.getInstance(TrustManagerFactory.getDefaultAlgorithm());
    } catch (GeneralSecurityException e) {
      throw new IllegalStateException("no TrustManagerFactory for default algorithm", e);
    }
  }

  private static KeyManagerFactory keyManagerFactory() {
    try {
      return KeyManagerFactory.getInstance(KeyManagerFactory.getDefaultAlgorithm());
    } catch (GeneralSecurityException e) {
      throw new IllegalStateException("no KeyManagerFactory for default algorithm", e);
    }
  }

  private static KeyStore newInMemoryKeyStore() {
    try {
      KeyStore store = KeyStore.getInstance(KeyStore.getDefaultType());
      store.load(null, null);
      return store;
    } catch (GeneralSecurityException | IOException e) {
      throw new IllegalStateException("cannot create in-memory KeyStore", e);
    }
  }

  private static CertificateFactory certificateFactory() {
    try {
      return CertificateFactory.getInstance("X.509");
    } catch (GeneralSecurityException e) {
      throw new IllegalStateException("no X.509 CertificateFactory", e);
    }
  }

  /** One PEM armor block: the armor type and the base64-decoded DER payload. */
  private record PemBlock(String type, byte[] der) {}

  /**
   * Splits PEM armor blocks ({@code -----BEGIN <type>----- … -----END <type>-----}) and decodes
   * each payload. Non-armor text (including any file header) is ignored.
   */
  private static List<PemBlock> parsePem(byte[] content) {
    String text = new String(content, StandardCharsets.ISO_8859_1);
    List<PemBlock> blocks = new ArrayList<>();
    String[] lines = text.split("\r?\n");
    int i = 0;
    while (i < lines.length) {
      String line = lines[i].trim();
      if (!line.startsWith("-----BEGIN ") || !line.endsWith("-----")) {
        i++;
        continue;
      }
      String type = line.substring("-----BEGIN ".length(), line.length() - "-----".length()).trim();
      StringBuilder body = new StringBuilder();
      i++;
      boolean closed = false;
      while (i < lines.length) {
        String payload = lines[i].trim();
        i++;
        if (payload.startsWith("-----END ") && payload.endsWith("-----")) {
          closed = true;
          break;
        }
        body.append(payload);
      }
      if (!closed) {
        throw new IllegalStateException("unterminated PEM armor block BEGIN " + type);
      }
      blocks.add(new PemBlock(type, Base64.getMimeDecoder().decode(body.toString())));
    }
    return blocks;
  }
}
