use camel_language_api::LanguageError;

/// Maximum accepted source size, in bytes.
const MAX_SOURCE_BYTES: usize = 16 * 1024;
/// Maximum number of AST nodes for one source parse.
const MAX_AST_NODES: usize = 512;
/// Maximum semantic nesting depth for one source parse.
const MAX_DEPTH: usize = 64;

/// Shared parse-time depth accounting. `construction` protects parser
/// recursion (grouping/call/unary); `grouping` tracks only grouping
/// parentheses; `slot_parens` accumulates grouping depth inside `${...}`
/// slots so the single source-wide bound spans slot boundaries.
#[derive(Default)]
struct ParseDepth {
    construction: usize,
    grouping: usize,
    max_grouping: usize,
    slot_parens: usize,
}

impl ParseDepth {
    fn enter(&mut self, input: &str) -> Result<(), LanguageError> {
        self.construction += 1;
        if self.construction > MAX_DEPTH {
            Err(parse_error(input, "expression exceeds 64 depth limit"))
        } else {
            Ok(())
        }
    }

    fn paren_depth(&self) -> usize {
        self.max_grouping.max(self.slot_parens)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Header(String),
    Body,
    /// Access a field (or nested path) of a JSON body.
    /// Example: `${body.user.address.city}` → `BodyField([Key("user"), Key("address"), Key("city")])`
    /// Array indexing: `${body.items.0}` → `BodyField([Key("items"), Index(0)])`
    BodyField(Vec<PathSegment>),
    ExchangeProperty(String),
    ExceptionMessage,
    /// The whole error as `<variant_name>: <Display>`; `Null` when no error.
    Exception,
    LanguageDelegate {
        language: String,
        expression: String,
    },
    StringLit(String),
    IntLit(i64),
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
    Neg(Box<Expr>),
    Call {
        kind: CallKind,
        args: Vec<Expr>,
    },
}

/// Accepted Simple scalar function calls.
#[derive(Debug, Clone, PartialEq)]
pub enum CallKind {
    Int,
    String,
    Default,
    Min,
    Max,
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

/// Re-export the shared PathSegment so Expr::BodyField stays source-compatible
/// (variants Key(String) and Index(usize) are identical) while avoiding
/// duplicate parsing logic. See rc-o6o Phase 2 §3.2.
pub use camel_api::PathSegment;

#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    Eq,
    Ne,
    Gt,
    Lt,
    Gte,
    Lte,
    Contains,
    Add,
    Sub,
    Mul,
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Expr(String),
    Text(String),
    Word(String),
    StringLit(String),
    EscapedString(String),
    IntLit(i64),
    /// An integer literal in `(i64::MAX, u64::MAX]`. Kept as a marker so a
    /// unary minus can fold `9223372036854775808` to `i64::MIN`; any other
    /// use is an out-of-range parse error.
    WideIntLit(u64),
    BoolLit(bool),
    Null,
    Op(Op),
    And,
    Or,
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Pow,
    Question,
    Coalesce,
    In,
    LParen,
    RParen,
    Comma,
}

pub fn parse(input: &str) -> Result<Expr, LanguageError> {
    check_source_len(input)?;
    let input = input.trim();
    if input.is_empty() {
        return Err(LanguageError::ParseError {
            expr: input.to_string(),
            reason: "empty expression".to_string(),
        });
    }
    if has_unquoted_container_syntax(input) {
        return Err(parse_error(input, "json/map construction is deferred"));
    }
    let mut tokens = tokenize(input)?;
    let has_grammar = tokens.iter().any(is_grammar_token);

    if has_grammar {
        // Whitespace-only text tokens carry no grammar meaning.
        tokens.retain(|t| !matches!(t, Token::Text(s) if s.trim().is_empty()));
        reject_deferred(input, &tokens)?;

        let mut pos = 0;
        let mut depth = ParseDepth::default();
        let expr = parse_or(&tokens, &mut pos, input, &mut depth)
            .map_err(|err| retarget_spaced_hyphen_error(err, &tokens, input))?;
        if pos != tokens.len() {
            return Err(parse_error(
                input,
                // allow-secret: parser token, not a credential
                format!("unexpected token after position {pos}"),
            ));
        }
        validate_expr(&expr, input, depth.paren_depth())?;
        return Ok(expr);
    }

    if tokens.len() == 1 {
        let mut depth = ParseDepth::default();
        let expr = match &tokens[0] {
            Token::Word(word) => parse_identifier(word, input)?,
            token => token_to_atom(token, input, &mut depth)?,
        };
        validate_expr(&expr, input, depth.paren_depth())?;
        return Ok(expr);
    }

    if !tokens.iter().any(|t| matches!(t, Token::Expr(_)))
        && tokens
            .iter()
            .all(|t| matches!(t, Token::Text(_) | Token::Word(_)))
    {
        // No slot and no grammar: the source is one literal run.
        let raw: String = tokens.iter().map(token_text).collect();
        return Ok(Expr::StringLit(raw));
    }

    let mut depth = ParseDepth::default();
    let expr = build_interpolated(&tokens, input, &mut depth)?;
    validate_expr(&expr, input, depth.paren_depth())?;
    Ok(expr)
}

/// Parse a value/expression source in TEMPLATE mode: literal text plus
/// `${...}` interpolation slots only.
///
/// Characters and words outside a slot (`=`, `<`, `>`, quotes, `null`,
/// `true`, `contains`, whitespace, …) stay verbatim and are never interpreted
/// as predicate grammar. A whole `${...}` slot spanning the outer-trimmed
/// source returns its atom unchanged (native value); a source with no slot is
/// a raw [`Expr::StringLit`] of the trimmed text. A `${` that is never closed
/// is always a compile-time parse error — it is never treated as literal
/// text (rc-84p3k). See mission359 design §A.
pub fn parse_template(input: &str) -> Result<Expr, LanguageError> {
    check_source_len(input)?;
    let src = input.trim();
    if src.is_empty() {
        return Err(LanguageError::ParseError {
            expr: String::new(),
            reason: "empty expression".to_string(),
        });
    }
    // One depth counter for the whole source: slots share it instead of
    // resetting per slot.
    let mut depth = ParseDepth::default();

    // Whole-slot rule: exactly one `${...}` spanning the trimmed source. The
    // slot keeps its native value (no stringification wrapper).
    if let Some(slot) = whole_slot_source(src) {
        let expr = parse_slot_content(slot, &mut depth, 0)?;
        validate_expr(&expr, src, depth.paren_depth())?;
        return Ok(expr);
    }

    let mut parts: Vec<InterpolatedPart> = Vec::new();
    let mut literal = String::new();
    let mut completed_slot = false;
    let mut i = 0usize;

    while i < src.len() {
        let rest = &src[i..];

        if rest.starts_with("${") {
            let start = i + 2;
            let Some(rel_end) = src[start..].find('}') else {
                // Unterminated `${`: always a compile-time error, never
                // literal text (rc-84p3k).
                return Err(unclosed_interpolation(src, i));
            };
            if !literal.is_empty() {
                parts.push(InterpolatedPart::Literal(std::mem::take(&mut literal)));
            }
            let end = start + rel_end;
            let sub = parse_slot_content(&src[start..end], &mut depth, 0)?;
            parts.push(InterpolatedPart::Expr(Box::new(sub)));
            completed_slot = true;
            i = end + 1;
            continue;
        }

        let ch = rest.chars().next().unwrap(); // allow-unwrap
        literal.push(ch);
        i += ch.len_utf8();
    }

    let expr = if !completed_slot {
        // No `${}` slot: the whole trimmed source is a raw string literal.
        Expr::StringLit(src.to_string())
    } else {
        if !literal.is_empty() {
            parts.push(InterpolatedPart::Literal(literal));
        }
        Expr::Interpolated(parts)
    };
    validate_expr(&expr, src, depth.paren_depth())?;
    Ok(expr)
}

/// Return the inner source of a string that is exactly one `${...}` token
/// spanning it, else `None`.
fn whole_slot_source(src: &str) -> Option<&str> {
    let rest = src.strip_prefix("${")?;
    let close = rest.find('}')?;
    (2 + close + 1 == src.len()).then(|| &rest[..close])
}

/// Parse the content of one `${...}` slot.
///
/// The atom fast path applies only when the content is one atom with no
/// residual tokens; anything carrying grammar tokens takes the bounded
/// expression path, so `${header.n + 1}` is arithmetic rather than a lookup
/// named `n + 1`. Delegate forms (`nope:x`) carry no grammar tokens and stay
/// atoms so unknown languages fail at evaluation, not at creation.
fn parse_slot_content(
    s: &str,
    depth: &mut ParseDepth,
    enclosing_grouping: usize,
) -> Result<Expr, LanguageError> {
    let content = s.trim();
    // An empty (or whitespace-only) `${...}` slot is malformed: there is no
    // expression to evaluate (rc-84p3k).
    if content.is_empty() {
        return Err(LanguageError::ParseError {
            expr: format!("${{{s}}}"),
            reason: "empty interpolation: no expression between '${' and '}'".to_string(),
        });
    }

    // A `${` inside slot content is a nested interpolation — malformed, never
    // part of a path key (rc-84p3k).
    if s.contains("${") {
        return Err(LanguageError::ParseError {
            expr: format!("${{{s}}}"),
            reason: "nested slots are not allowed".to_string(),
        });
    }

    let (expr, parens) = if let Some(delegate) = parse_delegate_slot(s.trim_start()) {
        // A valid `language:payload` prefix is a delegate: the payload is
        // opaque and must never be tokenized as Simple expressions
        // (`${nope:x+1}`). Check this before the atom/expression paths.
        (delegate, 0)
    } else {
        let tokens = tokenize(content)?;
        let has_grammar = tokens.iter().any(is_grammar_token);
        // Lookup keys may contain hyphens (rc-o6o.4), which the tokenizer sees
        // as `-`. Resolution rule: a hyphen is part of a key only when
        // followed by a letter or underscore (`my-header`); a hyphen followed
        // by a digit or whitespace is subtraction (`header.n-1` is
        // `header.n - 1`). Hyphen+letter keys are preserved only in the
        // single-atom fast path; in an expression (`${header.some-key + 1}`)
        // the hyphen stays a subtraction operator, so the right side is an
        // unknown identifier (documented limitation).
        // Narrow key-continuation exception: only a hyphen followed by a
        // letter/underscore continues a lookup key (`header.my-header`). Any
        // other operator character terminates the atom and routes to
        // expression parsing, with or without whitespace (`header.n+1`).
        let hyphenated_lookup = !content.chars().any(char::is_whitespace)
            && is_scoped_lookup_source(content)
            && content.contains('-')
            && content
                .match_indices('-')
                .all(|(idx, _)| hyphen_precedes_key_char(content, idx))
            && tokens.iter().any(|t| matches!(t, Token::Minus))
            && tokens
                .iter()
                .all(|t| !is_grammar_token(t) || matches!(t, Token::Minus));
        if (!has_grammar || hyphenated_lookup) && !tokens.is_empty() {
            if let [token] = tokens.as_slice() {
                let expr = match token {
                    Token::Word(word) => parse_identifier(word, content)?,
                    // A lone text run is not an atom: `${@}` is a compile error.
                    Token::Text(_) => return Err(parse_error(content, "unrecognized token")),
                    other => token_to_atom(other, content, depth)?,
                };
                (expr, 0)
            } else if let Ok(atom) = parse_expr_atom(content) {
                (atom, 0)
            } else {
                let parens = max_grouping_paren_nesting(&tokens);
                (parse_slot_expression(tokens, content, depth)?, parens)
            }
        } else {
            let parens = max_grouping_paren_nesting(&tokens);
            (parse_slot_expression(tokens, content, depth)?, parens)
        }
    };

    // Each slot inherits the enclosing expression's grouping depth; the
    // enclosing source retains this slot's contribution.
    validate_expr(&expr, s, enclosing_grouping + parens)?;
    depth.slot_parens += enclosing_grouping + parens;
    Ok(expr)
}

/// Recognize a `<language>:<payload>` slot before Simple tokenization. The
/// language prefix uses the existing delegate charset (ASCII lowercase,
/// digits, hyphen); the payload is kept verbatim.
fn parse_delegate_slot(content: &str) -> Option<Expr> {
    let (language, expression) = content.split_once(':')?;
    if language.is_empty()
        || expression.is_empty()
        || !language
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return None;
    }
    Some(Expr::LanguageDelegate {
        language: language.to_string(),
        expression: expression.to_string(),
    })
}

/// True when the byte at `idx` is `-` followed by a lookup-key character
/// (letter or underscore). A hyphen before a digit or whitespace is the
/// subtraction operator.
fn hyphen_precedes_key_char(content: &str, idx: usize) -> bool {
    content[idx + 1..]
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
}

fn is_scoped_lookup_source(s: &str) -> bool {
    s.starts_with("body.")
        || s.starts_with("header.")
        || s.starts_with("property.")
        || s.starts_with("exchangeProperty.")
}

/// Parse a slot's already-tokenized expression content, sharing the source
/// depth budget.
fn parse_slot_expression(
    mut tokens: Vec<Token>,
    input: &str,
    depth: &mut ParseDepth,
) -> Result<Expr, LanguageError> {
    tokens.retain(|t| !matches!(t, Token::Text(s) if s.trim().is_empty()));
    reject_deferred(input, &tokens)?;
    let mut pos = 0usize;
    let expr = parse_or(&tokens, &mut pos, input, depth)
        .map_err(|err| retarget_spaced_hyphen_error(err, &tokens, input))?;
    if pos != tokens.len() {
        return Err(parse_error(
            input,
            // allow-secret: parser token, not a credential
            format!("unexpected token after position {pos}"),
        ));
    }
    Ok(expr)
}

/// An unterminated `${` is a compile-time parse error in both TEMPLATE and
/// predicate mode — never literal text (rc-84p3k). The message names the
/// construct and the byte offset of the `${` relative to the trimmed source
/// the parser scans. Compile-time source text only; no runtime exchange data.
fn unclosed_interpolation(src: &str, at: usize) -> LanguageError {
    LanguageError::ParseError {
        expr: src.to_string(),
        reason: format!(
            "unclosed interpolation: '${{' at byte offset {at} (trimmed source) has no closing '}}'"
        ),
    }
}

fn tokenize(input: &str) -> Result<Vec<Token>, LanguageError> {
    let mut tokens = Vec::new();
    let mut i = 0usize;
    let mut text_buf = String::new();

    fn flush_text(tokens: &mut Vec<Token>, text_buf: &mut String) {
        if !text_buf.is_empty() {
            tokens.push(Token::Text(std::mem::take(text_buf)));
        }
    }

    while i < input.len() {
        let rest = &input[i..];

        if rest.starts_with("${") {
            let start = i + 2;
            let Some(rel_end) = input[start..].find('}') else {
                // Unterminated `${`: always a compile-time error, never
                // literal text (rc-84p3k).
                return Err(unclosed_interpolation(input, i));
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
                .ok_or_else(|| parse_error(input, "unclosed single-quoted string"))?;
            let content = &after_quote[..rel_end];
            tokens.push(Token::StringLit(content.to_string()));
            i += 1 + rel_end + 1;
            continue;
        }

        if let Some(after_quote) = rest.strip_prefix('"') {
            flush_text(&mut tokens, &mut text_buf);
            let rel_end = find_closing_double_quote(after_quote)
                .ok_or_else(|| parse_error(input, "unclosed double-quoted string"))?;
            let content = &after_quote[..rel_end];
            tokens.push(Token::EscapedString(content.to_string()));
            i += 1 + rel_end + 1;
            continue;
        }

        let first = rest.chars().next().unwrap(); // allow-unwrap

        // A word starts a fresh token when only whitespace precedes it.
        if (first.is_ascii_alphabetic() || first == '_') && text_buf.trim().is_empty() {
            flush_text(&mut tokens, &mut text_buf);
            let mut len = 0usize;
            for (idx, ch) in rest.char_indices() {
                if ch.is_ascii_alphanumeric() || ch == '_' || ch == '.' {
                    len = idx + ch.len_utf8();
                } else if ch == '-' && hyphen_precedes_key_char(rest, idx) {
                    // A hyphen followed by a letter/underscore continues the
                    // identifier run (documented key-continuation rule).
                    len = idx + ch.len_utf8();
                } else {
                    break;
                }
            }
            let word = &rest[..len];
            tokens.push(classify_word(word));
            i += len;
            continue;
        }

        // An integer literal starts a fresh token when only whitespace precedes it.
        if first.is_ascii_digit() && text_buf.trim().is_empty() {
            flush_text(&mut tokens, &mut text_buf);
            let mut len = 0usize;
            for (idx, ch) in rest.char_indices() {
                if ch.is_ascii_digit() {
                    len = idx + ch.len_utf8();
                } else {
                    break;
                }
            }
            let after = &rest[len..];
            if after.starts_with('.') || after.starts_with('e') || after.starts_with('E') {
                return Err(parse_error(input, "floats are deferred"));
            }
            let digits = &rest[..len];
            let magnitude: u128 = digits
                .parse()
                .map_err(|_| parse_error(input, "integer literal out of range"))?;
            if magnitude > u64::MAX as u128 {
                return Err(parse_error(input, "integer literal out of range"));
            }
            if magnitude <= i64::MAX as u128 {
                tokens.push(Token::IntLit(magnitude as i64));
            } else {
                tokens.push(Token::WideIntLit(magnitude as u64));
            }
            i += len;
            continue;
        }

        if rest.starts_with("??") {
            flush_text(&mut tokens, &mut text_buf);
            tokens.push(Token::Coalesce);
            i += 2;
            continue;
        }

        if rest.starts_with("**") {
            flush_text(&mut tokens, &mut text_buf);
            tokens.push(Token::Pow);
            i += 2;
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

        let single = match first {
            '>' => Some(Token::Op(Op::Gt)),
            '<' => Some(Token::Op(Op::Lt)),
            '+' => Some(Token::Plus),
            '-' => Some(Token::Minus),
            '*' => Some(Token::Star),
            '/' => Some(Token::Slash),
            '%' => Some(Token::Percent),
            '?' => Some(Token::Question),
            '(' => Some(Token::LParen),
            ')' => Some(Token::RParen),
            ',' => Some(Token::Comma),
            _ => None,
        };
        if let Some(token) = single {
            flush_text(&mut tokens, &mut text_buf);
            tokens.push(token);
            i += first.len_utf8();
            continue;
        }

        text_buf.push(first);
        i += first.len_utf8();
    }

    flush_text(&mut tokens, &mut text_buf);
    Ok(tokens)
}

fn classify_word(word: &str) -> Token {
    // Keywords are only recognized for hyphen-free runs, so lookup keys such
    // as `check-in` / `x-contains` stay identifiers.
    if word.contains('-') {
        return Token::Word(word.to_string());
    }
    match word {
        "contains" => Token::Op(Op::Contains),
        "true" => Token::BoolLit(true),
        "false" => Token::BoolLit(false),
        "null" => Token::Null,
        "in" => Token::In,
        _ => Token::Word(word.to_string()),
    }
}

fn is_grammar_token(token: &Token) -> bool {
    matches!(
        token,
        Token::Op(_)
            | Token::And
            | Token::Or
            | Token::Plus
            | Token::Minus
            | Token::Star
            | Token::Slash
            | Token::Percent
            | Token::Pow
            | Token::Question
            | Token::Coalesce
            | Token::In
            | Token::LParen
            | Token::RParen
            | Token::Comma
    )
}

fn reject_deferred(input: &str, tokens: &[Token]) -> Result<(), LanguageError> {
    for token in tokens {
        let reason = match token {
            Token::Question => Some("ternary is deferred"),
            Token::Coalesce => Some("null-coalescing is deferred"),
            Token::Pow => Some("exponentiation is deferred"),
            Token::Slash => Some("division is deferred"),
            Token::Percent => Some("modulo is deferred"),
            Token::In => Some("in is deferred"),
            _ => None,
        };
        if let Some(reason) = reason {
            return Err(parse_error(input, reason));
        }
    }
    Ok(())
}

fn token_text(token: &Token) -> &str {
    match token {
        Token::Text(s) | Token::Word(s) => s,
        _ => "",
    }
}

fn parse_error(expr: &str, reason: impl Into<String>) -> LanguageError {
    LanguageError::ParseError {
        expr: expr.to_string(),
        reason: reason.into(),
    }
}

fn check_source_len(input: &str) -> Result<(), LanguageError> {
    if input.len() > MAX_SOURCE_BYTES {
        return Err(parse_error(input, "source exceeds 16 KiB limit"));
    }
    Ok(())
}

fn validate_expr(expr: &Expr, input: &str, paren_depth: usize) -> Result<(), LanguageError> {
    if count_nodes(expr) > MAX_AST_NODES {
        return Err(parse_error(input, "AST exceeds 512 node limit"));
    }
    // Parenthesis grouping and constructed-tree depth share one bound: a
    // subtree inside N grouping levels counts as N + its own semantic depth.
    if paren_depth + expr_depth(expr) > MAX_DEPTH {
        return Err(parse_error(input, "expression exceeds 64 depth limit"));
    }
    Ok(())
}

/// Maximum nesting of GROUPING parentheses in a token stream. Call
/// parentheses (`int(...)`) are excluded because the `Call` node already
/// contributes to [`expr_depth`]; counting them twice would reject valid
/// nested calls.
fn max_grouping_paren_nesting(tokens: &[Token]) -> usize {
    let mut stack: Vec<bool> = Vec::new();
    let mut current = 0usize;
    let mut max = 0usize;
    let mut previous_was_word = false;
    for token in tokens {
        match token {
            Token::LParen => {
                let grouping = !previous_was_word;
                stack.push(grouping);
                if grouping {
                    current += 1;
                    max = max.max(current);
                }
                previous_was_word = false;
            }
            Token::RParen => {
                if let Some(grouping) = stack.pop()
                    && grouping
                {
                    current = current.saturating_sub(1);
                }
                previous_was_word = false;
            }
            Token::Word(_) => previous_was_word = true,
            _ => previous_was_word = false,
        }
    }
    max
}

/// Detect `{`/`}`/`[`/`]` outside quoted strings and `${...}` slots.
/// Predicate mode rejects container-literal syntax; template mode keeps raw
/// text.
fn has_unquoted_container_syntax(input: &str) -> bool {
    let mut i = 0usize;
    while i < input.len() {
        let rest = &input[i..];
        let Some(ch) = rest.chars().next() else {
            break;
        };
        if rest.starts_with("${") {
            // Skip the whole slot; unclosed slots are reported by the tokenizer.
            let start = i + 2;
            match input[start..].find('}') {
                Some(rel_end) => i = start + rel_end + 1,
                None => return false,
            }
            continue;
        }
        if ch == '\'' {
            let after = &rest[1..];
            match after.find('\'') {
                Some(rel_end) => i += 1 + rel_end + 1,
                None => return false,
            }
            continue;
        }
        if ch == '"' {
            let after = &rest[1..];
            match find_closing_double_quote(after) {
                Some(rel_end) => i += 1 + rel_end + 1,
                None => return false,
            }
            continue;
        }
        if matches!(ch, '{' | '}' | '[' | ']') {
            return true;
        }
        i += ch.len_utf8();
    }
    false
}

/// Semantic depth of the CONSTRUCTED tree. Atoms contribute 0; each
/// binary/logical/unary/call node contributes 1, and an interpolation wrapper
/// adds 1 plus 1 for each slot boundary. Parenthesis grouping is enforced by
/// the construction-time counter (parentheses do not appear in the AST).
fn expr_depth(expr: &Expr) -> usize {
    match expr {
        Expr::BinOp { left, right, .. } | Expr::LogicalOp { left, right, .. } => {
            1 + expr_depth(left).max(expr_depth(right))
        }
        Expr::Neg(inner) => 1 + expr_depth(inner),
        Expr::Call { args, .. } => 1 + args.iter().map(expr_depth).max().unwrap_or(0),
        Expr::Interpolated(parts) => {
            1 + parts
                .iter()
                .map(|part| match part {
                    InterpolatedPart::Literal(_) => 0,
                    InterpolatedPart::Expr(sub) => 1 + expr_depth(sub),
                })
                .max()
                .unwrap_or(0)
        }
        _ => 0,
    }
}

fn count_nodes(expr: &Expr) -> usize {
    match expr {
        Expr::BinOp { left, right, .. } | Expr::LogicalOp { left, right, .. } => {
            1 + count_nodes(left) + count_nodes(right)
        }
        Expr::Neg(inner) => 1 + count_nodes(inner),
        Expr::Call { args, .. } => 1 + args.iter().map(count_nodes).sum::<usize>(),
        Expr::Interpolated(parts) => {
            1 + parts
                .iter()
                .map(|part| match part {
                    InterpolatedPart::Literal(_) => 1,
                    InterpolatedPart::Expr(sub) => 1 + count_nodes(sub),
                })
                .sum::<usize>()
        }
        _ => 1,
    }
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

fn parse_or(
    tokens: &[Token],
    pos: &mut usize,
    input: &str,
    depth: &mut ParseDepth,
) -> Result<Expr, LanguageError> {
    let mut left = parse_and(tokens, pos, input, depth)?;
    while matches!(tokens.get(*pos), Some(Token::Or)) {
        *pos += 1;
        let right = parse_and(tokens, pos, input, depth)?;
        left = Expr::LogicalOp {
            left: Box::new(left),
            op: LogicalOp::Or,
            right: Box::new(right),
        };
    }
    Ok(left)
}

fn parse_and(
    tokens: &[Token],
    pos: &mut usize,
    input: &str,
    depth: &mut ParseDepth,
) -> Result<Expr, LanguageError> {
    let mut left = parse_cmp(tokens, pos, input, depth)?;
    while matches!(tokens.get(*pos), Some(Token::And)) {
        *pos += 1;
        let right = parse_cmp(tokens, pos, input, depth)?;
        left = Expr::LogicalOp {
            left: Box::new(left),
            op: LogicalOp::And,
            right: Box::new(right),
        };
    }
    Ok(left)
}

fn parse_cmp(
    tokens: &[Token],
    pos: &mut usize,
    input: &str,
    depth: &mut ParseDepth,
) -> Result<Expr, LanguageError> {
    let left = parse_sum(tokens, pos, input, depth)?;
    if let Some(Token::Op(op)) = tokens.get(*pos)
        && is_comparison_op(op)
    {
        let op = op.clone();
        *pos += 1;
        let right = parse_sum(tokens, pos, input, depth)?;
        if let Some(Token::Op(next)) = tokens.get(*pos)
            && is_comparison_op(next)
        {
            return Err(parse_error(input, "comparisons do not chain"));
        }
        return Ok(Expr::BinOp {
            left: Box::new(left),
            op,
            right: Box::new(right),
        });
    }
    Ok(left)
}

fn is_comparison_op(op: &Op) -> bool {
    matches!(
        op,
        Op::Eq | Op::Ne | Op::Gt | Op::Lt | Op::Gte | Op::Lte | Op::Contains
    )
}

fn parse_sum(
    tokens: &[Token],
    pos: &mut usize,
    input: &str,
    depth: &mut ParseDepth,
) -> Result<Expr, LanguageError> {
    let mut left = parse_product(tokens, pos, input, depth)?;
    loop {
        let op = match tokens.get(*pos) {
            Some(Token::Plus) => Op::Add,
            Some(Token::Minus) => Op::Sub,
            _ => break,
        };
        *pos += 1;
        let right = parse_product(tokens, pos, input, depth)?;
        if matches!(op, Op::Add) && is_string_literal_expr(&left) && is_string_literal_expr(&right)
        {
            return Err(parse_error(input, "string concatenation is deferred"));
        }
        left = Expr::BinOp {
            left: Box::new(left),
            op,
            right: Box::new(right),
        };
    }
    Ok(left)
}

fn is_string_literal_expr(expr: &Expr) -> bool {
    matches!(expr, Expr::StringLit(_) | Expr::EscapedString(_))
}

fn parse_product(
    tokens: &[Token],
    pos: &mut usize,
    input: &str,
    depth: &mut ParseDepth,
) -> Result<Expr, LanguageError> {
    let mut left = parse_unary(tokens, pos, input, depth)?;
    while matches!(tokens.get(*pos), Some(Token::Star)) {
        *pos += 1;
        let right = parse_unary(tokens, pos, input, depth)?;
        left = Expr::BinOp {
            left: Box::new(left),
            op: Op::Mul,
            right: Box::new(right),
        };
    }
    Ok(left)
}

fn parse_unary(
    tokens: &[Token],
    pos: &mut usize,
    input: &str,
    depth: &mut ParseDepth,
) -> Result<Expr, LanguageError> {
    if matches!(tokens.get(*pos), Some(Token::Minus)) {
        *pos += 1;
        if let Some(Token::WideIntLit(magnitude)) = tokens.get(*pos) {
            let magnitude = *magnitude;
            *pos += 1;
            if magnitude == (i64::MAX as u64) + 1 {
                return Ok(Expr::IntLit(i64::MIN));
            }
            return Err(parse_error(input, "integer literal out of range"));
        }
        let saved = depth.construction;
        depth.enter(input)?;
        let operand = parse_unary(tokens, pos, input, depth)?;
        depth.construction = saved;
        return Ok(Expr::Neg(Box::new(operand)));
    }
    parse_atom(tokens, pos, input, depth)
}

fn parse_atom(
    tokens: &[Token],
    pos: &mut usize,
    input: &str,
    depth: &mut ParseDepth,
) -> Result<Expr, LanguageError> {
    let Some(token) = tokens.get(*pos) else {
        return Err(parse_error(input, "expected atom but found end of input"));
    };
    match token {
        Token::LParen => {
            let saved_construction = depth.construction;
            let saved_grouping = depth.grouping;
            depth.enter(input)?;
            depth.grouping += 1;
            depth.max_grouping = depth.max_grouping.max(depth.grouping);
            *pos += 1;
            let inner = parse_or(tokens, pos, input, depth)?;
            depth.grouping = saved_grouping;
            depth.construction = saved_construction;
            match tokens.get(*pos) {
                Some(Token::RParen) => {
                    *pos += 1;
                    Ok(inner)
                }
                _ => Err(parse_error(input, "expected ')'")),
            }
        }
        Token::Word(word) => {
            if matches!(tokens.get(*pos + 1), Some(Token::LParen)) {
                parse_call(word, tokens, pos, input, depth)
            } else {
                *pos += 1;
                parse_identifier(word, input)
            }
        }
        Token::IntLit(value) => {
            *pos += 1;
            Ok(Expr::IntLit(*value))
        }
        Token::WideIntLit(_) => Err(parse_error(input, "integer literal out of range")),
        Token::StringLit(s) => {
            *pos += 1;
            Ok(Expr::StringLit(s.clone()))
        }
        Token::EscapedString(s) => {
            *pos += 1;
            Ok(Expr::EscapedString(unescape_double_quoted(s)))
        }
        Token::BoolLit(value) => {
            *pos += 1;
            Ok(Expr::Bool(*value))
        }
        Token::Null => {
            *pos += 1;
            Ok(Expr::Null)
        }
        Token::Expr(source) => {
            *pos += 1;
            let enclosing = depth.grouping;
            parse_slot_content(source, depth, enclosing)
        }
        _ => Err(parse_error(
            input,
            "operator cannot appear where an atom is expected",
        )),
    }
}

fn parse_call(
    name: &str,
    tokens: &[Token],
    pos: &mut usize,
    input: &str,
    depth: &mut ParseDepth,
) -> Result<Expr, LanguageError> {
    let kind = match name {
        "int" => CallKind::Int,
        "string" => CallKind::String,
        "default" => CallKind::Default,
        "min" => CallKind::Min,
        "max" => CallKind::Max,
        _ => return Err(parse_error(input, "unknown function")),
    };
    *pos += 2; // consume the function name and '('

    let saved_depth = depth.construction;
    depth.enter(input)?;

    let mut args = Vec::new();
    if matches!(tokens.get(*pos), Some(Token::RParen)) {
        *pos += 1;
    } else {
        loop {
            args.push(parse_or(tokens, pos, input, depth)?);
            match tokens.get(*pos) {
                Some(Token::Comma) => *pos += 1,
                Some(Token::RParen) => {
                    *pos += 1;
                    break;
                }
                _ => return Err(parse_error(input, "expected ',' or ')'")),
            }
        }
    }

    let expected = if matches!(kind, CallKind::Int | CallKind::String) {
        1
    } else {
        2
    };
    if args.len() != expected {
        return Err(parse_error(input, "call arity"));
    }
    depth.construction = saved_depth;
    Ok(Expr::Call { kind, args })
}

fn parse_identifier(word: &str, input: &str) -> Result<Expr, LanguageError> {
    if is_lookup_word(word) {
        parse_expr_atom(word)
    } else {
        Err(parse_error(input, "unknown identifier"))
    }
}

/// True when `word` names a Simple lookup: `body`, `exception`,
/// `exception.message`, or a scoped path under `body.`, `header.`,
/// `property.`, or `exchangeProperty.`.
fn is_lookup_word(word: &str) -> bool {
    word == "body"
        || word == "exception"
        || word == "exception.message"
        || word.starts_with("body.")
        || word.starts_with("header.")
        || word.starts_with("property.")
        || word.starts_with("exchangeProperty.")
}

/// Dotted lookup prefix of a scoped lookup path, when `word` has one.
fn lookup_key_prefix(word: &str) -> Option<&'static str> {
    ["body.", "header.", "property.", "exchangeProperty."]
        .into_iter()
        .find(|prefix| word.starts_with(prefix))
}

/// Rewrite the generic `unknown identifier` error for a spaced-hyphen key
/// typo into a message that names the intended hyphenated key.
///
/// Only fires when the whole token slice is exactly `Word(left) - Word(right)`
/// and `left` is a prefix-qualified lookup while `right` is not a lookup.
/// Keyless lookups (`body`, `exception`, `exception.message`) and longer/other
/// expressions keep their original error. Source text only; no exchange data.
fn retarget_spaced_hyphen_error(
    err: LanguageError,
    tokens: &[Token],
    _input: &str,
) -> LanguageError {
    let LanguageError::ParseError { expr, reason } = &err else {
        return err;
    };
    if reason != "unknown identifier" {
        return err;
    }
    let [Token::Word(left), Token::Minus, Token::Word(right)] = tokens else {
        return err;
    };
    let Some(prefix) = lookup_key_prefix(left) else {
        return err;
    };
    if is_lookup_word(right) {
        return err;
    }
    let key = &left[prefix.len()..];
    LanguageError::ParseError {
        expr: expr.clone(),
        reason: format!(
            "spaced hyphen: '{left} - {right}' parses as subtraction, and '{right}' is not a lookup; write a hyphenated key without spaces: '{prefix}{key}-{right}'"
        ),
    }
}

fn token_to_atom(
    token: &Token,
    input: &str,
    depth: &mut ParseDepth,
) -> Result<Expr, LanguageError> {
    match token {
        Token::Expr(s) => {
            let enclosing = depth.grouping;
            parse_slot_content(s, depth, enclosing)
        }
        Token::StringLit(s) => Ok(Expr::StringLit(s.clone())),
        Token::EscapedString(s) => Ok(Expr::EscapedString(unescape_double_quoted(s))),
        Token::IntLit(value) => Ok(Expr::IntLit(*value)),
        Token::WideIntLit(_) => Err(parse_error(input, "integer literal out of range")),
        Token::BoolLit(value) => Ok(Expr::Bool(*value)),
        Token::Null => Ok(Expr::Null),
        Token::Text(s) | Token::Word(s) => Ok(Expr::StringLit(s.clone())),
        _ => Err(parse_error(
            input,
            "operator cannot appear where an atom is expected",
        )),
    }
}

fn parse_expr_atom(s: &str) -> Result<Expr, LanguageError> {
    // An empty (or whitespace-only) `${...}` slot is malformed: there is no
    // expression to evaluate (rc-84p3k).
    if s.trim().is_empty() {
        return Err(LanguageError::ParseError {
            expr: format!("${{{s}}}"),
            reason: "empty interpolation: no expression between '${' and '}'".to_string(),
        });
    }

    // A `${` inside slot content is a nested, unterminated interpolation —
    // malformed, never part of a path key (rc-84p3k).
    if s.contains("${") {
        return Err(LanguageError::ParseError {
            expr: format!("${{{s}}}"),
            reason: "nested slots are not allowed".to_string(),
        });
    }

    // Bare `body` means whole body. ExchangeLookupPath::parse also handles
    // this (returns Body(vec![])), but Simple's Expr::Body variant is
    // distinct from Expr::BodyField, so we special-case it here.
    if s == "body" {
        return Ok(Expr::Body);
    }

    if s == "exception" {
        return Ok(Expr::Exception);
    }

    if s == "exception.message" {
        return Ok(Expr::ExceptionMessage);
    }

    // Language delegate: `${jsonpath:$.a[0]}` etc. NOT a lookup path; the
    // `:` would be rejected by ExchangeLookupPath anyway, so this stays
    // inline.
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

    // Scoped forms: body.x.y / header.x / property.x / exchangeProperty.x.
    // Delegate to the shared grammar so SQL `:#` and Simple `${...}` agree.
    if s.starts_with("body.")
        || s.starts_with("header.")
        || s.starts_with("property.")
        || s.starts_with("exchangeProperty.")
    {
        return lookup_path_to_expr(s);
    }

    Err(LanguageError::ParseError {
        expr: s.to_string(),
        reason: "unrecognized token".to_string(),
    })
}

/// Parse a scoped form via [`camel_api::ExchangeLookupPath`] and project to
/// Simple's [`Expr`]. Errors are translated into [`LanguageError::ParseError`]
/// preserving the `${...}` rendering callers expect.
fn lookup_path_to_expr(s: &str) -> Result<Expr, LanguageError> {
    let path = camel_api::ExchangeLookupPath::parse(s).map_err(|e| LanguageError::ParseError {
        expr: format!("${{{s}}}"),
        reason: e.to_string(),
    })?;
    match path {
        camel_api::ExchangeLookupPath::Body(segments) => Ok(Expr::BodyField(segments)),
        camel_api::ExchangeLookupPath::Header(key) => Ok(Expr::Header(key)),
        camel_api::ExchangeLookupPath::Property(key) => Ok(Expr::ExchangeProperty(key)),
        // `Unscoped` (and any future variant) reach here only for inputs the
        // `starts_with` guards above did not recognise — return a parse error.
        _ => Err(LanguageError::ParseError {
            expr: format!("${{{s}}}"), // allow-secret
            reason: "unrecognized token".to_string(),
        }),
    }
}

fn build_interpolated(
    tokens: &[Token],
    input: &str,
    depth: &mut ParseDepth,
) -> Result<Expr, LanguageError> {
    let mut parts = Vec::new();
    let mut literal = String::new();
    for token in tokens {
        match token {
            Token::Text(s) | Token::Word(s) => literal.push_str(s),
            other => {
                if !literal.is_empty() {
                    parts.push(InterpolatedPart::Literal(std::mem::take(&mut literal)));
                }
                parts.push(InterpolatedPart::Expr(Box::new(token_to_atom(
                    other, input, depth,
                )?)));
            }
        }
    }
    if !literal.is_empty() {
        parts.push(InterpolatedPart::Literal(literal));
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

// NOTE: parse_body_path was removed in rc-o6o Phase 2 Task 10. Its logic
// lives in camel_api::ExchangeLookupPath::parse (via parse_body_segments
// in crates/camel-api/src/exchange_lookup.rs).

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_header_body_property_and_delegate_atoms() {
        assert_eq!(
            parse("${header.foo}").unwrap(),
            Expr::Header("foo".to_string())
        );
        assert_eq!(parse("${body}").unwrap(), Expr::Body);
        assert_eq!(
            parse("${exchangeProperty.id}").unwrap(),
            Expr::ExchangeProperty("id".to_string())
        );
        assert_eq!(
            parse("${jsonpath:$.a[0]}").unwrap(),
            Expr::LanguageDelegate {
                language: "jsonpath".to_string(),
                expression: "$.a[0]".to_string(),
            }
        );
    }

    #[test]
    fn parse_body_field_nested_and_index_paths() {
        assert_eq!(
            parse("${body.user.address.city}").unwrap(),
            Expr::BodyField(vec![
                PathSegment::Key("user".to_string()),
                PathSegment::Key("address".to_string()),
                PathSegment::Key("city".to_string()),
            ])
        );
        assert_eq!(
            parse("${body.items.0}").unwrap(),
            Expr::BodyField(vec![
                PathSegment::Key("items".to_string()),
                PathSegment::Index(0)
            ])
        );
        assert_eq!(
            parse("${body.01}").unwrap(),
            Expr::BodyField(vec![PathSegment::Key("01".to_string())])
        );
    }

    #[test]
    fn parse_literals_and_escaped_string() {
        assert_eq!(parse("'abc'").unwrap(), Expr::StringLit("abc".to_string()));
        assert_eq!(
            parse("\"a\\n\\t\\\"b\"").unwrap(),
            Expr::EscapedString("a\n\t\"b".to_string())
        );
        assert_eq!(parse("12").unwrap(), Expr::IntLit(12));
        assert!(
            parse("12.5")
                .unwrap_err()
                .to_string()
                .contains("floats are deferred")
        );
        assert_eq!(parse("true").unwrap(), Expr::Bool(true));
        assert_eq!(parse("false").unwrap(), Expr::Bool(false));
        assert_eq!(parse("null").unwrap(), Expr::Null);
    }

    #[test]
    fn parse_comparison_and_logical_precedence() {
        assert_eq!(
            parse("${header.a} >= 10").unwrap(),
            Expr::BinOp {
                left: Box::new(Expr::Header("a".to_string())),
                op: Op::Gte,
                right: Box::new(Expr::IntLit(10)),
            }
        );
        assert_eq!(
            parse("${header.a} == '1' || ${header.b} == '2' && ${header.c} == '3'").unwrap(),
            Expr::LogicalOp {
                left: Box::new(Expr::BinOp {
                    left: Box::new(Expr::Header("a".to_string())),
                    op: Op::Eq,
                    right: Box::new(Expr::StringLit("1".to_string())),
                }),
                op: LogicalOp::Or,
                right: Box::new(Expr::LogicalOp {
                    left: Box::new(Expr::BinOp {
                        left: Box::new(Expr::Header("b".to_string())),
                        op: Op::Eq,
                        right: Box::new(Expr::StringLit("2".to_string())),
                    }),
                    op: LogicalOp::And,
                    right: Box::new(Expr::BinOp {
                        left: Box::new(Expr::Header("c".to_string())),
                        op: Op::Eq,
                        right: Box::new(Expr::StringLit("3".to_string())),
                    }),
                }),
            }
        );
    }

    #[test]
    fn parse_interpolated_text_and_expressions() {
        assert_eq!(
            parse("Got ${body} from ${header.source}").unwrap(),
            Expr::Interpolated(vec![
                InterpolatedPart::Literal("Got ".to_string()),
                InterpolatedPart::Expr(Box::new(Expr::Body)),
                InterpolatedPart::Literal(" from ".to_string()),
                InterpolatedPart::Expr(Box::new(Expr::Header("source".to_string()))),
            ])
        );
    }

    #[test]
    fn parse_error_cases() {
        assert!(parse("").is_err());
        assert!(parse("${header.}").is_err());
        assert!(parse("${exchangeProperty.}").is_err());
        assert!(parse("${body..name}").is_err());
        assert!(parse("'abc").is_err());
        assert!(parse("\"abc").is_err());
        assert!(parse("&& true").is_err());
        assert!(parse("${Unknown:expr}").is_err());
    }

    #[test]
    fn parse_exception_message() {
        assert_eq!(
            parse("${exception.message}").unwrap(),
            Expr::ExceptionMessage
        );
    }

    #[test]
    fn parse_exception_message_in_interpolation() {
        let result = parse("Error: ${exception.message}").unwrap();
        assert_eq!(
            result,
            Expr::Interpolated(vec![
                InterpolatedPart::Literal("Error: ".to_string()),
                InterpolatedPart::Expr(Box::new(Expr::ExceptionMessage)),
            ])
        );
    }

    #[test]
    fn parse_exception_bare() {
        assert_eq!(parse("${exception}").unwrap(), Expr::Exception);
    }

    #[test]
    fn parse_exception_in_interpolation() {
        let result = parse("Error: ${exception}").unwrap();
        assert_eq!(
            result,
            Expr::Interpolated(vec![
                InterpolatedPart::Literal("Error: ".to_string()),
                InterpolatedPart::Expr(Box::new(Expr::Exception)),
            ])
        );
    }

    #[test]
    fn parse_exception_kind_unsupported() {
        assert!(parse("${exception.kind}").is_err());
    }

    #[test]
    fn parse_uses_shared_lookup_path_abstraction() {
        // Smoke: scoped forms still parse identically after the ExchangeLookupPath
        // migration. If the migration is wired, these all succeed exactly as
        // before. rc-o6o Phase 2 §4.2 audit.
        assert_eq!(
            parse("${body.user.address.city}").unwrap(),
            Expr::BodyField(vec![
                PathSegment::Key("user".into()),
                PathSegment::Key("address".into()),
                PathSegment::Key("city".into()),
            ])
        );
        assert_eq!(parse("${header.foo}").unwrap(), Expr::Header("foo".into()));
        assert_eq!(
            parse("${exchangeProperty.id}").unwrap(),
            Expr::ExchangeProperty("id".into())
        );
        // Bare body and exception.message stay inline (not ExchangeLookupPath).
        assert_eq!(parse("${body}").unwrap(), Expr::Body);
        assert_eq!(
            parse("${exception.message}").unwrap(),
            Expr::ExceptionMessage
        );
        // Trailing dot must still be rejected.
        assert!(parse("${body.}").is_err());
        assert!(parse("${header.}").is_err());
    }

    #[test]
    fn parse_template_operator_text_is_literal() {
        assert_eq!(
            parse_template("Got -> ${header.h}").unwrap(),
            Expr::Interpolated(vec![
                InterpolatedPart::Literal("Got -> ".to_string()),
                InterpolatedPart::Expr(Box::new(Expr::Header("h".to_string()))),
            ])
        );
    }

    #[test]
    fn parse_template_whole_slot_returns_atom() {
        assert_eq!(
            parse_template("${header.h}").unwrap(),
            Expr::Header("h".to_string())
        );
    }

    #[test]
    fn parse_template_whole_quoted_literal_is_raw() {
        assert_eq!(
            parse_template("'abc'").unwrap(),
            Expr::StringLit("'abc'".to_string())
        );
    }

    #[test]
    fn parse_template_preserves_whitespace() {
        assert_eq!(
            parse_template("${header.h} 3 items").unwrap(),
            Expr::Interpolated(vec![
                InterpolatedPart::Expr(Box::new(Expr::Header("h".to_string()))),
                InterpolatedPart::Literal(" 3 items".to_string()),
            ])
        );
    }

    #[test]
    fn parse_template_outer_trim() {
        assert_eq!(
            parse_template("Got ${header.h}\n").unwrap(),
            Expr::Interpolated(vec![
                InterpolatedPart::Literal("Got ".to_string()),
                InterpolatedPart::Expr(Box::new(Expr::Header("h".to_string()))),
            ])
        );
    }

    #[test]
    fn parse_template_empty_is_error() {
        assert!(parse_template("").is_err());
        assert!(parse_template("   ").is_err());
    }

    #[test]
    fn parse_template_unclosed_after_slot_is_error() {
        assert!(parse_template("${header.h} ${").is_err());
    }

    /// Malformed interpolations fail at compile time in TEMPLATE mode with a
    /// message naming the construct and the byte offset of the offending `${`
    /// in the trimmed source (rc-84p3k).
    #[test]
    fn parse_template_malformed_interpolation_table() {
        let unclosed = [
            ("Got ${name", 4),
            ("${header.h} trailing ${", 21),
            ("${header.h} ${", 12),
            ("${", 0),
            ("${body", 0),
        ];
        for (src, at) in unclosed {
            let err = parse_template(src).expect_err(&format!(
                "unclosed source {src:?} must fail at compile time"
            ));
            assert!(
                matches!(err, LanguageError::ParseError { .. }),
                "source {src:?}: {err:?}"
            );
            let msg = err.to_string();
            assert!(
                msg.contains("unclosed interpolation"),
                "source {src:?} message must name the construct, got: {msg}"
            );
            assert!(
                msg.contains(&format!("byte offset {at}")),
                "source {src:?} message must name the position, got: {msg}"
            );
        }
        // Empty slots are malformed too.
        for src in ["${}", "${   }"] {
            let err = parse_template(src)
                .expect_err(&format!("empty slot {src:?} must fail at compile time"));
            assert!(
                err.to_string().contains("empty interpolation"),
                "source {src:?} must report an empty interpolation, got: {err}"
            );
        }
        // Nested / unbalanced braces inside a slot fail at compile time. A
        // quoted `${` inside slot content is rejected too: the nested-${}
        // rule has no quote exemption (rc-84p3k; documented narrowing — such
        // sources previously compiled to path keys containing `${`).
        for src in [
            "${${header.a}}",
            "${body{x}",
            "${header.a${b}",
            "${header.a'${'b}",
        ] {
            assert!(
                parse_template(src).is_err(),
                "source {src:?} must fail at compile time"
            );
        }
    }

    /// The reported offset is relative to the trimmed source the parser scans,
    /// not the raw argument (rc-84p3k; semantics pinned here).
    #[test]
    fn parse_template_unclosed_offset_is_trimmed_source() {
        let err = parse_template("  Got ${name").unwrap_err();
        assert!(
            err.to_string().contains("byte offset 4"),
            "offset must be relative to the trimmed source, got: {err}"
        );
    }

    #[test]
    fn parse_template_unknown_slot_is_error() {
        assert!(matches!(
            parse_template("${unknown}"),
            Err(LanguageError::ParseError { .. })
        ));
    }

    #[test]
    fn parse_template_adjacent_literal_and_slot() {
        assert_eq!(
            parse_template("Got${body}").unwrap(),
            Expr::Interpolated(vec![
                InterpolatedPart::Literal("Got".to_string()),
                InterpolatedPart::Expr(Box::new(Expr::Body)),
            ])
        );
    }

    #[test]
    fn parse_template_unclosed_at_start_is_error() {
        assert!(matches!(
            parse_template("${error"),
            Err(LanguageError::ParseError { .. })
        ));
    }

    #[test]
    fn parse_template_adjacent_two_slots() {
        assert_eq!(
            parse_template("${header.a}${header.b}").unwrap(),
            Expr::Interpolated(vec![
                InterpolatedPart::Expr(Box::new(Expr::Header("a".to_string()))),
                InterpolatedPart::Expr(Box::new(Expr::Header("b".to_string()))),
            ])
        );
    }

    /// Malformed interpolations fail at compile time in PREDICATE mode too —
    /// the same rule as TEMPLATE mode, via the same message contract
    /// (rc-84p3k). Offsets are byte offsets in the trimmed source.
    #[test]
    fn parse_predicate_malformed_interpolation_table() {
        let unclosed = [
            ("Got ${name", 4),
            ("${header.h} trailing ${", 21),
            ("${", 0),
            ("${body", 0),
        ];
        for (src, at) in unclosed {
            let err = parse(src).expect_err(&format!(
                "unclosed source {src:?} must fail at compile time"
            ));
            assert!(
                matches!(err, LanguageError::ParseError { .. }),
                "source {src:?}: {err:?}"
            );
            let msg = err.to_string();
            assert!(
                msg.contains("unclosed interpolation")
                    && msg.contains(&format!("byte offset {at}")),
                "source {src:?} message must name construct and position, got: {msg}"
            );
        }
        for src in ["${}", "${   }"] {
            let err =
                parse(src).expect_err(&format!("empty slot {src:?} must fail at compile time"));
            assert!(
                err.to_string().contains("empty interpolation"),
                "source {src:?} must report an empty interpolation, got: {err}"
            );
        }
        for src in ["${${header.a}}", "${body{x}", "${header.a'${'b}"] {
            assert!(
                parse(src).is_err(),
                "source {src:?} must fail at compile time"
            );
        }
    }

    /// Legitimate literal text in PREDICATE mode still compiles: quoted
    /// strings protect `${` and braces; unquoted container syntax is deferred.
    #[test]
    fn parse_predicate_mode_literal_table() {
        assert_eq!(
            parse("'Got ${name'").unwrap(),
            Expr::StringLit("Got ${name".to_string())
        );
        assert_eq!(
            parse("\"a${b\"").unwrap(),
            Expr::EscapedString("a${b".to_string())
        );
        assert_eq!(
            parse("Got $100 from ACME").unwrap(),
            Expr::StringLit("Got $100 from ACME".to_string())
        );
        let err = parse("braces { } stay").unwrap_err();
        assert!(
            err.to_string()
                .contains("json/map construction is deferred"),
            "{err}"
        );
    }

    fn int_call(name: &str) -> Expr {
        Expr::Call {
            kind: CallKind::Int,
            args: vec![Expr::Header(name.to_string())],
        }
    }

    #[test]
    fn precedence_climbs_correctly() {
        let expr = parse("int(header.a) + 2 * 3 >= int(header.b) - 1 && property.c || property.d")
            .unwrap();
        let expected = Expr::LogicalOp {
            left: Box::new(Expr::LogicalOp {
                left: Box::new(Expr::BinOp {
                    left: Box::new(Expr::BinOp {
                        left: Box::new(int_call("a")),
                        op: Op::Add,
                        right: Box::new(Expr::BinOp {
                            left: Box::new(Expr::IntLit(2)),
                            op: Op::Mul,
                            right: Box::new(Expr::IntLit(3)),
                        }),
                    }),
                    op: Op::Gte,
                    right: Box::new(Expr::BinOp {
                        left: Box::new(int_call("b")),
                        op: Op::Sub,
                        right: Box::new(Expr::IntLit(1)),
                    }),
                }),
                op: LogicalOp::And,
                right: Box::new(Expr::ExchangeProperty("c".to_string())),
            }),
            op: LogicalOp::Or,
            right: Box::new(Expr::ExchangeProperty("d".to_string())),
        };
        assert_eq!(expr, expected);

        let regrouped = parse("(int(header.a) + 2) * 3").unwrap();
        assert_eq!(
            regrouped,
            Expr::BinOp {
                left: Box::new(Expr::BinOp {
                    left: Box::new(int_call("a")),
                    op: Op::Add,
                    right: Box::new(Expr::IntLit(2)),
                }),
                op: Op::Mul,
                right: Box::new(Expr::IntLit(3)),
            }
        );
    }

    #[test]
    fn comparison_does_not_chain() {
        let err = parse("1 < 2 < 3").unwrap_err();
        assert!(
            err.to_string().contains("comparisons do not chain"),
            "{err}"
        );
    }

    #[test]
    fn deferred_constructs_rejected() {
        let cases = [
            ("1.5", "floats are deferred"),
            ("7 / 2", "division is deferred"),
            ("10 % 3", "modulo is deferred"),
            ("'a' + 'b'", "string concatenation is deferred"),
            ("x ? y : z", "ternary is deferred"),
            ("n in (1, 2)", "in is deferred"),
            ("a ?? b", "null-coalescing is deferred"),
            ("2 ** 3", "exponentiation is deferred"),
        ];
        for (src, reason) in cases {
            let err = parse(src).unwrap_err();
            assert!(
                err.to_string().contains(reason),
                "source {src:?} must report {reason:?}, got: {err}"
            );
        }
    }

    #[test]
    fn bare_identifier_and_now_rejected() {
        assert!(parse("x == 1").is_err());
        assert!(parse("now() > 1000").is_err());
        assert!(parse("property.x == 1").is_ok());
    }

    #[test]
    fn accepted_calls_parse() {
        assert!(
            parse(
                "min(int(header.base), int(header.cap)) <= int(property.nowMs) && default(property.giveUpAt, '0') != 'null'"
            )
            .is_ok()
        );
        let err = parse("int(header.a, header.b)").unwrap_err();
        assert!(err.to_string().contains("call arity"), "{err}");
        let err = parse("default(header.x)").unwrap_err();
        assert!(err.to_string().contains("call arity"), "{err}");
    }

    #[test]
    fn integer_literal_boundaries() {
        assert_eq!(
            parse("9223372036854775807").unwrap(),
            Expr::IntLit(i64::MAX)
        );
        assert_eq!(
            parse("-9223372036854775808").unwrap(),
            Expr::IntLit(i64::MIN)
        );
        let err = parse("9223372036854775808").unwrap_err();
        assert!(
            err.to_string().contains("integer literal out of range"),
            "{err}"
        );
        assert!(parse("99999999999999999999").is_err());
    }

    fn int_property_call(name: &str) -> Expr {
        Expr::Call {
            kind: CallKind::Int,
            args: vec![Expr::ExchangeProperty(name.to_string())],
        }
    }

    #[test]
    fn slot_expression_parses() {
        assert_eq!(
            parse_template("retry-at-${int(property.nowMs) + int(header.delay)}").unwrap(),
            Expr::Interpolated(vec![
                InterpolatedPart::Literal("retry-at-".to_string()),
                InterpolatedPart::Expr(Box::new(Expr::BinOp {
                    left: Box::new(int_property_call("nowMs")),
                    op: Op::Add,
                    right: Box::new(int_call("delay")),
                })),
            ])
        );
    }

    #[test]
    fn lookup_leading_slot_expression() {
        assert_eq!(
            parse_template("${header.n + 1}").unwrap(),
            Expr::BinOp {
                left: Box::new(Expr::Header("n".to_string())),
                op: Op::Add,
                right: Box::new(Expr::IntLit(1)),
            }
        );
        assert_eq!(
            parse_template("${header.a < header.b}").unwrap(),
            Expr::BinOp {
                left: Box::new(Expr::Header("a".to_string())),
                op: Op::Lt,
                right: Box::new(Expr::Header("b".to_string())),
            }
        );
    }

    #[test]
    fn whole_slot_expression_is_native() {
        assert_eq!(
            parse_template("${int(header.n) * 2}").unwrap(),
            Expr::BinOp {
                left: Box::new(int_call("n")),
                op: Op::Mul,
                right: Box::new(Expr::IntLit(2)),
            }
        );
    }

    #[tokio::test]
    async fn slot_expression_evaluates_values() {
        use crate::SimpleLanguage;
        use camel_api::ExpressionErrorClass;
        use camel_language_api::{Exchange, Language, Value};

        let lang = SimpleLanguage::new();
        let expr = lang
            .create_expression("${int(header.a) + header.b}")
            .unwrap();
        let mut ex = Exchange::default();
        ex.input.set_header("a", Value::String("10".to_string()));
        ex.input.set_header("b", Value::String("9".to_string()));
        assert_eq!(expr.evaluate(&ex).await.unwrap(), Value::Number(19.into()));

        let expr = lang.create_expression("${int(property.n) + 1}").unwrap();
        let mut ex = Exchange::default();
        ex.set_property("n".to_string(), Value::String(i64::MAX.to_string()));
        let err = expr.evaluate(&ex).await.unwrap_err();
        assert!(
            matches!(
                err,
                LanguageError::EvalFailure {
                    class: ExpressionErrorClass::Arithmetic,
                    ..
                }
            ),
            "{err:?}"
        );
    }

    #[test]
    fn nested_slot_rejected() {
        let err = parse_template("${default(header.x, '${header.y}')}").unwrap_err();
        assert!(err.to_string().contains("nested slots"), "{err}");
    }

    #[test]
    fn unknown_language_still_delegates() {
        assert_eq!(
            parse_template("${nope:x}").unwrap(),
            Expr::LanguageDelegate {
                language: "nope".to_string(),
                expression: "x".to_string(),
            }
        );
    }

    #[test]
    fn delegate_payload_with_operators_is_verbatim() {
        assert_eq!(
            parse_template("${nope:x+1}").unwrap(),
            Expr::LanguageDelegate {
                language: "nope".to_string(),
                expression: "x+1".to_string(),
            }
        );
        assert_eq!(
            parse_template("${jsonpath:$.store.book[0].title}").unwrap(),
            Expr::LanguageDelegate {
                language: "jsonpath".to_string(),
                expression: "$.store.book[0].title".to_string(),
            }
        );
        assert_eq!(
            parse_template("${nope:x y}").unwrap(),
            Expr::LanguageDelegate {
                language: "nope".to_string(),
                expression: "x y".to_string(),
            }
        );
        // Trailing whitespace is part of the delegated payload.
        assert_eq!(
            parse_template("${nope:x }").unwrap(),
            Expr::LanguageDelegate {
                language: "nope".to_string(),
                expression: "x ".to_string(),
            }
        );
    }

    #[test]
    fn slot_text_token_rejected() {
        let err = parse_template("${@}").unwrap_err();
        assert!(err.to_string().contains("unrecognized token"), "{err}");
        // A single quoted string literal is a valid atom.
        assert_eq!(
            parse_template("${'quoted'}").unwrap(),
            Expr::StringLit("quoted".to_string())
        );
    }

    #[test]
    fn slot_hyphen_key_vs_subtraction() {
        assert_eq!(
            parse_template("${header.my-header}").unwrap(),
            Expr::Header("my-header".to_string())
        );
        assert_eq!(
            parse_template("${header.n-1}").unwrap(),
            Expr::BinOp {
                left: Box::new(Expr::Header("n".to_string())),
                op: Op::Sub,
                right: Box::new(Expr::IntLit(1)),
            }
        );
        // A hyphen followed by a letter continues the identifier run, even
        // in an expression, so the key stays one lookup.
        assert_eq!(
            parse_template("${header.some-key + 1}").unwrap(),
            Expr::BinOp {
                left: Box::new(Expr::Header("some-key".to_string())),
                op: Op::Add,
                right: Box::new(Expr::IntLit(1)),
            }
        );
    }

    #[test]
    fn spaced_hyphen_subtraction_targets_key_error() {
        for src in ["header.some - key", "header.some -key"] {
            let err = parse(src).unwrap_err();
            let msg = err.to_string();
            assert!(msg.contains("spaced hyphen"), "{src}: {msg}");
            assert!(msg.contains("header.some-key"), "{src}: {msg}");
        }
        for src in ["${header.some - key}", "a ${header.some - key} b"] {
            let err = parse_template(src).unwrap_err();
            let msg = err.to_string();
            assert!(msg.contains("spaced hyphen"), "{src}: {msg}");
            assert!(msg.contains("header.some-key"), "{src}: {msg}");
        }
    }

    #[test]
    fn generic_unknown_identifier_preserved() {
        for src in [
            "key - 1",
            "check-in",
            "body - key",
            "exception.message - key",
            "header.n - max(1, x)",
        ] {
            let err = parse(src).unwrap_err();
            let msg = err.to_string();
            assert!(msg.contains("unknown identifier"), "{src}: {msg}");
        }
    }

    #[test]
    fn subtraction_and_lookup_unchanged() {
        assert_eq!(
            parse("header.n - 1").unwrap(),
            Expr::BinOp {
                left: Box::new(Expr::Header("n".to_string())),
                op: Op::Sub,
                right: Box::new(Expr::IntLit(1)),
            }
        );
        assert_eq!(
            parse("header.some - header.other").unwrap(),
            Expr::BinOp {
                left: Box::new(Expr::Header("some".to_string())),
                op: Op::Sub,
                right: Box::new(Expr::Header("other".to_string())),
            }
        );
        assert_eq!(
            parse("header.some-key == 'x'").unwrap(),
            Expr::BinOp {
                left: Box::new(Expr::Header("some-key".to_string())),
                op: Op::Eq,
                right: Box::new(Expr::StringLit("x".to_string())),
            }
        );
        assert_eq!(
            parse_template("${header.some-key + 1}").unwrap(),
            Expr::BinOp {
                left: Box::new(Expr::Header("some-key".to_string())),
                op: Op::Add,
                right: Box::new(Expr::IntLit(1)),
            }
        );
    }

    #[test]
    fn hyphenated_keyword_keys_stay_lookups() {
        assert_eq!(
            parse_template("${header.check-in}").unwrap(),
            Expr::Header("check-in".to_string())
        );
        assert_eq!(
            parse_template("${header.x-contains}").unwrap(),
            Expr::Header("x-contains".to_string())
        );
        // A whole-word keyword inside a lookup path is still a lookup key.
        assert_eq!(
            parse_template("${header.in}").unwrap(),
            Expr::Header("in".to_string())
        );
        // Hyphenated bare identifiers are unknown identifiers, not keywords.
        let err = parse("check-in").unwrap_err();
        assert!(err.to_string().contains("unknown identifier"), "{err}");
        // Standalone hyphen-free keywords remain operators.
        let err = parse("n in (1, 2)").unwrap_err();
        assert!(err.to_string().contains("in is deferred"), "{err}");
    }

    #[test]
    fn unknown_slot_still_compile_error() {
        assert!(matches!(
            parse_template("${unknown}"),
            Err(LanguageError::ParseError { .. })
        ));
    }

    #[test]
    fn slot_budget_shared() {
        let template: String = "${int(header.a) + int(header.b)}".repeat(100);
        let err = parse_template(&template).unwrap_err();
        assert!(err.to_string().contains("512 node limit"), "{err}");
    }

    #[test]
    fn parse_time_limits() {
        let oversized = "a".repeat(17 * 1024);
        let err = parse(&oversized).unwrap_err();
        assert!(err.to_string().contains("source exceeds"), "{err}");

        // Doubling builds a balanced tree: nodes grow exponentially while
        // depth stays logarithmic, so the node limit trips before depth.
        let mut tree = "header.a + header.b".to_string();
        for _ in 0..8 {
            tree = format!("({tree} + {tree})");
        }
        let err = parse(&tree).unwrap_err();
        assert!(err.to_string().contains("512 node limit"), "{err}");

        let nested_65 = format!("{}header.a{}", "(".repeat(65), ")".repeat(65));
        let err = parse(&nested_65).unwrap_err();
        assert!(err.to_string().contains("64 depth limit"), "{err}");

        let nested_64 = format!("{}header.a{}", "(".repeat(64), ")".repeat(64));
        assert!(parse(&nested_64).is_ok());
    }

    #[test]
    fn combined_depth_limit() {
        // 40 grouping parentheses around 40 nested calls exceed the single
        // combined semantic depth budget even though each part alone is under.
        let mixed = format!(
            "{}{}header.a{}{}",
            "(".repeat(40),
            "int(".repeat(40),
            ")".repeat(40),
            ")".repeat(40)
        );
        let err = parse(&mixed).unwrap_err();
        assert!(err.to_string().contains("64 depth limit"), "{err}");

        // Either construct alone at exactly 64 nesting levels is accepted.
        let calls_64 = format!("{}header.a{}", "int(".repeat(64), ")".repeat(64));
        assert!(parse(&calls_64).is_ok());

        let parens_64 = format!("{}header.a{}", "(".repeat(64), ")".repeat(64));
        assert!(parse(&parens_64).is_ok());
    }

    #[test]
    fn standalone_identifier_resolution() {
        let err = parse("x").unwrap_err();
        assert!(err.to_string().contains("unknown identifier"), "{err}");

        assert_eq!(parse("header.x").unwrap(), Expr::Header("x".to_string()));
        assert_eq!(parse("body").unwrap(), Expr::Body);

        // Template mode keeps the literal-text fallback.
        assert_eq!(
            parse_template("x").unwrap(),
            Expr::StringLit("x".to_string())
        );
    }

    #[test]
    fn string_concat_with_grouping_rejected() {
        let err = parse("'1' + ('2')").unwrap_err();
        assert!(
            err.to_string().contains("string concatenation is deferred"),
            "{err}"
        );
        // A lookup plus a string literal is dynamic, not static concatenation.
        assert!(parse("header.a + 'x'").is_ok());
    }

    #[test]
    fn source_limit_checked_before_trim() {
        let padded = format!("{}1", " ".repeat(17 * 1024));
        let err = parse(&padded).unwrap_err();
        assert!(
            err.to_string().contains("source exceeds 16 KiB limit"),
            "{err}"
        );
        let err = parse_template(&padded).unwrap_err();
        assert!(
            err.to_string().contains("source exceeds 16 KiB limit"),
            "{err}"
        );
    }

    #[test]
    fn constructed_depth_limit() {
        // A 70-term left-nested sum stays under the node limit but its tree
        // depth exceeds 64.
        let chain_70 = vec!["1"; 70].join(" + ");
        let err = parse(&chain_70).unwrap_err();
        assert!(err.to_string().contains("64 depth limit"), "{err}");

        // A 63-deep chain (64 terms, 63 binary nodes) is valid.
        let chain_64 = vec!["1"; 64].join(" + ");
        assert!(parse(&chain_64).is_ok());

        // The Interpolated wrapper and slot boundary add two semantic levels,
        // so a slot chain that is valid on its own exceeds the bound only
        // when embedded in template text.
        let template = format!("prefix ${{{chain_64}}} suffix");
        let err = parse_template(&template).unwrap_err();
        assert!(err.to_string().contains("64 depth limit"), "{err}");

        // Parenthesis depth is still handled by construction-time accounting.
        let parens_64 = format!("{}header.a{}", "(".repeat(64), ")".repeat(64));
        assert!(parse(&parens_64).is_ok());
    }

    #[test]
    fn predicate_slots_use_expression_grammar() {
        assert!(parse("${header.n > 1}").is_ok());
        assert!(parse("${int(header.n) + 1}").is_ok());
        // Atom and delegate paths are preserved.
        assert_eq!(parse("${header.n}").unwrap(), Expr::Header("n".to_string()));
        assert_eq!(
            parse("${nope:x}").unwrap(),
            Expr::LanguageDelegate {
                language: "nope".to_string(),
                expression: "x".to_string(),
            }
        );
    }

    #[test]
    fn slot_operator_terminates_atom() {
        let compact = parse_template("${header.n+1}").unwrap();
        let spaced = parse_template("${header.n + 1}").unwrap();
        assert_eq!(
            compact,
            Expr::BinOp {
                left: Box::new(Expr::Header("n".to_string())),
                op: Op::Add,
                right: Box::new(Expr::IntLit(1)),
            }
        );
        assert_eq!(compact, spaced);

        assert_eq!(
            parse_template("${header.n>1}").unwrap(),
            Expr::BinOp {
                left: Box::new(Expr::Header("n".to_string())),
                op: Op::Gt,
                right: Box::new(Expr::IntLit(1)),
            }
        );

        // The documented hyphen+letter key-continuation exception stays.
        assert_eq!(
            parse_template("${header.my-header}").unwrap(),
            Expr::Header("my-header".to_string())
        );
        assert_eq!(
            parse_template("${header.n-1}").unwrap(),
            Expr::BinOp {
                left: Box::new(Expr::Header("n".to_string())),
                op: Op::Sub,
                right: Box::new(Expr::IntLit(1)),
            }
        );
    }

    #[test]
    fn predicate_rejects_container_literal_syntax() {
        for src in ["{}", "{'a': 1}", "[]", "[1]"] {
            let err = parse(src).unwrap_err();
            assert!(
                err.to_string()
                    .contains("json/map construction is deferred"),
                "source {src:?}: {err}"
            );
        }
        // Quoted braces/brackets are strings, not construction.
        assert_eq!(parse("'{'").unwrap(), Expr::StringLit("{".to_string()));
        assert_eq!(parse("'[)'").unwrap(), Expr::StringLit("[)".to_string()));
        assert_eq!(
            parse("\"}\"").unwrap(),
            Expr::EscapedString("}".to_string())
        );
        // Slots themselves use braces but stay valid.
        assert!(parse("${header.type} == 'order'").is_ok());
        // Template mode keeps raw text verbatim.
        assert_eq!(
            parse_template("x{}y").unwrap(),
            Expr::StringLit("x{}y".to_string())
        );
        assert_eq!(
            parse_template("x[1]y").unwrap(),
            Expr::StringLit("x[1]y".to_string())
        );
    }

    fn slot_wrapped(depth: usize, inner: &str) -> String {
        format!("{}{}{}", "(".repeat(depth), inner, ")".repeat(depth))
    }

    #[test]
    fn slot_grouping_depth_crosses_boundaries() {
        let deep_64 = slot_wrapped(64, "1");
        let err = parse(&format!("${{{deep_64}}} == 1")).unwrap_err();
        assert!(err.to_string().contains("64 depth limit"), "{err}");

        let deep_61 = slot_wrapped(61, "1");
        assert!(parse(&format!("${{{deep_61}}} == 1")).is_ok());

        // Accumulated slot grouping is retained across adjacent slots.
        let a = slot_wrapped(40, "1");
        let b = slot_wrapped(40, "2");
        let err = parse(&format!("${{{a}}} == ${{{b}}}")).unwrap_err();
        assert!(err.to_string().contains("64 depth limit"), "{err}");
    }

    #[test]
    fn combined_paren_and_expression_depth() {
        let parens_64 = format!("{}1 + 1{}", "(".repeat(64), ")".repeat(64));
        let err = parse(&parens_64).unwrap_err();
        assert!(err.to_string().contains("64 depth limit"), "{err}");

        let parens_61 = format!("{}1 + 1{}", "(".repeat(61), ")".repeat(61));
        assert!(parse(&parens_61).is_ok());

        // The interpolation wrapper + slot boundary add two levels to the
        // slot's own combined depth.
        let slot = format!("{}1 + 1{}", "(".repeat(62), ")".repeat(62));
        let template = format!("x${{{slot}}}y");
        let err = parse_template(&template).unwrap_err();
        assert!(err.to_string().contains("64 depth limit"), "{err}");
    }
}
