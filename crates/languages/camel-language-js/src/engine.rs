//! [`JsEngine`] trait — abstraction over the JS runtime.

use std::collections::HashMap;

use serde_json::Value;

use crate::error::JsLanguageError;

/// A snapshot of exchange state passed into and out of JS evaluation.
#[derive(Debug, Clone, Default)]
pub struct JsExchange {
    pub headers: HashMap<String, Value>,
    pub body: Value,
    pub properties: HashMap<String, Value>,
}

impl JsExchange {
    pub fn from_headers_body_properties(
        headers: HashMap<String, Value>,
        body: Value,
        properties: HashMap<String, Value>,
    ) -> Self {
        Self {
            headers,
            body,
            properties,
        }
    }
}

/// The result of evaluating a JS expression.
#[derive(Debug, Clone, PartialEq)]
pub struct JsEvalResult {
    /// The return value of the expression (last evaluated value).
    pub return_value: Value,
    /// Possibly-modified headers after execution.
    pub headers: HashMap<String, Value>,
    /// Possibly-modified body after execution.
    pub body: Value,
    /// Possibly-modified properties after execution.
    pub properties: HashMap<String, Value>,
}

/// Abstraction over a JavaScript engine capable of evaluating expressions
/// against a [`JsExchange`] context.
pub trait JsEngine: Send + Sync + 'static {
    /// Evaluate `source` JavaScript code with the given exchange context.
    ///
    /// Returns the result including the (potentially mutated) exchange state.
    /// This is the writable path used by `MutatingExpression`.
    fn eval(&self, source: &str, exchange: JsExchange) -> Result<JsEvalResult, JsLanguageError>;

    /// Evaluate `source` in read-only mode: the exchange snapshot exposed to
    /// the script refuses mutations at runtime instead of silently discarding
    /// them (change `language-value-boundary`, B4).
    ///
    /// The default is FAIL-CLOSED: it returns
    /// [`JsLanguageError::ReadOnlyUnsupported`] and never calls
    /// [`JsEngine::eval`]. A custom engine that only implements the writable
    /// path therefore cannot silently run a read-only expression or predicate
    /// against a writable snapshot; it must override this method to opt in.
    /// The Boa engine overrides it with the throwing read-only snapshot.
    fn eval_read_only(
        &self,
        _source: &str,
        _exchange: JsExchange,
    ) -> Result<JsEvalResult, JsLanguageError> {
        Err(JsLanguageError::ReadOnlyUnsupported)
    }

    /// Validate that `source` is syntactically valid JavaScript without executing it.
    fn validate(&self, source: &str) -> Result<(), JsLanguageError>;
}
