//! [`BoaEngine`] — JS engine backed by [Boa](https://boajs.dev).
//!
//! `boa_engine::Context` is `!Send`, so execution happens in a fresh context
//! per `eval` call. This avoids cross-thread sharing issues and prevents state
//! leaks between evaluations.

use boa_engine::{Context, JsValue, Source, js_string};

use crate::{
    bindings,
    engine::{JsEngine, JsEvalResult, JsExchange},
    error::JsLanguageError,
    value::js_to_value,
};

/// A [`JsEngine`] implementation backed by Boa.
///
/// Each call to [`eval`](BoaEngine::eval) creates a fresh `Context`.
/// This is intentional: it prevents state leaks between independent expressions.
#[derive(Debug, Clone, Default)]
pub struct BoaEngine;

impl BoaEngine {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl JsEngine for BoaEngine {
    fn eval(&self, source: &str, exchange: JsExchange) -> Result<JsEvalResult, JsLanguageError> {
        let mut ctx = Context::default();

        // Set up console -> tracing
        bindings::register_console(&mut ctx);

        // Set up `camel` global
        let camel_obj = bindings::build_camel_global(&exchange, &mut ctx).map_err(|e| {
            JsLanguageError::Execution {
                message: e.to_string(),
            }
        })?;

        ctx.global_object()
            .set(
                js_string!("camel"),
                JsValue::from(camel_obj),
                false,
                &mut ctx,
            )
            .map_err(|e| JsLanguageError::Execution {
                message: format!("camel global set: {e}"),
            })?;

        // Execute
        let result = ctx
            .eval(Source::from_bytes(source.as_bytes()))
            .map_err(|e| JsLanguageError::Execution {
                message: e.to_string(),
            })?;

        let return_value = js_to_value(&result, &mut ctx)?;

        // Extract modified exchange state
        let modified = bindings::extract_camel_state(&mut ctx)?;

        Ok(JsEvalResult {
            return_value,
            headers: modified.headers,
            body: modified.body,
            properties: modified.properties,
        })
    }

    fn validate(&self, source: &str) -> Result<(), JsLanguageError> {
        // TODO(perf): Context::default() initializes full JS runtime. For high-throughput
        // validation, consider a cached or pooled context.
        let mut ctx = Context::default();
        let _script =
            boa_engine::Script::parse(Source::from_bytes(source.as_bytes()), None, &mut ctx)
                .map_err(|e| JsLanguageError::Parse {
                    message: e.to_string(),
                })?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_exchange() -> JsExchange {
        JsExchange::from_headers_body_properties(
            [("foo".to_string(), json!("bar"))].into_iter().collect(),
            json!("hello"),
            [("key".to_string(), json!("val"))].into_iter().collect(),
        )
    }

    #[test]
    fn test_eval_return_value() {
        let engine = BoaEngine::new();
        let result = engine.eval("1 + 1", JsExchange::default()).unwrap();
        assert_eq!(result.return_value.as_i64().unwrap(), 2);
    }

    #[test]
    fn test_eval_header_access() {
        let engine = BoaEngine::new();
        let ex = make_exchange();
        let result = engine.eval("camel.headers.get('foo')", ex).unwrap();
        assert_eq!(result.return_value.as_str().unwrap(), "bar");
    }

    #[test]
    fn test_eval_body_getter() {
        let engine = BoaEngine::new();
        let ex = make_exchange();
        let result = engine.eval("camel.body", ex).unwrap();
        assert_eq!(result.return_value.as_str().unwrap(), "hello");
    }

    #[test]
    fn test_mutating_set_header_propagates() {
        let engine = BoaEngine::new();
        let ex = make_exchange();
        let result = engine
            .eval("camel.headers.set('newkey', 'newval'); 'done'", ex)
            .unwrap();
        assert_eq!(result.return_value.as_str().unwrap(), "done");
        assert_eq!(
            result.headers.get("newkey").unwrap().as_str().unwrap(),
            "newval"
        );
    }

    #[test]
    fn test_mutating_body_propagates() {
        let engine = BoaEngine::new();
        let ex = make_exchange();
        let result = engine
            .eval("camel.body = 'modified'; camel.body", ex)
            .unwrap();
        assert_eq!(result.body.as_str().unwrap(), "modified");
    }

    #[test]
    fn test_console_log_no_crash() {
        let engine = BoaEngine::new();
        let result = engine
            .eval("console.log('test'); 42", JsExchange::default())
            .unwrap();
        assert_eq!(result.return_value.as_i64().unwrap(), 42);
    }

    #[test]
    fn test_validate_valid() {
        let engine = BoaEngine::new();
        assert!(engine.validate("let x = 1 + 1;").is_ok());
    }

    #[test]
    fn test_validate_invalid() {
        let engine = BoaEngine::new();
        assert!(engine.validate("let x = {{{").is_err());
    }

    #[test]
    fn test_eval_property_access() {
        let engine = BoaEngine::new();
        let ex = make_exchange();
        let result = engine.eval("camel.properties.get('key')", ex).unwrap();
        assert_eq!(result.return_value.as_str().unwrap(), "val");
    }

    #[test]
    fn test_eval_runtime_error_returns_err() {
        let engine = BoaEngine::new();
        let result = engine.eval("throw new Error('boom')", JsExchange::default());
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("boom")
                || msg.to_lowercase().contains("execution")
                || msg.to_lowercase().contains("error")
        );
    }

    #[test]
    fn test_eval_syntax_error_returns_err() {
        let engine = BoaEngine::new();
        let result = engine.eval("let x = {{{", JsExchange::default());
        assert!(result.is_err());
    }

    #[test]
    fn test_eval_missing_header_returns_undefined() {
        let engine = BoaEngine::new();
        let ex = make_exchange();
        // Getting a key that doesn't exist should return undefined (maps to null in serde_json)
        let result = engine.eval("camel.headers.get('nonexistent')", ex).unwrap();
        assert!(result.return_value.is_null());
    }

    #[test]
    fn test_properties_mutation_propagates() {
        let engine = BoaEngine::new();
        let ex = make_exchange();
        let result = engine
            .eval("camel.properties.set('newprop', 'newval'); 'done'", ex)
            .unwrap();
        assert_eq!(result.return_value.as_str().unwrap(), "done");
        assert_eq!(
            result.properties.get("newprop").unwrap().as_str().unwrap(),
            "newval"
        );
    }

    #[test]
    fn test_headers_keys() {
        let engine = BoaEngine::new();
        let ex = make_exchange();
        let result = engine.eval("camel.headers.keys()", ex).unwrap();
        let keys: Vec<&str> = result
            .return_value
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert!(keys.contains(&"foo"));
    }

    #[test]
    fn test_headers_has() {
        let engine = BoaEngine::new();
        let ex = make_exchange();
        let r1 = engine.eval("camel.headers.has('foo')", ex.clone()).unwrap();
        assert!(r1.return_value.as_bool().unwrap());
        let r2 = engine.eval("camel.headers.has('missing')", ex).unwrap();
        assert!(!r2.return_value.as_bool().unwrap());
    }

    #[test]
    fn test_headers_remove() {
        let engine = BoaEngine::new();
        let ex = make_exchange();
        let result = engine
            .eval("camel.headers.remove('foo'); camel.headers.has('foo')", ex)
            .unwrap();
        assert!(!result.return_value.as_bool().unwrap());
        assert!(!result.headers.contains_key("foo"));
    }
}
