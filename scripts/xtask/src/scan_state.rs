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

/// True for attribute lines that open a test scope: `#[cfg(test)]`,
/// `#[test]`, `#[tokio::test]`, and `#[cfg(all(...))]` / `#[cfg(any(...))]`
/// conjunctions whose DIRECT predicate list contains `test` (e.g.
/// `#[cfg(all(test, feature = "llm"))]`, used by camel-component-api).
/// Nested predicates do not count: `#[cfg(not(test))]` compiles its body
/// in non-test builds and stays production scope.
pub fn is_test_attr_line(trimmed: &str) -> bool {
    trimmed.starts_with("#[cfg(test)]")
        || trimmed.starts_with("#[test]")
        || trimmed.starts_with("#[tokio::test]")
        || cfg_conjunction_has_direct_test_predicate(trimmed)
}

/// `#[cfg(all/any(...))]` where a top-level predicate of the conjunction
/// is exactly `test`. Nested conjunctions (e.g. `all(test, any(...))`)
/// still count — the `test` predicate is direct; `not(test)` does not.
fn cfg_conjunction_has_direct_test_predicate(trimmed: &str) -> bool {
    let Some(rest) = trimmed.strip_prefix("#[cfg(") else {
        return false;
    };
    let Some(body) = rest
        .strip_prefix("all(")
        .or_else(|| rest.strip_prefix("any("))
    else {
        return false;
    };
    // The conjunction body runs to its matching close paren; predicates
    // are its top-level comma-separated segments.
    let mut depth = 1usize;
    let mut end = body.len();
    for (i, ch) in body.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    end = i;
                    break;
                }
            }
            _ => {}
        }
    }
    body[..end].split(',').any(|pred| pred.trim() == "test")
}

/// Literal/comment-aware test-scope tracker for the line-based lints
/// (rc-xkx42 extraction): [`scan_line`] brace counting plus the
/// pending-test-attribute handshake, previously duplicated verbatim in
/// `lint_unwrap_src`, `lint_cancel_tokens`, and `lint_log_levels`.
///
/// Feed every line's TRIMMED text to [`TestScopeTracker::line_in_test_scope`]
/// in file order; it answers whether the line is exempt from
/// production-scope checks. All consumers share the canonical attribute
/// set of [`is_test_attr_line`].
pub struct TestScopeTracker {
    scan: ScanState,
    pending_test_attr: bool,
    test_scope_entry_depth: Option<i32>,
    brace_depth: i32,
}

impl Default for TestScopeTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl TestScopeTracker {
    /// Fresh tracker for the start of a file.
    pub fn new() -> Self {
        Self {
            scan: ScanState::Normal,
            pending_test_attr: false,
            test_scope_entry_depth: None,
            brace_depth: 0,
        }
    }

    /// Feed one TRIMMED source line; returns `true` when the line is a
    /// test-scope attribute line, the line that opens the test scope, or a
    /// line inside it — i.e. the caller must skip it for production-scope
    /// checks.
    pub fn line_in_test_scope(&mut self, trimmed: &str) -> bool {
        if self.test_scope_entry_depth.is_none() && is_test_attr_line(trimmed) {
            self.pending_test_attr = true;
        }

        // Captured BEFORE the scan: a line that both opens and closes a
        // test scope (one-liner `fn t() { .. }` under the attribute) is
        // itself inside the scope even though the depth balances out.
        let entering = self.pending_test_attr && self.test_scope_entry_depth.is_none();

        let scan = &mut self.scan;
        let pending = &mut self.pending_test_attr;
        let entry = &mut self.test_scope_entry_depth;
        let depth = &mut self.brace_depth;
        scan_line(trimmed, scan, &mut |brace| match brace {
            Brace::Open => {
                *depth += 1;
                if *pending && entry.is_none() {
                    *entry = Some(*depth - 1);
                    *pending = false;
                }
            }
            Brace::Close => {
                *depth -= 1;
                if let Some(e) = *entry
                    && *depth <= e
                {
                    *entry = None;
                }
            }
        });

        // An attribute on a non-block item (`#[test] fn f();`-style) never
        // opens a scope.
        if self.pending_test_attr && self.test_scope_entry_depth.is_none() && trimmed.contains(';')
        {
            self.pending_test_attr = false;
        }

        self.pending_test_attr || entering || self.test_scope_entry_depth.is_some()
    }

    /// The scanner is in normal code context at end of line. Callers use it
    /// to distinguish a pure comment line from one inside a multi-line
    /// block comment.
    pub fn in_normal_context(&self) -> bool {
        self.scan == ScanState::Normal
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

    /// Feed a whole source through a tracker, returning per-line
    /// in-test-scope flags.
    fn scope_flags(src: &str) -> Vec<bool> {
        let mut t = TestScopeTracker::new();
        src.lines()
            .map(|l| t.line_in_test_scope(l.trim()))
            .collect()
    }

    #[test]
    fn tracker_covers_cfg_test_mod_and_reopens_production() {
        let flags = scope_flags(
            "fn prod() {}\n#[cfg(test)]\nmod tests {\n    fn t() {}\n}\nfn after() {}\n",
        );
        // The mod's closing-brace line itself returns false (the scope
        // exits while scanning it) — harmless: no code to flag on it.
        assert_eq!(
            flags,
            vec![false, true, true, true, false, false],
            "attr + mod body skipped; production resumes after the closing brace"
        );
    }

    #[test]
    fn tracker_attr_on_non_block_item_only_skips_attr_line() {
        // The attribute's `;`-terminated target line is skipped (entering),
        // but never OPENS a scope: production resumes on the next line.
        let flags = scope_flags("#[test]\nextern crate foo;\nfn prod() {}\n");
        assert_eq!(flags, vec![true, true, false]);
    }

    #[test]
    fn tracker_recognizes_tokio_test_and_cfg_conjunctions() {
        let flags = scope_flags("#[tokio::test]\nasync fn t() {}\nfn prod() {}\n");
        assert_eq!(flags, vec![true, true, false]);

        let flags = scope_flags("#[cfg(all(test, feature = \"llm\"))]\nmod m {}\nfn prod() {}\n");
        assert_eq!(flags, vec![true, true, false]);
    }

    #[test]
    fn tracker_does_not_treat_not_test_as_test_scope() {
        // `not(test)` compiles its body in production builds.
        let flags = scope_flags("#[cfg(not(test))]\nfn prod_only() {}\nfn prod() {}\n");
        assert_eq!(flags, vec![false, false, false]);
    }

    #[test]
    fn tracker_string_literal_close_brace_does_not_exit_scope() {
        let flags = scope_flags(
            "#[cfg(test)]\nmod tests {\n    let s = \"}\";\n    fn u() {}\n}\nfn prod() {}\n",
        );
        assert_eq!(
            flags,
            vec![true, true, true, true, false, false],
            "literal close-brace must not close the test scope early"
        );
    }

    #[test]
    fn tracker_one_liner_test_fn_line_is_skipped() {
        // Same-line open+close under the attribute: the fn line is still
        // inside the test scope (the pre-scan `entering` capture).
        let flags = scope_flags("#[test]\nfn t() { prod_code(); }\nfn prod() {}\n");
        assert_eq!(flags, vec![true, true, false]);
    }
}
