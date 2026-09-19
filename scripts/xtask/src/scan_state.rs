//! Shared literal/comment-aware line scanner for the xtask lints.
//!
//! Extracted from `lint_unwrap_src` (rc-4fs) by rc-drcgf so every
//! line-based lint counts braces by the same rules: a `{` or `}` inside a
//! string literal, char literal, raw string, line comment, or block
//! comment is NOT a brace event. Before this module, `lint_cancel_tokens`
//! counted raw characters, so a `"}"` in a test mod closed the test scope
//! early (spurious sites) and a `// {` comment pinned it open (missed
//! sites).
//!
//! The state persists ACROSS lines and is owned by the caller: pass the
//! same `&mut ScanState` for every line of the file. `StringLit`,
//! `CharLit`, `BlockComment`, and `RawStr` can span lines; `LineComment`
//! ends at the newline (reset inside [`scan_line`]).

/// Character-level scan state. Persists across the lines of one file.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ScanState {
    Normal,
    StringLit,
    CharLit,
    LineComment,
    BlockComment,
    RawStr(usize),
}

/// A brace seen in normal code context (never inside a literal/comment).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Brace {
    Open,
    Close,
}

/// Advance `state` over one line, invoking `on_brace` for every `{` / `}`
/// in normal code context. Callers pass their trimmed line, matching the
/// original inline scanners.
pub fn scan_line(line: &str, state: &mut ScanState, on_brace: &mut impl FnMut(Brace)) {
    let mut chars = line.chars().peekable();
    while let Some(ch) = chars.next() {
        match state {
            ScanState::Normal => match ch {
                '{' => on_brace(Brace::Open),
                '}' => on_brace(Brace::Close),
                '/' if chars.peek() == Some(&'/') => {
                    *state = ScanState::LineComment;
                    chars.next();
                }
                '/' if chars.peek() == Some(&'*') => {
                    *state = ScanState::BlockComment;
                    chars.next();
                }
                '"' => {
                    *state = ScanState::StringLit;
                }
                '\'' => {
                    // Distinguish char literal ('a', '\n') from lifetime
                    // ('a, 'static, '_).
                    let next = chars.peek().copied();
                    let is_lifetime = next
                        .map(|c| c.is_ascii_alphanumeric() || c == '_')
                        .unwrap_or(false)
                        && {
                            let mut tmp = chars.clone();
                            tmp.next();
                            !matches!(tmp.peek(), Some('\'') | Some('\\'))
                        };
                    if !is_lifetime {
                        *state = ScanState::CharLit;
                    }
                }
                'r' => {
                    // Potential raw string: r"..." or r#"..."# etc.
                    let mut hash_count: usize = 0;
                    let mut lookahead = chars.clone();
                    while lookahead.peek() == Some(&'#') {
                        hash_count += 1;
                        lookahead.next();
                    }
                    if lookahead.peek() == Some(&'"') {
                        *state = ScanState::RawStr(hash_count);
                        for _ in 0..hash_count {
                            chars.next();
                        }
                        chars.next();
                    }
                }
                _ => {}
            },
            ScanState::StringLit => match ch {
                '\\' => {
                    chars.next();
                }
                '"' => {
                    *state = ScanState::Normal;
                }
                _ => {}
            },
            ScanState::CharLit => match ch {
                '\\' => {
                    chars.next();
                }
                '\'' => {
                    *state = ScanState::Normal;
                }
                _ => {}
            },
            ScanState::LineComment => {
                // Consume remaining chars; state resets at EOL below.
            }
            ScanState::BlockComment => {
                if ch == '*' && chars.peek() == Some(&'/') {
                    *state = ScanState::Normal;
                    chars.next();
                }
            }
            ScanState::RawStr(n) => {
                if ch == '"' {
                    let mut count = 0;
                    let mut lookahead = chars.clone();
                    while lookahead.peek() == Some(&'#') {
                        count += 1;
                        lookahead.next();
                    }
                    if count >= *n {
                        *state = ScanState::Normal;
                        for _ in 0..count {
                            chars.next();
                        }
                    }
                }
            }
        }
    }

    // Line comments end at the newline boundary.
    if *state == ScanState::LineComment {
        *state = ScanState::Normal;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Run `scan_line` over every line of `src` and collect brace events.
    fn events(src: &str) -> Vec<Brace> {
        let mut state = ScanState::Normal;
        let mut ev = Vec::new();
        for line in src.lines() {
            scan_line(line, &mut state, &mut |b| ev.push(b));
        }
        ev
    }

    #[test]
    fn code_braces_reported_in_order() {
        assert_eq!(
            events("fn a() { if x { } }\n"),
            vec![Brace::Open, Brace::Open, Brace::Close, Brace::Close]
        );
    }

    #[test]
    fn string_literal_braces_ignored() {
        assert_eq!(
            events("fn a() { let s = \"{}\"; }\n"),
            vec![Brace::Open, Brace::Close]
        );
    }

    #[test]
    fn escaped_quote_does_not_end_string() {
        // `\"` inside a string keeps the scanner in StringLit; the brace
        // after it is still literal content.
        assert_eq!(events("let s = \"\\\"{\";\n"), vec![]);
    }

    #[test]
    fn char_literal_braces_ignored() {
        assert_eq!(
            events("fn a() { let c = '{'; }\n"),
            vec![Brace::Open, Brace::Close]
        );
    }

    #[test]
    fn lifetime_is_not_char_literal() {
        // `'a` is a lifetime; without the disambiguation the scanner would
        // enter CharLit and swallow the following real brace.
        assert_eq!(
            events("fn a<'a>(x: &'a str) { }\n"),
            vec![Brace::Open, Brace::Close]
        );
    }

    #[test]
    fn line_comment_braces_ignored() {
        assert_eq!(
            events("fn a() {\n    // } closes nothing\n}\n"),
            vec![Brace::Open, Brace::Close]
        );
    }

    #[test]
    fn block_comment_spans_lines() {
        assert_eq!(
            events("fn a() { /* { }\n still comment */ }\n"),
            vec![Brace::Open, Brace::Close]
        );
    }

    #[test]
    fn raw_string_with_hashes_ignored() {
        assert_eq!(
            events("fn a() { let s = r#\"{\"#; }\n"),
            vec![Brace::Open, Brace::Close]
        );
    }

    #[test]
    fn unterminated_string_carries_state_to_next_line() {
        let mut state = ScanState::Normal;
        let mut ev = Vec::new();
        scan_line("let s = \"}", &mut state, &mut |b| ev.push(b));
        assert_eq!(state, ScanState::StringLit);
        scan_line("}\"; fn a() { }", &mut state, &mut |b| ev.push(b));
        // The `}` before the closing quote is literal; after `"`, code
        // resumes.
        assert_eq!(ev, vec![Brace::Open, Brace::Close]);
        assert_eq!(state, ScanState::Normal);
    }
}
