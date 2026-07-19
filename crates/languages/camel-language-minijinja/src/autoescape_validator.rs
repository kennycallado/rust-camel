//! Lexical validator for the S7 autoescape-wrapper contract.
//!
//! MiniJinja has no stable public AST walk, so this is a source-level
//! tokeniser that recognises `{% %}`, `{# #}`, `{{ }}`, and
//! `{% raw %}...{% endraw %}` blocks. The validator must ignore tag-like
//! text inside comments, raw blocks, and string literals.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AutoescapeError {
    MissingWrapper,
    InvalidMode(String),
    NotEnclosingWholeTemplate,
    DuplicateWrapper,
    MalformedSyntax(String),
}

impl fmt::Display for AutoescapeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingWrapper => write!(
                f,
                "template must be wrapped in exactly one {{% autoescape %}} block"
            ),
            Self::InvalidMode(m) => write!(f, "invalid autoescape mode: {m}"),
            Self::NotEnclosingWholeTemplate => {
                write!(f, "autoescape wrapper must enclose the entire template")
            }
            Self::DuplicateWrapper => write!(f, "multiple autoescape wrappers are not allowed"),
            Self::MalformedSyntax(s) => write!(f, "malformed template syntax: {s}"),
        }
    }
}

impl std::error::Error for AutoescapeError {}

pub fn validate_autoescape_wrapper(source: &str) -> Result<(), AutoescapeError> {
    let tokens = tokenize(source)?;
    walk(&tokens)
}

#[derive(Debug, Clone)]
enum Token {
    Statement(String),
    RawOpen,
    RawClose,
    Expression,
    Comment,
    Text { ws_only: bool },
    Malformed(&'static str),
}

/// Byte-level tokeniser. Tracks string literals and backslash escapes so
/// tag-like substrings inside `{% %}` and `{{ }}` blocks do not falsely
/// terminate the block.
fn tokenize(source: &str) -> Result<Vec<Token>, AutoescapeError> {
    let bytes = source.as_bytes();
    let mut tokens: Vec<Token> = Vec::new();
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i..].starts_with(b"{#") {
            // Comment: scan for `#}` literally (no string state).
            match find_subsequence(&bytes[i + 2..], b"#}") {
                Some(p) => {
                    i += 2 + p + 2;
                    tokens.push(Token::Comment);
                }
                None => {
                    tokens.push(Token::Malformed("comment"));
                    i = bytes.len();
                }
            }
        } else if bytes[i..].starts_with(b"{%") {
            // Statement: string-aware scan for `%}`.
            match scan_block(&bytes[i + 2..], b"%}") {
                ScanOutcome::Found(end) => {
                    let body = &source[i + 2..i + 2 + end];
                    let body = body.trim();
                    let tok = classify_statement(body);
                    i += 2 + end + 2;
                    tokens.push(tok);
                }
                ScanOutcome::Missing => {
                    tokens.push(Token::Malformed("statement"));
                    i = bytes.len();
                }
            }
        } else if bytes[i..].starts_with(b"{{") {
            // Expression: string-aware scan for `}}`.
            match scan_block(&bytes[i + 2..], b"}}") {
                ScanOutcome::Found(end) => {
                    i += 2 + end + 2;
                    tokens.push(Token::Expression);
                }
                ScanOutcome::Missing => {
                    tokens.push(Token::Malformed("expression"));
                    i = bytes.len();
                }
            }
        } else {
            // Text: accumulate until next `{%`, `{{`, `{#`, or EOF.
            // A bare `{` (not followed by `%`, `{`, or `#`) is literal text.
            let start = i;
            while i < bytes.len()
                && !bytes[i..].starts_with(b"{%")
                && !bytes[i..].starts_with(b"{{")
                && !bytes[i..].starts_with(b"{#")
            {
                i += 1;
            }
            let text = &source[start..i];
            let ws_only = text.bytes().all(|b| b.is_ascii_whitespace());
            tokens.push(Token::Text { ws_only });
        }
    }

    Ok(tokens)
}

fn classify_statement(body: &str) -> Token {
    if body == "raw" || body.starts_with("raw ") || body.starts_with("raw\t") {
        Token::RawOpen
    } else if body == "endraw" || body.starts_with("endraw ") || body.starts_with("endraw\t") {
        Token::RawClose
    } else {
        Token::Statement(body.to_string())
    }
}

enum ScanOutcome {
    Found(usize),
    Missing,
}

/// Scan `slice` for the closing delimiter while respecting string literals
/// and backslash escapes. Returns the byte offset of the closing delimiter's
/// first byte within `slice`, or `Missing` if EOF is reached.
fn scan_block(slice: &[u8], close: &[u8; 2]) -> ScanOutcome {
    debug_assert_eq!(close.len(), 2);
    let mut j = 0;
    let mut in_string: Option<u8> = None;
    let mut escaped = false;

    while j < slice.len() {
        let b = slice[j];
        if escaped {
            escaped = false;
            j += 1;
            continue;
        }
        match in_string {
            None => {
                if b == b'\'' {
                    in_string = Some(b'\'');
                } else if b == b'"' {
                    in_string = Some(b'"');
                } else if b == b'\\' {
                    // backslash outside a string is just a normal char.
                } else if b == close[0] && j + 1 < slice.len() && slice[j + 1] == close[1] {
                    return ScanOutcome::Found(j);
                }
            }
            Some(q) => {
                if b == b'\\' {
                    escaped = true;
                } else if b == q {
                    in_string = None;
                }
            }
        }
        j += 1;
    }

    ScanOutcome::Missing
}

fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return if needle.is_empty() { Some(0) } else { None };
    }
    for k in 0..=haystack.len() - needle.len() {
        if &haystack[k..k + needle.len()] == needle {
            return Some(k);
        }
    }
    None
}

fn walk(tokens: &[Token]) -> Result<(), AutoescapeError> {
    let mut raw_depth: u32 = 0;
    let mut wrapper_seen: Option<String> = None;
    let mut closer_seen: bool = false;
    let mut bad_text_outside: bool = false;

    for tok in tokens {
        match tok {
            Token::Malformed(s) => return Err(AutoescapeError::MalformedSyntax((*s).to_string())),
            Token::RawOpen => raw_depth += 1,
            Token::RawClose => raw_depth = raw_depth.saturating_sub(1),
            Token::Comment => {}
            Token::Expression => {}
            Token::Text { ws_only: false } if raw_depth == 0 && wrapper_seen.is_none() => {
                bad_text_outside = true;
            }
            Token::Text { ws_only: false } if raw_depth == 0 && closer_seen => {
                bad_text_outside = true;
            }
            Token::Text { .. } => {}
            Token::Statement(body) if raw_depth > 0 => {}
            Token::Statement(body) => {
                let trimmed = body.trim_start();
                if let Some(rest) = trimmed.strip_prefix("autoescape") {
                    if rest.is_empty() || rest.starts_with(char::is_whitespace) {
                        if wrapper_seen.is_some() {
                            return Err(AutoescapeError::DuplicateWrapper);
                        }
                        let rest_trimmed = rest.trim_start();
                        let mode = parse_mode(rest_trimmed)?;
                        wrapper_seen = Some(mode);
                    }
                } else if trimmed == "endautoescape" {
                    if wrapper_seen.is_none() || closer_seen {
                        return Err(AutoescapeError::MissingWrapper);
                    }
                    closer_seen = true;
                }
            }
        }
    }

    if bad_text_outside && wrapper_seen.is_some() {
        return Err(AutoescapeError::NotEnclosingWholeTemplate);
    }

    match (wrapper_seen, closer_seen) {
        (Some(_), true) => Ok(()),
        (Some(_), false) => Err(AutoescapeError::NotEnclosingWholeTemplate),
        (None, _) => Err(AutoescapeError::MissingWrapper),
    }
}

fn parse_mode(rest: &str) -> Result<String, AutoescapeError> {
    let trimmed = rest.trim_start();
    let bytes = trimmed.as_bytes();
    if bytes.is_empty() {
        return Err(AutoescapeError::InvalidMode(rest.to_string()));
    }
    let quote = bytes[0];
    if quote != b'"' && quote != b'\'' {
        return Err(AutoescapeError::InvalidMode(rest.to_string()));
    }
    let close = trimmed[1..]
        .find(quote as char)
        .ok_or_else(|| AutoescapeError::InvalidMode(rest.to_string()))?;
    let mode = &trimmed[1..1 + close];
    match mode {
        "html" | "json" | "none" => Ok(mode.to_string()),
        _ => Err(AutoescapeError::InvalidMode(rest.to_string())),
    }
}

#[cfg(test)]
mod autoescape_validator_tests {
    use super::{AutoescapeError, validate_autoescape_wrapper};

    fn ok(src: &str) {
        assert!(
            validate_autoescape_wrapper(src).is_ok(),
            "should accept: {src}"
        );
    }

    #[test]
    fn accepts_html() {
        ok("{% autoescape \"html\" %}x{{ n }}{% endautoescape %}");
    }

    #[test]
    fn accepts_json() {
        ok("{% autoescape \"json\" %}x{% endautoescape %}");
    }

    #[test]
    fn accepts_none() {
        ok("{% autoescape \"none\" %}x{% endautoescape %}");
    }

    #[test]
    fn rejects_missing_wrapper() {
        assert_eq!(
            validate_autoescape_wrapper("hi {{ name }}"),
            Err(AutoescapeError::MissingWrapper)
        );
    }

    #[test]
    fn rejects_unquoted_mode() {
        assert!(matches!(
            validate_autoescape_wrapper("{% autoescape html %}x{% endautoescape %}"),
            Err(AutoescapeError::InvalidMode(_))
        ));
    }

    #[test]
    fn rejects_unknown_mode() {
        assert!(matches!(
            validate_autoescape_wrapper("{% autoescape \"sql\" %}x{% endautoescape %}"),
            Err(AutoescapeError::InvalidMode(_))
        ));
    }

    #[test]
    fn rejects_trailing_text() {
        assert_eq!(
            validate_autoescape_wrapper("{% autoescape \"html\" %}x{% endautoescape %} trailing"),
            Err(AutoescapeError::NotEnclosingWholeTemplate)
        );
    }

    #[test]
    fn rejects_leading_text() {
        assert_eq!(
            validate_autoescape_wrapper("leading {% autoescape \"html\" %}x{% endautoescape %}"),
            Err(AutoescapeError::NotEnclosingWholeTemplate)
        );
    }

    #[test]
    fn rejects_duplicate_wrappers() {
        let src = "{% autoescape \"html\" %}{% autoescape \"html\" %}x{% endautoescape %}{% endautoescape %}";
        assert_eq!(
            validate_autoescape_wrapper(src),
            Err(AutoescapeError::DuplicateWrapper)
        );
    }

    #[test]
    fn rejects_nested_wrappers() {
        let src = "{% autoescape \"html\" %}inner {% autoescape \"json\" %}x{% endautoescape %}{% endautoescape %}";
        assert_eq!(
            validate_autoescape_wrapper(src),
            Err(AutoescapeError::DuplicateWrapper)
        );
    }

    #[test]
    fn ignores_tag_inside_comment() {
        ok("{# {% autoescape html %} #}{% autoescape \"html\" %}x{% endautoescape %}");
    }

    #[test]
    fn rejects_when_only_occurrence_is_inside_comment() {
        assert_eq!(
            validate_autoescape_wrapper("{# {% autoescape html %} #}x"),
            Err(AutoescapeError::MissingWrapper)
        );
    }

    #[test]
    fn ignores_tag_inside_raw_block() {
        ok(
            "{% raw %}{% autoescape html %}{% endraw %}{% autoescape \"html\" %}x{% endautoescape %}",
        );
    }

    #[test]
    fn ignores_tag_inside_string_literal() {
        ok("{% autoescape \"none\" %}{{ \"{% autoescape html %}\" }}{% endautoescape %}");
    }

    #[test]
    fn ignores_tag_inside_statement_string_literal() {
        ok(
            "{% autoescape \"none\" %}{% if headers.x == \"{% autoescape html %}\" %}match{% endif %}{% endautoescape %}",
        );
    }

    #[test]
    fn ignores_close_tag_inside_statement_string_literal() {
        ok(
            "{% autoescape \"none\" %}{% if headers.x == \"%}\" %}match{% endif %}{% endautoescape %}",
        );
    }

    #[test]
    fn handles_escaped_quote_inside_string_literal() {
        ok(
            "{% autoescape \"none\" %}{% if headers.x == \"a\\\"b\" %}match{% endif %}{% endautoescape %}",
        );
    }

    #[test]
    fn rejects_malformed_syntax() {
        let result = validate_autoescape_wrapper("{% autoescape \"html\" }x{% endautoescape %}");
        assert!(result.is_err());
    }

    // Regression for Task 5 tokenizer infinite-loop bug (discovered via Task 16
    // JSON calibration template `{"v":[...]}`). The text accumulator must
    // terminate ONLY on `{%` / `{{` / `{#` (actual tag openings). A bare `{`
    // (e.g. followed by `"`, `[`, space, or EOF) is literal text — the
    // accumulator advances past it instead of looping forever.
    #[test]
    fn handles_bare_brace_as_literal_text() {
        // JSON-shaped template body — exercises bare `{` followed by `"`.
        ok(r#"{% autoescape "json" %}{"v": [{{ headers.x }}]}{% endautoescape %}"#);
        // Bare `{` at various positions inside the wrapper.
        ok(r#"{% autoescape "none" %}{ standalone brace{% endautoescape %}"#);
        ok(r#"{% autoescape "none" %}text { with { braces{% endautoescape %}"#);
        // Bare `{` followed by EOF (no tag follows) — must NOT infinite-loop.
        // Returns NotEnclosingWholeTemplate (missing endautoescape) but does
        // NOT hang, which is the regression guard.
        assert_eq!(
            validate_autoescape_wrapper(r#"{% autoescape "none" %}trailing {"#),
            Err(AutoescapeError::NotEnclosingWholeTemplate)
        );
    }
}
