//! Query template parsing and parameter resolution for SQL components.
//!
//! This module provides parsing of Camel-style SQL templates with parameter
//! placeholders and resolution of those parameters from an Exchange.

use camel_api::{Body, CamelError, Exchange};

/// A parsed parameter slot in a query template.
#[derive(Debug, Clone, PartialEq)]
pub enum ParamSlot {
    /// Positional parameter (#) — index is the 0-based position among all positional params
    Positional(usize),
    /// Named parameter (:#name) — resolved from headers/body/properties
    Named(String),
    /// IN clause parameter (:#in:name) — expanded to multiple $N placeholders
    InClause(String),
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

    while i < chars.len() {
        if chars[i] == placeholder {
            // Check if preceded by ':'
            if i > 0 && chars[i - 1] == ':' {
                // Named or InClause parameter
                // Check if followed by 'in:' for InClause
                // Pattern: :#in:name
                //          ^  ^^^^
                //          |  check these chars after #
                let is_in_clause = check_in_prefix(&chars, i + 1);

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

/// Extracts a parameter name starting at the given position.
/// Parameter names consist of alphanumeric characters and underscores.
/// Returns (name, end_position) where end_position is the index after the name.
fn extract_param_name(chars: &[char], start: usize) -> (String, usize) {
    let mut name = String::new();
    let mut i = start;

    while i < chars.len() {
        let c = chars[i];
        if c.is_alphanumeric() || c == '_' {
            name.push(c);
            i += 1;
        } else {
            break;
        }
    }

    (name, i)
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
///
/// # Returns
/// A `PreparedQuery` with the final SQL and bindings.
pub fn resolve_params(
    tpl: &QueryTemplate,
    exchange: &Exchange,
) -> Result<PreparedQuery, CamelError> {
    let mut sql_parts = Vec::new();
    let mut bindings = Vec::new();
    let mut placeholder_num = 1usize;

    // Pre-extract body as JSON if available
    let body_json = match &exchange.input.body {
        Body::Json(val) => Some(val),
        _ => None,
    };

    // Check if body is an array for positional params
    let body_array = body_json.as_ref().and_then(|v| v.as_array());

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
                let value = resolve_named_param(name, body_json, &exchange.input, exchange)?;
                sql_parts.push(format!("${}", placeholder_num));
                placeholder_num += 1;
                bindings.push(value);
            }
            ParamSlot::InClause(name) => {
                let value = resolve_named_param(name, body_json, &exchange.input, exchange)?;

                let arr = value.as_array().ok_or_else(|| {
                    CamelError::ProcessorError(format!(
                        "IN clause parameter '{}' must be an array, got {:?}",
                        name, value
                    ))
                })?;

                if arr.is_empty() {
                    // Empty IN clause - output empty to let the parentheses in the template
                    // create "IN ()" which is syntactically valid but matches nothing
                    // (no additional output needed - template already has parentheses)
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
                    sql_parts.push(placeholders.join(", "));
                    bindings.extend(arr.iter().cloned());
                }
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

/// Resolves a named parameter from body (JSON object), headers, or properties.
fn resolve_named_param(
    name: &str,
    body_json: Option<&serde_json::Value>,
    message: &camel_api::Message,
    exchange: &Exchange,
) -> Result<serde_json::Value, CamelError> {
    // 1. Try body if it's a JSON object
    if let Some(json) = body_json {
        if let Some(obj) = json.as_object() {
            if let Some(value) = obj.get(name) {
                return Ok(value.clone());
            }
        }
    }

    // 2. Try headers
    if let Some(value) = message.header(name) {
        return Ok(value.clone());
    }

    // 3. Try properties
    if let Some(value) = exchange.property(name) {
        return Ok(value.clone());
    }

    Err(CamelError::ProcessorError(format!(
        "Named parameter '{}' not found in body, headers, or properties",
        name
    )))
}

/// Returns true if the SQL starts with "SELECT" (case-insensitive, after trimming whitespace).
pub fn is_select_query(sql: &str) -> bool {
    sql.trim().to_uppercase().starts_with("SELECT")
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_api::{Body, Exchange, Message};

    #[test]
    fn test_parse_no_params() {
        let tpl = parse_query_template("select * from users", '#').unwrap();
        assert_eq!(tpl.fragments.len(), 1);
        assert!(tpl.params.is_empty());
    }

    #[test]
    fn test_parse_positional_params() {
        let tpl = parse_query_template("insert into t values (#, #)", '#').unwrap();
        assert_eq!(tpl.params.len(), 2);
        assert!(matches!(tpl.params[0], ParamSlot::Positional(0)));
        assert!(matches!(tpl.params[1], ParamSlot::Positional(1)));
    }

    #[test]
    fn test_parse_named_params() {
        let tpl =
            parse_query_template("select * from t where id = :#id and name = :#name", '#').unwrap();
        assert_eq!(tpl.params.len(), 2);
        assert!(matches!(&tpl.params[0], ParamSlot::Named(n) if n == "id"));
        assert!(matches!(&tpl.params[1], ParamSlot::Named(n) if n == "name"));
    }

    #[test]
    fn test_parse_mixed_params() {
        let tpl =
            parse_query_template("select * from t where id = :#id and status = #", '#').unwrap();
        assert_eq!(tpl.params.len(), 2);
        assert!(matches!(&tpl.params[0], ParamSlot::Named(n) if n == "id"));
        assert!(matches!(tpl.params[1], ParamSlot::Positional(0)));
    }

    #[test]
    fn test_parse_in_clause() {
        let tpl = parse_query_template("select * from t where id in (:#in:ids)", '#').unwrap();
        assert_eq!(tpl.params.len(), 1);
        assert!(matches!(&tpl.params[0], ParamSlot::InClause(n) if n == "ids"));
    }

    #[test]
    fn test_resolve_named_from_headers() {
        let tpl = parse_query_template("select * from t where id = :#id", '#').unwrap();
        let mut msg = Message::default();
        msg.set_header("id", serde_json::json!(42));
        let ex = Exchange::new(msg);

        let prepared = resolve_params(&tpl, &ex).unwrap();
        assert_eq!(prepared.sql, "select * from t where id = $1");
        assert_eq!(prepared.bindings.len(), 1);
        assert_eq!(prepared.bindings[0], serde_json::json!(42));
    }

    #[test]
    fn test_resolve_named_from_body_map() {
        let tpl = parse_query_template("select * from t where id = :#id", '#').unwrap();
        let msg = Message::new(Body::Json(serde_json::json!({"id": 99})));
        let ex = Exchange::new(msg);

        let prepared = resolve_params(&tpl, &ex).unwrap();
        assert_eq!(prepared.bindings[0], serde_json::json!(99));
    }

    #[test]
    fn test_resolve_positional_from_body_array() {
        let tpl = parse_query_template("insert into t values (#, #)", '#').unwrap();
        let msg = Message::new(Body::Json(serde_json::json!(["foo", 42])));
        let ex = Exchange::new(msg);

        let prepared = resolve_params(&tpl, &ex).unwrap();
        assert_eq!(prepared.sql, "insert into t values ($1, $2)");
        assert_eq!(prepared.bindings[0], serde_json::json!("foo"));
        assert_eq!(prepared.bindings[1], serde_json::json!(42));
    }

    #[test]
    fn test_resolve_named_from_properties() {
        let tpl = parse_query_template("select * from t where id = :#myProp", '#').unwrap();
        let mut ex = Exchange::new(Message::default());
        ex.set_property("myProp", serde_json::json!(7));

        let prepared = resolve_params(&tpl, &ex).unwrap();
        assert_eq!(prepared.bindings[0], serde_json::json!(7));
    }

    #[test]
    fn test_resolve_named_not_found() {
        let tpl = parse_query_template("select * from t where id = :#missing", '#').unwrap();
        let ex = Exchange::new(Message::default());

        let result = resolve_params(&tpl, &ex);
        assert!(result.is_err());
    }

    #[test]
    fn test_resolve_in_clause_expansion() {
        let tpl = parse_query_template("select * from t where id in (:#in:ids)", '#').unwrap();
        let mut msg = Message::default();
        msg.set_header("ids", serde_json::json!([1, 2, 3]));
        let ex = Exchange::new(msg);

        let prepared = resolve_params(&tpl, &ex).unwrap();
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
    fn test_build_sql_correct_placeholders() {
        let tpl = parse_query_template(
            "select * from t where a = :#x and b = # and c in (:#in:ids)",
            '#',
        )
        .unwrap();
        let mut msg = Message::new(Body::Json(serde_json::json!(["pos_val"])));
        msg.set_header("x", serde_json::json!("hello"));
        msg.set_header("ids", serde_json::json!([10, 20]));
        let ex = Exchange::new(msg);

        let prepared = resolve_params(&tpl, &ex).unwrap();
        assert_eq!(
            prepared.sql,
            "select * from t where a = $1 and b = $2 and c in ($3, $4)"
        );
        assert_eq!(prepared.bindings.len(), 4);
    }

    #[test]
    fn test_is_select() {
        assert!(is_select_query("SELECT * FROM t"));
        assert!(is_select_query("  select * from t"));
        assert!(!is_select_query("INSERT INTO t VALUES (1)"));
        assert!(!is_select_query("UPDATE t SET x = 1"));
        assert!(!is_select_query("DELETE FROM t"));
    }

    #[test]
    fn test_parse_trailing_param() {
        let tpl = parse_query_template("select * from t where id = #", '#').unwrap();
        assert_eq!(tpl.params.len(), 1);
        assert_eq!(tpl.fragments.len(), 2);
        assert_eq!(tpl.fragments[0], "select * from t where id = ");
        assert_eq!(tpl.fragments[1], "");
    }

    #[test]
    fn test_parse_leading_param() {
        let tpl = parse_query_template("# = id", '#').unwrap();
        assert_eq!(tpl.params.len(), 1);
        assert_eq!(tpl.fragments.len(), 2);
        assert_eq!(tpl.fragments[0], "");
        assert_eq!(tpl.fragments[1], " = id");
    }

    #[test]
    fn test_parse_consecutive_params() {
        let tpl = parse_query_template("# # #", '#').unwrap();
        assert_eq!(tpl.params.len(), 3);
        assert_eq!(tpl.fragments.len(), 4);
        assert_eq!(tpl.fragments[0], "");
        assert_eq!(tpl.fragments[1], " ");
        assert_eq!(tpl.fragments[2], " ");
        assert_eq!(tpl.fragments[3], "");
    }

    #[test]
    fn test_resolution_priority_body_over_headers() {
        // Body should take priority over headers
        let tpl = parse_query_template("select * from t where id = :#id", '#').unwrap();
        let mut msg = Message::new(Body::Json(serde_json::json!({"id": 1})));
        msg.set_header("id", serde_json::json!(2)); // This should be ignored
        let ex = Exchange::new(msg);

        let prepared = resolve_params(&tpl, &ex).unwrap();
        assert_eq!(prepared.bindings[0], serde_json::json!(1)); // From body, not header
    }

    #[test]
    fn test_resolution_priority_headers_over_properties() {
        // Headers should take priority over properties
        let tpl = parse_query_template("select * from t where id = :#id", '#').unwrap();
        let mut msg = Message::default();
        msg.set_header("id", serde_json::json!(10));
        let mut ex = Exchange::new(msg);
        ex.set_property("id", serde_json::json!(20)); // This should be ignored

        let prepared = resolve_params(&tpl, &ex).unwrap();
        assert_eq!(prepared.bindings[0], serde_json::json!(10)); // From header, not property
    }

    #[test]
    fn test_custom_placeholder_char() {
        let tpl = parse_query_template("select * from t where id = :@id", '@').unwrap();
        assert_eq!(tpl.params.len(), 1);
        assert!(matches!(&tpl.params[0], ParamSlot::Named(n) if n == "id"));
    }

    #[test]
    fn test_empty_in_clause() {
        let tpl = parse_query_template("select * from t where id in (:#in:ids)", '#').unwrap();
        let mut msg = Message::default();
        msg.set_header("ids", serde_json::json!([]));
        let ex = Exchange::new(msg);

        let prepared = resolve_params(&tpl, &ex).unwrap();
        assert_eq!(prepared.sql, "select * from t where id in ()");
        assert!(prepared.bindings.is_empty());
    }
}
