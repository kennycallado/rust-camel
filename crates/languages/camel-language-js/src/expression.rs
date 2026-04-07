//! Expression, MutatingExpression, and Predicate implementations for JS.

use std::sync::Arc;

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
        other => LanguageError::EvalError(other.to_string()),
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
fn snapshot_exchange(exchange: &Exchange) -> JsExchange {
    let body = match &exchange.input.body {
        Body::Json(v) => v.clone(),
        Body::Text(s) | Body::Xml(s) => Value::String(s.clone()),
        // Empty, Bytes, and Stream have no meaningful JS representation — map to null.
        Body::Empty | Body::Bytes(_) | Body::Stream(_) => Value::Null,
    };

    JsExchange::from_headers_body_properties(
        exchange.input.headers.clone(),
        body,
        exchange.properties.clone(),
    )
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
}

impl JsExpression {
    pub fn new(script: String, engine: Arc<dyn JsEngine>) -> Self {
        Self { script, engine }
    }
}

impl Expression for JsExpression {
    fn evaluate(&self, exchange: &Exchange) -> Result<Value, LanguageError> {
        let js_exchange = snapshot_exchange(exchange);
        let result = self
            .engine
            .eval(&self.script, js_exchange)
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
}

impl JsMutatingExpression {
    pub fn new(script: String, engine: Arc<dyn JsEngine>) -> Self {
        Self { script, engine }
    }
}

impl MutatingExpression for JsMutatingExpression {
    fn evaluate(&self, exchange: &mut Exchange) -> Result<Value, LanguageError> {
        let original_headers = exchange.input.headers.clone();
        let original_properties = exchange.properties.clone();
        let original_body = exchange.input.body.clone();

        let js_exchange = snapshot_exchange(exchange);

        match self.engine.eval(&self.script, js_exchange) {
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
}

impl JsPredicate {
    pub fn new(script: String, engine: Arc<dyn JsEngine>) -> Self {
        Self { script, engine }
    }
}

impl Predicate for JsPredicate {
    fn matches(&self, exchange: &Exchange) -> Result<bool, LanguageError> {
        let js_exchange = snapshot_exchange(exchange);
        let result = self
            .engine
            .eval(&self.script, js_exchange)
            .map_err(|e| js_err_to_lang_err(&self.script, e))?;
        Ok(is_truthy(&result.return_value))
    }
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
    use std::sync::Arc;

    use camel_language_api::Message;
    use serde_json::json;

    use super::*;
    use crate::engines::boa::BoaEngine;

    fn make_exchange() -> Exchange {
        let mut msg = Message::default();
        msg.headers.insert("foo".to_string(), json!("bar"));
        msg.body = Body::Text("hello".to_string());
        let mut ex = Exchange::new(msg);
        ex.properties.insert("key".to_string(), json!("val"));
        ex
    }

    fn engine() -> Arc<dyn JsEngine> {
        Arc::new(BoaEngine::new())
    }

    #[test]
    fn test_expression_arithmetic() {
        let expr = JsExpression::new("1 + 2".to_string(), engine());
        let ex = make_exchange();
        let result = expr.evaluate(&ex).unwrap();
        assert_eq!(result.as_i64().unwrap(), 3);
    }

    #[test]
    fn test_expression_reads_header() {
        let expr = JsExpression::new("camel.headers.get('foo')".to_string(), engine());
        let ex = make_exchange();
        let result = expr.evaluate(&ex).unwrap();
        assert_eq!(result.as_str().unwrap(), "bar");
    }

    #[test]
    fn test_expression_reads_body() {
        let expr = JsExpression::new("camel.body".to_string(), engine());
        let ex = make_exchange();
        let result = expr.evaluate(&ex).unwrap();
        assert_eq!(result.as_str().unwrap(), "hello");
    }

    #[test]
    fn test_expression_does_not_mutate() {
        let expr = JsExpression::new(
            "camel.headers.set('x', 'y'); camel.body = 'changed'".to_string(),
            engine(),
        );
        let ex = make_exchange();
        let _ = expr.evaluate(&ex).unwrap();
        assert!(!ex.input.headers.contains_key("x"));
        assert_eq!(ex.input.body.as_text(), Some("hello"));
    }

    #[test]
    fn test_mutating_expression_propagates_header() {
        let expr = JsMutatingExpression::new(
            "camel.headers.set('newkey', 'newval'); 'done'".to_string(),
            engine(),
        );
        let mut ex = make_exchange();
        let result = expr.evaluate(&mut ex).unwrap();
        assert_eq!(result.as_str().unwrap(), "done");
        assert_eq!(
            ex.input.headers.get("newkey").unwrap().as_str().unwrap(),
            "newval"
        );
    }

    #[test]
    fn test_mutating_expression_propagates_body() {
        let expr =
            JsMutatingExpression::new("camel.body = 'modified'; camel.body".to_string(), engine());
        let mut ex = make_exchange();
        let _ = expr.evaluate(&mut ex).unwrap();
        assert_eq!(ex.input.body.as_text(), Some("modified"));
    }

    #[test]
    fn test_mutating_expression_preserves_bytes_body() {
        let expr =
            JsMutatingExpression::new("camel.headers.set('x', '1'); 'done'".to_string(), engine());
        let mut ex = Exchange::new(Message::default());
        ex.input.body = Body::from(b"binary data".to_vec());

        let _ = expr.evaluate(&mut ex).unwrap();

        assert!(
            matches!(ex.input.body, Body::Bytes(_)),
            "Bytes body should be preserved"
        );
    }

    #[test]
    fn test_mutating_expression_preserves_empty_body() {
        let expr =
            JsMutatingExpression::new("camel.headers.set('x', '1'); 'done'".to_string(), engine());
        let mut ex = Exchange::new(Message::default());
        ex.input.body = Body::Empty;

        let _ = expr.evaluate(&mut ex).unwrap();

        assert!(
            matches!(ex.input.body, Body::Empty),
            "Empty body should be preserved when JS does not set camel.body"
        );
    }

    #[test]
    fn test_mutating_expression_preserves_stream_body() {
        let expr =
            JsMutatingExpression::new("camel.headers.set('x', '1'); 'done'".to_string(), engine());
        let mut ex = Exchange::new(Message::default());
        ex.input.body = Body::Stream(camel_api::StreamBody {
            stream: std::sync::Arc::new(tokio::sync::Mutex::new(None)),
            metadata: camel_api::StreamMetadata::default(),
        });

        let _ = expr.evaluate(&mut ex).unwrap();

        assert!(
            matches!(ex.input.body, Body::Stream(_)),
            "Stream body should be preserved when JS does not set camel.body"
        );
    }

    #[test]
    fn test_mutating_expression_can_override_bytes_body() {
        let expr =
            JsMutatingExpression::new("camel.body = 'replaced'; 'done'".to_string(), engine());
        let mut ex = Exchange::new(Message::default());
        ex.input.body = Body::from(b"binary".to_vec());

        let _ = expr.evaluate(&mut ex).unwrap();

        assert!(
            !matches!(ex.input.body, Body::Bytes(_)),
            "Body should be updated when JS explicitly sets camel.body"
        );
        assert_eq!(ex.input.body.as_text(), Some("replaced"));
    }

    #[test]
    fn test_mutating_expression_atomic_rollback_on_error() {
        let expr = JsMutatingExpression::new(
            "camel.headers.set('x', 'y'); throw new Error('fail')".to_string(),
            engine(),
        );
        let mut ex = make_exchange();
        let result = expr.evaluate(&mut ex);
        assert!(result.is_err());
        assert!(!ex.input.headers.contains_key("x"));
        assert_eq!(ex.input.body.as_text(), Some("hello"));
    }

    #[test]
    fn test_predicate_true() {
        let pred = JsPredicate::new("true".to_string(), engine());
        let ex = make_exchange();
        assert!(pred.matches(&ex).unwrap());
    }

    #[test]
    fn test_predicate_false() {
        let pred = JsPredicate::new("false".to_string(), engine());
        let ex = make_exchange();
        assert!(!pred.matches(&ex).unwrap());
    }

    #[test]
    fn test_predicate_header_condition() {
        let pred = JsPredicate::new("camel.headers.get('foo') === 'bar'".to_string(), engine());
        let ex = make_exchange();
        assert!(pred.matches(&ex).unwrap());
    }

    #[test]
    fn test_predicate_truthy_string() {
        let pred = JsPredicate::new("'non-empty'".to_string(), engine());
        let ex = make_exchange();
        assert!(pred.matches(&ex).unwrap());
    }

    #[test]
    fn test_predicate_falsy_null() {
        let pred = JsPredicate::new("null".to_string(), engine());
        let ex = make_exchange();
        assert!(!pred.matches(&ex).unwrap());
    }

    #[test]
    fn test_is_truthy_values() {
        assert!(is_truthy(&json!(true)));
        assert!(is_truthy(&json!(1)));
        assert!(is_truthy(&json!("x")));
        assert!(is_truthy(&json!({})));
        assert!(!is_truthy(&json!(false)));
        assert!(!is_truthy(&json!(0)));
        assert!(!is_truthy(&json!("")));
        assert!(!is_truthy(&Value::Null));
    }
}
