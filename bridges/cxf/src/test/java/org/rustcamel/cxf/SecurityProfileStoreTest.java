package org.rustcamel.cxf;

import static org.junit.jupiter.api.Assertions.*;

import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.HashMap;
import java.util.Map;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.DisplayName;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

@DisplayName("SecurityProfileStore")
class SecurityProfileStoreTest {

  @TempDir static Path tempDir;

  private static Path ksFile;
  private static Path tsFile;
  private static Path ksFile2;

  @BeforeAll
  static void createFakeKeystores() throws IOException {
    ksFile = tempDir.resolve("ks.jks");
    Files.writeString(ksFile, "fake");
    tsFile = tempDir.resolve("ts.jks");
    Files.writeString(tsFile, "fake");
    ksFile2 = tempDir.resolve("ks2.jks");
    Files.writeString(ksFile2, "fake");
  }

  /**
   * Creates a testable SecurityProfileStore that reads env vars from the given map instead of
   * System.getenv().
   */
  private SecurityProfileStore createStore(Map<String, String> envVars) {
    return new SecurityProfileStore() {
      @Override
      String getenv(String key) {
        return envVars.get(key);
      }
    };
  }

  @Test
  @DisplayName("empty env returns empty profiles")
  void emptyEnvReturnsEmptyProfiles() {
    SecurityProfileStore store = createStore(new HashMap<>());
    assertTrue(store.isEmpty());
    assertTrue(store.profiles().isEmpty());
  }

  @Test
  @DisplayName("null CXF_PROFILES returns empty profiles")
  void nullCxfProfilesReturnsEmpty() {
    // CXF_PROFILES not in map → getenv returns null
    SecurityProfileStore store = createStore(new HashMap<>());
    assertTrue(store.isEmpty());
  }

  @Test
  @DisplayName("blank CXF_PROFILES returns empty profiles")
  void blankCxfProfilesReturnsEmpty() {
    SecurityProfileStore store = createStore(Map.of("CXF_PROFILES", "  "));
    assertTrue(store.isEmpty());
  }

  @Test
  @DisplayName("single valid profile parsed correctly")
  void singleValidProfile() {
    Map<String, String> env = new HashMap<>();
    env.put("CXF_PROFILES", "baleares");
    env.put("CXF_PROFILE_BALEARES_KEYSTORE_PATH", ksFile.toString());
    env.put("CXF_PROFILE_BALEARES_KEYSTORE_PASSWORD", "secret");

    SecurityProfileStore store = createStore(env);
    assertFalse(store.isEmpty());
    assertEquals(1, store.profiles().size());

    SecurityProfile profile = store.getProfile("baleares");
    assertEquals("baleares", profile.name());
    assertEquals(ksFile.toString(), profile.keystorePath());
    assertEquals("secret", profile.keystorePassword());
    // Defaults
    assertEquals("clientkey", profile.sigUsername());
    assertEquals("serverkey", profile.encUsername());
    assertEquals("Signature", profile.resolveActionsOut());
  }

  @Test
  @DisplayName("CXF_PROFILES with multiple names creates multiple profiles")
  void multipleProfiles() {
    Map<String, String> env = new HashMap<>();
    env.put("CXF_PROFILES", "alpha,beta");
    env.put("CXF_PROFILE_ALPHA_KEYSTORE_PATH", ksFile.toString());
    env.put("CXF_PROFILE_ALPHA_KEYSTORE_PASSWORD", "pass1");
    env.put("CXF_PROFILE_BETA_KEYSTORE_PATH", ksFile2.toString());
    env.put("CXF_PROFILE_BETA_KEYSTORE_PASSWORD", "pass2");

    SecurityProfileStore store = createStore(env);
    assertEquals(2, store.profiles().size());
    assertTrue(store.profiles().containsKey("alpha"));
    assertTrue(store.profiles().containsKey("beta"));

    assertEquals(ksFile.toString(), store.getProfile("alpha").keystorePath());
    assertEquals(ksFile2.toString(), store.getProfile("beta").keystorePath());
  }

  @Test
  @DisplayName("profile with full env vars populates all fields")
  void fullProfileFromEnv() {
    Map<String, String> env = new HashMap<>();
    env.put("CXF_PROFILES", "full");
    env.put("CXF_PROFILE_FULL_KEYSTORE_PATH", ksFile.toString());
    env.put("CXF_PROFILE_FULL_KEYSTORE_PASSWORD", "kspass");
    env.put("CXF_PROFILE_FULL_TRUSTSTORE_PATH", tsFile.toString());
    env.put("CXF_PROFILE_FULL_TRUSTSTORE_PASSWORD", "tspass");
    env.put("CXF_PROFILE_FULL_SIG_USERNAME", "siguser");
    env.put("CXF_PROFILE_FULL_SIG_PASSWORD", "sigpass");
    env.put("CXF_PROFILE_FULL_ENC_USERNAME", "encuser");
    env.put("CXF_PROFILE_FULL_SECURITY_ACTIONS_OUT", "Signature Encrypt");
    env.put("CXF_PROFILE_FULL_SECURITY_ACTIONS_IN", "Signature");
    env.put("CXF_PROFILE_FULL_WSDL_PATH", "/wsdl/test.wsdl");
    env.put("CXF_PROFILE_FULL_SERVICE_NAME", "TestService");
    env.put("CXF_PROFILE_FULL_PORT_NAME", "TestPort");
    env.put("CXF_PROFILE_FULL_ADDRESS", "http://localhost:8080/svc");
    env.put("CXF_PROFILE_FULL_SIGNATURE_ALGORITHM", "rsa-sha256");
    env.put("CXF_PROFILE_FULL_SIGNATURE_DIGEST_ALGORITHM", "sha256");
    env.put("CXF_PROFILE_FULL_SIGNATURE_C14N_ALGORITHM", "exc-c14n");
    env.put("CXF_PROFILE_FULL_SIGNATURE_PARTS", "Body");

    SecurityProfileStore store = createStore(env);
    SecurityProfile p = store.getProfile("full");

    assertEquals(ksFile.toString(), p.keystorePath());
    assertEquals("kspass", p.keystorePassword());
    assertEquals(tsFile.toString(), p.truststorePath());
    assertEquals("tspass", p.truststorePassword());
    assertEquals("siguser", p.sigUsername());
    assertEquals("sigpass", p.sigPassword());
    assertEquals("encuser", p.encUsername());
    assertEquals("Signature Encrypt", p.securityActionsOut());
    assertEquals("Signature", p.securityActionsIn());
    assertEquals("/wsdl/test.wsdl", p.wsdlPath());
    assertEquals("TestService", p.serviceName());
    assertEquals("TestPort", p.portName());
    assertEquals("http://localhost:8080/svc", p.address());
    assertEquals("rsa-sha256", p.signatureAlgorithm());
    assertEquals("sha256", p.signatureDigestAlgorithm());
    assertEquals("exc-c14n", p.signatureC14nAlgorithm());
    assertEquals("Body", p.signatureParts());
  }

  @Test
  @DisplayName("invalid profile name with hyphens rejected")
  void invalidProfileNameHyphens() {
    Map<String, String> env = new HashMap<>();
    env.put("CXF_PROFILES", "my-profile");

    IllegalStateException ex = assertThrows(IllegalStateException.class, () -> createStore(env));
    assertTrue(ex.getMessage().contains("Invalid profile name"));
    assertTrue(ex.getMessage().contains("my-profile"));
  }

  @Test
  @DisplayName("invalid profile name with uppercase rejected")
  void invalidProfileNameUppercase() {
    Map<String, String> env = new HashMap<>();
    env.put("CXF_PROFILES", "MyProfile");

    IllegalStateException ex = assertThrows(IllegalStateException.class, () -> createStore(env));
    assertTrue(ex.getMessage().contains("Invalid profile name"));
    assertTrue(ex.getMessage().contains("MyProfile"));
  }

  @Test
  @DisplayName("duplicate profile name rejected")
  void duplicateProfileName() {
    Map<String, String> env = new HashMap<>();
    env.put("CXF_PROFILES", "alpha,alpha");
    env.put("CXF_PROFILE_ALPHA_KEYSTORE_PATH", ksFile.toString());
    env.put("CXF_PROFILE_ALPHA_KEYSTORE_PASSWORD", "pass");

    IllegalStateException ex = assertThrows(IllegalStateException.class, () -> createStore(env));
    assertTrue(ex.getMessage().contains("Duplicate profile name"));
    assertTrue(ex.getMessage().contains("alpha"));
  }

  @Test
  @DisplayName("getProfile returns correct profile")
  void getProfileReturnsCorrectProfile() {
    Map<String, String> env = new HashMap<>();
    env.put("CXF_PROFILES", "one,two");
    env.put("CXF_PROFILE_ONE_KEYSTORE_PATH", ksFile.toString());
    env.put("CXF_PROFILE_ONE_KEYSTORE_PASSWORD", "p1");
    env.put("CXF_PROFILE_TWO_KEYSTORE_PATH", ksFile2.toString());
    env.put("CXF_PROFILE_TWO_KEYSTORE_PASSWORD", "p2");

    SecurityProfileStore store = createStore(env);
    assertEquals(ksFile.toString(), store.getProfile("one").keystorePath());
    assertEquals(ksFile2.toString(), store.getProfile("two").keystorePath());
  }

  @Test
  @DisplayName("getProfile with null throws IllegalArgumentException")
  void getProfileWithNullThrows() {
    SecurityProfileStore store = createStore(new HashMap<>());
    assertThrows(IllegalArgumentException.class, () -> store.getProfile(null));
  }

  @Test
  @DisplayName("getProfile with blank throws IllegalArgumentException")
  void getProfileWithBlankThrows() {
    SecurityProfileStore store = createStore(new HashMap<>());
    assertThrows(IllegalArgumentException.class, () -> store.getProfile(""));
    assertThrows(IllegalArgumentException.class, () -> store.getProfile("   "));
  }

  @Test
  @DisplayName("getProfile with unknown name throws IllegalArgumentException")
  void getProfileWithUnknownNameThrows() {
    SecurityProfileStore store = createStore(new HashMap<>());
    IllegalArgumentException ex =
        assertThrows(IllegalArgumentException.class, () -> store.getProfile("nonexistent"));
    assertTrue(ex.getMessage().contains("Unknown security profile"));
    assertTrue(ex.getMessage().contains("nonexistent"));
  }

  @Test
  @DisplayName("profile without keystore creates profile with null keystorePath")
  void profileWithoutKeystore() {
    Map<String, String> env = new HashMap<>();
    env.put("CXF_PROFILES", "plain");

    SecurityProfileStore store = createStore(env);
    SecurityProfile p = store.getProfile("plain");
    assertNull(p.keystorePath());
    assertFalse(p.hasSecurity());
    assertFalse(p.canSignOutbound());
  }

  @Test
  @DisplayName("profiles() returns unmodifiable map")
  void profilesReturnsUnmodifiable() {
    Map<String, String> env = new HashMap<>();
    env.put("CXF_PROFILES", "test");
    env.put("CXF_PROFILE_TEST_KEYSTORE_PATH", ksFile.toString());
    env.put("CXF_PROFILE_TEST_KEYSTORE_PASSWORD", "pass");

    SecurityProfileStore store = createStore(env);
    assertThrows(UnsupportedOperationException.class, () -> store.profiles().put("hack", null));
  }

  @Test
  @DisplayName("CXF_PROFILES with trailing/leading commas handled gracefully")
  void cxfProfilesWithExtraCommas() {
    Map<String, String> env = new HashMap<>();
    env.put("CXF_PROFILES", ",alpha,");
    env.put("CXF_PROFILE_ALPHA_KEYSTORE_PATH", ksFile.toString());
    env.put("CXF_PROFILE_ALPHA_KEYSTORE_PASSWORD", "pass");

    // Should parse "alpha" (blank entries skipped)
    SecurityProfileStore store = createStore(env);
    assertEquals(1, store.profiles().size());
  }

  @Test
  @DisplayName("underscore in profile name is valid")
  void underscoreProfileNameIsValid() {
    Map<String, String> env = new HashMap<>();
    env.put("CXF_PROFILES", "my_profile");
    env.put("CXF_PROFILE_MY_PROFILE_KEYSTORE_PATH", ksFile.toString());
    env.put("CXF_PROFILE_MY_PROFILE_KEYSTORE_PASSWORD", "pass");

    SecurityProfileStore store = createStore(env);
    assertFalse(store.isEmpty());
    assertEquals("my_profile", store.getProfile("my_profile").name());
  }

  @Test
  @DisplayName("missing keystore file rejected at startup")
  void missingKeystoreFileRejected() {
    Map<String, String> env = new HashMap<>();
    env.put("CXF_PROFILES", "broken");
    env.put("CXF_PROFILE_BROKEN_KEYSTORE_PATH", "/nonexistent/path/ks.jks");
    env.put("CXF_PROFILE_BROKEN_KEYSTORE_PASSWORD", "pass");

    IllegalStateException ex = assertThrows(IllegalStateException.class, () -> createStore(env));
    assertTrue(ex.getMessage().contains("Keystore file not found"));
    assertTrue(ex.getMessage().contains("broken"));
  }
}
