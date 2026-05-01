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
    LanguageDelegate {
        language: String,
        expression: String,
    },
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
    LogicalOp {
        left: Box<Expr>,
        op: LogicalOp,
        right: Box<Expr>,
    },
    Bool(bool),
}

#[derive(Debug, Clone, PartialEq)]
pub enum LogicalOp {
    And,
    Or,
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

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Expr(String),
    Text(String),
    StringLit(String),
    EscapedString(String),
    NumberLit(f64),
    BoolLit(bool),
    Null,
    Op(Op),
    And,
    Or,
}

pub fn parse(input: &str) -> Result<Expr, LanguageError> {
    let input = input.trim();
    if input.is_empty() {
        return Err(LanguageError::ParseError {
            expr: input.to_string(),
            reason: "empty expression".to_string(),
        });
    }

    let tokens = tokenize(input)?;
    let has_ops = tokens
        .iter()
        .any(|t| matches!(t, Token::Op(_) | Token::And | Token::Or));

    if has_ops {
        let mut pos = 0;
        let expr = parse_or(&tokens, &mut pos)?;
        if pos != tokens.len() {
            return Err(LanguageError::ParseError {
                expr: input.to_string(),
                reason: format!("unexpected token after position {pos}"),
            });
        }
        Ok(expr)
    } else if tokens.len() == 1 {
        token_to_atom(&tokens[0])
    } else {
        build_interpolated(&tokens)
    }
}

fn tokenize(input: &str) -> Result<Vec<Token>, LanguageError> {
    let mut tokens = Vec::new();
    let mut i = 0usize;
    let mut text_buf = String::new();

    fn flush_text(tokens: &mut Vec<Token>, text_buf: &mut String) {
        if text_buf.is_empty() {
            return;
        }
        if !text_buf.trim().is_empty() {
            tokens.push(Token::Text(std::mem::take(text_buf)));
        } else {
            text_buf.clear();
        }
    }

    while i < input.len() {
        let rest = &input[i..];

        if rest.starts_with("${") {
            let start = i + 2;
            let Some(rel_end) = input[start..].find('}') else {
                if !text_buf.is_empty() || tokens.iter().any(|t| matches!(t, Token::Text(_))) {
                    text_buf.push_str("${");
                    i += 2;
                    continue;
                }
                return Err(LanguageError::ParseError {
                    expr: input.to_string(),
                    reason: "unclosed interpolation: missing '}'".to_string(),
                });
            };
            flush_text(&mut tokens, &mut text_buf);
            let end = start + rel_end;
            tokens.push(Token::Expr(input[start..end].to_string()));
            i = end + 1;
            continue;
        }

        if let Some(after_quote) = rest.strip_prefix('\'') {
            flush_text(&mut tokens, &mut text_buf);
            let rel_end = after_quote
                .find('\'')
                .ok_or_else(|| LanguageError::ParseError {
                    expr: input.to_string(),
                    reason: "unclosed single-quoted string".to_string(),
                })?;
            let content = &after_quote[..rel_end];
            tokens.push(Token::StringLit(content.to_string()));
            i += 1 + rel_end + 1;
            continue;
        }

        if let Some(after_quote) = rest.strip_prefix('"') {
            flush_text(&mut tokens, &mut text_buf);
            let rel_end = find_closing_double_quote(after_quote).ok_or_else(|| {
                LanguageError::ParseError {
                    expr: input.to_string(),
                    reason: "unclosed double-quoted string".to_string(),
                }
            })?;
            let content = &after_quote[..rel_end];
            tokens.push(Token::EscapedString(content.to_string()));
            i += 1 + rel_end + 1;
            continue;
        }

        if rest.starts_with("&&") {
            flush_text(&mut tokens, &mut text_buf);
            tokens.push(Token::And);
            i += 2;
            continue;
        }

        if rest.starts_with("||") {
            flush_text(&mut tokens, &mut text_buf);
            tokens.push(Token::Or);
            i += 2;
            continue;
        }

        let two_char_op = if rest.starts_with(">=") {
            Some(Op::Gte)
        } else if rest.starts_with("<=") {
            Some(Op::Lte)
        } else if rest.starts_with("!=") {
            Some(Op::Ne)
        } else if rest.starts_with("==") {
            Some(Op::Eq)
        } else {
            None
        };
        if let Some(op) = two_char_op {
            flush_text(&mut tokens, &mut text_buf);
            tokens.push(Token::Op(op));
            i += 2;
            continue;
        }

        let one_char_op = if rest.starts_with('>') {
            Some(Op::Gt)
        } else if rest.starts_with('<') {
            Some(Op::Lt)
        } else {
            None
        };
        if let Some(op) = one_char_op {
            flush_text(&mut tokens, &mut text_buf);
            tokens.push(Token::Op(op));
            i += 1;
            continue;
        }

        if rest.starts_with("contains")
            && word_boundary(input, i, "contains")
            && text_buf.trim().is_empty()
            && tokens.last().is_some_and(is_value_token)
        {
            flush_text(&mut tokens, &mut text_buf);
            tokens.push(Token::Op(Op::Contains));
            i += "contains".len();
            continue;
        }

        if rest.starts_with("true") && word_boundary(input, i, "true") {
            flush_text(&mut tokens, &mut text_buf);
            tokens.push(Token::BoolLit(true));
            i += "true".len();
            continue;
        }

        if rest.starts_with("false") && word_boundary(input, i, "false") {
            flush_text(&mut tokens, &mut text_buf);
            tokens.push(Token::BoolLit(false));
            i += "false".len();
            continue;
        }

        if rest.starts_with("null") && word_boundary(input, i, "null") {
            flush_text(&mut tokens, &mut text_buf);
            tokens.push(Token::Null);
            i += "null".len();
            continue;
        }

        if rest.chars().next().is_some_and(|c| c.is_ascii_digit()) && text_buf.trim().is_empty() {
            let num_len = consume_number_len(rest);
            if num_len > 0 {
                let raw_num = &rest[..num_len];
                let num = raw_num
                    .parse::<f64>()
                    .map_err(|_| LanguageError::ParseError {
                        expr: input.to_string(),
                        reason: format!("invalid number literal: {raw_num}"),
                    })?;
                if !num.is_finite() {
                    return Err(LanguageError::ParseError {
                        expr: input.to_string(),
                        reason: format!("non-finite number literal: {raw_num}"),
                    });
                }
                flush_text(&mut tokens, &mut text_buf);
                tokens.push(Token::NumberLit(num));
                i += num_len;
                continue;
            }
        }

        let ch = rest.chars().next().unwrap();
        text_buf.push(ch);
        i += ch.len_utf8();
    }

    flush_text(&mut tokens, &mut text_buf);
    Ok(tokens)
}

fn word_boundary(input: &str, start: usize, word: &str) -> bool {
    let before = input[..start].chars().next_back();
    let after = input[start + word.len()..].chars().next();
    let before_ok = before.is_none_or(|c| !(c.is_alphanumeric() || c == '_'));
    let after_ok = after.is_none_or(|c| !(c.is_alphanumeric() || c == '_'));
    before_ok && after_ok
}

fn consume_number_len(s: &str) -> usize {
    let mut len = 0usize;
    let mut seen_dot = false;
    for (idx, ch) in s.char_indices() {
        if ch.is_ascii_digit() {
            len = idx + ch.len_utf8();
            continue;
        }
        if ch == '.' && !seen_dot {
            seen_dot = true;
            len = idx + ch.len_utf8();
            continue;
        }
        break;
    }
    len
}

fn find_closing_double_quote(s: &str) -> Option<usize> {
    let mut escaped = false;
    for (idx, ch) in s.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if ch == '"' {
            return Some(idx);
        }
    }
    None
}

fn is_value_token(token: &Token) -> bool {
    matches!(
        token,
        Token::Expr(_)
            | Token::StringLit(_)
            | Token::EscapedString(_)
            | Token::NumberLit(_)
            | Token::BoolLit(_)
            | Token::Null
    )
}

fn parse_or(tokens: &[Token], pos: &mut usize) -> Result<Expr, LanguageError> {
    let mut left = parse_and(tokens, pos)?;
    while *pos < tokens.len() && matches!(tokens[*pos], Token::Or) {
        *pos += 1;
        let right = parse_and(tokens, pos)?;
        left = Expr::LogicalOp {
            left: Box::new(left),
            op: LogicalOp::Or,
            right: Box::new(right),
        };
    }
    Ok(left)
}

fn parse_and(tokens: &[Token], pos: &mut usize) -> Result<Expr, LanguageError> {
    let mut left = parse_comparison(tokens, pos)?;
    while *pos < tokens.len() && matches!(tokens[*pos], Token::And) {
        *pos += 1;
        let right = parse_comparison(tokens, pos)?;
        left = Expr::LogicalOp {
            left: Box::new(left),
            op: LogicalOp::And,
            right: Box::new(right),
        };
    }
    Ok(left)
}

fn parse_comparison(tokens: &[Token], pos: &mut usize) -> Result<Expr, LanguageError> {
    let left = parse_atom_from_tokens(tokens, pos)?;
    if *pos < tokens.len()
        && let Token::Op(op) = &tokens[*pos]
    {
        let op = op.clone();
        *pos += 1;
        let right = parse_atom_from_tokens(tokens, pos)?;
        return Ok(Expr::BinOp {
            left: Box::new(left),
            op,
            right: Box::new(right),
        });
    }
    Ok(left)
}

fn parse_atom_from_tokens(tokens: &[Token], pos: &mut usize) -> Result<Expr, LanguageError> {
    let token = tokens.get(*pos).ok_or_else(|| LanguageError::ParseError {
        expr: String::new(),
        reason: "expected atom but found end of input".to_string(),
    })?;
    *pos += 1;
    token_to_atom(token)
}

fn token_to_atom(token: &Token) -> Result<Expr, LanguageError> {
    match token {
        Token::Expr(s) => parse_expr_atom(s),
        Token::StringLit(s) => Ok(Expr::StringLit(s.clone())),
        Token::EscapedString(s) => Ok(Expr::EscapedString(unescape_double_quoted(s))),
        Token::NumberLit(n) => Ok(Expr::NumberLit(*n)),
        Token::BoolLit(b) => Ok(Expr::Bool(*b)),
        Token::Null => Ok(Expr::Null),
        Token::Text(s) => Ok(Expr::StringLit(s.clone())),
        Token::Op(_) | Token::And | Token::Or => Err(LanguageError::ParseError {
            expr: format!("{token:?}"),
            reason: "operator cannot appear where an atom is expected".to_string(),
        }),
    }
}

fn parse_expr_atom(s: &str) -> Result<Expr, LanguageError> {
    if s == "body" {
        return Ok(Expr::Body);
    }

    if let Some(path_str) = s.strip_prefix("body.") {
        let segments = parse_body_path(path_str)?;
        return Ok(Expr::BodyField(segments));
    }

    if let Some(key) = s.strip_prefix("header.") {
        if key.is_empty() {
            return Err(LanguageError::ParseError {
                expr: s.to_string(),
                reason: "header key must not be empty".to_string(),
            });
        }
        return Ok(Expr::Header(key.to_string()));
    }

    if let Some(key) = s.strip_prefix("exchangeProperty.") {
        if key.is_empty() {
            return Err(LanguageError::ParseError {
                expr: s.to_string(),
                reason: "exchange property key must not be empty".to_string(),
            });
        }
        return Ok(Expr::ExchangeProperty(key.to_string()));
    }

    if let Some((language, expression)) = s.split_once(':')
        && !expression.is_empty()
        && !language.is_empty()
        && language
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Ok(Expr::LanguageDelegate {
            language: language.to_string(),
            expression: expression.to_string(),
        });
    }

    Err(LanguageError::ParseError {
        expr: s.to_string(),
        reason: "unrecognized token".to_string(),
    })
}

fn build_interpolated(tokens: &[Token]) -> Result<Expr, LanguageError> {
    let mut parts = Vec::new();
    for token in tokens {
        match token {
            Token::Text(s) => parts.push(InterpolatedPart::Literal(s.clone())),
            _ => parts.push(InterpolatedPart::Expr(Box::new(token_to_atom(token)?))),
        }
    }
    Ok(Expr::Interpolated(parts))
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
