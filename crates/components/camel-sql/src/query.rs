//! Query template parsing and parameter resolution for SQL components.
//!
//! This module provides parsing of Camel-style SQL templates with parameter
//! placeholders and resolution of those parameters from an Exchange.
//!
//! TODO(SQL-006): Named parameters using `:name`-style syntax (e.g.
//! `SELECT * FROM users WHERE name = :name`) are not yet supported in
//! downstream SQL execution. The query template parser supports `:#name`
//! (Camel-style `:#` prefix) and positional `#`, but the underlying database
//! driver binding currently only supports positional `$1, $2, ...` params.

use camel_api::ExchangeLookupPath;
use camel_component_api::{Body, CamelError, Exchange};

/// A parsed parameter slot in a query template.
#[derive(Debug, Clone, PartialEq)]
pub enum ParamSlot {
    /// Positional parameter (#) — index is the 0-based position among all positional params
    Positional(usize),
    /// Named parameter (:#name) — resolved from headers/body/properties
    Named(String),
    /// IN clause parameter (:#in:name) — expanded to multiple $N placeholders
    InClause(String),
    /// Dynamic expression parameter (:#${expr}) — resolved from body/header/property paths
    Expression(String),
}

/// A parsed query template with fragments and parameter slots.
#[derive(Debug, Clone)]
pub struct QueryTemplate {
    /// Text fragments between parameters. Always has one more element than params.
    pub fragments: Vec<String>,
    /// Parameter slots in order of appearance.
    pub params: Vec<ParamSlot>,
}

/// A fully resolved query ready for execution.
#[derive(Debug, Clone)]
pub struct PreparedQuery {
    /// The final SQL with $1, $2, ... placeholders.
    pub sql: String,
    /// The binding values in order.
    pub bindings: Vec<serde_json::Value>,
}

/// Parses a Camel-style SQL template into fragments and parameter slots.
///
/// Token types (using `#` as default placeholder):
/// - `:#in:name` → InClause("name") — IN clause expansion
/// - `:#${expr}` → Expression("expr") — dynamic expression (body.field, header.name, property.key)
/// - `:#name` → Named("name") — named parameter (name is alphanumeric + underscore)
/// - `#` (standalone) → Positional(N) — positional parameter (N increments per positional)
///
/// # Arguments
/// * `template` - The SQL template string to parse
/// * `placeholder` - The character used as placeholder (typically '#')
///
/// # Returns
/// A `QueryTemplate` with fragments and parameter slots.
pub fn parse_query_template(
    template: &str,
    placeholder: char,
) -> Result<QueryTemplate, CamelError> {
    let mut fragments = Vec::new();
    let mut params = Vec::new();
    let mut positional_index = 0usize;

    let chars: Vec<char> = template.chars().collect();
    let mut i = 0usize;
    let mut last_param_end = 0usize;
    let mut in_literal = false;

    while i < chars.len() {
        // Handle string literals
        if chars[i] == '\'' {
            // Handle escaped quote '' inside literal
            if in_literal && i + 1 < chars.len() && chars[i + 1] == '\'' {
                i += 2;
                continue;
            }
            in_literal = !in_literal;
            i += 1;
            continue;
        }

        // Only process placeholder when not inside a string literal
        if !in_literal && chars[i] == placeholder {
            // Check if preceded by ':'
            if i > 0 && chars[i - 1] == ':' {
                // Named, Expression, or InClause parameter
                // Check if followed by 'in:' for InClause
                // Pattern: :#in:name
                //          ^  ^^^^
                //          |  check these chars after #
                let is_in_clause = check_in_prefix(&chars, i + 1);
                // Check for expression syntax :#${expr}
                // Pattern: :#${expr}
                //          ^  ^^
                //          |  check $ and { after #
                let is_expression =
                    i + 2 < chars.len() && chars[i + 1] == '$' && chars[i + 2] == '{';

                if is_in_clause {
                    // InClause parameter - extract name after ':#in:'
                    // i is at '#', so name starts at i + 4 (after 'in:')
                    let name_start = i + 4;
                    let (name, name_end) = extract_param_name(&chars, name_start);

                    if name.is_empty() {
                        return Err(CamelError::ProcessorError(format!(
                            "Empty IN clause parameter name at position {}",
                            i
                        )));
                    }

                    // Fragment ends at ':' (position i-1)
                    fragments.push(chars[last_param_end..(i - 1)].iter().collect());

                    params.push(ParamSlot::InClause(name));
                    last_param_end = name_end;
                    i = name_end;
                } else if is_expression {
                    // Expression parameter - extract content between { and }
                    // i is at '#', so $ is at i + 1, { is at i + 2
                    let brace_start = i + 2;
                    if let Some(brace_end) = find_matching_brace(&chars, brace_start) {
                        // Extract expression content (between { and })
                        let expr_content: String =
                            chars[(brace_start + 1)..brace_end].iter().collect();

                        if expr_content.is_empty() {
                            return Err(CamelError::ProcessorError(format!(
                                "Empty expression at position {}",
                                i
                            )));
                        }

                        // Fragment ends at ':' (position i-1)
                        fragments.push(chars[last_param_end..(i - 1)].iter().collect());

                        params.push(ParamSlot::Expression(expr_content));
                        last_param_end = brace_end + 1;
                        i = brace_end + 1;
                    } else {
                        return Err(CamelError::ProcessorError(format!(
                            "Unclosed expression at position {}",
                            i
                        )));
                    }
                } else {
                    // Named parameter - extract name after ':#'
                    let name_start = i + 1;
                    let (name, name_end) = extract_param_name(&chars, name_start);

                    if name.is_empty() {
                        return Err(CamelError::ProcessorError(format!(
                            "Empty named parameter name at position {}",
                            i
                        )));
                    }

                    // Fragment ends at ':' (position i-1)
                    fragments.push(chars[last_param_end..(i - 1)].iter().collect());

                    params.push(ParamSlot::Named(name));
                    last_param_end = name_end;
                    i = name_end;
                }
            } else {
                // Positional parameter
                // Fragment ends at this '#'
                fragments.push(chars[last_param_end..i].iter().collect());

                params.push(ParamSlot::Positional(positional_index));
                positional_index += 1;
                last_param_end = i + 1;
                i += 1;
            }
        } else {
            i += 1;
        }
    }

    // Add final fragment (everything after last param, or whole string if no params)
    fragments.push(chars[last_param_end..].iter().collect());

    Ok(QueryTemplate { fragments, params })
}

/// Check if chars starting at position `start` begin with "in:"
fn check_in_prefix(chars: &[char], start: usize) -> bool {
    let in_prefix: Vec<char> = "in:".chars().collect();
    if start + in_prefix.len() > chars.len() {
        return false;
    }
    chars[start..start + in_prefix.len()] == in_prefix[..]
}

/// Extract a parameter name starting at the given position.
///
/// Reads until a SQL delimiter is encountered. The terminator set is:
/// whitespace, `?`, `(`, `)`, `,`, `;`, `=`, `<`, `>`, `!`, `'`, `"`, `` ` ``,
/// `/`, and `:`. Single `:` and Postgres `::` casts both terminate the name
/// (see rc-o6o Phase 2 §3.2 — `:#id::text` resolves `id` and leaves `::text`
/// as SQL). Names may contain alphanumerics, `_`, `-`, `.`, and other
/// characters valid in map keys.
///
/// Returns `(name, end_position)` where `end_position` is the index of the
/// terminator (or `chars.len()` at end-of-string).
fn extract_param_name(chars: &[char], start: usize) -> (String, usize) {
    let mut name = String::new();
    let mut i = start;
    while i < chars.len() {
        let c = chars[i];
        if is_name_terminator(c) {
            break;
        }
        name.push(c);
        i += 1;
    }
    (name, i)
}

/// Returns true when `c` ends a placeholder name. See [`extract_param_name`].
fn is_name_terminator(c: char) -> bool {
    c.is_whitespace()
        || matches!(
            c,
            '?' | '(' | ')' | ',' | ';' | '=' | '<' | '>' | '!' | '\'' | '"' | '`' | '/' | ':'
        )
}

/// Finds the closing brace matching an opening brace at `start` (0-indexed into chars).
/// Returns the index of `}` or None if not found.
fn find_matching_brace(chars: &[char], start: usize) -> Option<usize> {
    // Simple scan for matching } (SQL expressions won't have nested braces)
    chars[start..]
        .iter()
        .position(|&c| c == '}')
        .map(|p| start + p)
}

/// Resolves parameter values from the exchange and builds the final SQL.
///
/// Resolution rules:
/// - **Named**: Look up value in order: (1) body if it's a JSON object → look for key,
///   (2) exchange input headers, (3) exchange properties. Error if not found anywhere.
/// - **Positional**: Body must be a JSON array → use index. Error if out of bounds.
/// - **InClause**: Resolve same as Named, but value must be a JSON array → expand to
///   multiple `$N, $N+1, ...` placeholders.
///
/// The output SQL uses `$1, $2, ...` numbered placeholders (sqlx style).
///
/// # Arguments
/// * `tpl` - The parsed query template
/// * `exchange` - The exchange containing message body, headers, and properties
/// * `in_separator` - Separator between values in IN clause expansion
///
/// # Returns
/// A `PreparedQuery` with the final SQL and bindings.
pub fn resolve_params(
    tpl: &QueryTemplate,
    exchange: &Exchange,
    in_separator: &str,
) -> Result<PreparedQuery, CamelError> {
    let mut sql_parts = Vec::new();
    let mut bindings = Vec::new();
    let mut placeholder_num = 1usize;

    // Pre-extract body as JSON array for POSITIONAL params only.
    // (Named / InClause / Expression now route through ExchangeLookupPath.)
    let body_array = match &exchange.input.body {
        Body::Json(val) => val.as_array(),
        _ => None,
    };

    for (i, param) in tpl.params.iter().enumerate() {
        // Add fragment before this param
        sql_parts.push(tpl.fragments[i].clone());

        match param {
            ParamSlot::Positional(idx) => {
                let arr = body_array.ok_or_else(|| {
                    CamelError::ProcessorError(
                        "Positional parameter requires body to be a JSON array".to_string(),
                    )
                })?;

                let value = arr.get(*idx).ok_or_else(|| {
                    CamelError::ProcessorError(format!(
                        "Positional parameter index {} out of bounds (array length {})",
                        idx,
                        arr.len()
                    ))
                })?;

                sql_parts.push(format!("${}", placeholder_num));
                placeholder_num += 1;
                bindings.push(value.clone());
            }
            ParamSlot::Named(name) => {
                let value = resolve_named_param(name, exchange)?;
                sql_parts.push(format!("${}", placeholder_num));
                placeholder_num += 1;
                bindings.push(value);
            }
            ParamSlot::InClause(name) => {
                let value = resolve_named_param(name, exchange)?;

                let arr = value.as_array().ok_or_else(|| {
                    CamelError::ProcessorError(format!(
                        "IN clause parameter '{}' must be an array, got type {}",
                        name,
                        match &value {
                            serde_json::Value::Null => "null",
                            serde_json::Value::Bool(_) => "bool",
                            serde_json::Value::Number(_) => "number",
                            serde_json::Value::String(_) => "string",
                            serde_json::Value::Array(_) => "array",
                            serde_json::Value::Object(_) => "object",
                        }
                    ))
                })?;

                if arr.is_empty() {
                    // Empty IN clause - emit NULL to produce valid SQL like "IN (NULL)"
                    // which matches nothing (no bindings needed for NULL literal)
                    sql_parts.push("NULL".to_string());
                } else {
                    let placeholders: Vec<String> = arr
                        .iter()
                        .map(|_| {
                            let p = format!("${}", placeholder_num);
                            placeholder_num += 1;
                            p
                        })
                        .collect();

                    // Just output comma-separated placeholders, template has parentheses
                    sql_parts.push(placeholders.join(in_separator));
                    bindings.extend(arr.iter().cloned());
                }
            }
            ParamSlot::Expression(expr) => {
                let value = resolve_expression_param(expr, exchange)?;
                sql_parts.push(format!("${}", placeholder_num));
                placeholder_num += 1;
                bindings.push(value);
            }
        }
    }

    // Add final fragment
    sql_parts.push(tpl.fragments.last().cloned().unwrap_or_default());

    Ok(PreparedQuery {
        sql: sql_parts.join(""),
        bindings,
    })
}

/// Resolves a named parameter (`:#name`, `:#in:name`, or the identifier portion
/// of `:#${expr}`) via the shared [`ExchangeLookupPath`] grammar.
///
/// Resolution order:
/// - `body.x.y` walks JSON; `header.x.y` / `property.x.y` / `exchangeProperty.x.y`
///   are flat keys (maps not nested); anything else is unscoped (body JSON key
///   → header → property).
///
/// Returns `CamelError::ProcessorError` when the path does not resolve.
fn resolve_named_param(name: &str, exchange: &Exchange) -> Result<serde_json::Value, CamelError> {
    let path = ExchangeLookupPath::parse(name).map_err(|e| {
        CamelError::ProcessorError(format!("Invalid placeholder name {name:?}: {e}"))
    })?;
    // SQL binds scalars. Bare reserved scope names like `:#body` parse as
    // `Body(vec![])` (whole body, meaningful for Simple `${body}`) but are
    // ambiguous for SQL — reject explicitly per e_gpt oracle blessing #2.
    if let ExchangeLookupPath::Body(segments) = &path
        && segments.is_empty()
    {
        return Err(CamelError::ProcessorError(format!(
            "SQL placeholder ':#{name}' requires a path (e.g. ':#{name}.field') — bare scope not supported"
        )));
    }
    path.lookup(exchange).ok_or_else(|| {
        CamelError::ProcessorError(format!(
            "Named parameter '{name}' not found in body, headers, or properties"
        ))
    })
}

/// Resolves an expression parameter (`:#${expr}`). The expr is the literal
/// content between `{` and `}` — it follows the same [`ExchangeLookupPath``
/// grammar as `:#name`. Simple language delegates (`:#${jsonpath:$.a}`) are
/// NOT supported here; that path belongs to the Simple language itself, not
/// SQL's `:#${...}` shape.
fn resolve_expression_param(
    expr: &str,
    exchange: &Exchange,
) -> Result<serde_json::Value, CamelError> {
    // Same grammar, distinct error message so users can tell which placeholder
    // form they hit (the SQL fragment around the param is the real clue, but
    // the message prefix removes ambiguity for log-only contexts).
    let path = ExchangeLookupPath::parse(expr)
        .map_err(|e| CamelError::ProcessorError(format!("Invalid expression {expr:?}: {e}")))?;
    // Same bare-scope rejection as `resolve_named_param` — `:#${body}` is
    // ambiguous for SQL just like `:#body`.
    if let ExchangeLookupPath::Body(segments) = &path
        && segments.is_empty()
    {
        return Err(CamelError::ProcessorError(format!(
            "SQL expression ':#${{{expr}}}' requires a path (e.g. ':#${{{expr}.field}}') — bare scope not supported"
        )));
    }
    path.lookup(exchange).ok_or_else(|| {
        CamelError::ProcessorError(format!(
            "Expression '{expr}' not found in body, headers, or properties"
        ))
    })
}

/// Returns true if SQL should run through select path.
pub fn is_select_query(sql: &str) -> bool {
    let trimmed = sql.trim_start().to_uppercase();
    if trimmed.starts_with("WITH") {
        return trimmed.contains("SELECT")
            && !trimmed.contains("INSERT INTO")
            && !trimmed.contains("UPDATE ")
            && !trimmed.contains("DELETE FROM");
    }

    trimmed.starts_with("SELECT")
        || trimmed.starts_with("VALUES")
        || trimmed.starts_with("TABLE")
        || trimmed.starts_with("SHOW")
        || trimmed.starts_with("EXPLAIN")
}

const DEFAULT_IN_SEPARATOR: &str = ", ";

pub fn resolve_params_default(
    tpl: &QueryTemplate,
    exchange: &Exchange,
) -> Result<PreparedQuery, CamelError> {
    resolve_params(tpl, exchange, DEFAULT_IN_SEPARATOR)
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_component_api::{Body, Exchange, Message};

    #[test]
    fn parse_no_params() {
        let tpl = parse_query_template("select * from users", '#').unwrap();
        assert_eq!(tpl.fragments.len(), 1);
        assert!(tpl.params.is_empty());
    }

    #[test]
    fn parse_positional_params() {
        let tpl = parse_query_template("insert into t values (#, #)", '#').unwrap();
        assert_eq!(tpl.params.len(), 2);
        assert!(matches!(tpl.params[0], ParamSlot::Positional(0)));
        assert!(matches!(tpl.params[1], ParamSlot::Positional(1)));
    }

    #[test]
    fn parse_named_params() {
        let tpl =
            parse_query_template("select * from t where id = :#id and name = :#name", '#').unwrap();
        assert_eq!(tpl.params.len(), 2);
        assert!(matches!(&tpl.params[0], ParamSlot::Named(n) if n == "id"));
        assert!(matches!(&tpl.params[1], ParamSlot::Named(n) if n == "name"));
    }

    #[test]
    fn parse_mixed_params() {
        let tpl =
            parse_query_template("select * from t where id = :#id and status = #", '#').unwrap();
        assert_eq!(tpl.params.len(), 2);
        assert!(matches!(&tpl.params[0], ParamSlot::Named(n) if n == "id"));
        assert!(matches!(tpl.params[1], ParamSlot::Positional(0)));
    }

    #[test]
    fn parse_in_clause() {
        let tpl = parse_query_template("select * from t where id in (:#in:ids)", '#').unwrap();
        assert_eq!(tpl.params.len(), 1);
        assert!(matches!(&tpl.params[0], ParamSlot::InClause(n) if n == "ids"));
    }

    #[test]
    fn resolve_named_from_headers() {
        let tpl = parse_query_template("select * from t where id = :#id", '#').unwrap();
        let mut msg = Message::default();
        msg.set_header("id", serde_json::json!(42));
        let ex = Exchange::new(msg);

        let prepared = resolve_params(&tpl, &ex, ", ").unwrap();
        assert_eq!(prepared.sql, "select * from t where id = $1");
        assert_eq!(prepared.bindings.len(), 1);
        assert_eq!(prepared.bindings[0], serde_json::json!(42));
    }

    #[test]
    fn resolve_named_from_body_map() {
        let tpl = parse_query_template("select * from t where id = :#id", '#').unwrap();
        let msg = Message::new(Body::Json(serde_json::json!({"id": 99})));
        let ex = Exchange::new(msg);

        let prepared = resolve_params(&tpl, &ex, ", ").unwrap();
        assert_eq!(prepared.bindings[0], serde_json::json!(99));
    }

    #[test]
    fn resolve_positional_from_body_array() {
        let tpl = parse_query_template("insert into t values (#, #)", '#').unwrap();
        let msg = Message::new(Body::Json(serde_json::json!(["foo", 42])));
        let ex = Exchange::new(msg);

        let prepared = resolve_params(&tpl, &ex, ", ").unwrap();
        assert_eq!(prepared.sql, "insert into t values ($1, $2)");
        assert_eq!(prepared.bindings[0], serde_json::json!("foo"));
        assert_eq!(prepared.bindings[1], serde_json::json!(42));
    }

    #[test]
    fn resolve_named_from_properties() {
        let tpl = parse_query_template("select * from t where id = :#myProp", '#').unwrap();
        let mut ex = Exchange::new(Message::default());
        ex.set_property("myProp", serde_json::json!(7));

        let prepared = resolve_params(&tpl, &ex, ", ").unwrap();
        assert_eq!(prepared.bindings[0], serde_json::json!(7));
    }

    #[test]
    fn resolve_named_not_found() {
        let tpl = parse_query_template("select * from t where id = :#missing", '#').unwrap();
        let ex = Exchange::new(Message::default());

        let result = resolve_params(&tpl, &ex, ", ");
        assert!(result.is_err());
    }

    #[test]
    fn resolve_in_clause_expansion() {
        let tpl = parse_query_template("select * from t where id in (:#in:ids)", '#').unwrap();
        let mut msg = Message::default();
        msg.set_header("ids", serde_json::json!([1, 2, 3]));
        let ex = Exchange::new(msg);

        let prepared = resolve_params(&tpl, &ex, ", ").unwrap();
        assert_eq!(prepared.sql, "select * from t where id in ($1, $2, $3)");
        assert_eq!(
            prepared.bindings,
            vec![
                serde_json::json!(1),
                serde_json::json!(2),
                serde_json::json!(3)
            ]
        );
    }

    #[test]
    fn build_sql_correct_placeholders() {
        let tpl = parse_query_template(
            "select * from t where a = :#x and b = # and c in (:#in:ids)",
            '#',
        )
        .unwrap();
        let mut msg = Message::new(Body::Json(serde_json::json!(["pos_val"])));
        msg.set_header("x", serde_json::json!("hello"));
        msg.set_header("ids", serde_json::json!([10, 20]));
        let ex = Exchange::new(msg);

        let prepared = resolve_params(&tpl, &ex, ", ").unwrap();
        assert_eq!(
            prepared.sql,
            "select * from t where a = $1 and b = $2 and c in ($3, $4)"
        );
        assert_eq!(prepared.bindings.len(), 4);
    }

    #[test]
    fn is_select() {
        assert!(is_select_query("SELECT * FROM t"));
        assert!(is_select_query("  select * from t"));
        assert!(is_select_query("WITH cte AS (SELECT 1) SELECT * FROM cte"));
        assert!(is_select_query(
            "with results as (select 1) select * from results"
        ));
        assert!(!is_select_query(
            "WITH cte AS (SELECT id FROM t) INSERT INTO other SELECT * FROM cte"
        ));
        assert!(is_select_query("TABLE users"));
        assert!(is_select_query("SHOW TABLES"));
        assert!(is_select_query("EXPLAIN SELECT * FROM t"));
        assert!(!is_select_query("INSERT INTO t VALUES (1)"));
        assert!(!is_select_query("UPDATE t SET x = 1"));
        assert!(!is_select_query("DELETE FROM t"));
    }

    #[test]
    fn parse_trailing_param() {
        let tpl = parse_query_template("select * from t where id = #", '#').unwrap();
        assert_eq!(tpl.params.len(), 1);
        assert_eq!(tpl.fragments.len(), 2);
        assert_eq!(tpl.fragments[0], "select * from t where id = ");
        assert_eq!(tpl.fragments[1], "");
    }

    #[test]
    fn parse_leading_param() {
        let tpl = parse_query_template("# = id", '#').unwrap();
        assert_eq!(tpl.params.len(), 1);
        assert_eq!(tpl.fragments.len(), 2);
        assert_eq!(tpl.fragments[0], "");
        assert_eq!(tpl.fragments[1], " = id");
    }

    #[test]
    fn parse_consecutive_params() {
        let tpl = parse_query_template("# # #", '#').unwrap();
        assert_eq!(tpl.params.len(), 3);
        assert_eq!(tpl.fragments.len(), 4);
        assert_eq!(tpl.fragments[0], "");
        assert_eq!(tpl.fragments[1], " ");
        assert_eq!(tpl.fragments[2], " ");
        assert_eq!(tpl.fragments[3], "");
    }

    #[test]
    fn resolution_priority_body_over_headers() {
        // Body should take priority over headers
        let tpl = parse_query_template("select * from t where id = :#id", '#').unwrap();
        let mut msg = Message::new(Body::Json(serde_json::json!({"id": 1})));
        msg.set_header("id", serde_json::json!(2)); // This should be ignored
        let ex = Exchange::new(msg);

        let prepared = resolve_params(&tpl, &ex, ", ").unwrap();
        assert_eq!(prepared.bindings[0], serde_json::json!(1)); // From body, not header
    }

    #[test]
    fn resolution_priority_headers_over_properties() {
        // Headers should take priority over properties
        let tpl = parse_query_template("select * from t where id = :#id", '#').unwrap();
        let mut msg = Message::default();
        msg.set_header("id", serde_json::json!(10));
        let mut ex = Exchange::new(msg);
        ex.set_property("id", serde_json::json!(20)); // This should be ignored

        let prepared = resolve_params(&tpl, &ex, ", ").unwrap();
        assert_eq!(prepared.bindings[0], serde_json::json!(10)); // From header, not property
    }

    #[test]
    fn custom_placeholder_char() {
        let tpl = parse_query_template("select * from t where id = :@id", '@').unwrap();
        assert_eq!(tpl.params.len(), 1);
        assert!(matches!(&tpl.params[0], ParamSlot::Named(n) if n == "id"));
    }

    #[test]
    fn parse_expression_param() {
        let tpl = parse_query_template("select * from t where id = :#${body.id}", '#').unwrap();
        assert_eq!(tpl.params.len(), 1);
        assert!(matches!(&tpl.params[0], ParamSlot::Expression(e) if e == "body.id"));
    }

    #[test]
    fn resolve_expression_from_body() {
        let tpl = parse_query_template("select * from t where id = :#${body.id}", '#').unwrap();
        let msg = Message::new(Body::Json(serde_json::json!({"id": 42})));
        let ex = Exchange::new(msg);
        let prepared = resolve_params(&tpl, &ex, ", ").unwrap();
        assert_eq!(prepared.sql, "select * from t where id = $1");
        assert_eq!(prepared.bindings[0], serde_json::json!(42));
    }

    #[test]
    fn resolve_expression_from_header() {
        let tpl =
            parse_query_template("select * from t where name = :#${header.name}", '#').unwrap();
        let mut msg = Message::default();
        msg.set_header("name", serde_json::json!("alice"));
        let ex = Exchange::new(msg);
        let prepared = resolve_params(&tpl, &ex, ", ").unwrap();
        assert_eq!(prepared.bindings[0], serde_json::json!("alice"));
    }

    #[test]
    fn resolve_expression_from_property() {
        let tpl =
            parse_query_template("select * from t where k = :#${property.myKey}", '#').unwrap();
        let mut ex = Exchange::new(Message::default());
        ex.set_property("myKey", serde_json::json!(99));
        let prepared = resolve_params(&tpl, &ex, ", ").unwrap();
        assert_eq!(prepared.bindings[0], serde_json::json!(99));
    }

    #[test]
    fn parse_hash_in_string_literal() {
        // # inside a string literal should NOT be treated as a parameter
        let tpl =
            parse_query_template("select * from t where x = '#literal' and id = #", '#').unwrap();
        assert_eq!(tpl.params.len(), 1);
        assert!(matches!(tpl.params[0], ParamSlot::Positional(0)));
    }

    #[test]
    fn parse_escaped_quote_in_literal() {
        // '' inside a string literal is an escaped quote, not end of literal
        let tpl =
            parse_query_template("select * from t where x = 'it''s' and id = #", '#').unwrap();
        assert_eq!(tpl.params.len(), 1);
        assert!(matches!(tpl.params[0], ParamSlot::Positional(0)));
    }

    #[test]
    fn empty_in_clause_produces_null() {
        let tpl = parse_query_template("select * from t where id in (:#in:ids)", '#').unwrap();
        let mut msg = Message::default();
        msg.set_header("ids", serde_json::json!([]));
        let ex = Exchange::new(msg);
        let prepared = resolve_params(&tpl, &ex, ", ").unwrap();
        assert_eq!(prepared.sql, "select * from t where id in (NULL)");
        assert!(prepared.bindings.is_empty());
    }

    #[test]
    fn in_clause_custom_separator() {
        let tpl = parse_query_template("select * from t where id in (:#in:ids)", '#').unwrap();
        let mut msg = Message::default();
        msg.set_header("ids", serde_json::json!([1, 2, 3]));
        let ex = Exchange::new(msg);

        let prepared = resolve_params(&tpl, &ex, ";").unwrap();
        assert_eq!(prepared.sql, "select * from t where id in ($1;$2;$3)");
    }

    #[test]
    fn parse_kebab_case_named_param() {
        // `:#my-param` should parse the WHOLE "my-param" as the name. The current
        // parser stops at `-` (not alphanumeric), producing `Named("my")` and
        // leaking `-param` into the SQL fragment. rc-o6o Bug A.
        let tpl = parse_query_template("select * from t where id = :#my-param", '#').unwrap();
        assert_eq!(tpl.params.len(), 1);
        assert!(
            matches!(&tpl.params[0], ParamSlot::Named(n) if n == "my-param"),
            "got {:?}",
            tpl.params[0]
        );
        assert_eq!(tpl.fragments[0], "select * from t where id = ");
        assert_eq!(tpl.fragments[1], ""); // no trailing SQL leak
    }

    #[test]
    fn parse_dotted_named_param() {
        // Dots are now allowed inside names so `body.user.name` is one token.
        let tpl = parse_query_template("select * from t where x = :#body.user.name", '#').unwrap();
        assert!(
            matches!(&tpl.params[0], ParamSlot::Named(n) if n == "body.user.name"),
            "got {:?}",
            tpl.params[0]
        );
        assert_eq!(tpl.fragments[1], "");
    }

    #[test]
    fn parse_named_param_terminated_by_paren() {
        // `:#id)` — close paren terminates the name.
        let tpl = parse_query_template("fn(:#id)", '#').unwrap();
        assert!(
            matches!(&tpl.params[0], ParamSlot::Named(n) if n == "id"),
            "got {:?}",
            tpl.params[0]
        );
        assert_eq!(tpl.fragments[1], ")");
    }

    #[test]
    fn parse_named_param_terminated_by_comma() {
        let tpl = parse_query_template("fn(:#a, :#b)", '#').unwrap();
        assert!(matches!(&tpl.params[0], ParamSlot::Named(n) if n == "a"));
        assert!(matches!(&tpl.params[1], ParamSlot::Named(n) if n == "b"));
    }

    #[test]
    fn parse_named_param_terminated_by_equals() {
        let tpl = parse_query_template("select :#x=1", '#').unwrap();
        assert!(matches!(&tpl.params[0], ParamSlot::Named(n) if n == "x"));
        assert_eq!(tpl.fragments[1], "=1");
    }

    #[test]
    fn parse_named_param_terminated_by_semicolon() {
        let tpl = parse_query_template("select :#x;", '#').unwrap();
        assert!(matches!(&tpl.params[0], ParamSlot::Named(n) if n == "x"));
        assert_eq!(tpl.fragments[1], ";");
    }

    #[test]
    fn parse_in_clause_kebab_case_name() {
        // `:#in:my-ids` — IN-clause with hyphenated name.
        let tpl = parse_query_template("select * from t where id in (:#in:my-ids)", '#').unwrap();
        assert!(
            matches!(&tpl.params[0], ParamSlot::InClause(n) if n == "my-ids"),
            "got {:?}",
            tpl.params[0]
        );
    }

    #[test]
    fn resolve_named_dotted_body_path_walks_json() {
        // `:#body.user.name` should walk `body["user"]["name"]`. Today the parser
        // stops at `.`, capturing `body`, and resolution fails with "Named
        // parameter 'body' not found". rc-o6o Bug A (scoped lookup).
        let tpl =
            parse_query_template("select * from users where name = :#body.user.name", '#').unwrap();
        assert!(
            matches!(&tpl.params[0], ParamSlot::Named(n) if n == "body.user.name"),
            "got {:?}",
            tpl.params[0]
        );

        let msg = Message::new(Body::Json(serde_json::json!({
            "user": { "name": "alice" }
        })));
        let ex = Exchange::new(msg);

        let prepared = resolve_params(&tpl, &ex, ", ").unwrap();
        assert_eq!(prepared.sql, "select * from users where name = $1");
        assert_eq!(prepared.bindings[0], serde_json::json!("alice"));
    }

    #[test]
    fn resolve_named_header_scope_flat_dotted_key() {
        // `:#header.correlation.id` — flat key "correlation.id".
        let tpl =
            parse_query_template("select * from t where x = :#header.correlation.id", '#').unwrap();
        let mut msg = Message::default();
        msg.set_header("correlation.id", serde_json::json!("abc-123"));
        let ex = Exchange::new(msg);

        let prepared = resolve_params(&tpl, &ex, ", ").unwrap();
        assert_eq!(prepared.bindings[0], serde_json::json!("abc-123"));
    }

    #[test]
    fn resolve_named_property_scope_alias() {
        // `:#exchangeProperty.tenantId` — property flat key "tenantId".
        let tpl =
            parse_query_template("select * from t where x = :#exchangeProperty.tenantId", '#')
                .unwrap();
        let mut ex = Exchange::new(Message::default());
        ex.set_property("tenantId", serde_json::json!("acme"));

        let prepared = resolve_params(&tpl, &ex, ", ").unwrap();
        assert_eq!(prepared.bindings[0], serde_json::json!("acme"));
    }

    #[test]
    fn resolve_named_unscoped_kebab_case_fallback_to_property() {
        // `:#my-param` — unscoped. Body and header miss; property hits.
        let tpl = parse_query_template("select * from t where x = :#my-param", '#').unwrap();
        let mut ex = Exchange::new(Message::default());
        ex.set_property("my-param", serde_json::json!(7));

        let prepared = resolve_params(&tpl, &ex, ", ").unwrap();
        assert_eq!(prepared.bindings[0], serde_json::json!(7));
    }

    #[test]
    fn resolve_expression_nested_body_walk() {
        // `:#${body.user.name}` now walks nested JSON (used to be flat-only).
        let tpl = parse_query_template("select * from users where name = :#${body.user.name}", '#')
            .unwrap();
        let msg = Message::new(Body::Json(serde_json::json!({
            "user": { "name": "bob" }
        })));
        let ex = Exchange::new(msg);

        let prepared = resolve_params(&tpl, &ex, ", ").unwrap();
        assert_eq!(prepared.bindings[0], serde_json::json!("bob"));
    }

    #[test]
    fn resolve_named_missing_returns_clear_error() {
        let tpl = parse_query_template("select * from t where x = :#nope", '#').unwrap();
        let ex = Exchange::new(Message::default());

        let err = resolve_params(&tpl, &ex, ", ").unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("nope"),
            "error should mention the missing name: {msg}"
        );
    }

    #[test]
    fn resolve_named_bare_body_rejected() {
        // e_gpt oracle blessing condition #2: bare `:#body` is ambiguous for SQL.
        // SQL binds scalars; "whole body" doesn't make sense as a SQL binding.
        // `parse("body")` returns `Body(vec![])` (whole body for Simple `${body}`);
        // SQL caller must explicitly reject this with a clear error.
        let tpl = parse_query_template("select * from t where x = :#body", '#').unwrap();
        let ex = Exchange::new(Message::new(Body::Json(serde_json::json!({"a": 1}))));

        let err = resolve_params(&tpl, &ex, ", ").unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("body") && (msg.contains("path") || msg.contains("requires")),
            "bare ':#body' must be rejected with a clear message mentioning path requirement: {msg}"
        );
    }

    #[test]
    fn resolve_named_bare_header_property_exchange_property_rejected() {
        // Bare reserved scope names (`:#header`, `:#property`, `:#exchangeProperty`)
        // error at ExchangeLookupPath::parse time (EmptyScopedKey). Verify the SQL
        // surface propagates the error clearly.
        for bare in ["header", "property", "exchangeProperty"] {
            let sql = format!("select * from t where x = :#{bare}");
            let tpl = parse_query_template(&sql, '#').unwrap();
            let ex = Exchange::new(Message::default());
            let err = resolve_params(&tpl, &ex, ", ").unwrap_err();
            let msg = format!("{err}");
            assert!(
                msg.contains(bare),
                "bare ':#{bare}' must error with a message mentioning the scope name: {msg}"
            );
        }
    }

    #[test]
    fn resolve_expression_bare_body_rejected() {
        // `:#${body}` is the explicit-expression form; same bare-scope rejection
        // as `:#body` (e_gpt oracle re-bless concern — SQL binds scalars).
        let tpl = parse_query_template("select * from t where x = :#${body}", '#').unwrap();
        let ex = Exchange::new(Message::new(Body::Json(serde_json::json!({"a": 1}))));

        let err = resolve_params(&tpl, &ex, ", ").unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("body") && (msg.contains("path") || msg.contains("requires")),
            "bare ':#${{body}}' must be rejected: {msg}"
        );
    }

    #[test]
    fn parse_postgres_cast_after_placeholder_leaves_double_colon_as_sql() {
        // `:#id::text` — placeholder name is `id`; `::text` is a Postgres cast
        // that MUST remain in the SQL fragment. rc-o6o Phase 2 §3.2 mandatory
        // contract test.
        let tpl = parse_query_template("select :#id::text as id_text from t", '#').unwrap();
        assert!(
            matches!(&tpl.params[0], ParamSlot::Named(n) if n == "id"),
            "got {:?}",
            tpl.params[0]
        );
        assert_eq!(tpl.fragments[0], "select ");
        assert_eq!(tpl.fragments[1], "::text as id_text from t");
    }

    #[test]
    fn resolve_postgres_cast_binds_id_value() {
        let tpl = parse_query_template("select :#id::text as id_text from t", '#').unwrap();
        let mut msg = Message::default();
        msg.set_header("id", serde_json::json!(42));
        let ex = Exchange::new(msg);

        let prepared = resolve_params(&tpl, &ex, ", ").unwrap();
        assert_eq!(prepared.sql, "select $1::text as id_text from t");
        assert_eq!(prepared.bindings[0], serde_json::json!(42));
    }

    #[test]
    fn parse_single_colon_terminates_name() {
        // `:#id:x` — single colon terminates. `:x` remains in SQL.
        // (No real SQL meaning; verifies the single-vs-double-colon rule.)
        let tpl = parse_query_template("select :#id:x from t", '#').unwrap();
        assert!(
            matches!(&tpl.params[0], ParamSlot::Named(n) if n == "id"),
            "got {:?}",
            tpl.params[0]
        );
        assert_eq!(tpl.fragments[1], ":x from t");
    }

    #[test]
    fn resolve_in_clause_kebab_case_name_from_property() {
        // `:#in:my-ids` — IN-clause with hyphenated name. Resolves via Unscoped
        // fallback to a property holding an array.
        let tpl = parse_query_template("select * from t where id in (:#in:my-ids)", '#').unwrap();
        let mut ex = Exchange::new(Message::default());
        ex.set_property("my-ids", serde_json::json!([1, 2, 3]));

        let prepared = resolve_params(&tpl, &ex, ", ").unwrap();
        assert_eq!(prepared.sql, "select * from t where id in ($1, $2, $3)");
        assert_eq!(
            prepared.bindings,
            vec![
                serde_json::json!(1),
                serde_json::json!(2),
                serde_json::json!(3)
            ]
        );
    }
}
