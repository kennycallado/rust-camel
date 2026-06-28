//! [`BoaEngine`] — JS engine backed by [Boa](https://boajs.dev).
//!
//! Each call to [`eval`](BoaEngine::eval) creates a fresh `Context`.
//! If [`JsLimitsConfig`](camel_language_api::JsLimitsConfig) fields are `None`, the rust-camel runtime defaults apply:
//!
//! | Limit | Default |
//! |---|---|
//! | `execution_timeout_ms` | 5,000 ms |
//! | `max_loop_iterations` | 100,000 (Boa upstream is `u64::MAX`) |
//! | `max_recursion_depth` | 512 (Boa 0.21 upstream default, pinned) |
//! | `max_stack_size` | 10,240 (Boa 0.21 upstream default, pinned) |
//!
//! **Heap cap:** not supported by Boa 0.21.

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
#[derive(Debug, Clone)]
pub struct BoaEngine {
    limits: camel_language_api::JsLimitsConfig,
}

impl BoaEngine {
    #[must_use]
    pub fn new(limits: camel_language_api::JsLimitsConfig) -> Self {
        Self { limits }
    }
}

impl Default for BoaEngine {
    fn default() -> Self {
        Self::new(camel_language_api::JsLimitsConfig::default())
    }
}

// ── Private resolver ──────────────────────────────────────────────────────────

/// Resolved (concrete) JS limits after folding `Option` → `T` with rust-camel
/// runtime defaults. Produced by [`resolve_js_limits`].
///
/// **Heap cap gap:** Boa 0.21 does not expose a heap-size limit. The
/// [`JsLimitsConfig`] struct intentionally lacks a `max_heap_size` field;
/// `deny_unknown_fields` in serde rejects it if a user tries to set it.
///
/// Note: `execution_timeout_ms` is NOT in this struct — it is applied at the
/// [`Language`](camel_language_api::Language) level via `eval_async` tokio
/// timeout in `expression.rs`, not through Boa's `RuntimeLimits`.
struct ResolvedJsLimits {
    max_loop_iterations: u64,
    max_recursion_depth: usize,
    max_stack_size: usize,
}

/// Resolve a `JsLimitsConfig` (all-`Option`) into concrete values, applying
/// rust-camel runtime defaults where the user did not specify a value.
fn resolve_js_limits(limits: &camel_language_api::JsLimitsConfig) -> ResolvedJsLimits {
    ResolvedJsLimits {
        // Boa upstream default for loop is u64::MAX — unacceptable for buggy scripts.
        max_loop_iterations: limits.max_loop_iterations.unwrap_or(100_000),
        max_recursion_depth: limits.max_recursion_depth.unwrap_or(512),
        max_stack_size: limits.max_stack_size.unwrap_or(10_240),
    }
}

impl JsEngine for BoaEngine {
    fn eval(&self, source: &str, exchange: JsExchange) -> Result<JsEvalResult, JsLanguageError> {
        let mut ctx = Context::default();

        // Apply resource limits before executing any script
        let resolved = resolve_js_limits(&self.limits);
        {
            let runtime_limits = ctx.runtime_limits_mut();
            runtime_limits.set_loop_iteration_limit(resolved.max_loop_iterations);
            runtime_limits.set_recursion_limit(resolved.max_recursion_depth);
            runtime_limits.set_stack_size_limit(resolved.max_stack_size);
        }

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
        let engine = BoaEngine::default();
        let result = engine.eval("1 + 1", JsExchange::default()).unwrap();
        assert_eq!(result.return_value.as_i64().unwrap(), 2);
    }

    #[test]
    fn test_eval_header_access() {
        let engine = BoaEngine::default();
        let ex = make_exchange();
        let result = engine.eval("camel.headers.get('foo')", ex).unwrap();
        assert_eq!(result.return_value.as_str().unwrap(), "bar");
    }

    #[test]
    fn test_eval_body_getter() {
        let engine = BoaEngine::default();
        let ex = make_exchange();
        let result = engine.eval("camel.body", ex).unwrap();
        assert_eq!(result.return_value.as_str().unwrap(), "hello");
    }

    #[test]
    fn test_mutating_set_header_propagates() {
        let engine = BoaEngine::default();
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
        let engine = BoaEngine::default();
        let ex = make_exchange();
        let result = engine
            .eval("camel.body = 'modified'; camel.body", ex)
            .unwrap();
        assert_eq!(result.body.as_str().unwrap(), "modified");
    }

    #[test]
    fn test_console_log_no_crash() {
        let engine = BoaEngine::default();
        let result = engine
            .eval("console.log('test'); 42", JsExchange::default())
            .unwrap();
        assert_eq!(result.return_value.as_i64().unwrap(), 42);
    }

    #[test]
    fn test_validate_valid() {
        let engine = BoaEngine::default();
        assert!(engine.validate("let x = 1 + 1;").is_ok());
    }

    #[test]
    fn test_validate_invalid() {
        let engine = BoaEngine::default();
        assert!(engine.validate("let x = {{{").is_err());
    }

    #[test]
    fn test_eval_property_access() {
        let engine = BoaEngine::default();
        let ex = make_exchange();
        let result = engine.eval("camel.properties.get('key')", ex).unwrap();
        assert_eq!(result.return_value.as_str().unwrap(), "val");
    }

    #[test]
    fn test_eval_property_function_access() {
        let engine = BoaEngine::default();
        let ex = make_exchange();
        let result = engine.eval("camel.property('key')", ex).unwrap();
        assert_eq!(result.return_value.as_str().unwrap(), "val");
    }

    #[test]
    fn test_eval_runtime_error_returns_err() {
        let engine = BoaEngine::default();
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
        let engine = BoaEngine::default();
        let result = engine.eval("let x = {{{", JsExchange::default());
        assert!(result.is_err());
    }

    #[test]
    fn test_eval_missing_header_returns_undefined() {
        let engine = BoaEngine::default();
        let ex = make_exchange();
        // Getting a key that doesn't exist should return undefined (maps to null in serde_json)
        let result = engine.eval("camel.headers.get('nonexistent')", ex).unwrap();
        assert!(result.return_value.is_null());
    }

    #[test]
    fn test_properties_mutation_propagates() {
        let engine = BoaEngine::default();
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
    fn test_set_property_function_mutation_propagates() {
        let engine = BoaEngine::default();
        let ex = make_exchange();
        let result = engine
            .eval("camel.set_property('newprop', 'newval'); 'done'", ex)
            .unwrap();
        assert_eq!(result.return_value.as_str().unwrap(), "done");
        assert_eq!(
            result.properties.get("newprop").unwrap().as_str().unwrap(),
            "newval"
        );
    }

    #[test]
    fn test_headers_keys() {
        let engine = BoaEngine::default();
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
        let engine = BoaEngine::default();
        let ex = make_exchange();
        let r1 = engine.eval("camel.headers.has('foo')", ex.clone()).unwrap();
        assert!(r1.return_value.as_bool().unwrap());
        let r2 = engine.eval("camel.headers.has('missing')", ex).unwrap();
        assert!(!r2.return_value.as_bool().unwrap());
    }

    #[test]
    fn test_headers_remove() {
        let engine = BoaEngine::default();
        let ex = make_exchange();
        let result = engine
            .eval("camel.headers.remove('foo'); camel.headers.has('foo')", ex)
            .unwrap();
        assert!(!result.return_value.as_bool().unwrap());
        assert!(!result.headers.contains_key("foo"));
    }

    #[test]
    fn test_boa_infinite_loop_trips_loop_iteration_limit() {
        use camel_language_api::JsLimitsConfig;
        let limits = JsLimitsConfig {
            max_loop_iterations: Some(1_000),
            ..Default::default()
        };
        let engine = BoaEngine::new(limits);
        let result = engine.eval("while (true) {}", JsExchange::default());
        assert!(
            result.is_err(),
            "while(true) must trip loop_iteration_limit"
        );
        let msg = format!("{}", result.unwrap_err());
        assert!(
            msg.to_lowercase().contains("loop")
                || msg.to_lowercase().contains("limit")
                || msg.to_lowercase().contains("iteration"),
            "error should reference loop limit: {msg}"
        );
    }

    #[test]
    fn test_boa_deep_recursion_trips_recursion_limit() {
        use camel_language_api::JsLimitsConfig;
        let limits = JsLimitsConfig {
            max_recursion_depth: Some(10),
            ..Default::default()
        };
        let engine = BoaEngine::new(limits);
        // Recursive fn that immediately recurses (no base case).
        let script = "(function f() { return f(); })()";
        let result = engine.eval(script, JsExchange::default());
        assert!(result.is_err(), "deep recursion must trip recursion_limit");
    }
}
