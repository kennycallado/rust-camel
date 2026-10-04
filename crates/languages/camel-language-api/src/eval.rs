//! Evaluation carriers: bind language expressions/predicates to trusted
//! route-level metadata and transport evaluation failures as typed
//! [`camel_api::CamelError::ExpressionFailed`] errors.
//!
//! Language crates produce [`Expression`]/[`Predicate`] implementations whose
//! failures are [`LanguageError`]s. Route execution needs the opposite: a
//! fallible closure returning `Result<_, CamelError>` enriched with route
//! metadata (route id, step id, verb) and the trusted compile-time
//! destination. [`to_expression_failed`] performs that mapping; the carrier
//! structs package it behind the closure shapes
//! `camel_api::ValueSource`/`PredicateSource` consume.

use std::sync::Arc;

use camel_api::{
    BoxBoolFuture, BoxValueFuture, CamelError, ConversionDetail, ExpressionErrorClass, Value,
};

use crate::error::LanguageError;
use crate::{Exchange, Expression, Predicate};

/// Generic conversion-target placeholders a language crate may report when it
/// did not know the compile-time destination at conversion time. When
/// [`to_expression_failed`] sees one and [`EvalMeta::target`] is known, the
/// placeholder is rewritten to the trusted destination so route-level
/// diagnostics name the real target.
///
/// Entry-level placeholders (`"header entry"`, `"property entry"`) are
/// deliberately excluded: they name a scope surface the evaluating step does
/// not own, so rewriting them to the step's destination would misattribute an
/// unrelated inbound refusal (for example reading an unrelated property rhai
/// cannot represent) to the wrong slot.
const GENERIC_TARGETS: [&str; 2] = ["value", "body"];

/// Trusted route-level metadata describing WHERE and HOW an expression is
/// evaluated.
///
/// All fields are compile-time configuration (operator-supplied route
/// definitions, ADR-0032): none of them ever carry runtime exchange data.
#[derive(Clone, Debug)]
pub struct EvalMeta {
    /// Language the expression is written in (e.g. `rhai`).
    pub language: String,
    /// Id of the route owning the evaluating step.
    pub route_id: String,
    /// Id of the evaluating step.
    pub step_id: String,
    /// DSL verb that evaluates the expression (e.g. `set_property`).
    pub verb: String,
    /// TRUSTED compile-time destination: a property key, a header key, or
    /// `body`. Nested value keys inside a map are NOT part of the target —
    /// the target names the exchange slot, never data inside it.
    pub target: Option<String>,
}

/// Map a [`LanguageError`] onto [`CamelError::ExpressionFailed`] using the
/// trusted route metadata.
///
/// The class and position come from the error itself (defaulting to class
/// `Runtime`, position `None` when the variant carries none). `ConversionError`
/// populates `conversion`; when its target is a generic placeholder (see
/// [`GENERIC_TARGETS`]) and `meta.target` is known, the placeholder is
/// rewritten to the trusted destination. Runtime-derived strings (script map
/// keys, exchange header keys) are never written into the
/// [`ConversionDetail`].
pub fn to_expression_failed(err: LanguageError, meta: &EvalMeta) -> CamelError {
    let class = err.class().unwrap_or(ExpressionErrorClass::Runtime);
    let position = err.position();
    let conversion = match &err {
        LanguageError::ConversionError {
            source_type,
            target,
        } => {
            let target = match meta.target.as_deref() {
                Some(trusted) if GENERIC_TARGETS.contains(&target.as_str()) => trusted.to_string(),
                _ => target.clone(),
            };
            Some(ConversionDetail {
                source_type: source_type.clone(),
                target,
            })
        }
        _ => None,
    };
    CamelError::ExpressionFailed {
        language: meta.language.clone(),
        route_id: meta.route_id.clone(),
        step_id: meta.step_id.clone(),
        verb: meta.verb.clone(),
        class,
        position,
        conversion,
        cause: None,
    }
}

/// An [`Expression`] bound to trusted route metadata, evaluating to
/// `Result<Value, CamelError>`.
#[derive(Clone)]
pub struct LanguageExpressionEval {
    expr: Arc<dyn Expression>,
    meta: EvalMeta,
}

impl LanguageExpressionEval {
    /// Bind an expression to route metadata.
    pub fn new(expr: Arc<dyn Expression>, meta: EvalMeta) -> Self {
        Self { expr, meta }
    }

    /// The trusted route metadata this carrier was built with.
    pub fn meta(&self) -> &EvalMeta {
        &self.meta
    }

    /// Evaluate the bound expression, mapping failures to
    /// [`CamelError::ExpressionFailed`] via [`to_expression_failed`].
    pub async fn evaluate(&self, exchange: &Exchange) -> Result<Value, CamelError> {
        self.expr
            .evaluate(exchange)
            .await
            .map_err(|err| to_expression_failed(err, &self.meta))
    }

    /// Convert into a clone-based async closure matching the async arm of
    /// `camel_api::ValueSource`. The closure clones the expression handle, the
    /// metadata and the exchange per call (the boxed future is `'static`).
    ///
    /// The `Exchange` clone is the real per-call cost: a deep copy
    /// proportional to payload size for `Json`/`Text`/`Xml` bodies and
    /// header/property maps, while `Bytes` is refcount-cheap and `Stream`
    /// shares its single-consumption handle, so read-only isolation for
    /// streams rests on the stream-read guard, not the clone. If profiling
    /// ever flags this, the upgrade path is a lifetime-carrying
    /// `BoxValueFuture<'a>` or an owned-Exchange API.
    pub fn into_value_fn(self) -> Arc<dyn Fn(&Exchange) -> BoxValueFuture + Send + Sync> {
        let expr = self.expr;
        let meta = self.meta;
        Arc::new(move |exchange: &Exchange| {
            let expr = Arc::clone(&expr);
            let meta = meta.clone();
            let exchange = exchange.clone();
            Box::pin(async move {
                expr.evaluate(&exchange)
                    .await
                    .map_err(|err| to_expression_failed(err, &meta))
            }) as BoxValueFuture
        })
    }
}

/// A [`Predicate`] bound to trusted route metadata, evaluating to
/// `Result<bool, CamelError>`.
#[derive(Clone)]
pub struct LanguagePredicateEval {
    pred: Arc<dyn Predicate>,
    meta: EvalMeta,
}

impl LanguagePredicateEval {
    /// Bind a predicate to route metadata.
    pub fn new(pred: Arc<dyn Predicate>, meta: EvalMeta) -> Self {
        Self { pred, meta }
    }

    /// The trusted route metadata this carrier was built with.
    pub fn meta(&self) -> &EvalMeta {
        &self.meta
    }

    /// Evaluate the bound predicate, mapping failures to
    /// [`CamelError::ExpressionFailed`] via [`to_expression_failed`].
    pub async fn matches(&self, exchange: &Exchange) -> Result<bool, CamelError> {
        self.pred
            .matches(exchange)
            .await
            .map_err(|err| to_expression_failed(err, &self.meta))
    }

    /// Convert into a clone-based async closure matching the async arm of
    /// `camel_api::PredicateSource`. The closure clones the predicate handle,
    /// the metadata and the exchange per call (the boxed future is
    /// `'static`).
    ///
    /// The `Exchange` clone is the real per-call cost: a deep copy
    /// proportional to payload size for `Json`/`Text`/`Xml` bodies and
    /// header/property maps, while `Bytes` is refcount-cheap and `Stream`
    /// shares its single-consumption handle, so read-only isolation for
    /// streams rests on the stream-read guard, not the clone. If profiling
    /// ever flags this, the upgrade path is a lifetime-carrying
    /// `BoxBoolFuture<'a>` or an owned-Exchange API.
    pub fn into_bool_fn(self) -> Arc<dyn Fn(&Exchange) -> BoxBoolFuture + Send + Sync> {
        let pred = self.pred;
        let meta = self.meta;
        Arc::new(move |exchange: &Exchange| {
            let pred = Arc::clone(&pred);
            let meta = meta.clone();
            let exchange = exchange.clone();
            Box::pin(async move {
                pred.matches(&exchange)
                    .await
                    .map_err(|err| to_expression_failed(err, &meta))
            }) as BoxBoolFuture
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Exchange, Message};
    use camel_api::{ErrorPosition, ExpressionErrorClass};

    fn test_meta(target: Option<&str>) -> EvalMeta {
        EvalMeta {
            language: "rhai".into(),
            route_id: "r1".into(),
            step_id: "set_property#0".into(),
            verb: "set_property".into(),
            target: target.map(str::to_string),
        }
    }

    #[test]
    fn to_expression_failed_maps_class_and_position() {
        let err = LanguageError::EvalFailure {
            class: ExpressionErrorClass::Arithmetic,
            position: Some(ErrorPosition { line: 3, column: 8 }),
            detail: None,
        };
        let out = to_expression_failed(err, &test_meta(None));
        assert!(matches!(
            out,
            CamelError::ExpressionFailed {
                class: ExpressionErrorClass::Arithmetic,
                position: Some(p),
                ..
            } if p.line == 3 && p.column == 8
        ));
    }

    #[test]
    fn to_expression_failed_defaults_eval_error_to_runtime() {
        let out = to_expression_failed(LanguageError::EvalError("x".into()), &test_meta(None));
        assert!(matches!(
            out,
            CamelError::ExpressionFailed {
                class: ExpressionErrorClass::Runtime,
                position: None,
                ..
            }
        ));
    }

    #[test]
    fn to_expression_failed_rewrites_generic_conversion_target() {
        // Language crate did not know the destination: it reported the
        // generic placeholder `value`. `meta.target` is the trusted
        // compile-time destination and must win in route diagnostics.
        let err = LanguageError::ConversionError {
            source_type: "f64".into(),
            target: "value".into(),
        };
        let out = to_expression_failed(err, &test_meta(Some("property m")));
        if let CamelError::ExpressionFailed {
            conversion: Some(detail),
            ..
        } = out
        {
            assert_eq!(detail.source_type, "f64");
            assert_eq!(detail.target, "property m");
        } else {
            panic!("expected ExpressionFailed with conversion detail, got {out:?}");
        }
    }

    #[test]
    fn to_expression_failed_keeps_specific_conversion_target() {
        // A specific incoming target is kept as-is even when meta.target
        // is also known: the emitting crate knew the destination.
        let err = LanguageError::ConversionError {
            source_type: "int".into(),
            target: "f64".into(),
        };
        let out = to_expression_failed(err, &test_meta(Some("property m")));
        if let CamelError::ExpressionFailed {
            conversion: Some(detail),
            ..
        } = out
        {
            assert_eq!(detail.target, "f64");
        } else {
            panic!("expected ExpressionFailed with conversion detail, got {out:?}");
        }
    }

    /// Hand-rolled expression that always fails with a structured failure.
    struct FailingExpression;

    #[async_trait::async_trait]
    impl crate::Expression for FailingExpression {
        async fn evaluate(&self, _exchange: &Exchange) -> Result<crate::Value, LanguageError> {
            Err(LanguageError::EvalFailure {
                class: ExpressionErrorClass::Arithmetic,
                position: Some(ErrorPosition { line: 1, column: 1 }),
                detail: None,
            })
        }
    }

    #[tokio::test]
    async fn language_expression_eval_wraps_error() {
        let expr: Arc<dyn crate::Expression> = Arc::new(FailingExpression);
        let eval = LanguageExpressionEval::new(expr, test_meta(None));
        let exchange = Exchange::new(Message::default());
        let err = eval.evaluate(&exchange).await.unwrap_err();
        assert!(matches!(
            err,
            CamelError::ExpressionFailed { ref verb, .. } if verb == "set_property"
        ));
    }

    #[tokio::test]
    async fn value_fn_maps_errors_with_meta() {
        let expr: Arc<dyn crate::Expression> = Arc::new(FailingExpression);
        let value_fn = LanguageExpressionEval::new(expr, test_meta(Some("body"))).into_value_fn();
        let exchange = Exchange::new(Message::default());
        let err = value_fn(&exchange).await.unwrap_err();
        assert!(matches!(
            err,
            CamelError::ExpressionFailed {
                ref language,
                ref route_id,
                ref verb,
                ..
            } if language == "rhai" && route_id == "r1" && verb == "set_property"
        ));
    }

    /// Hand-rolled predicate that always fails with a conversion error whose
    /// target is the `header entry` placeholder.
    struct FailingPredicate;

    #[async_trait::async_trait]
    impl crate::Predicate for FailingPredicate {
        async fn matches(&self, _exchange: &Exchange) -> Result<bool, LanguageError> {
            Err(LanguageError::ConversionError {
                source_type: "null".into(),
                target: "header entry".into(),
            })
        }
    }

    #[tokio::test]
    async fn predicate_matches_and_bool_fn_map_errors() {
        let pred: Arc<dyn crate::Predicate> = Arc::new(FailingPredicate);
        let eval = LanguagePredicateEval::new(pred, test_meta(Some("header x")));
        let exchange = Exchange::new(Message::default());

        let err = eval.matches(&exchange).await.unwrap_err();
        if let CamelError::ExpressionFailed {
            class: ExpressionErrorClass::Conversion,
            conversion: Some(detail),
            ..
        } = err.clone()
        {
            // Entry-level placeholders are NOT rewritten: this failure names
            // the scope surface, not the step's destination.
            assert_eq!(detail.target, "header entry");
        } else {
            panic!("expected ExpressionFailed conversion, got {err:?}");
        }

        let bool_fn = eval.into_bool_fn();
        let err = bool_fn(&exchange).await.unwrap_err();
        assert!(matches!(err, CamelError::ExpressionFailed { .. }));
    }
}
