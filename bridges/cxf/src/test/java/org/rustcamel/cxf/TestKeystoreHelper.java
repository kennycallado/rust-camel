package org.rustcamel.cxf;

import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.security.KeyStore;
import java.security.cert.Certificate;
import java.security.cert.X509Certificate;
import java.util.Base64;
import java.util.List;

/**
 * Generates a self-signed JKS keystore for integration testing using keytool. Avoids BouncyCastle
 * to ensure full JCA provider compatibility (no BadPaddingException when using RSA/OAEP key
 * transport with WSS4J).
 */
class TestKeystoreHelper {

  static final String KEYSTORE_PASSWORD = "changeit";
  static final String KEY_ALIAS = "alice";
  static final String EC_KEY_ALIAS = "ec";
  static final String DEFAULT_DNAME = "CN=test-alice, O=RustCamel Test";

  /**
   * Creates a temporary JKS keystore with a self-signed RSA 2048 keypair via keytool. The file is
   * marked for deletion on JVM exit. Keystore and key passwords are both {@link
   * #KEYSTORE_PASSWORD}.
   *
   * @return path to the generated keystore file
   */
  static Path createTestKeystore() throws Exception {
    return createTestKeystore(KEYSTORE_PASSWORD, KEYSTORE_PASSWORD);
  }

  /**
   * Creates a temporary JKS keystore with a self-signed RSA 2048 keypair via keytool. The file is
   * marked for deletion on JVM exit.
   *
   * @param storePassword keystore password ({@code -storepass})
   * @param keyPassword private-key password ({@code -keypass}), may differ from the store password
   * @return path to the generated keystore file
   */
  static Path createTestKeystore(String storePassword, String keyPassword) throws Exception {
    Path ksPath = Files.createTempFile("test-keystore-", ".jks");
    ksPath.toFile().deleteOnExit();
    Files.delete(ksPath); // keytool refuses to write to an existing (even empty) file
    return generateKeystore(ksPath, storePassword, keyPassword, DEFAULT_DNAME);
  }

  /**
   * Creates a JKS keystore with a self-signed RSA 2048 keypair whose subject DN is {@code dname},
   * at exactly the given path. Used by the fixture generator for descriptors whose DN must exceed
   * the Rust-side 4096-byte bound while the DER certificate stays well under 64 KiB.
   *
   * @param ksPath destination keystore file (deleted first; keytool refuses existing files)
   * @param storePassword keystore password ({@code -storepass})
   * @param keyPassword private-key password ({@code -keypass})
   * @param dname X.500 distinguished name for the generated self-signed keypair
   * @return {@code ksPath}
   */
  static Path createTestKeystoreAt(
      Path ksPath, String storePassword, String keyPassword, String dname) throws Exception {
    if (Files.exists(ksPath)) {
      Files.delete(ksPath);
    }
    return generateKeystore(ksPath, storePassword, keyPassword, dname);
  }

  /**
   * Creates a temporary JKS keystore with a self-signed EC P-256 keypair under alias {@link
   * #EC_KEY_ALIAS}. Used to prove non-default signature families (ecdsa-sha256) are rejected by the
   * default inbound allow-list and accepted only when explicitly allow-listed. Keystore and key
   * passwords are both {@link #KEYSTORE_PASSWORD}.
   *
   * @return path to the generated keystore file
   */
  static Path createTestEcKeystore() throws Exception {
    Path ksPath = Files.createTempFile("test-ec-keystore-", ".jks");
    ksPath.toFile().deleteOnExit();
    Files.delete(ksPath); // keytool refuses to write to an existing (even empty) file
    keytool(
        "-genkeypair",
        "-alias",
        EC_KEY_ALIAS,
        "-keyalg",
        "EC",
        "-groupname",
        "secp256r1",
        "-validity",
        "365",
        "-dname",
        DEFAULT_DNAME,
        "-keystore",
        ksPath.toAbsolutePath().toString(),
        "-storetype",
        "JKS",
        "-storepass",
        KEYSTORE_PASSWORD,
        "-keypass",
        KEYSTORE_PASSWORD,
        "-noprompt");
    return ksPath;
  }

  private static Path generateKeystore(
      Path ksPath, String storePassword, String keyPassword, String dname) throws Exception {
    // Generate the keystore using the standard JDK keytool (resolve via java.home for portability)
    String keytool = Path.of(System.getProperty("java.home"), "bin", "keytool").toString();
    List<String> cmd =
        List.of(
            keytool,
            "-genkeypair",
            "-alias",
            KEY_ALIAS,
            "-keyalg",
            "RSA",
            "-keysize",
            "2048",
            "-validity",
            "365",
            "-dname",
            dname,
            "-keystore",
            ksPath.toAbsolutePath().toString(),
            "-storetype",
            "JKS",
            "-storepass",
            storePassword,
            "-keypass",
            keyPassword,
            "-noprompt");

    ProcessBuilder pb = new ProcessBuilder(cmd);
    pb.redirectErrorStream(true);
    Process proc = pb.start();
    int exit = proc.waitFor();
    if (exit != 0) {
      String output = new String(proc.getInputStream().readAllBytes());
      throw new IllegalStateException("keytool failed (exit " + exit + "): " + output);
    }

    return ksPath;
  }

  // ── Expired-certificate trust fixtures ───────────────────────────────────

  /**
   * Creates a temporary JKS keystore with an RSA 2048 self-signed keypair under {@link #KEY_ALIAS}
   * whose validity window ended 370 days ago. Keystore and key passwords are both {@link
   * #KEYSTORE_PASSWORD}.
   *
   * @return path to the generated keystore file
   */
  static Path createExpiredTestKeystore() throws Exception {
    Path ksPath = createTempKeystore("test-expired-keystore-");
    generateKeystore(ksPath, DEFAULT_DNAME, "-400d", "30", null);
    return ksPath;
  }

  /**
   * Creates a temporary JKS keystore with an RSA 2048 self-signed keypair under {@link #KEY_ALIAS}
   * whose validity window starts 10 days from now.
   *
   * @return path to the generated keystore file
   */
  static Path createFutureTestKeystore() throws Exception {
    Path ksPath = createTempKeystore("test-future-keystore-");
    generateKeystore(ksPath, DEFAULT_DNAME, "+10d", "365", null);
    return ksPath;
  }

  /**
   * Creates a temporary JKS keystore with a fresh RSA 2048 keypair whose subject is exactly {@link
   * #DEFAULT_DNAME} and whose certificate is expired. The key material differs from every other
   * generated store.
   *
   * @return path to the generated keystore file
   */
  static Path createExpiredSameDnDifferentKeyKeystore() throws Exception {
    Path ksPath = createTempKeystore("test-expired-same-dn-keystore-");
    generateKeystore(ksPath, DEFAULT_DNAME, "-400d", "30", null);
    return ksPath;
  }

  /**
   * Creates a temporary JKS keystore with an RSA 2048 self-signed keypair signed with SHA1withRSA
   * whose validity window ended 370 days ago.
   *
   * @return path to the generated keystore file
   */
  static Path createExpiredSha1TestKeystore() throws Exception {
    Path ksPath = createTempKeystore("test-expired-sha1-keystore-");
    generateKeystore(ksPath, DEFAULT_DNAME, "-400d", "30", "SHA1withRSA");
    return ksPath;
  }

  /**
   * Creates a two-certificate key entry under alias {@code leaf} in a JKS keystore inside {@code
   * dir}: a CA certificate under alias {@code ca} (basic-constraints CA) and an expired RSA 2048
   * leaf signed by that CA. The stored chain is leaf then CA.
   *
   * @param dir directory that receives the CA store, leaf store, CSR, and certificates
   * @return path to the leaf keystore
   */
  static Path createExpiredChainKeystore(Path dir) throws Exception {
    Files.createDirectories(dir);
    Path caStore = dir.resolve("expired-chain-ca.jks");
    Path leafStore = dir.resolve("expired-chain-leaf.jks");
    Files.deleteIfExists(caStore);
    Files.deleteIfExists(leafStore);

    keytool(
        "-genkeypair",
        "-alias",
        "ca",
        "-keyalg",
        "RSA",
        "-keysize",
        "2048",
        "-validity",
        "3650",
        "-dname",
        "CN=cxf-expired-chain-ca, O=RustCamel Test",
        "-ext",
        "bc:c",
        "-keystore",
        caStore.toAbsolutePath().toString(),
        "-storetype",
        "JKS",
        "-storepass",
        KEYSTORE_PASSWORD,
        "-keypass",
        KEYSTORE_PASSWORD,
        "-noprompt");

    keytool(
        "-genkeypair",
        "-alias",
        "leaf",
        "-keyalg",
        "RSA",
        "-keysize",
        "2048",
        "-startdate",
        "-400d",
        "-validity",
        "30",
        "-dname",
        DEFAULT_DNAME,
        "-keystore",
        leafStore.toAbsolutePath().toString(),
        "-storetype",
        "JKS",
        "-storepass",
        KEYSTORE_PASSWORD,
        "-keypass",
        KEYSTORE_PASSWORD,
        "-noprompt");

    Path caCert = dir.resolve("expired-chain-ca.cer");
    keytool(
        "-exportcert",
        "-alias",
        "ca",
        "-file",
        caCert.toString(),
        "-keystore",
        caStore.toString(),
        "-storepass",
        KEYSTORE_PASSWORD,
        "-noprompt");

    // The CA must be a trusted cert in the leaf store before the CA-signed reply is imported:
    // keytool builds the key entry chain of that reply against the store's trusted certs.
    keytool(
        "-importcert",
        "-alias",
        "ca",
        "-file",
        caCert.toString(),
        "-keystore",
        leafStore.toString(),
        "-storepass",
        KEYSTORE_PASSWORD,
        "-noprompt");

    Path csr = dir.resolve("expired-chain-leaf.csr");
    keytool(
        "-certreq",
        "-alias",
        "leaf",
        "-file",
        csr.toString(),
        "-keystore",
        leafStore.toString(),
        "-storepass",
        KEYSTORE_PASSWORD,
        "-keypass",
        KEYSTORE_PASSWORD);

    Path leafCert = dir.resolve("expired-chain-leaf.cer");
    keytool(
        "-gencert",
        "-alias",
        "ca",
        "-keystore",
        caStore.toString(),
        "-storepass",
        KEYSTORE_PASSWORD,
        "-keypass",
        KEYSTORE_PASSWORD,
        "-infile",
        csr.toString(),
        "-outfile",
        leafCert.toString(),
        "-startdate",
        "-400d",
        "-validity",
        "30");

    keytool(
        "-importcert",
        "-alias",
        "leaf",
        "-file",
        leafCert.toString(),
        "-keystore",
        leafStore.toString(),
        "-storepass",
        KEYSTORE_PASSWORD,
        "-keypass",
        KEYSTORE_PASSWORD,
        "-trustcacerts",
        "-noprompt");

    return leafStore;
  }

  /**
   * Creates a fresh JKS truststore containing the certificate exported from {@code sourceKeystore}
   * under {@code sourceAlias}, imported as the trusted entry {@code pinned}.
   *
   * @param sourceKeystore keystore holding the certificate to pin
   * @param sourceAlias alias of the certificate in the source keystore
   * @return path to the generated truststore file
   */
  static Path createTruststoreWithCertificate(Path sourceKeystore, String sourceAlias)
      throws Exception {
    Path truststore = createTempKeystore("test-pinned-truststore-");
    Path certFile = Files.createTempFile("test-pinned-cert-", ".cer");
    certFile.toFile().deleteOnExit();
    try {
      keytool(
          "-exportcert",
          "-alias",
          sourceAlias,
          "-file",
          certFile.toString(),
          "-keystore",
          sourceKeystore.toString(),
          "-storepass",
          KEYSTORE_PASSWORD,
          "-noprompt");
      keytool(
          "-importcert",
          "-alias",
          "pinned",
          "-file",
          certFile.toString(),
          "-keystore",
          truststore.toString(),
          "-storetype",
          "JKS",
          "-storepass",
          KEYSTORE_PASSWORD,
          "-noprompt");
    } finally {
      Files.deleteIfExists(certFile);
    }
    return truststore;
  }

  /** Loads the X.509 certificate under {@code alias} from a JKS store. */
  static X509Certificate certificateFrom(Path keystore, String alias) throws Exception {
    return (X509Certificate) loadJks(keystore, KEYSTORE_PASSWORD).getCertificate(alias);
  }

  /** Loads the X.509 key entry chain under {@code alias} from a JKS store. */
  static X509Certificate[] certificateChainFrom(Path keystore, String alias) throws Exception {
    Certificate[] chain = loadJks(keystore, KEYSTORE_PASSWORD).getCertificateChain(alias);
    X509Certificate[] certificates = new X509Certificate[chain.length];
    for (int i = 0; i < chain.length; i++) {
      certificates[i] = (X509Certificate) chain[i];
    }
    return certificates;
  }

  private static Path createTempKeystore(String prefix) throws Exception {
    Path ksPath = Files.createTempFile(prefix, ".jks");
    ksPath.toFile().deleteOnExit();
    Files.delete(ksPath); // keytool refuses to write to an existing (even empty) file
    return ksPath;
  }

  private static void generateKeystore(
      Path ksPath, String dname, String startDate, String validity, String sigAlg)
      throws Exception {
    List<String> cmd = new java.util.ArrayList<>();
    cmd.add("-genkeypair");
    cmd.add("-alias");
    cmd.add(KEY_ALIAS);
    cmd.add("-keyalg");
    cmd.add("RSA");
    cmd.add("-keysize");
    cmd.add("2048");
    cmd.add("-startdate");
    cmd.add(startDate);
    cmd.add("-validity");
    cmd.add(validity);
    cmd.add("-dname");
    cmd.add(dname);
    if (sigAlg != null) {
      cmd.add("-sigalg");
      cmd.add(sigAlg);
    }
    cmd.add("-keystore");
    cmd.add(ksPath.toAbsolutePath().toString());
    cmd.add("-storetype");
    cmd.add("JKS");
    cmd.add("-storepass");
    cmd.add(KEYSTORE_PASSWORD);
    cmd.add("-keypass");
    cmd.add(KEYSTORE_PASSWORD);
    cmd.add("-noprompt");
    keytool(cmd.toArray(new String[0]));
  }

  // ── Listener TLS material (Task 2.7) ─────────────────────────────────────
  //
  // Benign test-only material: ephemeral CA + leaves generated by the JDK keytool into a temp
  // directory and extracted as PEM via the JDK Base64/PKCS#8 APIs. No secrets, no real private
  // keys — everything is regenerated per test run and deleted with the temp directory.

  /** PEM/PKCS#12 listener TLS material: one server identity plus the mutual-TLS client CA. */
  record ListenerTlsMaterial(
      Path certPem,
      Path keyPem,
      Path clientCaPem,
      Path pkcs12Path,
      Path clientJksPath,
      String password) {}

  /**
   * Generates an ephemeral listener TLS set under {@code dir}: a CA, a server leaf signed by it
   * (SAN {@code ip:127.0.0.1,dns:localhost}), and a client leaf signed by it. Returns the server
   * certificate/key as PEM files (the {@code soap_tls_cert_path}/{@code soap_tls_key_path} form),
   * the CA as {@code soap_tls_client_ca_path} PEM, the same server identity as PKCS#12 (the {@code
   * soap_tls_pkcs12_path} form), and a client-identity JKS for tests that complete the mutual-TLS
   * handshake.
   */
  static ListenerTlsMaterial createListenerTlsMaterial(Path dir) throws Exception {
    Files.createDirectories(dir);
    Path caStore = dir.resolve("ca.jks");
    Path serverStore = dir.resolve("server.jks");
    Path clientStore = dir.resolve("client.jks");
    String password = KEYSTORE_PASSWORD;

    // CA (basic constraints CA:true) and the two leaves in their own stores.
    keytool(
        "-genkeypair",
        "-alias",
        "ca",
        "-keyalg",
        "RSA",
        "-keysize",
        "2048",
        "-validity",
        "365",
        "-dname",
        "CN=cxf-listener-test-ca",
        "-ext",
        "bc:c",
        "-keystore",
        caStore.toString(),
        "-storetype",
        "JKS",
        "-storepass",
        password,
        "-keypass",
        password,
        "-noprompt");
    keytool(
        "-genkeypair",
        "-alias",
        "server",
        "-keyalg",
        "RSA",
        "-keysize",
        "2048",
        "-validity",
        "365",
        "-dname",
        "CN=localhost",
        "-keystore",
        serverStore.toString(),
        "-storetype",
        "JKS",
        "-storepass",
        password,
        "-keypass",
        password,
        "-noprompt");
    keytool(
        "-genkeypair",
        "-alias",
        "client",
        "-keyalg",
        "RSA",
        "-keysize",
        "2048",
        "-validity",
        "365",
        "-dname",
        "CN=cxf-test-client",
        "-keystore",
        clientStore.toString(),
        "-storetype",
        "JKS",
        "-storepass",
        password,
        "-keypass",
        password,
        "-noprompt");
    Path caCert = dir.resolve("ca.cer");
    keytool(
        "-exportcert",
        "-alias",
        "ca",
        "-file",
        caCert.toString(),
        "-keystore",
        caStore.toString(),
        "-storepass",
        password,
        "-noprompt");
    // The CA must be a TRUSTED cert in each leaf store BEFORE the CA-signed reply is
    // imported: keytool builds the chain of the reply against the store's trusted certs.
    for (Path leafStore : List.of(serverStore, clientStore)) {
      keytool(
          "-importcert",
          "-alias",
          "ca",
          "-file",
          caCert.toString(),
          "-keystore",
          leafStore.toString(),
          "-storepass",
          password,
          "-noprompt");
    }
    signLeafWithCa(dir, caStore, serverStore, "server", "SAN=ip:127.0.0.1,dns:localhost", password);
    signLeafWithCa(dir, caStore, clientStore, "client", "eku=clientAuth", password);

    Path pkcs12 = dir.resolve("server.p12");
    keytool(
        "-importkeystore",
        "-srckeystore",
        serverStore.toString(),
        "-srcstorepass",
        password,
        "-srcalias",
        "server",
        "-destkeystore",
        pkcs12.toString(),
        "-deststoretype",
        "PKCS12",
        "-deststorepass",
        password,
        "-destkeypass",
        password,
        "-noprompt");

    KeyStore loaded = loadJks(serverStore, password);
    Path certPem = dir.resolve("server.pem");
    Path keyPem = dir.resolve("server-key.pem");
    Path clientCaPem = dir.resolve("client-ca.pem");
    Files.writeString(certPem, pem("CERTIFICATE", loaded.getCertificate("server").getEncoded()));
    Files.writeString(
        keyPem, pem("PRIVATE KEY", loaded.getKey("server", password.toCharArray()).getEncoded()));
    Files.writeString(
        clientCaPem,
        pem("CERTIFICATE", loadJks(caStore, password).getCertificate("ca").getEncoded()));
    return new ListenerTlsMaterial(certPem, keyPem, clientCaPem, pkcs12, clientStore, password);
  }

  /** A client {@link SSLContext} trusting {@code caPem} and presenting the client JKS identity. */
  static javax.net.ssl.SSLContext clientSslContext(Path caPem, Path clientJks, String password)
      throws Exception {
    KeyStore trust = KeyStore.getInstance(KeyStore.getDefaultType());
    trust.load(null, null);
    try (var in = Files.newInputStream(caPem)) {
      CertificateFactoryHolder.addCa(trust, in);
    }
    javax.net.ssl.TrustManagerFactory tmf =
        javax.net.ssl.TrustManagerFactory.getInstance(
            javax.net.ssl.TrustManagerFactory.getDefaultAlgorithm());
    tmf.init(trust);
    KeyStore identity = loadJks(clientJks, password);
    javax.net.ssl.KeyManagerFactory kmf =
        javax.net.ssl.KeyManagerFactory.getInstance(
            javax.net.ssl.KeyManagerFactory.getDefaultAlgorithm());
    kmf.init(identity, password.toCharArray());
    javax.net.ssl.SSLContext ctx = javax.net.ssl.SSLContext.getInstance("TLS");
    ctx.init(kmf.getKeyManagers(), tmf.getTrustManagers(), null);
    return ctx;
  }

  /** Trust-only {@link SSLContext}: trusts {@code caPem}, presents NO client identity. */
  static javax.net.ssl.SSLContext trustOnlySslContext(Path caPem) throws Exception {
    KeyStore trust = KeyStore.getInstance(KeyStore.getDefaultType());
    trust.load(null, null);
    try (var in = Files.newInputStream(caPem)) {
      CertificateFactoryHolder.addCa(trust, in);
    }
    javax.net.ssl.TrustManagerFactory tmf =
        javax.net.ssl.TrustManagerFactory.getInstance(
            javax.net.ssl.TrustManagerFactory.getDefaultAlgorithm());
    tmf.init(trust);
    javax.net.ssl.SSLContext ctx = javax.net.ssl.SSLContext.getInstance("TLS");
    ctx.init(null, tmf.getTrustManagers(), null);
    return ctx;
  }

  /** Loads a JKS keystore from {@code path} with {@code password}. */
  private static KeyStore loadJks(Path path, String password) throws Exception {
    KeyStore store = KeyStore.getInstance("JKS");
    try (var in = Files.newInputStream(path)) {
      store.load(in, password.toCharArray());
    }
    return store;
  }

  /** CSR + CA signature for one leaf, imported back into its store (replacing the self-signed). */
  private static void signLeafWithCa(
      Path dir, Path caStore, Path leafStore, String alias, String ext, String password)
      throws Exception {
    Path csr = dir.resolve(alias + ".csr");
    Path cer = dir.resolve(alias + ".cer");
    keytool(
        "-certreq",
        "-alias",
        alias,
        "-file",
        csr.toString(),
        "-keystore",
        leafStore.toString(),
        "-storepass",
        password,
        "-keypass",
        password);
    keytool(
        "-gencert",
        "-alias",
        "ca",
        "-keystore",
        caStore.toString(),
        "-storepass",
        password,
        "-keypass",
        password,
        "-infile",
        csr.toString(),
        "-outfile",
        cer.toString(),
        "-validity",
        "365",
        "-ext",
        ext);
    keytool(
        "-importcert",
        "-alias",
        alias,
        "-file",
        cer.toString(),
        "-keystore",
        leafStore.toString(),
        "-storepass",
        password,
        "-keypass",
        password,
        "-noprompt");
  }

  private static void keytool(String... args) throws Exception {
    String keytool = Path.of(System.getProperty("java.home"), "bin", "keytool").toString();
    List<String> cmd = new java.util.ArrayList<>();
    cmd.add(keytool);
    cmd.addAll(List.of(args));
    ProcessBuilder pb = new ProcessBuilder(cmd);
    pb.redirectErrorStream(true);
    Process proc = pb.start();
    int exit = proc.waitFor();
    if (exit != 0) {
      String output = new String(proc.getInputStream().readAllBytes(), StandardCharsets.UTF_8);
      throw new IllegalStateException("keytool failed (exit " + exit + "): " + output);
    }
  }

  /** PEM-encodes one DER blob with JDK Base64 (64-column MIME). */
  private static String pem(String type, byte[] der) {
    String body =
        Base64.getMimeEncoder(64, "\n".getBytes(StandardCharsets.US_ASCII)).encodeToString(der);
    return "-----BEGIN " + type + "-----\n" + body + "\n-----END " + type + "-----\n";
  }

  /** Small holder so the CA PEM import stays in one place. */
  private static final class CertificateFactoryHolder {
    static void addCa(KeyStore trust, java.io.InputStream in) throws Exception {
      java.security.cert.CertificateFactory cf =
          java.security.cert.CertificateFactory.getInstance("X.509");
      trust.setCertificateEntry("ca", cf.generateCertificate(in));
    }
  }
}
