package org.rustcamel.cxf;

import jakarta.enterprise.context.ApplicationScoped;
import java.util.Collections;
import java.util.LinkedHashMap;
import java.util.Map;
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

      SecurityProfile profile =
          SecurityProfile.builder(name)
              .wsdlPath(getenv(prefix + "WSDL_PATH"))
              .serviceName(getenv(prefix + "SERVICE_NAME"))
              .portName(getenv(prefix + "PORT_NAME"))
              .address(getenv(prefix + "ADDRESS"))
              .keystore(keystorePath, keystorePassword)
              .truststore(
                  getenv(prefix + "TRUSTSTORE_PATH"), getenv(prefix + "TRUSTSTORE_PASSWORD"))
              .sigUser(getenv(prefix + "SIG_USERNAME"), getenv(prefix + "SIG_PASSWORD"))
              .encUser(getenv(prefix + "ENC_USERNAME"))
              .actionsOut(getenv(prefix + "SECURITY_ACTIONS_OUT"))
              .actionsIn(getenv(prefix + "SECURITY_ACTIONS_IN"))
              .signatureAlgorithm(getenv(prefix + "SIGNATURE_ALGORITHM"))
              .signatureDigestAlgorithm(getenv(prefix + "SIGNATURE_DIGEST_ALGORITHM"))
              .signatureC14nAlgorithm(getenv(prefix + "SIGNATURE_C14N_ALGORITHM"))
              .signatureParts(getenv(prefix + "SIGNATURE_PARTS"))
              .build();

      profiles.put(name, profile);
      LOG.info("Loaded security profile: " + name);
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
}
