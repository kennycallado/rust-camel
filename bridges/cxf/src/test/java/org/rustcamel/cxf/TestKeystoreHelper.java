package org.rustcamel.cxf;

import java.nio.file.Files;
import java.nio.file.Path;
import java.util.List;

/**
 * Generates a self-signed JKS keystore for integration testing using keytool. Avoids BouncyCastle
 * to ensure full JCA provider compatibility (no BadPaddingException when using RSA/OAEP key
 * transport with WSS4J).
 */
class TestKeystoreHelper {

  static final String KEYSTORE_PASSWORD = "changeit";
  static final String KEY_ALIAS = "alice";

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
            "CN=test-alice, O=RustCamel Test",
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
}
