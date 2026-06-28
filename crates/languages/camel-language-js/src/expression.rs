//! Expression, MutatingExpression, and Predicate implementations for JS.

use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use camel_language_api::{Body, Exchange, Value};
use camel_language_api::{Expression, LanguageError, MutatingExpression, Predicate};

use crate::{
    engine::{JsEngine, JsEvalResult, JsExchange},
    error::JsLanguageError,
};

/// Convert a [`JsLanguageError`] into a [`LanguageError`].
fn js_err_to_lang_err(source: &str, e: JsLanguageError) -> LanguageError {
    match e {
        JsLanguageError::Parse { message } => LanguageError::ParseError {
            expr: source.to_string(),
            reason: message,
        },
        other => LanguageError::eval_error(source, other),
    }
}

/// Validate `script` and convert any error to a `LanguageError::ParseError`.
pub(crate) fn validate_to_parse_error(
    engine: &Arc<dyn JsEngine>,
    script: &str,
) -> Result<(), LanguageError> {
    engine
        .validate(script)
        .map_err(|e| LanguageError::ParseError {
            expr: script.to_string(),
            reason: e.to_string(),
        })
}

/// Build a [`JsExchange`] snapshot from an [`Exchange`] reference.
fn snapshot_exchange(exchange: &Exchange) -> Result<JsExchange, LanguageError> {
    let body = match &exchange.input.body {
        Body::Json(v) => v.clone(),
        Body::Text(s) | Body::Xml(s) => Value::String(s.clone()),
        Body::Empty | Body::Bytes(_) => Value::Null,
        Body::Stream(_) => {
            return Err(LanguageError::EvalError(
                "Body::Stream cannot be used in JS — add 'stream_cache' or 'convert_body_to' before this step".to_string(),
            ));
        }
    };

    Ok(JsExchange::from_headers_body_properties(
        exchange.input.headers.clone(),
        body,
        exchange.properties.clone(),
    ))
}

/// Apply [`JsEvalResult`] mutations back onto a mutable [`Exchange`].
fn apply_result_to_exchange(result: &JsEvalResult, exchange: &mut Exchange) {
    exchange.input.headers = result.headers.clone();
    exchange.properties = result.properties.clone();

    let original_was_representable = matches!(
        &exchange.input.body,
        Body::Json(_) | Body::Text(_) | Body::Xml(_)
    );
    if result.body != Value::Null || original_was_representable {
        exchange.input.body = value_to_body(&result.body);
    }
}

/// Convert a `Value` to a `Body`.
fn value_to_body(v: &Value) -> Body {
    match v {
        Value::String(s) => Body::from(s.as_str()),
        other => Body::from(other.clone()),
    }
}

/// A non-mutating JS expression.
///
/// Evaluates the script and returns its result. No mutations are applied to the exchange.
pub struct JsExpression {
    script: String,
    engine: Arc<dyn JsEngine>,
    execution_timeout_ms: u64,
}

impl JsExpression {
    pub fn new(script: String, engine: Arc<dyn JsEngine>, execution_timeout_ms: u64) -> Self {
        Self {
            script,
            engine,
            execution_timeout_ms,
        }
    }
}

#[async_trait]
impl Expression for JsExpression {
    async fn evaluate(&self, exchange: &Exchange) -> Result<Value, LanguageError> {
        let js_exchange = snapshot_exchange(exchange)?;
        let result = eval_async(
            Arc::clone(&self.engine),
            self.script.clone(),
            js_exchange,
            self.execution_timeout_ms,
        )
        .await
        .map_err(|e| js_err_to_lang_err(&self.script, e))?;
        Ok(result.return_value)
    }
}

/// A mutating JS expression.
///
/// Evaluates the script and propagates any mutations to `headers`, `properties`,
/// and `body` back to the exchange. On failure, changes are rolled back atomically.
pub struct JsMutatingExpression {
    script: String,
    engine: Arc<dyn JsEngine>,
    execution_timeout_ms: u64,
}

impl JsMutatingExpression {
    pub fn new(script: String, engine: Arc<dyn JsEngine>, execution_timeout_ms: u64) -> Self {
        Self {
            script,
            engine,
            execution_timeout_ms,
        }
    }
}

#[async_trait]
impl MutatingExpression for JsMutatingExpression {
    async fn evaluate(&self, exchange: &mut Exchange) -> Result<Value, LanguageError> {
        let js_exchange = snapshot_exchange(exchange)?;
        let original_headers = exchange.input.headers.clone();
        let original_properties = exchange.properties.clone();
        let original_body = exchange.input.body.clone();

        match eval_async(
            Arc::clone(&self.engine),
            self.script.clone(),
            js_exchange,
            self.execution_timeout_ms,
        )
        .await
        {
            Ok(result) => {
                apply_result_to_exchange(&result, exchange);
                Ok(result.return_value)
            }
            Err(e) => {
                exchange.input.headers = original_headers;
                exchange.properties = original_properties;
                exchange.input.body = original_body;
                Err(js_err_to_lang_err(&self.script, e))
            }
        }
    }
}

/// A JS predicate.
///
/// Evaluates the script and interprets the return value as a boolean.
pub struct JsPredicate {
    script: String,
    engine: Arc<dyn JsEngine>,
    execution_timeout_ms: u64,
}

impl JsPredicate {
    pub fn new(script: String, engine: Arc<dyn JsEngine>, execution_timeout_ms: u64) -> Self {
        Self {
            script,
            engine,
            execution_timeout_ms,
        }
    }
}

#[async_trait]
impl Predicate for JsPredicate {
    async fn matches(&self, exchange: &Exchange) -> Result<bool, LanguageError> {
        let js_exchange = snapshot_exchange(exchange)?;
        let result = eval_async(
            Arc::clone(&self.engine),
            self.script.clone(),
            js_exchange,
            self.execution_timeout_ms,
        )
        .await
        .map_err(|e| js_err_to_lang_err(&self.script, e))?;
        Ok(is_truthy(&result.return_value))
    }
}

/// Evaluate a JS expression asynchronously with a timeout.
///
/// Uses `tokio::task::spawn_blocking` to run the synchronous JS engine on a
/// blocking thread, and `tokio::time::timeout` to enforce the execution deadline.
async fn eval_async(
    engine: Arc<dyn JsEngine>,
    script: String,
    exchange: JsExchange,
    execution_timeout_ms: u64,
) -> Result<JsEvalResult, JsLanguageError> {
    let timeout = Duration::from_millis(execution_timeout_ms);
    tokio::time::timeout(timeout, async {
        let join = tokio::task::spawn_blocking(move || engine.eval(&script, exchange));
        join.await.map_err(|e| JsLanguageError::Execution {
            message: format!("JS execution task join error: {e}"),
        })?
    })
    .await
    .map_err(|_| JsLanguageError::Execution {
        message: "JS execution timeout".to_string(),
    })?
}

/// Interpret a [`Value`] as a JS-style truthy boolean.
fn is_truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().map(|f| f != 0.0).unwrap_or(false),
        Value::String(s) => !s.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, time::Instant};

    use camel_language_api::Message;
    use serde_json::json;

    use super::*;
    use crate::engines::boa::BoaEngine;
    use crate::error::JsLanguageError;

    fn make_exchange() -> Exchange {
        let mut msg = Message::default();
        msg.headers.insert("foo".to_string(), json!("bar"));
        msg.body = Body::Text("hello".to_string());
        let mut ex = Exchange::new(msg);
        ex.properties.insert("key".to_string(), json!("val"));
        ex
    }

    fn engine() -> Arc<dyn JsEngine> {
        Arc::new(BoaEngine::default())
    }

    #[derive(Debug)]
    struct SlowEngine;

    impl JsEngine for SlowEngine {
        fn eval(
            &self,
            _source: &str,
            exchange: JsExchange,
        ) -> Result<JsEvalResult, JsLanguageError> {
            std::thread::sleep(Duration::from_millis(500));
            Ok(JsEvalResult {
                return_value: json!("done"),
                headers: exchange.headers,
                body: exchange.body,
                properties: exchange.properties,
            })
        }

        fn validate(&self, _source: &str) -> Result<(), JsLanguageError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn test_expression_timeout_returns_error_before_eval_completes() {
        let expr = JsExpression::new("sleep500()".to_string(), Arc::new(SlowEngine), 100);
        let ex = make_exchange();

        let start = Instant::now();
        let result = expr.evaluate(&ex).await;
        let elapsed = start.elapsed();

        assert!(result.is_err(), "expected timeout error");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("JS execution timeout"),
            "unexpected error message: {err}"
        );
        assert!(
            elapsed < Duration::from_millis(500),
            "timeout should happen before 500ms, elapsed: {elapsed:?}"
        );
    }

    #[tokio::test]
    async fn test_expression_arithmetic() {
        let expr = JsExpression::new("1 + 2".to_string(), engine(), 5_000);
        let ex = make_exchange();
        let result = expr.evaluate(&ex).await.unwrap();
        assert_eq!(result.as_i64().unwrap(), 3);
    }

    #[tokio::test]
    async fn test_expression_reads_header() {
        let expr = JsExpression::new("camel.headers.get('foo')".to_string(), engine(), 5_000);
        let ex = make_exchange();
        let result = expr.evaluate(&ex).await.unwrap();
        assert_eq!(result.as_str().unwrap(), "bar");
    }

    #[tokio::test]
    async fn test_expression_reads_body() {
        let expr = JsExpression::new("camel.body".to_string(), engine(), 5_000);
        let ex = make_exchange();
        let result = expr.evaluate(&ex).await.unwrap();
        assert_eq!(result.as_str().unwrap(), "hello");
    }

    #[tokio::test]
    async fn test_expression_does_not_mutate() {
        let expr = JsExpression::new(
            "camel.headers.set('x', 'y'); camel.body = 'changed'".to_string(),
            engine(),
            5_000,
        );
        let ex = make_exchange();
        let _ = expr.evaluate(&ex).await.unwrap();
        assert!(!ex.input.headers.contains_key("x"));
        assert_eq!(ex.input.body.as_text(), Some("hello"));
    }

    #[tokio::test]
    async fn test_mutating_expression_propagates_header() {
        let expr = JsMutatingExpression::new(
            "camel.headers.set('newkey', 'newval'); 'done'".to_string(),
            engine(),
            5_000,
        );
        let mut ex = make_exchange();
        let result = expr.evaluate(&mut ex).await.unwrap();
        assert_eq!(result.as_str().unwrap(), "done");
        assert_eq!(
            ex.input.headers.get("newkey").unwrap().as_str().unwrap(),
            "newval"
        );
    }

    #[tokio::test]
    async fn test_mutating_expression_propagates_property() {
        let expr = JsMutatingExpression::new(
            "camel.set_property('newkey', 'newval'); 'done'".to_string(),
            engine(),
            5_000,
        );
        let mut ex = make_exchange();
        let result = expr.evaluate(&mut ex).await.unwrap();
        assert_eq!(result.as_str().unwrap(), "done");
        assert_eq!(
            ex.properties.get("newkey").unwrap().as_str().unwrap(),
            "newval"
        );
    }

    #[tokio::test]
    async fn test_expression_reads_property_function() {
        let expr = JsExpression::new("camel.property('key')".to_string(), engine(), 5_000);
        let ex = make_exchange();
        let result = expr.evaluate(&ex).await.unwrap();
        assert_eq!(result.as_str().unwrap(), "val");
    }

    #[tokio::test]
    async fn test_mutating_expression_propagates_body() {
        let expr = JsMutatingExpression::new(
            "camel.body = 'modified'; camel.body".to_string(),
            engine(),
            5_000,
        );
        let mut ex = make_exchange();
        let _ = expr.evaluate(&mut ex).await.unwrap();
        assert_eq!(ex.input.body.as_text(), Some("modified"));
    }

    #[tokio::test]
    async fn test_mutating_expression_preserves_bytes_body() {
        let expr = JsMutatingExpression::new(
            "camel.headers.set('x', '1'); 'done'".to_string(),
            engine(),
            5_000,
        );
        let mut ex = Exchange::new(Message::default());
        ex.input.body = Body::from(b"binary data".to_vec());

        let _ = expr.evaluate(&mut ex).await.unwrap();

        assert!(
            matches!(ex.input.body, Body::Bytes(_)),
            "Bytes body should be preserved"
        );
    }

    #[tokio::test]
    async fn test_mutating_expression_preserves_empty_body() {
        let expr = JsMutatingExpression::new(
            "camel.headers.set('x', '1'); 'done'".to_string(),
            engine(),
            5_000,
        );
        let mut ex = Exchange::new(Message::default());
        ex.input.body = Body::Empty;

        let _ = expr.evaluate(&mut ex).await.unwrap();

        assert!(
            matches!(ex.input.body, Body::Empty),
            "Empty body should be preserved when JS does not set camel.body"
        );
    }

    #[tokio::test]
    async fn test_mutating_expression_rejects_stream_body() {
        let expr = JsMutatingExpression::new(
            "camel.headers.set('x', '1'); 'done'".to_string(),
            engine(),
            5_000,
        );
        let mut ex = Exchange::new(Message::default());
        ex.input.body = Body::Stream(camel_api::StreamBody {
            stream: std::sync::Arc::new(tokio::sync::Mutex::new(None)),
            metadata: camel_api::StreamMetadata::default(),
        });

        let result = expr.evaluate(&mut ex).await;
        assert!(result.is_err(), "JS should reject Body::Stream with error");
    }

    #[tokio::test]
    async fn test_mutating_expression_can_override_bytes_body() {
        let expr = JsMutatingExpression::new(
            "camel.body = 'replaced'; 'done'".to_string(),
            engine(),
            5_000,
        );
        let mut ex = Exchange::new(Message::default());
        ex.input.body = Body::from(b"binary".to_vec());

        let _ = expr.evaluate(&mut ex).await.unwrap();

        assert!(
            !matches!(ex.input.body, Body::Bytes(_)),
            "Body should be updated when JS explicitly sets camel.body"
        );
        assert_eq!(ex.input.body.as_text(), Some("replaced"));
    }

    #[tokio::test]
    async fn test_mutating_expression_atomic_rollback_on_error() {
        let expr = JsMutatingExpression::new(
            "camel.headers.set('x', 'y'); throw new Error('fail')".to_string(),
            engine(),
            5_000,
        );
        let mut ex = make_exchange();
        let result = expr.evaluate(&mut ex).await;
        assert!(result.is_err());
        assert!(!ex.input.headers.contains_key("x"));
        assert_eq!(ex.input.body.as_text(), Some("hello"));
    }

    #[tokio::test]
    async fn test_predicate_true() {
        let pred = JsPredicate::new("true".to_string(), engine(), 5_000);
        let ex = make_exchange();
        assert!(pred.matches(&ex).await.unwrap());
    }

    #[tokio::test]
    async fn test_predicate_false() {
        let pred = JsPredicate::new("false".to_string(), engine(), 5_000);
        let ex = make_exchange();
        assert!(!pred.matches(&ex).await.unwrap());
    }

    #[tokio::test]
    async fn test_predicate_header_condition() {
        let pred = JsPredicate::new(
            "camel.headers.get('foo') === 'bar'".to_string(),
            engine(),
            5_000,
        );
        let ex = make_exchange();
        assert!(pred.matches(&ex).await.unwrap());
    }

    #[tokio::test]
    async fn test_predicate_truthy_string() {
        let pred = JsPredicate::new("'non-empty'".to_string(), engine(), 5_000);
        let ex = make_exchange();
        assert!(pred.matches(&ex).await.unwrap());
    }

    #[tokio::test]
    async fn test_predicate_falsy_null() {
        let pred = JsPredicate::new("null".to_string(), engine(), 5_000);
        let ex = make_exchange();
        assert!(!pred.matches(&ex).await.unwrap());
    }

    #[tokio::test]
    async fn test_is_truthy_values() {
        assert!(is_truthy(&json!(true)));
        assert!(is_truthy(&json!(1)));
        assert!(is_truthy(&json!("x")));
        assert!(is_truthy(&json!({})));
        assert!(!is_truthy(&json!(false)));
        assert!(!is_truthy(&json!(0)));
        assert!(!is_truthy(&json!("")));
        assert!(!is_truthy(&Value::Null));
    }

    // --- JS-003: Edge case tests ---

    #[tokio::test]
    async fn test_expression_empty_script_returns_undefined() {
        // An empty script should evaluate to undefined (maps to Null)
        let expr = JsExpression::new("".to_string(), engine(), 5_000);
        let ex = make_exchange();
        let result = expr.evaluate(&ex).await.unwrap();
        assert!(
            result.is_null(),
            "empty script should return null/undefined"
        );
    }

    #[tokio::test]
    async fn test_expression_syntax_error_returns_err() {
        let expr = JsExpression::new("let x = {{{".to_string(), engine(), 5_000);
        let ex = make_exchange();
        let result = expr.evaluate(&ex).await;
        assert!(result.is_err(), "syntax error should return Err");
    }

    #[tokio::test]
    async fn test_expression_non_string_return_value() {
        // A script returning a number (not string) should work correctly
        let expr = JsExpression::new("42".to_string(), engine(), 5_000);
        let ex = make_exchange();
        let result = expr.evaluate(&ex).await.unwrap();
        assert_eq!(result.as_i64().unwrap(), 42);
    }

    #[tokio::test]
    async fn test_expression_object_return_value() {
        // A script returning an object should produce a JSON object
        let expr = JsExpression::new("({a: 1, b: 'two'})".to_string(), engine(), 5_000);
        let ex = make_exchange();
        let result = expr.evaluate(&ex).await.unwrap();
        assert!(result.is_object(), "object result should be a JSON object");
        let obj = result.as_object().unwrap();
        assert_eq!(obj.get("a").unwrap().as_i64().unwrap(), 1);
        assert_eq!(obj.get("b").unwrap().as_str().unwrap(), "two");
    }

    #[tokio::test]
    async fn test_expression_array_return_value() {
        let expr = JsExpression::new("[1, 2, 3]".to_string(), engine(), 5_000);
        let ex = make_exchange();
        let result = expr.evaluate(&ex).await.unwrap();
        assert!(result.is_array());
        let arr = result.as_array().unwrap();
        assert_eq!(arr.len(), 3);
    }

    #[tokio::test]
    async fn test_expression_boolean_return_value() {
        let expr = JsExpression::new("true".to_string(), engine(), 5_000);
        let ex = make_exchange();
        let result = expr.evaluate(&ex).await.unwrap();
        assert!(result.is_boolean());
        assert!(result.as_bool().unwrap());
    }
}
