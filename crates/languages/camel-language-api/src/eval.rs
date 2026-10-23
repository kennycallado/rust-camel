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
/// `GENERIC_TARGETS`) and `meta.target` is known, the placeholder is
/// rewritten to the trusted destination. Runtime-derived strings (script map
/// keys, exchange header keys) are never written into the
/// [`ConversionDetail`].
///
/// The `cause` carries a fixed redacted detail (via
/// [`LanguageError::redacted_detail`]) when the error variant has a payload
/// that is contractually free of exchange/provider data; free-form payloads
/// keep `cause: None`.
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
        cause: err.redacted_detail().map(|d| {
            Box::new(CamelError::ProcessorErrorWithSource(
                d,
                Arc::new(ExpressionDetailCause),
            ))
        }),
    }
}

/// Crate-private provenance marker for the cause that [`to_expression_failed`]
/// attaches when it carries a contractually redacted detail.
///
/// `std::error::Error` requires `Debug`; the marker never carries payload.
#[derive(Debug)]
struct ExpressionDetailCause;

impl std::fmt::Display for ExpressionDetailCause {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("expression evaluation diagnostic")
    }
}

impl std::error::Error for ExpressionDetailCause {}

/// Whether `err` is an [`CamelError::ExpressionFailed`] produced by
/// [`to_expression_failed`] with a contractually redacted detail attached as
/// its cause.
///
/// Detection is exact mapper provenance, never shape matching: the chained
/// cause must be a [`CamelError::ProcessorErrorWithSource`] whose `source`
/// carries the crate-private `ExpressionDetailCause` marker. A hand-built
/// `ProcessorErrorWithSource` cannot carry that private type and is therefore
/// rejected.
pub fn is_expression_detail_cause(err: &CamelError) -> bool {
    let CamelError::ExpressionFailed {
        cause: Some(cause), ..
    } = err
    else {
        return false;
    };
    match &**cause {
        CamelError::ProcessorErrorWithSource(_, src) => src
            .as_ref()
            .downcast_ref::<ExpressionDetailCause>()
            .is_some(),
        _ => false,
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
                cause: None,
                ..
            }
        ));
    }

    #[test]
    fn to_expression_failed_carries_redacted_detail_as_cause() {
        use std::error::Error;

        let err = LanguageError::EvalFailure {
            class: ExpressionErrorClass::Arithmetic,
            position: None,
            detail: Some("overflow".into()),
        };
        let out = to_expression_failed(err, &test_meta(None));
        let CamelError::ExpressionFailed { cause, .. } = out else {
            panic!("expected ExpressionFailed");
        };
        let cause = cause.expect("redacted detail must be attached as cause");
        assert_eq!(cause.to_string(), "Processor error: overflow");
        assert!(
            cause.as_ref().source().is_some(),
            "cause must expose the marker as its source"
        );
    }

    #[test]
    fn to_expression_failed_keeps_cause_none_for_free_form() {
        let eval = to_expression_failed(LanguageError::EvalError("x".into()), &test_meta(None));
        assert!(matches!(
            eval,
            CamelError::ExpressionFailed { cause: None, .. }
        ));

        let parse = to_expression_failed(
            LanguageError::ParseError {
                expr: "e".into(),
                reason: "r".into(),
            },
            &test_meta(None),
        );
        assert!(matches!(
            parse,
            CamelError::ExpressionFailed { cause: None, .. }
        ));
    }

    #[test]
    fn redacted_detail_covers_typed_variants() {
        let mismatch = LanguageError::TypeMismatch {
            expected: "bool".into(),
            actual: "string".into(),
            position: None,
        };
        assert_eq!(
            mismatch.redacted_detail(),
            Some("expected bool, actual string".to_string())
        );

        let conversion = LanguageError::ConversionError {
            source_type: "null".into(),
            target: "f64".into(),
        };
        assert_eq!(
            conversion.redacted_detail(),
            Some("cannot convert null to f64".to_string())
        );

        assert_eq!(LanguageError::EvalError("x".into()).redacted_detail(), None);
        assert_eq!(
            LanguageError::ParseError {
                expr: "e".into(),
                reason: "r".into(),
            }
            .redacted_detail(),
            None
        );
    }

    #[test]
    fn is_expression_detail_cause_is_exact() {
        let mapped = to_expression_failed(
            LanguageError::EvalFailure {
                class: ExpressionErrorClass::Arithmetic,
                position: None,
                detail: Some("overflow".into()),
            },
            &test_meta(None),
        );
        assert!(is_expression_detail_cause(&mapped));

        let hand_built = CamelError::ExpressionFailed {
            language: "rhai".into(),
            route_id: "r1".into(),
            step_id: "set_property#0".into(),
            verb: "set_property".into(),
            class: ExpressionErrorClass::Runtime,
            position: None,
            conversion: None,
            cause: Some(Box::new(CamelError::ProcessorError(
                "caller diagnostic".into(),
            ))),
        };
        assert!(!is_expression_detail_cause(&hand_built));

        let no_cause = CamelError::ExpressionFailed {
            language: "rhai".into(),
            route_id: "r1".into(),
            step_id: "set_property#0".into(),
            verb: "set_property".into(),
            class: ExpressionErrorClass::Runtime,
            position: None,
            conversion: None,
            cause: None,
        };
        assert!(!is_expression_detail_cause(&no_cause));
    }

    #[test]
    fn mapped_typed_variants_carry_cause() {
        let mismatch = to_expression_failed(
            LanguageError::TypeMismatch {
                expected: "bool".into(),
                actual: "string".into(),
                position: None,
            },
            &test_meta(None),
        );
        assert!(is_expression_detail_cause(&mismatch));
        let CamelError::ExpressionFailed { class, cause, .. } = mismatch else {
            panic!("expected ExpressionFailed");
        };
        assert_eq!(class, ExpressionErrorClass::TypeMismatch);
        assert_eq!(
            cause.expect("typed mismatch cause").to_string(),
            "Processor error: expected bool, actual string"
        );

        let conversion = to_expression_failed(
            LanguageError::ConversionError {
                source_type: "null".into(),
                target: "f64".into(),
            },
            &test_meta(None),
        );
        assert!(is_expression_detail_cause(&conversion));
        let CamelError::ExpressionFailed { class, cause, .. } = conversion else {
            panic!("expected ExpressionFailed");
        };
        assert_eq!(class, ExpressionErrorClass::Conversion);
        assert_eq!(
            cause.expect("typed conversion cause").to_string(),
            "Processor error: cannot convert null to f64"
        );
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
