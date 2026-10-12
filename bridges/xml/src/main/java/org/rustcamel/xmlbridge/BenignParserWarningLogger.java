package org.rustcamel.xmlbridge;

import java.util.Set;
import net.sf.saxon.lib.Logger;
import net.sf.saxon.lib.StandardLogger;

/**
 * Saxon logger that drops two known-benign diagnostics and forwards everything else.
 *
 * <p>Saxon applies its configured parser settings ({@code accessExternalDTD} property, DTD {@code
 * validation} feature) to the hardened Xerces reader that {@code secureSaxSource} supplies. Xerces
 * does not recognize those two names at reader level and Saxon prints a warning on every parse. The
 * effective hardening does not depend on them: the reader already disables external entities,
 * disallows DOCTYPE, caps expansion and uses an empty entity resolver. Only the exact messages for
 * these two names are dropped; any other warning still reaches stderr.
 */
final class BenignParserWarningLogger extends StandardLogger {
  private static final String PREFIX = "Selected XML parser ";
  private static final String PROPERTY_MARKER = " does not recognize the property ";
  private static final String FEATURE_MARKER = " does not recognize the feature ";

  private static final Set<String> BENIGN_NAMES =
      Set.of(
          "http://javax.xml.XMLConstants/property/accessExternalDTD",
          "http://xml.org/sax/features/validation");

  static boolean isBenign(String message) {
    if (message == null || !message.startsWith(PREFIX)) {
      return false;
    }
    for (var marker : new String[] {PROPERTY_MARKER, FEATURE_MARKER}) {
      int at = message.indexOf(marker);
      if (at > 0 && BENIGN_NAMES.contains(message.substring(at + marker.length()).trim())) {
        return true;
      }
    }
    return false;
  }

  @Override
  public void println(String message, int severity) {
    if (severity == Logger.WARNING && isBenign(message)) {
      return;
    }
    super.println(message, severity);
  }
}
