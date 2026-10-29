//! Shared helpers for crate-local test modules.
//!
//! These helpers map a [`LanguageError`] through the production carrier so
//! every test module asserts against the same transport-level error surface.

use camel_api::CamelError;
use camel_api::ExpressionErrorClass;
use camel_language_api::LanguageError;

/// Fixed evaluation metadata used when exercising the production carrier.
pub(crate) fn eval_meta() -> camel_language_api::EvalMeta {
    camel_language_api::EvalMeta {
        language: "xpath".to_string(),
        route_id: "r1".to_string(),
        step_id: "s1".to_string(),
        verb: "set_body".to_string(),
        target: None,
    }
}

/// Map a language error through the production carrier and return its
/// class, the rendered cause, and the mapped error.
pub(crate) fn mapped_class_and_cause(
    err: LanguageError,
) -> (ExpressionErrorClass, String, CamelError) {
    let mapped = camel_language_api::to_expression_failed(err, &eval_meta());
    let (class, cause) = match &mapped {
        CamelError::ExpressionFailed { class, cause, .. } => (
            *class,
            cause
                .as_ref()
                .expect("expected a redacted cause")
                .to_string(),
        ),
        other => panic!("expected ExpressionFailed, got {other:?}"),
    };
    (class, cause, mapped)
}
