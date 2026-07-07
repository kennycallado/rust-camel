//! JSONPath language for rust-camel — evaluates `jsonpath_rust` queries against exchange bodies.
//!
//! Main types: `JsonPathLanguage`, `JsonPathConfig`, `JsonPathExpression`, `JsonPathPredicate`.
//! Provides expressions and predicates for JSON-based routing and transformation.

use async_trait::async_trait;
use camel_language_api::{Body, Exchange, Value};
use camel_language_api::{Expression, Language, LanguageError, Predicate};
use jsonpath_rust::JsonPath;
use serde_json::Value as JsonValue;

/// Default maximum nesting depth for JSON values.
const DEFAULT_MAX_DEPTH: usize = 64;

/// Default maximum input size in bytes for text-to-JSON conversion (M-L2).
/// Matches the spirit of the XPath language's input cap.
const DEFAULT_MAX_INPUT_BYTES: usize = 16 * 1024 * 1024; // 16 MiB

/// Configuration for [`JsonPathLanguage`] resource limits.
#[derive(Debug, Clone)]
pub struct JsonPathConfig {
    /// Maximum allowed input size in bytes for text-to-JSON conversion.
    /// When `None`, no size limit is enforced.
    pub max_input_bytes: Option<usize>,
    /// Maximum allowed nesting depth for JSON values.
    /// Defaults to [`DEFAULT_MAX_DEPTH`] (64).
    pub max_depth: Option<usize>,
}

impl JsonPathConfig {
    /// Returns the effective max depth, applying the default when not explicitly set.
    fn effective_max_depth(&self) -> usize {
        self.max_depth.unwrap_or(DEFAULT_MAX_DEPTH)
    }
}

impl Default for JsonPathConfig {
    fn default() -> Self {
        Self {
            max_input_bytes: Some(DEFAULT_MAX_INPUT_BYTES),
            max_depth: None,
        }
    }
}

/// JSONPath language implementation for rust-camel.
pub struct JsonPathLanguage {
    config: JsonPathConfig,
}

impl JsonPathLanguage {
    /// Create a new instance with default configuration.
    pub fn new() -> Self {
        Self {
            config: JsonPathConfig::default(),
        }
    }

    /// Create a new instance with the given configuration.
    pub fn with_config(config: JsonPathConfig) -> Self {
        Self { config }
    }
}

impl Default for JsonPathLanguage {
    fn default() -> Self {
        Self::new()
    }
}

struct JsonPathExpression {
    query: String,
    config: JsonPathConfig,
}

struct JsonPathPredicate {
    query: String,
    config: JsonPathConfig,
}

/// Check that a [`JsonValue`] does not exceed the given nesting depth.
fn check_depth(value: &JsonValue, max_depth: usize) -> Result<(), LanguageError> {
    fn recurse(value: &JsonValue, max_depth: usize, current: usize) -> Result<(), LanguageError> {
        if current > max_depth {
            return Err(LanguageError::EvalError(format!(
                "JSON nesting depth {current} exceeds limit of {max_depth}"
            )));
        }
        match value {
            JsonValue::Object(map) => {
                for v in map.values() {
                    recurse(v, max_depth, current + 1)?;
                }
                Ok(())
            }
            JsonValue::Array(arr) => {
                for v in arr {
                    recurse(v, max_depth, current + 1)?;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
    recurse(value, max_depth, 0)
}

/// Extract and validate JSON from an exchange body.
///
/// Applies resource limits from `config`:
/// - `max_input_bytes` is checked against text body length before parsing.
/// - `max_depth` is checked against the resulting JSON value (both pre-parsed and newly-parsed).
fn extract_json(exchange: &Exchange, config: &JsonPathConfig) -> Result<JsonValue, LanguageError> {
    let max_depth = config.effective_max_depth();

    match &exchange.input.body {
        Body::Json(v) => {
            // Already-parsed JSON: check depth only (no input_bytes limit applies).
            check_depth(v, max_depth)?;
            Ok(v.clone())
        }
        Body::Text(s) => {
            // Check input size before parsing.
            if let Some(limit) = config.max_input_bytes
                && s.len() > limit
            {
                return Err(LanguageError::EvalError(format!(
                    "input size {} bytes exceeds limit of {limit} bytes",
                    s.len()
                )));
            }
            let value: JsonValue = serde_json::from_str(s)
                .map_err(|e| LanguageError::EvalError(format!("body is not valid JSON: {e}")))?;
            check_depth(&value, max_depth)?;
            Ok(value)
        }
        other => other
            .clone()
            .try_into_json()
            .map_err(|e| {
                LanguageError::EvalError(format!("body is not JSON and cannot be coerced: {e}"))
            })
            .and_then(|b| match b {
                Body::Json(v) => {
                    check_depth(&v, max_depth)?;
                    Ok(v)
                }
                _ => Err(LanguageError::EvalError(
                    "body coercion did not produce JSON".into(),
                )),
            }),
    }
}

fn run_query(query: &str, json: &JsonValue) -> Result<JsonValue, LanguageError> {
    json.query(query)
        .map_err(|e| LanguageError::EvalError(format!("jsonpath query '{query}' failed: {e}")))
        .map(|results| match results.len() {
            0 => JsonValue::Null,
            1 => results[0].clone(),
            _ => JsonValue::Array(results.into_iter().cloned().collect()),
        })
}

#[async_trait]
impl Expression for JsonPathExpression {
    // TODO(JPT-004): The return type is currently raw serde_json::Value. For better
    // interoperability with Camel routing (e.g. header assignments, simple expressions),
    // the result should be coerced: single scalars unwrapped to their native types
    // (string, number, bool), arrays preserved, and Null mapped to a clear sentinel.
    async fn evaluate(&self, exchange: &Exchange) -> Result<Value, LanguageError> {
        let json = extract_json(exchange, &self.config)?;
        run_query(&self.query, &json)
    }
}

#[async_trait]
impl Predicate for JsonPathPredicate {
    async fn matches(&self, exchange: &Exchange) -> Result<bool, LanguageError> {
        let json = extract_json(exchange, &self.config)?;
        let result = run_query(&self.query, &json)?;
        Ok(is_truthy(&result))
    }
}

fn is_truthy(value: &JsonValue) -> bool {
    match value {
        JsonValue::Null => false,
        JsonValue::Bool(b) => *b,
        JsonValue::Number(n) => {
            if let Some(v) = n.as_i64() {
                return v != 0;
            }
            if let Some(v) = n.as_u64() {
                return v != 0;
            }
            if let Some(v) = n.as_f64() {
                return v != 0.0;
            }
            true
        }
        JsonValue::String(s) => !s.is_empty(),
        JsonValue::Array(arr) => !arr.is_empty(),
        JsonValue::Object(_) => true,
    }
}

impl Language for JsonPathLanguage {
    fn name(&self) -> &'static str {
        "jsonpath"
    }

    fn create_expression(&self, script: &str) -> Result<Box<dyn Expression>, LanguageError> {
        if !script.starts_with('$') {
            return Err(LanguageError::ParseError {
                expr: script.to_string(),
                reason: "JsonPath expression must start with '$'".into(),
            });
        }
        let empty = JsonValue::Object(serde_json::Map::new());
        empty.query(script).map_err(|e| LanguageError::ParseError {
            expr: script.to_string(),
            reason: e.to_string(),
        })?;
        Ok(Box::new(JsonPathExpression {
            query: script.to_string(),
            config: self.config.clone(),
        }))
    }

    fn create_predicate(&self, script: &str) -> Result<Box<dyn Predicate>, LanguageError> {
        if !script.starts_with('$') {
            return Err(LanguageError::ParseError {
                expr: script.to_string(),
                reason: "JsonPath expression must start with '$'".into(),
            });
        }
        let empty = JsonValue::Object(serde_json::Map::new());
        empty.query(script).map_err(|e| LanguageError::ParseError {
            expr: script.to_string(),
            reason: e.to_string(),
        })?;
        Ok(Box::new(JsonPathPredicate {
            query: script.to_string(),
            config: self.config.clone(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_language_api::Message;

    async fn exchange_with_json(json: &str) -> Exchange {
        let value: JsonValue = serde_json::from_str(json).unwrap();
        Exchange::new(Message::new(Body::Json(value)))
    }

    async fn exchange_with_text_body(text: &str) -> Exchange {
        Exchange::new(Message::new(Body::Text(text.to_string())))
    }

    async fn empty_exchange() -> Exchange {
        Exchange::new(Message::default())
    }

    async fn default_lang() -> JsonPathLanguage {
        JsonPathLanguage::new()
    }

    #[tokio::test]
    async fn expression_simple_path() {
        let lang = default_lang().await;
        let expr = lang.create_expression("$.store.name").unwrap();
        let ex = exchange_with_json(r#"{"store":{"name":"books"}}"#).await;
        let result = expr.evaluate(&ex).await.unwrap();
        assert_eq!(result, JsonValue::String("books".to_string()));
    }

    #[tokio::test]
    async fn expression_nested_path() {
        let lang = default_lang().await;
        let expr = lang.create_expression("$.a.b.c").unwrap();
        let ex = exchange_with_json(r#"{"a":{"b":{"c":42}}}"#).await;
        let result = expr.evaluate(&ex).await.unwrap();
        assert_eq!(result, JsonValue::Number(42.into()));
    }

    #[tokio::test]
    async fn expression_array_index() {
        let lang = default_lang().await;
        let expr = lang.create_expression("$.items[0]").unwrap();
        let ex = exchange_with_json(r#"{"items":["a","b","c"]}"#).await;
        let result = expr.evaluate(&ex).await.unwrap();
        assert_eq!(result, JsonValue::String("a".to_string()));
    }

    #[tokio::test]
    async fn expression_wildcard() {
        let lang = default_lang().await;
        let expr = lang.create_expression("$.items[*].name").unwrap();
        let ex = exchange_with_json(r#"{"items":[{"name":"a"},{"name":"b"}]}"#).await;
        let result = expr.evaluate(&ex).await.unwrap();
        assert_eq!(
            result,
            JsonValue::Array(vec![
                JsonValue::String("a".to_string()),
                JsonValue::String("b".to_string())
            ])
        );
    }

    #[tokio::test]
    async fn expression_root_path() {
        let lang = default_lang().await;
        let expr = lang.create_expression("$").unwrap();
        let ex = exchange_with_json(r#"{"x":1}"#).await;
        let result = expr.evaluate(&ex).await.unwrap();
        assert_eq!(result["x"], JsonValue::Number(1.into()));
    }

    #[tokio::test]
    async fn expression_text_body_with_valid_json() {
        let lang = default_lang().await;
        let expr = lang.create_expression("$.name").unwrap();
        let ex = exchange_with_text_body(r#"{"name":"test"}"#).await;
        let result = expr.evaluate(&ex).await.unwrap();
        assert_eq!(result, JsonValue::String("test".to_string()));
    }

    #[tokio::test]
    async fn expression_empty_body_is_error() {
        let lang = default_lang().await;
        let expr = lang.create_expression("$.x").unwrap();
        let ex = empty_exchange().await;
        let result = expr.evaluate(&ex).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn expression_invalid_jsonpath_syntax() {
        let lang = default_lang().await;
        let result = lang.create_expression("$[invalid");
        let err = match result {
            Err(e) => e,
            Ok(_) => panic!("expected ParseError"),
        };
        match err {
            LanguageError::ParseError { expr, reason } => {
                assert!(!expr.is_empty());
                assert!(!reason.is_empty());
            }
            other => panic!("expected ParseError, got {other:?}"),
        }
    }

    // --- JPT-005: $ prefix validation ---

    #[tokio::test]
    async fn expression_without_dollar_prefix_is_rejected() {
        let lang = default_lang().await;
        let result = lang.create_expression("store.name");
        assert!(result.is_err(), "expected error for missing $ prefix");
        let err = match result {
            Err(e) => e,
            Ok(_) => panic!("expected ParseError"),
        };
        match err {
            LanguageError::ParseError { expr, reason } => {
                assert_eq!(expr, "store.name");
                assert!(
                    reason.contains("'$'"),
                    "reason should mention '$', got: {reason}"
                );
            }
            other => panic!("expected ParseError, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn predicate_without_dollar_prefix_is_rejected() {
        let lang = default_lang().await;
        let result = lang.create_predicate("store.name");
        assert!(result.is_err(), "expected error for missing $ prefix");
        let err = match result {
            Err(e) => e,
            Ok(_) => panic!("expected ParseError"),
        };
        match err {
            LanguageError::ParseError { reason, .. } => {
                assert!(
                    reason.contains("'$'"),
                    "reason should mention '$', got: {reason}"
                );
            }
            other => panic!("expected ParseError, got {other:?}"),
        }
    }

    // --- JPT-006: Nested path and array index tests ---

    #[tokio::test]
    async fn expression_deeply_nested_path() {
        let lang = default_lang().await;
        let expr = lang.create_expression("$.a.b.c.d").unwrap();
        let ex = exchange_with_json(r#"{"a":{"b":{"c":{"d":"deep"}}}}"#).await;
        let result = expr.evaluate(&ex).await.unwrap();
        assert_eq!(result, JsonValue::String("deep".to_string()));
    }

    #[tokio::test]
    async fn expression_array_index_nested() {
        let lang = default_lang().await;
        let expr = lang.create_expression("$.data.items[1].name").unwrap();
        let ex = exchange_with_json(
            r#"{"data":{"items":[{"name":"first"},{"name":"second"},{"name":"third"}]}}"#,
        )
        .await;
        let result = expr.evaluate(&ex).await.unwrap();
        assert_eq!(result, JsonValue::String("second".to_string()));
    }

    #[tokio::test]
    async fn predicate_non_empty_array_is_true() {
        let lang = default_lang().await;
        let pred = lang.create_predicate("$.items[*]").unwrap();
        let ex = exchange_with_json(r#"{"items":[1,2,3]}"#).await;
        assert!(pred.matches(&ex).await.unwrap());
    }

    #[tokio::test]
    async fn predicate_empty_result_is_false() {
        let lang = default_lang().await;
        let pred = lang.create_predicate("$.missing").unwrap();
        let ex = exchange_with_json(r#"{"other":1}"#).await;
        assert!(!pred.matches(&ex).await.unwrap());
    }

    #[tokio::test]
    async fn predicate_boolean_true() {
        let lang = default_lang().await;
        let pred = lang.create_predicate("$.active").unwrap();
        let ex = exchange_with_json(r#"{"active":true}"#).await;
        assert!(pred.matches(&ex).await.unwrap());
    }

    #[tokio::test]
    async fn predicate_boolean_false() {
        let lang = default_lang().await;
        let pred = lang.create_predicate("$.active").unwrap();
        let ex = exchange_with_json(r#"{"active":false}"#).await;
        assert!(!pred.matches(&ex).await.unwrap());
    }

    #[tokio::test]
    async fn predicate_found_value_is_true() {
        let lang = default_lang().await;
        let pred = lang.create_predicate("$.name").unwrap();
        let ex = exchange_with_json(r#"{"name":"test"}"#).await;
        assert!(pred.matches(&ex).await.unwrap());
    }

    #[tokio::test]
    async fn predicate_zero_is_false() {
        let lang = default_lang().await;
        let pred = lang.create_predicate("$.val").unwrap();
        let ex = exchange_with_json(r#"{"val":0}"#).await;
        assert!(!pred.matches(&ex).await.unwrap());
    }

    #[tokio::test]
    async fn predicate_non_zero_is_true() {
        let lang = default_lang().await;
        let pred = lang.create_predicate("$.val").unwrap();
        let ex = exchange_with_json(r#"{"val":1}"#).await;
        assert!(pred.matches(&ex).await.unwrap());
    }

    #[tokio::test]
    async fn predicate_empty_string_is_false() {
        let lang = default_lang().await;
        let pred = lang.create_predicate("$.val").unwrap();
        let ex = exchange_with_json(r#"{"val":""}"#).await;
        assert!(!pred.matches(&ex).await.unwrap());
    }

    #[tokio::test]
    async fn predicate_non_empty_string_is_true() {
        let lang = default_lang().await;
        let pred = lang.create_predicate("$.val").unwrap();
        let ex = exchange_with_json(r#"{"val":"x"}"#).await;
        assert!(pred.matches(&ex).await.unwrap());
    }

    // --- Resource limit tests (A-26) ---

    #[tokio::test]
    async fn oversized_input_is_rejected() {
        let config = JsonPathConfig {
            max_input_bytes: Some(100),
            ..Default::default()
        };
        let lang = JsonPathLanguage::with_config(config);
        let expr = lang.create_expression("$.key").unwrap();
        // Build a valid JSON string that exceeds 100 bytes
        let big_value = "x".repeat(200);
        let big_json = format!(r#"{{"key":"{}"}}"#, big_value);
        assert!(big_json.len() > 100);
        let ex = exchange_with_text_body(&big_json).await;
        let result = expr.evaluate(&ex).await;
        assert!(
            result.is_err(),
            "expected error for oversized input, got {result:?}"
        );
    }

    #[tokio::test]
    async fn input_under_limit_is_accepted() {
        let config = JsonPathConfig {
            max_input_bytes: Some(1024),
            ..Default::default()
        };
        let lang = JsonPathLanguage::with_config(config);
        let expr = lang.create_expression("$.key").unwrap();
        let ex = exchange_with_text_body(r#"{"key":"value"}"#).await;
        let result = expr.evaluate(&ex).await;
        assert!(
            result.is_ok(),
            "expected success for input under limit, got {result:?}"
        );
    }

    #[tokio::test]
    async fn deeply_nested_input_is_rejected() {
        let config = JsonPathConfig {
            max_depth: Some(5),
            ..Default::default()
        };
        let lang = JsonPathLanguage::with_config(config);
        let expr = lang.create_expression("$.a").unwrap();
        // Build nesting of depth 10: {"a":{"a":{"a":...}}}
        let mut json = "1".to_string();
        for _ in 0..10 {
            json = format!(r#"{{"a":{json}}}"#);
        }
        let ex = exchange_with_text_body(&json).await;
        let result = expr.evaluate(&ex).await;
        assert!(
            result.is_err(),
            "expected error for deeply nested input, got {result:?}"
        );
    }

    #[tokio::test]
    async fn nesting_within_depth_limit_is_accepted() {
        let config = JsonPathConfig {
            max_depth: Some(10),
            ..Default::default()
        };
        let lang = JsonPathLanguage::with_config(config);
        let expr = lang.create_expression("$.a").unwrap();
        // Build nesting of depth 5
        let mut json = "1".to_string();
        for _ in 0..5 {
            json = format!(r#"{{"a":{json}}}"#);
        }
        let ex = exchange_with_text_body(&json).await;
        let result = expr.evaluate(&ex).await;
        assert!(
            result.is_ok(),
            "expected success for nesting within limit, got {result:?}"
        );
    }

    #[tokio::test]
    async fn default_config_has_safe_defaults() {
        // M-L2: max_input_bytes defaults to 16 MiB (was None).
        let config = JsonPathConfig::default();
        assert_eq!(config.max_input_bytes, Some(16 * 1024 * 1024));
        // max_depth still defaults to DEFAULT_MAX_DEPTH via effective_max_depth().
        assert_eq!(config.max_depth, None);
    }

    #[tokio::test]
    async fn default_config_rejects_oversized_text_body() {
        // Default config must reject a >16 MiB text body without an explicit cap.
        let lang = JsonPathLanguage::new(); // default config
        let expr = lang.create_expression("$.key").unwrap();
        let big_value = "x".repeat(16 * 1024 * 1024 + 10);
        let big_json = format!(r#"{{"key":"{}"}}"#, big_value);
        let ex = exchange_with_text_body(&big_json).await;
        let result = expr.evaluate(&ex).await;
        assert!(
            result.is_err(),
            "default config must reject oversized input, got {result:?}"
        );
    }

    #[tokio::test]
    async fn oversized_input_also_rejected_for_predicate() {
        let config = JsonPathConfig {
            max_input_bytes: Some(100),
            ..Default::default()
        };
        let lang = JsonPathLanguage::with_config(config);
        let pred = lang.create_predicate("$.key").unwrap();
        let big_value = "x".repeat(200);
        let big_json = format!(r#"{{"key":"{}"}}"#, big_value);
        let ex = exchange_with_text_body(&big_json).await;
        let result = pred.matches(&ex).await;
        assert!(
            result.is_err(),
            "expected error for oversized input in predicate, got {result:?}"
        );
    }

    #[tokio::test]
    async fn deeply_nested_input_rejected_for_predicate() {
        let config = JsonPathConfig {
            max_depth: Some(3),
            ..Default::default()
        };
        let lang = JsonPathLanguage::with_config(config);
        let pred = lang.create_predicate("$.a").unwrap();
        let mut json = "1".to_string();
        for _ in 0..5 {
            json = format!(r#"{{"a":{json}}}"#);
        }
        let ex = exchange_with_text_body(&json).await;
        let result = pred.matches(&ex).await;
        assert!(
            result.is_err(),
            "expected error for deeply nested input in predicate, got {result:?}"
        );
    }

    #[tokio::test]
    async fn body_json_no_input_size_check_but_depth_checked() {
        // When body is already Body::Json (already parsed), skip input_bytes check
        // but still enforce depth limit
        let config = JsonPathConfig {
            max_input_bytes: Some(10), // very small — but body is already JSON
            max_depth: Some(3),
        };
        let lang = JsonPathLanguage::with_config(config);
        let expr = lang.create_expression("$.a").unwrap();
        // Build a JSON value with nesting depth 5
        let mut json_str = "1".to_string();
        for _ in 0..5 {
            json_str = format!(r#"{{"a":{json_str}}}"#);
        }
        let ex = exchange_with_json(&json_str).await;
        let result = expr.evaluate(&ex).await;
        assert!(
            result.is_err(),
            "expected depth error for pre-parsed JSON, got {result:?}"
        );
    }
}
