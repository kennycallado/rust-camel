use camel_language_api::LanguageError;

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Header(String),
    Body,
    ExchangeProperty(String),
    StringLit(String),
    NumberLit(f64),
    Null,
    BinOp {
        left: Box<Expr>,
        op: Op,
        right: Box<Expr>,
    },
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

    // Try binary expression: <lhs> <op> <rhs>
    // Try operators longest-first to avoid partial matches (>= before >)
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

    parse_atom(input)
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

    if let Ok(n) = s.parse::<f64>() {
        return Ok(Expr::NumberLit(n));
    }

    Err(LanguageError::ParseError {
        expr: s.to_string(),
        reason: "unrecognized token".to_string(),
    })
}
