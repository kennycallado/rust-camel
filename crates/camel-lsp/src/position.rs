use tower_lsp::lsp_types::Position;

/// Convert a 0-based byte offset into an LSP Position.
///
/// Walks the source char-by-char, tracking 0-based line number and
/// UTF-16 code unit count within the current line. Characters outside
/// the Basic Multilingual Plane (`c as u32 > 0xFFFF`) contribute
/// 2 UTF-16 code units; all others contribute 1.
///
/// `byte_offset` is clamped to `source.len()` and then snapped DOWN to the
/// nearest UTF-8 character boundary, so a mid-character offset (e.g. a lint
/// span whose end lands inside a multi-byte char) never panics the `source[..end]`
/// slice. This makes the conversion total over every input, as the LSP spec
/// requires — malformed and non-ASCII input must never crash the server.
pub fn byte_offset_to_lsp(source: &str, byte_offset: usize) -> Position {
    let mut end = byte_offset.min(source.len());
    while end > 0 && !source.is_char_boundary(end) {
        end -= 1;
    }
    let mut line = 0u32;
    let mut character = 0u32;

    for c in source[..end].chars() {
        match c {
            '\n' => {
                line += 1;
                character = 0;
            }
            _ => {
                character += if (c as u32) > 0xFFFF { 2 } else { 1 };
            }
        }
    }

    Position { line, character }
}

/// Convert an LSP Position back to a 0-based byte offset.
///
/// Returns `source.len()` for positions that are out of bounds
/// (line or character past the end of the source). Never panics.
pub fn lsp_to_byte_offset(source: &str, position: Position) -> usize {
    let mut current_line = 0u32;
    let mut current_character = 0u32;
    let mut byte_offset = 0usize;

    for c in source.chars() {
        // Check if we've reached the target position
        if current_line == position.line && current_character == position.character {
            return byte_offset;
        }

        match c {
            '\n' => {
                // If target line is earlier than current, return current offset
                if position.line < current_line {
                    return byte_offset;
                }
                // If target line matches but character is beyond EOL, return this position
                if position.line == current_line && position.character > current_character {
                    return byte_offset;
                }
                current_line += 1;
                current_character = 0;
            }
            _ => {
                current_character += if (c as u32) > 0xFFFF { 2 } else { 1 };
            }
        }
        byte_offset += c.len_utf8();
    }

    // Reached end of source — return source.len() regardless
    source.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_offset_to_lsp_basic() {
        let source = "hello\nworld\n";
        let pos = byte_offset_to_lsp(source, 7);
        assert_eq!(
            pos,
            Position {
                line: 1,
                character: 1
            }
        );
    }

    #[test]
    fn byte_offset_to_lsp_non_ascii() {
        // é = U+00E9: 2 UTF-8 bytes, 1 UTF-16 code unit
        // "café\n" = 6 bytes: c(1) a(1) f(1) é(2) \n(1)
        let source = "café\ntest\n";
        let pos = byte_offset_to_lsp(source, 5);
        assert_eq!(
            pos,
            Position {
                line: 0,
                character: 4
            }
        );
    }

    #[test]
    fn byte_offset_to_lsp_emoji() {
        // 🌟 = U+1F31F: 4 UTF-8 bytes, 2 UTF-16 code units (surrogate pair)
        // "🌟\n" = 5 bytes: 🌟(4) \n(1)
        let source = "🌟\nx\n";
        let pos = byte_offset_to_lsp(source, 4);
        assert_eq!(
            pos,
            Position {
                line: 0,
                character: 2
            }
        );
    }

    #[test]
    fn lsp_to_byte_offset_roundtrip() {
        let source = "hello\nworld\n";
        let offset = lsp_to_byte_offset(
            source,
            Position {
                line: 1,
                character: 1,
            },
        );
        assert_eq!(offset, 7);
    }

    #[test]
    fn lsp_to_byte_offset_out_of_bounds() {
        let source = "hi\n";
        let offset = lsp_to_byte_offset(
            source,
            Position {
                line: 99,
                character: 99,
            },
        );
        assert_eq!(offset, source.len());
    }

    #[test]
    fn byte_offset_to_lsp_mid_multibyte_char_does_not_panic() {
        // Regression: a lint span (e.g. R-SYN's `start + 1` arithmetic) can
        // land inside a multi-byte char. `だ` occupies bytes 20..23 in this
        // string; offset 21 is mid-char. The conversion must snap down to the
        // char boundary (byte 20) instead of panicking on `source[..21]`.
        let source = "root:\n  child: val\n\tだめ: x";
        assert!(!source.is_char_boundary(21), "precondition: 21 is mid-char");
        let pos = byte_offset_to_lsp(source, 21);
        // Byte 20 is column 0 of line 2 (after the tab, at the start of `だ`).
        // The tab counts as one UTF-16 unit, so the char index is 1.
        assert_eq!(
            pos,
            Position {
                line: 2,
                character: 1
            }
        );
    }

    #[test]
    fn byte_offset_to_lsp_mid_emoji_does_not_panic() {
        // 🌟 occupies bytes 0..4; offsets 1,2,3 are all mid-char and must all
        // snap down to byte 0 (character 0) without panicking.
        let source = "🌟x";
        for off in [1usize, 2, 3] {
            let pos = byte_offset_to_lsp(source, off);
            assert_eq!(
                pos,
                Position {
                    line: 0,
                    character: 0
                }
            );
        }
    }

    #[test]
    fn byte_offset_to_lsp_clamped() {
        let source = "ab";
        let pos = byte_offset_to_lsp(source, 999);
        assert_eq!(
            pos,
            Position {
                line: 0,
                character: 2
            }
        );
    }

    #[test]
    fn lsp_to_byte_offset_zero() {
        let source = "hello";
        let offset = lsp_to_byte_offset(
            source,
            Position {
                line: 0,
                character: 0,
            },
        );
        assert_eq!(offset, 0);
    }

    #[test]
    fn roundtrip_multi_line() {
        let source = "line1\nline2\nline3\n";
        for offset in 0..source.len() {
            let pos = byte_offset_to_lsp(source, offset);
            let back = lsp_to_byte_offset(source, pos);
            // Roundtrip may not be exact for offsets inside multi-byte chars or newlines,
            // but back-to-forward should land on the same line/character
            let pos2 = byte_offset_to_lsp(source, back);
            assert_eq!(
                pos, pos2,
                "roundtrip mismatch at offset {offset}: {pos:?} -> {back} -> {pos2:?}"
            );
        }
    }

    #[test]
    fn lsp_to_byte_offset_past_eol() {
        // target character past end of line 0
        let source = "ab\ncd\n";
        let offset = lsp_to_byte_offset(
            source,
            Position {
                line: 0,
                character: 99,
            },
        );
        // Should return byte offset of the newline
        assert_eq!(offset, 2);
    }

    #[test]
    fn lsp_to_byte_offset_emoji_roundtrip() {
        let source = "🌟\nx\n";
        let pos = byte_offset_to_lsp(source, 4); // \n after emoji
        assert_eq!(
            pos,
            Position {
                line: 0,
                character: 2
            }
        );
        let offset = lsp_to_byte_offset(source, pos);
        assert_eq!(offset, 4);
    }
}
