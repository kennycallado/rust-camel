package org.rustcamel.cxf;

/** Java side of the cross-layer blank TLS-value policy (N4). Package-private by design. */
final class BlankTlsValue {

  private BlankTlsValue() {}

  /**
   * True when {@code value} is non-null and every codepoint is Unicode {@code White_Space} (the
   * empty string is blank). This is the exact set Rust's {@code char::is_whitespace} implements and
   * the Rust config validation uses — NOT {@code String.isBlank} ({@code Character.isWhitespace}),
   * which excludes NBSP U+00A0, U+2007 and U+202F while adding U+001C-U+001F. Both layers must
   * classify every codepoint identically, so this predicate encodes the Unicode set explicitly. A
   * {@code null} value is genuinely omitted and never blank; a nonblank value is never trimmed
   * (meaningful password spaces survive verbatim).
   */
  static boolean isBlank(String value) {
    return value != null && value.codePoints().allMatch(BlankTlsValue::isWhiteSpaceCodePoint);
  }

  /** The 25 Unicode {@code White_Space} codepoints. Shared with the SOAP-header validator. */
  static boolean isWhiteSpaceCodePoint(int codePoint) {
    switch (codePoint) {
      case '\t': // U+0009
      case '\n': // U+000A
      case '\u000B': // U+000B
      case '\f': // U+000C
      case '\r': // U+000D
      case ' ': // U+0020
      case '\u0085': // NEL
      case '\u00A0': // NBSP
      case '\u1680': // OGHAM SPACE MARK
      case '\u2028': // LINE SEPARATOR
      case '\u2029': // PARAGRAPH SEPARATOR
      case '\u202F': // NARROW NBSP
      case '\u205F': // MEDIUM MATHEMATICAL SPACE
      case '\u3000': // IDEOGRAPHIC SPACE
        return true;
      default:
        return codePoint >= '\u2000' && codePoint <= '\u200A'; // EN QUAD .. HAIR SPACE
    }
  }
}
