//! Bracket-nesting scanner used before protox parses any text.

/// Maximum bracket nesting depth accepted in `.proto` sources and in
/// descriptor-set option text.
pub(crate) const MAX_NESTING_DEPTH: usize = 64;

/// Which text dialect is scanned.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ScanMode {
    /// `.proto` source: `//` and `/* */` comments, quoted strings.
    ProtoSource,
    /// Descriptor option text (text format): as `ProtoSource`, plus `#`
    /// starts a line comment.
    OptionText,
}

/// Scans `text`. Openers are `{ < [`, closers are `} > ]`; the depth never
/// goes below 0. Returns `Err(depth)` with the first depth above `limit`.
pub(crate) fn scan_nesting(text: &[u8], limit: usize, mode: ScanMode) -> Result<(), usize> {
    let mut depth: usize = 0;
    let mut i = 0;
    while i < text.len() {
        let c = text[i];
        let next = text.get(i + 1).copied();
        if c == b'/' && next == Some(b'/') || (c == b'#' && mode == ScanMode::OptionText) {
            while i < text.len() && text[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if c == b'/' && next == Some(b'*') {
            i += 2;
            while i < text.len() && !(text[i] == b'*' && text.get(i + 1) == Some(&b'/')) {
                i += 1;
            }
            i += 2;
            continue;
        }
        if c == b'"' || c == b'\'' {
            i += 1;
            while i < text.len() {
                match text[i] {
                    b'\\' if text.get(i + 1).is_some_and(|n| *n != b'\n') => i += 2,
                    b'\n' => break,
                    q if q == c => {
                        i += 1;
                        break;
                    }
                    _ => i += 1,
                }
            }
            continue;
        }
        match c {
            b'{' | b'<' | b'[' => {
                depth += 1;
                if depth > limit {
                    return Err(depth);
                }
            }
            b'}' | b'>' | b']' => depth = depth.saturating_sub(1),
            _ => {}
        }
        i += 1;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rep(s: &str, n: usize) -> Vec<u8> {
        s.repeat(n).into_bytes()
    }

    #[test]
    fn ok_at_limit() {
        let mut text = rep("{", 64);
        text.extend_from_slice(&rep("}", 64));
        assert_eq!(scan_nesting(&text, 64, ScanMode::ProtoSource), Ok(()));
    }

    #[test]
    fn err_one_over_limit() {
        assert_eq!(
            scan_nesting(&rep("{", 65), 64, ScanMode::ProtoSource),
            Err(65)
        );
    }

    #[test]
    fn angle_and_square_count() {
        assert_eq!(
            scan_nesting(&rep("<", 65), 64, ScanMode::ProtoSource),
            Err(65)
        );
        assert_eq!(
            scan_nesting(&rep("[", 65), 64, ScanMode::ProtoSource),
            Err(65)
        );
    }

    #[test]
    fn line_comment_ignored() {
        let mut text = b"// ".to_vec();
        text.extend_from_slice(&rep("{", 100));
        text.push(b'\n');
        assert_eq!(scan_nesting(&text, 64, ScanMode::ProtoSource), Ok(()));
    }

    #[test]
    fn block_comment_ignored() {
        let mut text = b"/* ".to_vec();
        text.extend_from_slice(&rep("{", 100));
        text.extend_from_slice(b" */");
        assert_eq!(scan_nesting(&text, 64, ScanMode::ProtoSource), Ok(()));

        let mut unterminated = b"/* ".to_vec();
        unterminated.extend_from_slice(&rep("{", 100));
        assert_eq!(
            scan_nesting(&unterminated, 64, ScanMode::ProtoSource),
            Ok(())
        );
    }

    #[test]
    fn braces_in_string_ignored() {
        let mut double = vec![b'"'];
        double.extend_from_slice(&rep("{", 100));
        double.push(b'"');
        assert_eq!(scan_nesting(&double, 64, ScanMode::ProtoSource), Ok(()));

        let mut single = vec![b'\''];
        single.extend_from_slice(&rep("{", 100));
        single.push(b'\'');
        assert_eq!(scan_nesting(&single, 64, ScanMode::ProtoSource), Ok(()));
    }

    #[test]
    fn string_ends_at_newline() {
        let mut text = b"x = \"abc\n".to_vec();
        text.extend_from_slice(&rep("{", 100));
        assert_eq!(scan_nesting(&text, 64, ScanMode::ProtoSource), Err(65));
    }

    #[test]
    fn escaped_quote_does_not_end_string() {
        let mut text = br#"x = "\"" "#.to_vec();
        text.extend_from_slice(&rep("{", 100));
        assert_eq!(scan_nesting(&text, 64, ScanMode::ProtoSource), Err(65));

        // `x = "\"` + 70 `{` + `"` + newline: every opener sits inside the
        // string (the `\"` escape must not terminate it early).
        let mut inside = br#"x = "\""#.to_vec();
        inside.extend_from_slice(&rep("{", 70));
        inside.push(b'"');
        inside.push(b'\n');
        assert_eq!(scan_nesting(&inside, 64, ScanMode::ProtoSource), Ok(()));
    }

    #[test]
    fn backslash_before_newline_does_not_extend_string() {
        // A backslash directly before a newline does not consume it; the
        // string ends at the newline, so the following openers are counted.
        let mut text = b"x = \"abc\\\n".to_vec();
        text.extend_from_slice(&rep("{", 70));
        assert_eq!(scan_nesting(&text, 64, ScanMode::ProtoSource), Err(65));
    }

    #[test]
    fn closers_never_go_negative() {
        let mut text = rep("}", 100);
        text.extend_from_slice(&rep("{", 65));
        assert_eq!(scan_nesting(&text, 64, ScanMode::ProtoSource), Err(65));
    }

    #[test]
    fn hash_comment_only_in_option_text() {
        let text = rep("f < # >\n", 100);
        assert_eq!(scan_nesting(&text, 64, ScanMode::OptionText), Err(65));
        assert_eq!(scan_nesting(&text, 64, ScanMode::ProtoSource), Ok(()));
    }

    #[test]
    fn typical_proto_passes() {
        let text = b"map<string, string> m = 1 [deprecated = true]; message A { message B { } }";
        assert_eq!(scan_nesting(text, 64, ScanMode::ProtoSource), Ok(()));
    }
}
