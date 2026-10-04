use crate::error::CamelError;
use crate::exchange::Exchange;
use crate::value::Value;
use std::sync::Arc;

/// Boxed future yielding a dynamic [`Value`] or a [`CamelError`].
///
/// Used by the async arms of [`ValueSource`], [`TargetSource`] and
/// [`RecipientSource`] so language expression engines can be evaluated
/// lazily and fallibly from sync call sites.
pub type BoxValueFuture =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<Value, CamelError>> + Send>>;

/// Boxed future yielding a `bool` predicate result or a [`CamelError`].
///
/// Used by the async arm of [`PredicateSource`].
pub type BoxBoolFuture =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<bool, CamelError>> + Send>>;

/// Predicate that determines whether an exchange passes the filter.
/// Returns `true` to forward the exchange into the filter body; `false` to skip it.
///
/// This is a newtype around `Arc<dyn Fn(&Exchange) -> bool + Send + Sync>` so that
/// it can implement `Debug` (used by `#[derive(Debug)]` on `BuilderStep` and friends).
/// The `Deref` impl keeps the call-site ergonomic: `predicate(&exchange)` still works
/// because `FilterPredicate` derefs to the inner `dyn Fn`.
///
/// Pre-v1.0: this used to be a type alias. Converted to a newtype (H2) so the
/// containing structs (`WhenStep`, etc.) can be `#[derive(Debug)]`.
pub struct FilterPredicate(pub Arc<dyn Fn(&Exchange) -> bool + Send + Sync>);

impl FilterPredicate {
    /// Create a new `FilterPredicate` from a closure or function pointer.
    pub fn new<F>(f: F) -> Self
    where
        F: Fn(&Exchange) -> bool + Send + Sync + 'static,
    {
        FilterPredicate(Arc::new(f))
    }
}

impl Clone for FilterPredicate {
    fn clone(&self) -> Self {
        FilterPredicate(Arc::clone(&self.0))
    }
}

impl std::fmt::Debug for FilterPredicate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("FilterPredicate(..)")
    }
}

impl std::ops::Deref for FilterPredicate {
    type Target = dyn Fn(&Exchange) -> bool + Send + Sync;

    fn deref(&self) -> &Self::Target {
        &*self.0
    }
}

/// Fallible predicate source: either a synchronous [`FilterPredicate`] or an
/// asynchronous, fallible language predicate.
///
/// The sync arm always succeeds (`Ok(bool)`); the async arm may fail with a
/// [`CamelError`] that callers must propagate instead of silently treating the
/// exchange as skipped.
#[derive(Clone)]
#[non_exhaustive]
pub enum PredicateSource {
    /// Programmatic synchronous predicate.
    Sync(FilterPredicate),
    /// Language-backed asynchronous predicate.
    Async(Arc<dyn Fn(&Exchange) -> BoxBoolFuture + Send + Sync>),
}

impl PredicateSource {
    /// Evaluate the predicate against `exchange`, propagating async failures.
    pub async fn matches(&self, exchange: &Exchange) -> Result<bool, CamelError> {
        match self {
            Self::Sync(pred) => Ok(pred(exchange)),
            Self::Async(f) => f(exchange).await,
        }
    }
}

impl From<FilterPredicate> for PredicateSource {
    fn from(pred: FilterPredicate) -> Self {
        Self::Sync(pred)
    }
}

/// Fallible value source for dynamic setters and log messages.
#[derive(Clone)]
#[non_exhaustive]
pub enum ValueSource {
    /// Programmatic synchronous closure producing a [`Value`].
    Sync(Arc<dyn Fn(&Exchange) -> Value + Send + Sync>),
    /// Language-backed asynchronous expression.
    Async(Arc<dyn Fn(&Exchange) -> BoxValueFuture + Send + Sync>),
}

impl ValueSource {
    /// Evaluate the value against `exchange`, propagating async failures.
    pub async fn evaluate(&self, exchange: &Exchange) -> Result<Value, CamelError> {
        match self {
            Self::Sync(f) => Ok(f(exchange)),
            Self::Async(f) => f(exchange).await,
        }
    }
}

/// Error returned when a target/recipient expression yields a non-scalar.
const NON_SCALAR_TARGET: &str =
    "router target expression returned a non-scalar value (array/object); expected a string target";

/// Coerce a dynamic [`Value`] into an optional target string.
///
/// `Null` maps to `None`; strings pass through; other scalars are stringified;
/// arrays and objects are rejected.
fn coerce_target_value(value: Value) -> Result<Option<String>, CamelError> {
    match value {
        Value::Null => Ok(None),
        Value::String(s) => Ok(Some(s)),
        Value::Array(_) | Value::Object(_) => {
            Err(CamelError::ProcessorError(NON_SCALAR_TARGET.to_string()))
        }
        other => Ok(Some(other.to_string())),
    }
}

/// Fallible target source for dynamic routers and routing slips.
// The closure shape is part of the published contract; factoring it into a
// private alias would not change the type, so keep the signature literal.
#[allow(clippy::type_complexity)]
#[derive(Clone)]
#[non_exhaustive]
pub enum TargetSource {
    /// Programmatic synchronous closure producing an optional target.
    Sync(Arc<dyn Fn(&Exchange) -> Option<String> + Send + Sync>),
    /// Language-backed asynchronous expression.
    Async(Arc<dyn Fn(&Exchange) -> BoxValueFuture + Send + Sync>),
}

impl TargetSource {
    /// Resolve the target, propagating async failures and non-scalar values.
    pub async fn resolve(&self, exchange: &Exchange) -> Result<Option<String>, CamelError> {
        match self {
            Self::Sync(f) => Ok(f(exchange)),
            Self::Async(f) => coerce_target_value(f(exchange).await?),
        }
    }
}

/// Fallible recipient source for the recipient list EIP.
#[derive(Clone)]
#[non_exhaustive]
pub enum RecipientSource {
    /// Programmatic synchronous closure producing a recipient.
    Sync(Arc<dyn Fn(&Exchange) -> String + Send + Sync>),
    /// Language-backed asynchronous expression.
    Async(Arc<dyn Fn(&Exchange) -> BoxValueFuture + Send + Sync>),
}

impl RecipientSource {
    /// Resolve the recipient, propagating async failures and non-scalar values.
    ///
    /// `Null` coerces to the empty string; the empty string is allowed.
    pub async fn resolve(&self, exchange: &Exchange) -> Result<String, CamelError> {
        match self {
            Self::Sync(f) => Ok(f(exchange)),
            Self::Async(f) => Ok(coerce_target_value(f(exchange).await?)?.unwrap_or_default()),
        }
    }
}

impl From<Arc<dyn Fn(&Exchange) -> Value + Send + Sync>> for ValueSource {
    fn from(f: Arc<dyn Fn(&Exchange) -> Value + Send + Sync>) -> Self {
        Self::Sync(f)
    }
}

impl From<Arc<dyn Fn(&Exchange) -> BoxValueFuture + Send + Sync>> for ValueSource {
    fn from(f: Arc<dyn Fn(&Exchange) -> BoxValueFuture + Send + Sync>) -> Self {
        Self::Async(f)
    }
}

impl From<Arc<dyn Fn(&Exchange) -> Option<String> + Send + Sync>> for TargetSource {
    fn from(f: Arc<dyn Fn(&Exchange) -> Option<String> + Send + Sync>) -> Self {
        Self::Sync(f)
    }
}

impl From<Arc<dyn Fn(&Exchange) -> BoxValueFuture + Send + Sync>> for TargetSource {
    fn from(f: Arc<dyn Fn(&Exchange) -> BoxValueFuture + Send + Sync>) -> Self {
        Self::Async(f)
    }
}

impl From<Arc<dyn Fn(&Exchange) -> String + Send + Sync>> for RecipientSource {
    fn from(f: Arc<dyn Fn(&Exchange) -> String + Send + Sync>) -> Self {
        Self::Sync(f)
    }
}

impl From<Arc<dyn Fn(&Exchange) -> BoxValueFuture + Send + Sync>> for RecipientSource {
    fn from(f: Arc<dyn Fn(&Exchange) -> BoxValueFuture + Send + Sync>) -> Self {
        Self::Async(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Exchange, ExpressionErrorClass, Message};

    fn expression_failed() -> CamelError {
        CamelError::ExpressionFailed {
            language: "rhai".to_string(),
            route_id: "r1".to_string(),
            step_id: "step#0".to_string(),
            verb: "filter".to_string(),
            class: ExpressionErrorClass::Runtime,
            position: None,
            conversion: None,
            cause: None,
        }
    }

    #[test]
    fn test_filter_predicate_is_callable() {
        let pred = FilterPredicate::new(|ex: &Exchange| ex.input.body.as_text().is_some());
        let ex = Exchange::new(Message::new("hello"));
        assert!(pred(&ex));
    }

    #[test]
    fn test_filter_predicate_debug_is_redacted() {
        let pred = FilterPredicate::new(|_: &Exchange| true);
        assert_eq!(format!("{pred:?}"), "FilterPredicate(..)");
    }

    #[test]
    fn test_filter_predicate_clone_shares_arc() {
        let pred = FilterPredicate::new(|_: &Exchange| true);
        let cloned = pred.clone();
        assert!(matches!(cloned, FilterPredicate(_)));
    }

    #[tokio::test]
    async fn predicate_source_sync_returns_bool() {
        let source = PredicateSource::Sync(FilterPredicate::new(|_: &Exchange| true));
        let ex = Exchange::new(Message::new("hello"));
        assert!(source.matches(&ex).await.unwrap());
    }

    #[tokio::test]
    async fn predicate_source_async_propagates_error() {
        let source = PredicateSource::Async(Arc::new(|_: &Exchange| {
            Box::pin(async { Err(expression_failed()) }) as BoxBoolFuture
        }));
        let ex = Exchange::new(Message::new("hello"));
        let err = source.matches(&ex).await.unwrap_err();
        assert!(matches!(err, CamelError::ExpressionFailed { .. }));
    }

    #[tokio::test]
    async fn value_source_async_propagates_error() {
        let source = ValueSource::Async(Arc::new(|_: &Exchange| {
            Box::pin(async { Err(expression_failed()) }) as BoxValueFuture
        }));
        let ex = Exchange::new(Message::new("hello"));
        let err = source.evaluate(&ex).await.unwrap_err();
        assert!(matches!(err, CamelError::ExpressionFailed { .. }));
    }

    #[tokio::test]
    async fn target_source_null_maps_to_none_and_error_propagates() {
        let ex = Exchange::new(Message::new("hello"));

        let null_source = TargetSource::Async(Arc::new(|_: &Exchange| {
            Box::pin(async { Ok(Value::Null) }) as BoxValueFuture
        }));
        assert_eq!(null_source.resolve(&ex).await.unwrap(), None);

        let err_source = TargetSource::Async(Arc::new(|_: &Exchange| {
            Box::pin(async { Err(expression_failed()) }) as BoxValueFuture
        }));
        let err = err_source.resolve(&ex).await.unwrap_err();
        assert!(matches!(err, CamelError::ExpressionFailed { .. }));
    }

    #[tokio::test]
    async fn target_source_array_is_error() {
        let source = TargetSource::Async(Arc::new(|_: &Exchange| {
            Box::pin(async { Ok(Value::Array(vec![Value::String("a".into())])) }) as BoxValueFuture
        }));
        let ex = Exchange::new(Message::new("hello"));
        match source.resolve(&ex).await.unwrap_err() {
            CamelError::ProcessorError(msg) => {
                assert!(msg.contains("non-scalar"), "missing non-scalar: {msg}");
                assert!(msg.contains("string target"), "missing target: {msg}");
            }
            other => panic!("expected ProcessorError, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn recipient_source_null_maps_to_empty_and_non_scalar_is_error() {
        let ex = Exchange::new(Message::new("hello"));

        let null_source = RecipientSource::Async(Arc::new(|_: &Exchange| {
            Box::pin(async { Ok(Value::Null) }) as BoxValueFuture
        }));
        assert_eq!(null_source.resolve(&ex).await.unwrap(), "");

        let array_source = RecipientSource::Async(Arc::new(|_: &Exchange| {
            Box::pin(async { Ok(Value::Array(vec![])) }) as BoxValueFuture
        }));
        match array_source.resolve(&ex).await.unwrap_err() {
            CamelError::ProcessorError(msg) => {
                assert!(msg.contains("non-scalar"), "missing non-scalar: {msg}");
            }
            other => panic!("expected ProcessorError, got {other:?}"),
        }
    }
}
