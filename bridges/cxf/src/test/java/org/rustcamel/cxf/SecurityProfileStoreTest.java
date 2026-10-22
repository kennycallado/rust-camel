package org.rustcamel.cxf;

import static org.junit.jupiter.api.Assertions.*;
import static org.mockito.Mockito.mock;
import static org.mockito.Mockito.when;

import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.Optional;
import java.util.logging.Handler;
import java.util.logging.Level;
import java.util.logging.LogRecord;
import java.util.logging.Logger;
import javax.xml.namespace.QName;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.DisplayName;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;
import org.junit.jupiter.params.ParameterizedTest;
import org.junit.jupiter.params.provider.CsvSource;

@DisplayName("SecurityProfileStore")
class SecurityProfileStoreTest {

  private static final String RSA_SHA1 = "http://www.w3.org/2000/09/xmldsig#rsa-sha1";
  private static final String SHA1_DIGEST = "http://www.w3.org/2000/09/xmldsig#sha1";

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
    env.put(
        "CXF_PROFILE_FULL_SIGNATURE_ALGORITHM",
        "http://www.w3.org/2001/04/xmldsig-more#rsa-sha256");
    env.put(
        "CXF_PROFILE_FULL_SIGNATURE_DIGEST_ALGORITHM", "http://www.w3.org/2001/04/xmlenc#sha256");
    env.put("CXF_PROFILE_FULL_SIGNATURE_C14N_ALGORITHM", "http://www.w3.org/2001/10/xml-exc-c14n#");
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
    assertEquals("http://www.w3.org/2001/04/xmldsig-more#rsa-sha256", p.signatureAlgorithm());
    assertEquals("http://www.w3.org/2001/04/xmlenc#sha256", p.signatureDigestAlgorithm());
    assertEquals("http://www.w3.org/2001/10/xml-exc-c14n#", p.signatureC14nAlgorithm());
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

  @Test
  @DisplayName("invalid signature knob env var aborts store construction naming the variable")
  void invalidSignatureKnobEnvVarFailsConstruction() {
    Map<String, String> env = new HashMap<>();
    env.put("CXF_PROFILES", "bad");
    env.put("CXF_PROFILE_BAD_KEYSTORE_PATH", ksFile.toString());
    env.put("CXF_PROFILE_BAD_SECURITY_ACTIONS_OUT", "Encrypt");
    env.put("CXF_PROFILE_BAD_SIGNATURE_ALGORITHM", "not a uri");

    IllegalArgumentException ex =
        assertThrows(IllegalArgumentException.class, () -> createStore(env));
    assertTrue(ex.getMessage().contains("SIGNATURE_ALGORITHM"), ex.getMessage());
  }

  @Test
  @DisplayName("TLS keys are read and incomplete pairs are rejected")
  void readsTlsKeysAndRejectsIncompletePair() {
    // Valid: all six TLS keys parse, explicit types preserved.
    Map<String, String> env = new HashMap<>();
    env.put("CXF_PROFILES", "secure");
    env.put("CXF_PROFILE_SECURE_TLS_KEYSTORE_PATH", ksFile.toString());
    env.put("CXF_PROFILE_SECURE_TLS_KEYSTORE_PASSWORD", "kspass");
    env.put("CXF_PROFILE_SECURE_TLS_KEYSTORE_TYPE", "PKCS12");
    env.put("CXF_PROFILE_SECURE_TLS_TRUSTSTORE_PATH", tsFile.toString());
    env.put("CXF_PROFILE_SECURE_TLS_TRUSTSTORE_PASSWORD", "tspass");
    env.put("CXF_PROFILE_SECURE_TLS_TRUSTSTORE_TYPE", "JKS");

    SecurityProfile p = createStore(env).getProfile("secure");
    assertEquals(ksFile.toString(), p.tlsKeystorePath());
    assertEquals("kspass", p.tlsKeystorePassword());
    assertEquals("PKCS12", p.tlsKeystoreType());
    assertEquals(tsFile.toString(), p.tlsTruststorePath());
    assertEquals("tspass", p.tlsTruststorePassword());
    assertEquals("JKS", p.tlsTruststoreType());

    // Type defaults to PKCS12 when omitted.
    Map<String, String> defaults = new HashMap<>();
    defaults.put("CXF_PROFILES", "secure");
    defaults.put("CXF_PROFILE_SECURE_TLS_KEYSTORE_PATH", ksFile.toString());
    defaults.put("CXF_PROFILE_SECURE_TLS_KEYSTORE_PASSWORD", "kspass");
    assertEquals("PKCS12", createStore(defaults).getProfile("secure").tlsKeystoreType());

    // Unknown type rejects before ready.
    Map<String, String> badType = new HashMap<>();
    badType.put("CXF_PROFILES", "secure");
    badType.put("CXF_PROFILE_SECURE_TLS_KEYSTORE_PATH", ksFile.toString());
    badType.put("CXF_PROFILE_SECURE_TLS_KEYSTORE_PASSWORD", "kspass");
    badType.put("CXF_PROFILE_SECURE_TLS_KEYSTORE_TYPE", "GARBAGE");
    IllegalStateException typeEx =
        assertThrows(IllegalStateException.class, () -> createStore(badType));
    assertTrue(typeEx.getMessage().contains("GARBAGE"), typeEx.getMessage());

    // Bidirectional rejection: path without password, then password without path.
    Map<String, String> pathOnly = new HashMap<>();
    pathOnly.put("CXF_PROFILES", "secure");
    pathOnly.put("CXF_PROFILE_SECURE_TLS_KEYSTORE_PATH", ksFile.toString());
    IllegalStateException pathEx =
        assertThrows(IllegalStateException.class, () -> createStore(pathOnly));
    assertTrue(pathEx.getMessage().contains("TLS_KEYSTORE_PASSWORD"), pathEx.getMessage());

    Map<String, String> passwordOnly = new HashMap<>();
    passwordOnly.put("CXF_PROFILES", "secure");
    passwordOnly.put("CXF_PROFILE_SECURE_TLS_TRUSTSTORE_PASSWORD", "tspass");
    IllegalStateException passEx =
        assertThrows(IllegalStateException.class, () -> createStore(passwordOnly));
    assertTrue(passEx.getMessage().contains("TLS_TRUSTSTORE_PATH"), passEx.getMessage());
  }

  /**
   * Blank producer transport-TLS values both layers reject (N4 fail-closed parity): empty, ASCII
   * whitespace, and Unicode White_Space members where Java's {@code String.isBlank} and Rust's
   * {@code char::is_whitespace} disagree (NBSP U+00A0, NEL U+0085, mixed ASCII + U+202F).
   */
  private static final String[] BLANK_TLS_VALUES = {
    "", " ", "\t", " \t\r\n", "\u00A0", "\u0085", " \u202F",
  };

  @Test
  @DisplayName("producer transport TLS rejects explicitly blank store values (N4 parity)")
  void producerTlsRejectsBlankStoreValues() {
    // Blank keystore path beside an otherwise valid paired password.
    for (String blank : BLANK_TLS_VALUES) {
      Map<String, String> env = new HashMap<>();
      env.put("CXF_PROFILES", "secure");
      env.put("CXF_PROFILE_SECURE_TLS_KEYSTORE_PATH", blank);
      env.put("CXF_PROFILE_SECURE_TLS_KEYSTORE_PASSWORD", "kspass");
      IllegalStateException ex =
          assertThrows(
              IllegalStateException.class, () -> createStore(env), "blank keystore path: " + blank);
      assertTrue(ex.getMessage().contains("TLS_KEYSTORE_PATH"), ex.getMessage());
      assertTrue(ex.getMessage().contains("whitespace"), ex.getMessage());
      // Non-echo is only meaningful for distinctive (non-ASCII) blanks; plain
      // ASCII whitespace occurs in any message text.
      if (blank.codePoints().anyMatch(cp -> cp > 127)) {
        assertFalse(ex.getMessage().contains(blank), "error must not echo the value");
      }
    }

    // Blank truststore password beside an otherwise valid paired path.
    for (String blank : BLANK_TLS_VALUES) {
      Map<String, String> env = new HashMap<>();
      env.put("CXF_PROFILES", "secure");
      env.put("CXF_PROFILE_SECURE_TLS_TRUSTSTORE_PATH", tsFile.toString());
      env.put("CXF_PROFILE_SECURE_TLS_TRUSTSTORE_PASSWORD", blank);
      IllegalStateException ex =
          assertThrows(
              IllegalStateException.class,
              () -> createStore(env),
              "blank truststore password: " + blank);
      assertTrue(ex.getMessage().contains("TLS_TRUSTSTORE_PASSWORD"), ex.getMessage());
      assertTrue(ex.getMessage().contains("whitespace"), ex.getMessage());
      if (blank.codePoints().anyMatch(cp -> cp > 127)) {
        assertFalse(ex.getMessage().contains(blank), "error must not echo the value");
      }
    }

    // A both-blank pair previously passed validation and silently selected JVM
    // store defaults on the Java side.
    for (String blank : BLANK_TLS_VALUES) {
      Map<String, String> env = new HashMap<>();
      env.put("CXF_PROFILES", "secure");
      env.put("CXF_PROFILE_SECURE_TLS_KEYSTORE_PATH", blank);
      env.put("CXF_PROFILE_SECURE_TLS_KEYSTORE_PASSWORD", blank);
      IllegalStateException ex =
          assertThrows(
              IllegalStateException.class,
              () -> createStore(env),
              "both-blank keystore pair: " + blank);
      assertTrue(ex.getMessage().contains("TLS_KEYSTORE_PATH"), ex.getMessage());
      assertTrue(ex.getMessage().contains("whitespace"), ex.getMessage());
    }

    // Genuine omission stays valid and nonblank password bytes are preserved
    // verbatim (meaningful surrounding spaces are never trimmed).
    Map<String, String> spaced = new HashMap<>();
    spaced.put("CXF_PROFILES", "secure");
    spaced.put("CXF_PROFILE_SECURE_TLS_KEYSTORE_PATH", ksFile.toString());
    spaced.put("CXF_PROFILE_SECURE_TLS_KEYSTORE_PASSWORD", " pass word ");
    SecurityProfile p = createStore(spaced).getProfile("secure");
    assertEquals(" pass word ", p.tlsKeystorePassword());

    Map<String, String> omitted = new HashMap<>();
    omitted.put("CXF_PROFILES", "secure");
    omitted.put("CXF_PROFILE_SECURE_KEYSTORE_PATH", ksFile.toString());
    assertTrue(createStore(omitted).profiles().containsKey("secure"));
  }

  @Test
  @DisplayName("SHA-1 allow warning names the profile and states inbound acceptance")
  void sha1AllowedWarning_namesProfileAndStatesAcceptance() {
    SecurityProfile legacy =
        SecurityProfile.builder("legacy")
            .keystore(ksFile.toString(), "kspass")
            .truststore(tsFile.toString(), "tspass")
            .actionsIn("Signature")
            .allowedSignatureAlgorithms(RSA_SHA1)
            .build();

    assertEquals(
        Optional.of(
            "WS-Security profile 'legacy' accepts SHA-1 signature/digest algorithms on inbound"
                + " verification (opted-in via ALLOWED_*; accepted risk, see ADR-0085)"),
        SecurityProfileStore.sha1AllowedWarning(legacy));
  }

  @Test
  @DisplayName("SHA-1 allow warning is absent for the default SHA-2 lists")
  void sha1AllowedWarning_absentForDefaultProfile() {
    SecurityProfile modern =
        SecurityProfile.builder("modern")
            .keystore(ksFile.toString(), "kspass")
            .truststore(tsFile.toString(), "tspass")
            .actionsIn("Signature")
            .build();

    assertTrue(SecurityProfileStore.sha1AllowedWarning(modern).isEmpty());
  }

  @Test
  @DisplayName("signing-knob mismatch fires for legacy signing without inbound opt-in")
  void signingKnobMismatchWarning_firesForLegacyKnobWithoutInboundOptIn() {
    SecurityProfile legacy = legacySigningProfile("legacy").build();

    assertEquals(
        Optional.of(
            "WS-Security profile 'legacy' signs with SHA-1 ("
                + RSA_SHA1
                + ") but does not accept it on inbound verification; set"
                + " CXF_PROFILE_LEGACY_ALLOWED_SIGNATURE_ALGORITHMS/ALLOWED_DIGEST_ALGORITHMS if"
                + " legacy partners must be verified (intentional if this asymmetry is"
                + " deliberate)"),
        SecurityProfileStore.signingKnobMismatchWarning(legacy));
  }

  @Test
  @DisplayName("signing-knob mismatch joins both excluded SHA-1 URIs, signature first")
  void signingKnobMismatchWarning_joinsBothUris() {
    SecurityProfile legacy =
        legacySigningProfile("legacy").signatureDigestAlgorithm(SHA1_DIGEST).build();

    assertEquals(
        Optional.of(
            "WS-Security profile 'legacy' signs with SHA-1 ("
                + RSA_SHA1
                + ", "
                + SHA1_DIGEST
                + ") but does not accept it on inbound verification; set"
                + " CXF_PROFILE_LEGACY_ALLOWED_SIGNATURE_ALGORITHMS/ALLOWED_DIGEST_ALGORITHMS if"
                + " legacy partners must be verified (intentional if this asymmetry is"
                + " deliberate)"),
        SecurityProfileStore.signingKnobMismatchWarning(legacy));
  }

  @Test
  @DisplayName("signing-knob mismatch reports only the legacy digest URI")
  void signingKnobMismatchWarning_digestOnlyExclusion() {
    SecurityProfile profile =
        SecurityProfile.builder("legacydigest")
            .keystore(ksFile.toString(), "kspass")
            .truststore(tsFile.toString(), "tspass")
            .actionsIn("Signature")
            .signatureDigestAlgorithm(SHA1_DIGEST)
            .build();

    assertEquals(
        Optional.of(
            "WS-Security profile 'legacydigest' signs with SHA-1 ("
                + SHA1_DIGEST
                + ") but does not accept it on inbound verification; set"
                + " CXF_PROFILE_LEGACYDIGEST_ALLOWED_SIGNATURE_ALGORITHMS/ALLOWED_DIGEST_ALGORITHMS"
                + " if legacy partners must be verified (intentional if this asymmetry is"
                + " deliberate)"),
        SecurityProfileStore.signingKnobMismatchWarning(profile));
    assertFalse(
        SecurityProfileStore.signingKnobMismatchWarning(profile).orElseThrow().contains(RSA_SHA1),
        "a modern signature knob must not add the rsa-sha1 URI to the warning");
  }

  @Test
  @DisplayName("signing-knob mismatch is silent when the allow-list covers the legacy URIs")
  void signingKnobMismatchWarning_silentWhenAllowListCovers() {
    SecurityProfile legacy =
        legacySigningProfile("legacy")
            .signatureDigestAlgorithm(SHA1_DIGEST)
            .allowedSignatureAlgorithms(RSA_SHA1)
            .allowedDigestAlgorithms(SHA1_DIGEST)
            .build();

    assertTrue(SecurityProfileStore.signingKnobMismatchWarning(legacy).isEmpty());
  }

  @Test
  @DisplayName("signing-knob mismatch is silent without inbound verification material")
  void signingKnobMismatchWarning_silentWithoutVerificationMaterial() {
    // The builder forbids signing knobs without a keystore, so a real profile with legacy knobs
    // always carries verification material. Mock the profile to exercise the material guard.
    SecurityProfile signingOnly = mock(SecurityProfile.class);
    when(signingOnly.name()).thenReturn("signonly");
    when(signingOnly.canVerifyInbound()).thenReturn(false);
    when(signingOnly.signatureAlgorithm()).thenReturn(RSA_SHA1);
    when(signingOnly.signatureDigestAlgorithm()).thenReturn(SHA1_DIGEST);

    assertTrue(SecurityProfileStore.signingKnobMismatchWarning(signingOnly).isEmpty());
  }

  @Test
  @DisplayName("store construction logs the SHA-1 allow warning once")
  void storeConstruction_logsSha1AllowedWarning() {
    Map<String, String> env = new HashMap<>();
    env.put("CXF_PROFILES", "legacy");
    env.put("CXF_PROFILE_LEGACY_KEYSTORE_PATH", ksFile.toString());
    env.put("CXF_PROFILE_LEGACY_KEYSTORE_PASSWORD", "kspass");
    env.put("CXF_PROFILE_LEGACY_TRUSTSTORE_PATH", tsFile.toString());
    env.put("CXF_PROFILE_LEGACY_TRUSTSTORE_PASSWORD", "tspass");
    env.put("CXF_PROFILE_LEGACY_SECURITY_ACTIONS_IN", "Signature");
    env.put("CXF_PROFILE_LEGACY_ALLOWED_SIGNATURE_ALGORITHMS", RSA_SHA1);

    List<LogRecord> records = new ArrayList<>();
    Handler handler =
        new Handler() {
          @Override
          public void publish(LogRecord record) {
            records.add(record);
          }

          @Override
          public void flush() {}

          @Override
          public void close() {}
        };
    Logger logger = Logger.getLogger(SecurityProfileStore.class.getName());
    Level previousLevel = logger.getLevel();
    logger.addHandler(handler);
    logger.setLevel(Level.ALL);
    try {
      createStore(env);
    } finally {
      logger.removeHandler(handler);
      logger.setLevel(previousLevel);
    }

    List<String> sha1Warnings = new ArrayList<>();
    for (LogRecord record : records) {
      if (record.getLevel() == Level.WARNING
          && record.getMessage() != null
          && record.getMessage().contains("accepts SHA-1")) {
        sha1Warnings.add(record.getMessage());
      }
    }
    assertEquals(
        List.of(
            "WS-Security profile 'legacy' accepts SHA-1 signature/digest algorithms on inbound"
                + " verification (opted-in via ALLOWED_*; accepted risk, see ADR-0085)"),
        sha1Warnings);
  }

  @Test
  @DisplayName("store parses the allow-expired-pinned-certificate opt-in")
  void store_parses_allow_expired_pinned_certificate_env() {
    Map<String, String> env = new HashMap<>();
    env.put("CXF_PROFILES", "opted");
    env.put("CXF_PROFILE_OPTED_KEYSTORE_PATH", ksFile.toString());
    env.put("CXF_PROFILE_OPTED_KEYSTORE_PASSWORD", "kspass");
    env.put("CXF_PROFILE_OPTED_ALLOW_EXPIRED_PINNED_CERTIFICATE", "true");

    SecurityProfileStore store = createStore(env);
    assertTrue(store.getProfile("opted").allowExpiredPinnedCertificate());
  }

  @Test
  @DisplayName("store defaults the allow-expired-pinned-certificate opt-in to false")
  void store_defaults_allow_expired_pinned_certificate_false() {
    Map<String, String> env = new HashMap<>();
    env.put("CXF_PROFILES", "opted");
    env.put("CXF_PROFILE_OPTED_KEYSTORE_PATH", ksFile.toString());
    env.put("CXF_PROFILE_OPTED_KEYSTORE_PASSWORD", "kspass");

    SecurityProfileStore store = createStore(env);
    assertFalse(store.getProfile("opted").allowExpiredPinnedCertificate());
  }

  @Test
  @DisplayName("store rejects a non-boolean allow-expired-pinned-certificate value")
  void store_rejects_non_boolean_allow_expired_pinned_certificate() {
    Map<String, String> env = new HashMap<>();
    env.put("CXF_PROFILES", "opted");
    env.put("CXF_PROFILE_OPTED_KEYSTORE_PATH", ksFile.toString());
    env.put("CXF_PROFILE_OPTED_KEYSTORE_PASSWORD", "kspass");
    env.put("CXF_PROFILE_OPTED_ALLOW_EXPIRED_PINNED_CERTIFICATE", "yes");

    IllegalStateException ex = assertThrows(IllegalStateException.class, () -> createStore(env));
    assertTrue(
        ex.getMessage().contains("CXF_PROFILE_OPTED_ALLOW_EXPIRED_PINNED_CERTIFICATE"),
        "expected message naming the variable but was: " + ex.getMessage());
  }

  private static Map<String, String> timestampWindowEnv() {
    Map<String, String> env = new HashMap<>();
    env.put("CXF_PROFILES", "win");
    env.put("CXF_PROFILE_WIN_KEYSTORE_PATH", ksFile.toString());
    env.put("CXF_PROFILE_WIN_KEYSTORE_PASSWORD", "kspass");
    return env;
  }

  @Test
  @DisplayName("store parses the timestamp window env vars")
  void store_parses_timestamp_window_env() {
    Map<String, String> env = timestampWindowEnv();
    env.put("CXF_PROFILE_WIN_TIMESTAMP_TTL_SECONDS", "900");
    env.put("CXF_PROFILE_WIN_TIMESTAMP_FUTURE_TOLERANCE_SECONDS", "120");

    SecurityProfile p = createStore(env).getProfile("win");
    assertEquals(900, p.timestampTtlSeconds());
    assertEquals(120, p.timestampFutureToleranceSeconds());
  }

  @Test
  @DisplayName("store defaults the timestamp window to 300 and 60")
  void store_defaults_timestamp_window() {
    SecurityProfile p = createStore(timestampWindowEnv()).getProfile("win");
    assertEquals(300, p.timestampTtlSeconds());
    assertEquals(60, p.timestampFutureToleranceSeconds());
  }

  @ParameterizedTest
  @CsvSource({
    "TIMESTAMP_TTL_SECONDS, 0",
    "TIMESTAMP_TTL_SECONDS, -5",
    "TIMESTAMP_TTL_SECONDS, abc",
    "TIMESTAMP_FUTURE_TOLERANCE_SECONDS, 0",
    "TIMESTAMP_FUTURE_TOLERANCE_SECONDS, -5",
    "TIMESTAMP_FUTURE_TOLERANCE_SECONDS, abc",
  })
  @DisplayName("store rejects an invalid timestamp window value naming the variable")
  void store_rejects_invalid_timestamp_window(String knob, String raw) {
    Map<String, String> env = timestampWindowEnv();
    String envVar = "CXF_PROFILE_WIN_" + knob;
    env.put(envVar, raw);

    IllegalStateException ex = assertThrows(IllegalStateException.class, () -> createStore(env));
    assertTrue(ex.getMessage().contains(envVar), ex.getMessage());
  }

  @ParameterizedTest
  @CsvSource({
    "TIMESTAMP_TTL_SECONDS, 2147483648",
    "TIMESTAMP_TTL_SECONDS, 18446744073709551615",
    "TIMESTAMP_FUTURE_TOLERANCE_SECONDS, 2147483648",
    "TIMESTAMP_FUTURE_TOLERANCE_SECONDS, 18446744073709551615",
  })
  @DisplayName("store rejects a timestamp window above Integer.MAX_VALUE naming the maximum")
  void store_rejects_timestamp_window_above_int_max(String knob, String raw) {
    Map<String, String> env = timestampWindowEnv();
    String envVar = "CXF_PROFILE_WIN_" + knob;
    env.put(envVar, raw);

    IllegalStateException ex = assertThrows(IllegalStateException.class, () -> createStore(env));
    assertTrue(ex.getMessage().contains(envVar), ex.getMessage());
    assertTrue(ex.getMessage().contains("2147483647"), ex.getMessage());
  }

  @ParameterizedTest
  @CsvSource({"TIMESTAMP_TTL_SECONDS", "TIMESTAMP_FUTURE_TOLERANCE_SECONDS"})
  @DisplayName("store accepts a timestamp window at Integer.MAX_VALUE")
  void store_accepts_timestamp_window_at_int_max(String knob) {
    Map<String, String> env = timestampWindowEnv();
    env.put("CXF_PROFILE_WIN_" + knob, "2147483647");

    SecurityProfile p = createStore(env).getProfile("win");
    int effective =
        "TIMESTAMP_TTL_SECONDS".equals(knob)
            ? p.timestampTtlSeconds()
            : p.timestampFutureToleranceSeconds();
    assertEquals(Integer.MAX_VALUE, effective);
  }

  @Test
  @DisplayName("timestamp window warning fires only above the defaults")
  void timestamp_window_warning_decision() {
    SecurityProfile trusted =
        SecurityProfile.builder("window").truststore(tsFile.toString(), "tspass").build();
    assertTrue(SecurityProfileStore.timestampWindowWarning(trusted).isEmpty());

    SecurityProfile shortTtl =
        SecurityProfile.builder("window")
            .truststore(tsFile.toString(), "tspass")
            .timestampTtlSeconds(120)
            .build();
    assertTrue(SecurityProfileStore.timestampWindowWarning(shortTtl).isEmpty());

    SecurityProfile explicitDefaults =
        SecurityProfile.builder("window")
            .truststore(tsFile.toString(), "tspass")
            .timestampTtlSeconds(300)
            .timestampFutureToleranceSeconds(60)
            .build();
    assertTrue(SecurityProfileStore.timestampWindowWarning(explicitDefaults).isEmpty());

    SecurityProfile raisedTtl =
        SecurityProfile.builder("window")
            .truststore(tsFile.toString(), "tspass")
            .timestampTtlSeconds(900)
            .build();
    String ttlMessage = SecurityProfileStore.timestampWindowWarning(raisedTtl).orElseThrow();
    assertTrue(ttlMessage.contains("TIMESTAMP_TTL_SECONDS=900"), ttlMessage);
    assertTrue(ttlMessage.contains("TIMESTAMP_FUTURE_TOLERANCE_SECONDS=60"), ttlMessage);
    assertPresentWarningNamesProfileAndAdr(ttlMessage);

    SecurityProfile raisedFuture =
        SecurityProfile.builder("window")
            .truststore(tsFile.toString(), "tspass")
            .timestampFutureToleranceSeconds(120)
            .build();
    String futureMessage = SecurityProfileStore.timestampWindowWarning(raisedFuture).orElseThrow();
    assertTrue(futureMessage.contains("TIMESTAMP_TTL_SECONDS=300"), futureMessage);
    assertTrue(futureMessage.contains("TIMESTAMP_FUTURE_TOLERANCE_SECONDS=120"), futureMessage);
    assertPresentWarningNamesProfileAndAdr(futureMessage);
  }

  private static void assertPresentWarningNamesProfileAndAdr(String message) {
    assertTrue(message.contains("'window'"), message);
    assertTrue(message.contains("ADR-0088"), message);
    assertTrue(message.contains("3600 s"), message);
  }

  @ParameterizedTest
  @CsvSource({
    "900, , 1",
    ", 120, 1",
    "900, 120, 1",
    ", , 0",
    "120, , 0",
  })
  @DisplayName("store logs the timestamp window warning at startup")
  void store_logs_timestamp_window_warning(String ttl, String future, int expectedWarnings) {
    Map<String, String> env = timestampWindowEnv();
    if (ttl != null) {
      env.put("CXF_PROFILE_WIN_TIMESTAMP_TTL_SECONDS", ttl);
    }
    if (future != null) {
      env.put("CXF_PROFILE_WIN_TIMESTAMP_FUTURE_TOLERANCE_SECONDS", future);
    }

    List<LogRecord> records = new ArrayList<>();
    Handler handler =
        new Handler() {
          @Override
          public void publish(LogRecord record) {
            records.add(record);
          }

          @Override
          public void flush() {}

          @Override
          public void close() {}
        };
    Logger logger = Logger.getLogger(SecurityProfileStore.class.getName());
    Level previousLevel = logger.getLevel();
    logger.addHandler(handler);
    logger.setLevel(Level.ALL);
    try {
      createStore(env);
    } finally {
      logger.removeHandler(handler);
      logger.setLevel(previousLevel);
    }

    List<String> warnings = new ArrayList<>();
    for (LogRecord record : records) {
      if (record.getLevel() == Level.WARNING
          && record.getMessage() != null
          && record.getMessage().contains("widens the inbound Timestamp window")) {
        warnings.add(record.getMessage());
      }
    }
    assertEquals(expectedWarnings, warnings.size(), warnings.toString());
    for (String warning : warnings) {
      assertTrue(warning.contains("'win'"), warning);
      assertTrue(warning.contains("ADR-0088"), warning);
    }
  }

  @Test
  @DisplayName("EXPOSED_SOAP_HEADERS reaches the profile as a validated QName")
  void exposedSoapHeaders_envVarReachesTheProfile() {
    Map<String, String> env = new HashMap<>();
    env.put("CXF_PROFILES", "p1");
    env.put(
        "CXF_PROFILE_P1_EXPOSED_SOAP_HEADERS",
        "{http://tempuri.org/commonSchema}requestHeaderType");

    SecurityProfile profile = createStore(env).getProfile("p1");

    assertEquals(
        List.of(new QName("http://tempuri.org/commonSchema", "requestHeaderType")),
        profile.exposedSoapHeaders());
  }

  @Test
  @DisplayName("blank EXPOSED_SOAP_HEADERS leaves the allow-list empty")
  void exposedSoapHeaders_blankVarIsEmpty() {
    Map<String, String> env = new HashMap<>();
    env.put("CXF_PROFILES", "p1");
    env.put("CXF_PROFILE_P1_EXPOSED_SOAP_HEADERS", "   ");

    SecurityProfile profile = createStore(env).getProfile("p1");

    assertEquals(List.of(), profile.exposedSoapHeaders());
  }

  @Test
  @DisplayName("a reserved WS-Security QName fails startup")
  void exposedSoapHeaders_reservedNamespaceFailsStartup() {
    Map<String, String> env = new HashMap<>();
    env.put("CXF_PROFILES", "p1");
    env.put(
        "CXF_PROFILE_P1_EXPOSED_SOAP_HEADERS",
        "{http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-wssecurity-secext-1.0.xsd}"
            + "Security");

    IllegalArgumentException ex =
        assertThrows(IllegalArgumentException.class, () -> createStore(env));

    assertTrue(
        ex.getMessage().contains("reserved namespace"),
        "expected a reserved-namespace failure but was: " + ex.getMessage());
  }

  private static SecurityProfile.Builder legacySigningProfile(String name) {
    return SecurityProfile.builder(name)
        .keystore(ksFile.toString(), "kspass")
        .truststore(tsFile.toString(), "tspass")
        .actionsOut("Signature")
        .actionsIn("Signature")
        .signatureAlgorithm(RSA_SHA1);
  }
}
