//! [`JsLanguage`] — the JavaScript [`Language`] implementation for rust-camel.

use std::sync::Arc;

use camel_language_api::{
    Expression, JsLimitsConfig, Language, LanguageError, MutatingExpression, Predicate,
};

use crate::{
    engine::JsEngine,
    engines::boa::BoaEngine,
    expression::{JsExpression, JsMutatingExpression, JsPredicate, validate_to_parse_error},
};

// ── Resource limits ───────────────────────────────────────────────────────────
//
// Coverage (via Boa 0.21 RuntimeLimits):
//   - Loop iteration count
//   - Recursion depth
//   - Stack size
//
// Not covered (Boa 0.21 does not expose):
//   - Heap cap — `deny_unknown_fields` in serde rejects any `max-heap-size`
//     field in Camel.toml if a user tries to set it.
//
// Defaults when not configured (rust-camel runtime defaults, ADR-0011):
//   - execution_timeout_ms: 5_000
//   - max_loop_iterations: 100_000  (Boa upstream is u64::MAX)
//   - max_recursion_depth: 512      (Boa 0.21 upstream default, pinned)
//   - max_stack_size:     10_240     (Boa 0.21 upstream default, pinned)

/// JavaScript language plugin backed by [Boa](https://boajs.dev).
///
/// Implements the [`Language`] trait to produce JS-backed expressions, predicates,
/// and mutating expressions for use in Apache Camel route definitions.
///
/// # Thread Safety
///
/// `JsLanguage` is `Clone + Send + Sync`. Each evaluation creates a fresh Boa
/// `Context` (via `BoaEngine`), so no shared mutable state exists across evaluations.
///
/// # Example
///
/// ```no_run
/// use camel_language_js::JsLanguage;
/// use camel_language_api::Language;
///
/// let lang = JsLanguage::new();
/// let expr = lang.create_expression("camel.headers.get('foo')").unwrap();
/// ```
#[derive(Clone)]
pub struct JsLanguage {
    engine: Arc<dyn JsEngine>,
    limits: JsLimitsConfig,
}

impl JsLanguage {
    /// Create a new `JsLanguage` with the default [`BoaEngine`] and default limits.
    pub fn new() -> Self {
        Self::with_limits(JsLimitsConfig::default())
    }

    /// Create a `JsLanguage` with a custom [`JsLimitsConfig`].
    ///
    /// Uses the default [`BoaEngine`] internally.
    pub fn with_limits(limits: JsLimitsConfig) -> Self {
        Self {
            engine: Arc::new(BoaEngine::new(limits.clone())),
            limits,
        }
    }

    /// Create a `JsLanguage` with a custom [`JsEngine`] implementation.
    ///
    /// Useful for testing or providing an alternative JS runtime.
    /// Uses default limits.
    pub fn with_engine<E: JsEngine>(engine: E) -> Self {
        Self::with_engine_and_limits(engine, JsLimitsConfig::default())
    }

    /// Create a `JsLanguage` with a custom engine and custom limits.
    pub fn with_engine_and_limits<E: JsEngine>(engine: E, limits: JsLimitsConfig) -> Self {
        Self {
            engine: Arc::new(engine),
            limits,
        }
    }

    /// Resolve the execution timeout from limits or the default rust-camel value.
    fn execution_timeout_ms(&self) -> u64 {
        self.limits.execution_timeout_ms.unwrap_or(5_000)
    }
}

impl Default for JsLanguage {
    fn default() -> Self {
        Self::new()
    }
}

impl Language for JsLanguage {
    fn name(&self) -> &'static str {
        "js"
    }

    fn create_expression(&self, script: &str) -> Result<Box<dyn Expression>, LanguageError> {
        validate_to_parse_error(&self.engine, script)?;
        Ok(Box::new(JsExpression::new(
            script.to_string(),
            Arc::clone(&self.engine),
            self.execution_timeout_ms(),
        )))
    }

    fn create_predicate(&self, script: &str) -> Result<Box<dyn Predicate>, LanguageError> {
        validate_to_parse_error(&self.engine, script)?;
        Ok(Box::new(JsPredicate::new(
            script.to_string(),
            Arc::clone(&self.engine),
            self.execution_timeout_ms(),
        )))
    }

    fn create_mutating_expression(
        &self,
        script: &str,
    ) -> Result<Box<dyn MutatingExpression>, LanguageError> {
        validate_to_parse_error(&self.engine, script)?;
        Ok(Box::new(JsMutatingExpression::new(
            script.to_string(),
            Arc::clone(&self.engine),
            self.execution_timeout_ms(),
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_language_api::{Body, Exchange, Message};
    use serde_json::json;

    async fn make_exchange() -> Exchange {
        let mut msg = Message::default();
        msg.headers.insert("env".to_string(), json!("prod"));
        msg.body = Body::Text("payload".to_string());
        let mut ex = Exchange::new(msg);
        ex.properties.insert("trace".to_string(), json!("on"));
        ex
    }

    #[tokio::test]
    async fn test_language_name() {
        assert_eq!(JsLanguage::new().name(), "js");
    }

    #[tokio::test]
    async fn test_create_expression_valid() {
        let lang = JsLanguage::new();
        let result = lang.create_expression("1 + 1");
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_create_expression_invalid_syntax() {
        let lang = JsLanguage::new();
        let result = lang.create_expression("let x = {{{");
        assert!(result.is_err());
        // Should be a ParseError
        assert!(matches!(result, Err(LanguageError::ParseError { .. })));
    }

    #[tokio::test]
    async fn test_create_predicate_valid() {
        let lang = JsLanguage::new();
        assert!(lang.create_predicate("true").is_ok());
    }

    #[tokio::test]
    async fn test_create_predicate_invalid() {
        let lang = JsLanguage::new();
        assert!(matches!(
            lang.create_predicate("let !!!"),
            Err(LanguageError::ParseError { .. })
        ));
    }

    #[tokio::test]
    async fn test_create_mutating_expression_invalid_syntax() {
        let lang = JsLanguage::new();
        let result = lang.create_mutating_expression("let !!!");
        assert!(result.is_err());
        assert!(matches!(result, Err(LanguageError::ParseError { .. })));
    }

    #[tokio::test]
    async fn test_create_mutating_expression_valid() {
        let lang = JsLanguage::new();
        assert!(
            lang.create_mutating_expression("camel.headers.set('k','v')")
                .is_ok()
        );
    }

    #[tokio::test]
    async fn test_expression_evaluate() {
        let lang = JsLanguage::new();
        let expr = lang.create_expression("camel.headers.get('env')").unwrap();
        let ex = make_exchange().await;
        let val = expr.evaluate(&ex).await.unwrap();
        assert_eq!(val.as_str().unwrap(), "prod");
    }

    #[tokio::test]
    async fn test_predicate_matches() {
        let lang = JsLanguage::new();
        let pred = lang
            .create_predicate("camel.headers.get('env') === 'prod'")
            .unwrap();
        let ex = make_exchange().await;
        assert!(pred.matches(&ex).await.unwrap());
    }

    #[tokio::test]
    async fn test_mutating_expression_propagates() {
        let lang = JsLanguage::new();
        let expr = lang
            .create_mutating_expression(
                "camel.headers.set('added', 'yes'); camel.body = 'new'; 'done'",
            )
            .unwrap();
        let mut ex = make_exchange().await;
        let result = expr.evaluate(&mut ex).await.unwrap();
        assert_eq!(result.as_str().unwrap(), "done");
        assert_eq!(
            ex.input.headers.get("added").unwrap().as_str().unwrap(),
            "yes"
        );
        assert_eq!(ex.input.body.as_text().unwrap(), "new");
    }

    #[tokio::test]
    async fn test_default_creates_js_language() {
        let lang = JsLanguage::default();
        assert_eq!(lang.name(), "js");
    }

    #[tokio::test]
    async fn test_clone_works() {
        let lang = JsLanguage::new();
        let lang2 = lang.clone();
        assert_eq!(lang2.name(), "js");
        // Both clones should work independently
        let ex = make_exchange().await;
        let expr = lang2.create_expression("42").unwrap();
        let val = expr.evaluate(&ex).await.unwrap();
        assert_eq!(val.as_i64().unwrap(), 42);
    }
}
