package org.rustcamel.cxf;

import jakarta.enterprise.context.ApplicationScoped;
import java.util.ArrayList;
import java.util.Collections;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Locale;
import java.util.Map;
import java.util.Optional;
import java.util.logging.Logger;

/**
 * CDI bean that reads security profiles exclusively from environment variables. No MicroProfile
 * Config dynamic keys (dead in native-image). The Rust side generates {@code CXF_PROFILE_<NAME>_*}
 * env vars; Java reads them.
 */
@ApplicationScoped
public class SecurityProfileStore {
  private static final Logger LOG = Logger.getLogger(SecurityProfileStore.class.getName());

  private final Map<String, SecurityProfile> profiles = new LinkedHashMap<>();

  public SecurityProfileStore() {
    String profilesList = getenv("CXF_PROFILES");
    if (profilesList == null || profilesList.isBlank()) {
      LOG.info("No security profiles configured (plain SOAP mode)");
      return;
    }

    for (String rawName : profilesList.split(",")) {
      String name = rawName.trim();
      if (name.isBlank()) continue;
      if (!name.matches("[a-z0-9_]+")) {
        throw new IllegalStateException(
            "Invalid profile name: '" + name + "' (must match [a-z0-9_]+)");
      }
      if (profiles.containsKey(name)) {
        throw new IllegalStateException("Duplicate profile name: '" + name + "'");
      }

      String prefix = "CXF_PROFILE_" + name.toUpperCase() + "_";

      String keystorePath = getenv(prefix + "KEYSTORE_PATH");
      String keystorePassword = getenv(prefix + "KEYSTORE_PASSWORD");

      if (keystorePath != null) {
        java.io.File ksFile = new java.io.File(keystorePath);
        if (!ksFile.exists()) {
          throw new IllegalStateException(
              "Keystore file not found for profile '" + name + "': " + keystorePath);
        }
      }

      String tlsKeystorePath = getenv(prefix + "TLS_KEYSTORE_PATH");
      String tlsKeystorePassword = getenv(prefix + "TLS_KEYSTORE_PASSWORD");
      String tlsKeystoreType = normalizeTlsType(getenv(prefix + "TLS_KEYSTORE_TYPE"));
      String tlsTruststorePath = getenv(prefix + "TLS_TRUSTSTORE_PATH");
      String tlsTruststorePassword = getenv(prefix + "TLS_TRUSTSTORE_PASSWORD");
      String tlsTruststoreType = normalizeTlsType(getenv(prefix + "TLS_TRUSTSTORE_TYPE"));

      // Fail before ready on an incomplete path/password pair or an
      // unsupported store type (Task 1.5). Eager store loading is Task 1.6.
      validateTlsStore(name, "keystore", tlsKeystorePath, tlsKeystorePassword, tlsKeystoreType);
      validateTlsStore(
          name, "truststore", tlsTruststorePath, tlsTruststorePassword, tlsTruststoreType);

      Integer faultStatus = parseFaultStatus(name, getenv(prefix + "FAULT_STATUS"));

      SecurityProfile profile =
          SecurityProfile.builder(name)
              .wsdlPath(getenv(prefix + "WSDL_PATH"))
              .serviceName(getenv(prefix + "SERVICE_NAME"))
              .portName(getenv(prefix + "PORT_NAME"))
              .address(getenv(prefix + "ADDRESS"))
              .keystore(keystorePath, keystorePassword)
              .truststore(
                  getenv(prefix + "TRUSTSTORE_PATH"), getenv(prefix + "TRUSTSTORE_PASSWORD"))
              .tlsKeystore(tlsKeystorePath, tlsKeystorePassword, tlsKeystoreType)
              .tlsTruststore(tlsTruststorePath, tlsTruststorePassword, tlsTruststoreType)
              .sigUser(getenv(prefix + "SIG_USERNAME"), getenv(prefix + "SIG_PASSWORD"))
              .encUser(getenv(prefix + "ENC_USERNAME"))
              .actionsOut(getenv(prefix + "SECURITY_ACTIONS_OUT"))
              .actionsIn(getenv(prefix + "SECURITY_ACTIONS_IN"))
              .signatureAlgorithm(getenv(prefix + "SIGNATURE_ALGORITHM"))
              .signatureDigestAlgorithm(getenv(prefix + "SIGNATURE_DIGEST_ALGORITHM"))
              .signatureC14nAlgorithm(getenv(prefix + "SIGNATURE_C14N_ALGORITHM"))
              .signatureParts(getenv(prefix + "SIGNATURE_PARTS"))
              .allowedSignatureAlgorithms(getenv(prefix + "ALLOWED_SIGNATURE_ALGORITHMS"))
              .allowedDigestAlgorithms(getenv(prefix + "ALLOWED_DIGEST_ALGORITHMS"))
              .faultStatus(faultStatus)
              .build();

      profiles.put(name, profile);
      LOG.info("Loaded security profile: " + name);
      sha1AllowedWarning(profile).ifPresent(LOG::warning);
      signingKnobMismatchWarning(profile).ifPresent(LOG::warning);
    }

    LOG.info("Security profiles loaded: " + profiles.keySet());
  }

  /**
   * Returns the profile for the given name.
   *
   * @throws IllegalArgumentException if name is null, blank, or unknown
   */
  public SecurityProfile getProfile(String name) {
    if (name == null || name.isBlank()) {
      throw new IllegalArgumentException("Profile name must not be null or blank");
    }
    SecurityProfile profile = profiles.get(name);
    if (profile == null) {
      throw new IllegalArgumentException("Unknown security profile: " + name);
    }
    return profile;
  }

  public boolean isEmpty() {
    return profiles.isEmpty();
  }

  public Map<String, SecurityProfile> profiles() {
    return Collections.unmodifiableMap(profiles);
  }

  /** Package-private for testing — allows overriding env var source. */
  String getenv(String key) {
    return System.getenv(key);
  }

  /**
   * Startup WARN when a profile's effective inbound allow-list opts into SHA-1. The message names
   * the profile and points at the accepted-risk ADR. Pure decision point so tests can assert the
   * exact wording without log capture.
   */
  static Optional<String> sha1AllowedWarning(SecurityProfile profile) {
    if (!profile.inboundAllowsSha1()) {
      return Optional.empty();
    }
    return Optional.of(
        "WS-Security profile '"
            + profile.name()
            + "' accepts SHA-1 signature/digest algorithms on inbound verification (opted-in via"
            + " ALLOWED_*; accepted risk, see ADR-0085)");
  }

  /**
   * Startup WARN when a signing knob names a SHA-1 URI that the effective inbound allow-list
   * excludes, on a profile that has inbound verification material. Signing and verifying are
   * independent surfaces, so an intentionally asymmetric profile (sign SHA-1, verify SHA-2 only) is
   * valid; the wording tells the operator both how to opt in and that the asymmetry may be
   * deliberate. At most one WARN per profile: when both axes are excluded their URIs are joined
   * with {@code ", "}, signature URI first.
   */
  static Optional<String> signingKnobMismatchWarning(SecurityProfile profile) {
    if (!profile.canVerifyInbound()) {
      return Optional.empty();
    }

    List<String> excluded = new ArrayList<>();
    String signatureAlgorithm = profile.signatureAlgorithm();
    if (isSha1Uri(signatureAlgorithm)
        && !profile.effectiveAllowedSignatureAlgorithms().contains(signatureAlgorithm)) {
      excluded.add(signatureAlgorithm);
    }
    String signatureDigestAlgorithm = profile.signatureDigestAlgorithm();
    if (isSha1Uri(signatureDigestAlgorithm)
        && !profile.effectiveAllowedDigestAlgorithms().contains(signatureDigestAlgorithm)) {
      excluded.add(signatureDigestAlgorithm);
    }
    if (excluded.isEmpty()) {
      return Optional.empty();
    }

    return Optional.of(
        "WS-Security profile '"
            + profile.name()
            + "' signs with SHA-1 ("
            + String.join(", ", excluded)
            + ") but does not accept it on inbound verification; set CXF_PROFILE_"
            + profile.name().toUpperCase(Locale.ROOT)
            + "_ALLOWED_SIGNATURE_ALGORITHMS/ALLOWED_DIGEST_ALGORITHMS if legacy partners must be"
            + " verified (intentional if this asymmetry is deliberate)");
  }

  private static boolean isSha1Uri(String uri) {
    return uri != null && SecurityProfile.SHA1_ALGORITHM_URIS.contains(uri);
  }

  /** Default the TLS store type to PKCS12 when unset. */
  private static String normalizeTlsType(String type) {
    return (type == null || type.isBlank()) ? "PKCS12" : type;
  }

  /**
   * Parses the optional {@code CXF_PROFILE_<NAME>_FAULT_STATUS} override and fails loud before the
   * profile is built so the bridge exits before announcing ready. Only 400-599 is valid: the
   * override applies to WSS faults only and must be an HTTP error status.
   *
   * @return the parsed status, or {@code null} when unset/blank
   * @throws IllegalStateException on a non-integer or out-of-range value
   */
  static Integer parseFaultStatus(String profile, String raw) {
    if (raw == null || raw.isBlank()) {
      return null;
    }
    int value;
    try {
      value = Integer.parseInt(raw.trim());
    } catch (NumberFormatException e) {
      throw new IllegalStateException(
          "fault_status for profile '"
              + profile
              + "' is not an integer: '"
              + raw
              + "' (expected an integer in 400-599)");
    }
    if (value < 400 || value > 599) {
      throw new IllegalStateException(
          "fault_status for profile '"
              + profile
              + "' is out of range: "
              + value
              + " (expected 400-599; fault_status overrides WSS faults only)");
    }
    return value;
  }

  /**
   * Fail-fast check for one TLS store. An explicitly supplied empty or whitespace-only (Unicode
   * {@code White_Space}, see {@link BlankTlsValue}) {@code *_path} or {@code *_password} is
   * rejected — blank values are invalid, never silently unset; a both-blank pair previously fell
   * back to JVM store defaults. Only a genuinely absent ({@code null}) variable counts as unset.
   * Beyond that, {@code *_path} and {@code *_password} must be supplied together, and {@code
   * *_type} must be {@code JKS} or {@code PKCS12}. Throws {@link IllegalStateException} before the
   * profile is built so the bridge exits before announcing ready. This mirrors the Rust-side {@code
   * CxfTlsConfig} validation, which rejects the same blanks at config load.
   */
  static void validateTlsStore(
      String profile, String store, String path, String password, String type) {
    String keyPrefix = "CXF_PROFILE_" + profile.toUpperCase() + "_TLS_" + store.toUpperCase() + "_";
    if (BlankTlsValue.isBlank(path)) {
      throw new IllegalStateException(
          keyPrefix
              + "PATH is empty or whitespace-only; supply a nonblank value or remove the"
              + " variable (blank TLS values are rejected, not treated as unset)");
    }
    if (BlankTlsValue.isBlank(password)) {
      throw new IllegalStateException(
          keyPrefix
              + "PASSWORD is empty or whitespace-only; supply a nonblank value or remove the"
              + " variable (blank TLS values are rejected, not treated as unset)");
    }
    boolean hasPath = path != null;
    boolean hasPassword = password != null;
    if (hasPath != hasPassword) {
      String present = hasPath ? keyPrefix + "PATH" : keyPrefix + "PASSWORD";
      String missing = hasPath ? keyPrefix + "PASSWORD" : keyPrefix + "PATH";
      throw new IllegalStateException(
          "TLS "
              + store
              + " for profile '"
              + profile
              + "' is incomplete: "
              + present
              + " is set but "
              + missing
              + " is not");
    }
    if (!"JKS".equals(type) && !"PKCS12".equals(type)) {
      throw new IllegalStateException(
          "Unsupported TLS "
              + store
              + " type for profile '"
              + profile
              + "': '"
              + type
              + "' (expected JKS or PKCS12)");
    }
  }
}
