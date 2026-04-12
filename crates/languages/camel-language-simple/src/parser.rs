use camel_language_api::LanguageError;

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Header(String),
    Body,
    /// Access a field (or nested path) of a JSON body.
    /// Example: `${body.user.address.city}` → `BodyField([Key("user"), Key("address"), Key("city")])`
    /// Array indexing: `${body.items.0}` → `BodyField([Key("items"), Index(0)])`
    BodyField(Vec<PathSegment>),
    ExchangeProperty(String),
    StringLit(String),
    NumberLit(f64),
    Null,
    BinOp {
        left: Box<Expr>,
        op: Op,
        right: Box<Expr>,
    },
    /// A string that mixes literal text with `${...}` interpolations.
    /// Example: `"Got ${body} from ${header.source}"` →
    ///   `[Literal("Got "), Expr(Body), Literal(" from "), Expr(Header("source"))]`
    Interpolated(Vec<InterpolatedPart>),
    EscapedString(String),
}

/// One segment inside an `Expr::Interpolated`.
#[derive(Debug, Clone, PartialEq)]
pub enum InterpolatedPart {
    /// Plain text segment.
    Literal(String),
    /// An evaluated sub-expression (`${...}`).
    Expr(Box<Expr>),
}

/// A single segment in a body JSON path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathSegment {
    /// Named object field: `"name"`, `"user"`.
    Key(String),
    /// Array index: `0`, `1`.
    Index(usize),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    Eq,
    Ne,
    Gt,
    Lt,
    Gte,
    Lte,
    Contains,
}

pub fn parse(input: &str) -> Result<Expr, LanguageError> {
    let input = input.trim();

    // Try binary expression first (highest priority).
    // Try operators longest-first to avoid partial matches (>= before >).
    let ops = [
        (">=", Op::Gte),
        ("<=", Op::Lte),
        ("!=", Op::Ne),
        ("==", Op::Eq),
        (">", Op::Gt),
        ("<", Op::Lt),
        (" contains ", Op::Contains),
    ];

    for (op_str, op) in &ops {
        // Find the first occurrence of op_str that is NOT inside a single-quoted
        // string literal. We count the number of unescaped single quotes before
        // each candidate position: an odd count means we are inside a string.
        if let Some(pos) = find_op_outside_quotes(input, op_str) {
            let left = parse_atom(input[..pos].trim())?;
            let right = parse_atom(input[pos + op_str.len()..].trim())?;
            return Ok(Expr::BinOp {
                left: Box::new(left),
                op: op.clone(),
                right: Box::new(right),
            });
        }
    }

    // If input contains `${`:
    //   - Pure single token (`${...}` with nothing before/after) → parse_atom
    //     which validates it and returns the proper error on bad keys.
    //   - Mixed (text + `${...}`) → parse_interpolated.
    if input.contains("${") {
        if is_pure_interpolation(input) {
            return parse_atom(input);
        } else {
            return parse_interpolated(input);
        }
    }

    // No `${` at all — try known atoms first, then fall back to plain StringLit.
    // This enables `log: "Hello World"` without requiring single-quote wrapping.
    if let Ok(expr) = parse_atom(input) {
        return Ok(expr);
    }
    Ok(Expr::StringLit(input.to_string()))
}

/// Returns `true` when `input` is exactly one `${...}` token — the entire
/// string starts with `${` and ends with the matching `}`.
fn is_pure_interpolation(input: &str) -> bool {
    if !input.starts_with("${") {
        return false;
    }
    // Find the closing `}` and make sure it's at the very end.
    // NOTE: We use `.find('}')` which locates the *first* `}`, not a properly
    // matching/nested one. This is intentional — Simple Language does not support
    // nested `${...}` expressions, so finding the first `}` is correct here.
    if let Some(end) = input.find('}') {
        end == input.len() - 1
    } else {
        false
    }
}

/// Parse a string that contains a mix of literal text and `${...}` tokens.
fn parse_interpolated(input: &str) -> Result<Expr, LanguageError> {
    let mut parts: Vec<InterpolatedPart> = Vec::new();
    let mut remaining = input;

    while !remaining.is_empty() {
        if let Some(start) = remaining.find("${") {
            // Text before the `${`
            if start > 0 {
                parts.push(InterpolatedPart::Literal(remaining[..start].to_string()));
            }
            let after_dollar = &remaining[start..]; // starts with "${"
            // Find the matching `}`
            if let Some(end) = after_dollar.find('}') {
                let token = &after_dollar[..=end]; // e.g. "${header.x}"
                let expr = parse_atom(token)?;
                parts.push(InterpolatedPart::Expr(Box::new(expr)));
                remaining = &after_dollar[end + 1..];
            } else {
                // No closing brace — treat the rest as a literal
                parts.push(InterpolatedPart::Literal(after_dollar.to_string()));
                remaining = "";
            }
        } else {
            // No more `${` — rest is literal text
            parts.push(InterpolatedPart::Literal(remaining.to_string()));
            remaining = "";
        }
    }

    Ok(Expr::Interpolated(parts))
}

/// Find the byte position of `op` in `input` that is outside single-quoted
/// string literals. Returns `None` if every occurrence is inside quotes.
fn find_op_outside_quotes(input: &str, op: &str) -> Option<usize> {
    for (pos, _) in input.match_indices(op) {
        // Count single quotes strictly before this position.
        let quote_count = input[..pos].chars().filter(|&c| c == '\'').count();
        if quote_count % 2 == 0 {
            return Some(pos);
        }
    }
    None
}

fn parse_atom(s: &str) -> Result<Expr, LanguageError> {
    let s = s.trim();

    if s == "null" {
        return Ok(Expr::Null);
    }

    if s.starts_with("${header.") && s.ends_with('}') {
        let key = &s[9..s.len() - 1];
        if key.is_empty() {
            return Err(LanguageError::ParseError {
                expr: s.to_string(),
                reason: "header key must not be empty".to_string(),
            });
        }
        return Ok(Expr::Header(key.to_string()));
    }

    if s == "${body}" {
        return Ok(Expr::Body);
    }

    if s.starts_with("${body.") && s.ends_with('}') {
        let path_str = &s[7..s.len() - 1]; // strip "${body." prefix and "}" suffix
        let segments = parse_body_path(path_str)?;
        return Ok(Expr::BodyField(segments));
    }

    if s.starts_with("${exchangeProperty.") && s.ends_with('}') {
        let key = &s[19..s.len() - 1];
        if key.is_empty() {
            return Err(LanguageError::ParseError {
                expr: s.to_string(),
                reason: "exchange property key must not be empty".to_string(),
            });
        }
        return Ok(Expr::ExchangeProperty(key.to_string()));
    }

    // String literals: single-quoted, no escape sequences supported.
    // e.g., 'hello world' is valid, but 'it\'s' is NOT — the backslash
    // is treated as a literal character. This is consistent with Apache
    // Camel Simple's basic string literals.
    if s.starts_with('\'') && s.ends_with('\'') && s.len() >= 2 {
        return Ok(Expr::StringLit(s[1..s.len() - 1].to_string()));
    }

    if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 {
        let raw = &s[1..s.len() - 1];
        return Ok(Expr::EscapedString(unescape_double_quoted(raw)));
    }

    if let Ok(n) = s.parse::<f64>() {
        return Ok(Expr::NumberLit(n));
    }

    Err(LanguageError::ParseError {
        expr: s.to_string(),
        reason: "unrecognized token".to_string(),
    })
}

fn unescape_double_quoted(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            if let Some(next) = chars.next() {
                match next {
                    'n' => out.push('\n'),
                    'r' => out.push('\r'),
                    't' => out.push('\t'),
                    'b' => out.push('\u{0008}'),
                    'f' => out.push('\u{000C}'),
                    '/' => out.push('/'),
                    '\\' => out.push('\\'),
                    '"' => out.push('"'),
                    other => {
                        out.push('\\');
                        out.push(other);
                    }
                }
            } else {
                out.push('\\');
            }
        } else {
            out.push(ch);
        }
    }
    out
}

/// Parse `"user.address.city"` or `"items.0.name"` into `Vec<PathSegment>`.
/// Returns `ParseError` if any segment is empty.
fn parse_body_path(path: &str) -> Result<Vec<PathSegment>, LanguageError> {
    let mut segments = Vec::new();
    for seg in path.split('.') {
        if seg.is_empty() {
            return Err(LanguageError::ParseError {
                expr: format!("${{body.{path}}}"),
                reason: "body path segment must not be empty".to_string(),
            });
        }
        // Only treat as array index if: parses as usize AND has no leading zero
        // (except "0" itself). This way "01" is treated as Key("01").
        let is_index = seg.parse::<usize>().is_ok() && (seg == "0" || !seg.starts_with('0'));
        if is_index {
            segments.push(PathSegment::Index(seg.parse::<usize>().unwrap()));
        } else {
            segments.push(PathSegment::Key(seg.to_string()));
        }
    }
    Ok(segments)
}
