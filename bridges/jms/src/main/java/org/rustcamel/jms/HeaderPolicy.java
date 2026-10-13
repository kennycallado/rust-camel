package org.rustcamel.jms;

import java.util.List;
import java.util.Locale;

/**
 * Outbound JMS property denylist backstop applied by {@link JmsProducer} before any property is
 * written. Mirrors the Rust predicate {@code is_denied_send_header} in {@code
 * crates/components/camel-jms/src/headers.rs} and the {@code normalize_header_name} folding in
 * {@code config.rs}: casefold, then strip {@code -}, {@code _}, and {@code .}.
 *
 * <p>Denied names are the reserved {@code jms} provider-control namespace ({@code JMS}, {@code
 * JMSX}, and {@code JMS_} all fold to the {@code jms} prefix), the {@code camel} and {@code xcamel}
 * framework namespaces, and any name that contains a credential substring — the shared {@code
 * BROKER_SENSITIVE_KEY_SUBSTRINGS} list plus {@code apikey}, {@code authorization}, {@code bearer},
 * and {@code principal}.
 */
public final class HeaderPolicy {
  /**
   * Credential substrings reused verbatim from the Rust side: the 7-entry {@code
   * BROKER_SENSITIVE_KEY_SUBSTRINGS} constant followed by the JMS-specific additions. All entries
   * are already normalized (lowercase, no separators) so separator variants such as {@code api-key}
   * still match after key normalization.
   */
  private static final String[] DENIED_CREDENTIAL_SUBSTRINGS = {
    "password",
    "passwd",
    "secret",
    "credential",
    "token",
    "username",
    "user",
    "apikey",
    "authorization",
    "bearer",
    "principal"
  };

  private HeaderPolicy() {}

  /**
   * Returns whether the header name must never reach the broker as a JMS property, regardless of
   * the allowlist.
   */
  public static boolean isDenied(String key) {
    if (key == null) {
      return false;
    }
    String normalized = normalize(key);
    if (normalized.startsWith("jms")) {
      return true;
    }
    if (normalized.startsWith("xcamel")) {
      return true;
    }
    if (normalized.startsWith("camel")) {
      return true;
    }
    for (String needle : DENIED_CREDENTIAL_SUBSTRINGS) {
      if (normalized.contains(needle)) {
        return true;
      }
    }
    return false;
  }

  /**
   * Returns whether the header name is both not denied and present, in normalized form, in the
   * allowlist. An empty or null allowlist allows nothing.
   */
  public static boolean isAllowed(List<String> allowlist, String key) {
    if (key == null || allowlist == null || isDenied(key)) {
      return false;
    }
    String normalized = normalize(key);
    for (String allowed : allowlist) {
      if (allowed != null && normalize(allowed).equals(normalized)) {
        return true;
      }
    }
    return false;
  }

  static String normalize(String key) {
    return key.toLowerCase(Locale.ROOT).replace("-", "").replace("_", "").replace(".", "");
  }
}
