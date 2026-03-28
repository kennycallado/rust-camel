//! [`JsLanguage`] — the JavaScript [`Language`] implementation for rust-camel.

use std::sync::Arc;

use camel_language_api::{Expression, Language, LanguageError, MutatingExpression, Predicate};

use crate::{
    engine::JsEngine,
    engines::boa::BoaEngine,
    expression::{JsExpression, JsMutatingExpression, JsPredicate, validate_to_parse_error},
};

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
}

impl JsLanguage {
    /// Create a new `JsLanguage` with the default [`BoaEngine`].
    pub fn new() -> Self {
        Self {
            engine: Arc::new(BoaEngine::new()),
        }
    }

    /// Create a `JsLanguage` with a custom [`JsEngine`] implementation.
    ///
    /// Useful for testing or providing an alternative JS runtime.
    pub fn with_engine<E: JsEngine>(engine: E) -> Self {
        Self {
            engine: Arc::new(engine),
        }
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
        )))
    }

    fn create_predicate(&self, script: &str) -> Result<Box<dyn Predicate>, LanguageError> {
        validate_to_parse_error(&self.engine, script)?;
        Ok(Box::new(JsPredicate::new(
            script.to_string(),
            Arc::clone(&self.engine),
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
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_api::{Body, exchange::Exchange, message::Message};
    use serde_json::json;

    fn make_exchange() -> Exchange {
        let mut msg = Message::default();
        msg.headers.insert("env".to_string(), json!("prod"));
        msg.body = Body::Text("payload".to_string());
        let mut ex = Exchange::new(msg);
        ex.properties.insert("trace".to_string(), json!("on"));
        ex
    }

    #[test]
    fn test_language_name() {
        assert_eq!(JsLanguage::new().name(), "js");
    }

    #[test]
    fn test_create_expression_valid() {
        let lang = JsLanguage::new();
        let result = lang.create_expression("1 + 1");
        assert!(result.is_ok());
    }

    #[test]
    fn test_create_expression_invalid_syntax() {
        let lang = JsLanguage::new();
        let result = lang.create_expression("let x = {{{");
        assert!(result.is_err());
        // Should be a ParseError
        assert!(matches!(result, Err(LanguageError::ParseError { .. })));
    }

    #[test]
    fn test_create_predicate_valid() {
        let lang = JsLanguage::new();
        assert!(lang.create_predicate("true").is_ok());
    }

    #[test]
    fn test_create_predicate_invalid() {
        let lang = JsLanguage::new();
        assert!(matches!(
            lang.create_predicate("let !!!"),
            Err(LanguageError::ParseError { .. })
        ));
    }

    #[test]
    fn test_create_mutating_expression_invalid_syntax() {
        let lang = JsLanguage::new();
        let result = lang.create_mutating_expression("let !!!");
        assert!(result.is_err());
        assert!(matches!(result, Err(LanguageError::ParseError { .. })));
    }

    #[test]
    fn test_create_mutating_expression_valid() {
        let lang = JsLanguage::new();
        assert!(
            lang.create_mutating_expression("camel.headers.set('k','v')")
                .is_ok()
        );
    }

    #[test]
    fn test_expression_evaluate() {
        let lang = JsLanguage::new();
        let expr = lang.create_expression("camel.headers.get('env')").unwrap();
        let ex = make_exchange();
        let val = expr.evaluate(&ex).unwrap();
        assert_eq!(val.as_str().unwrap(), "prod");
    }

    #[test]
    fn test_predicate_matches() {
        let lang = JsLanguage::new();
        let pred = lang
            .create_predicate("camel.headers.get('env') === 'prod'")
            .unwrap();
        let ex = make_exchange();
        assert!(pred.matches(&ex).unwrap());
    }

    #[test]
    fn test_mutating_expression_propagates() {
        let lang = JsLanguage::new();
        let expr = lang
            .create_mutating_expression(
                "camel.headers.set('added', 'yes'); camel.body = 'new'; 'done'",
            )
            .unwrap();
        let mut ex = make_exchange();
        let result = expr.evaluate(&mut ex).unwrap();
        assert_eq!(result.as_str().unwrap(), "done");
        assert_eq!(
            ex.input.headers.get("added").unwrap().as_str().unwrap(),
            "yes"
        );
        assert_eq!(ex.input.body.as_text().unwrap(), "new");
    }

    #[test]
    fn test_default_creates_js_language() {
        let lang = JsLanguage::default();
        assert_eq!(lang.name(), "js");
    }

    #[test]
    fn test_clone_works() {
        let lang = JsLanguage::new();
        let lang2 = lang.clone();
        assert_eq!(lang2.name(), "js");
        // Both clones should work independently
        let ex = make_exchange();
        let expr = lang2.create_expression("42").unwrap();
        let val = expr.evaluate(&ex).unwrap();
        assert_eq!(val.as_i64().unwrap(), 42);
    }
}
