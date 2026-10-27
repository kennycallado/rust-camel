//! Shared helpers for the doTry segment (`do_try_segment.rs`).
//!
//! [`CatchMatcher`] matches a `doCatch` clause against a `CamelError`.
//! `chain_predicate_error` attaches the original try error as the `cause`
//! of a failed catch `when` / `on_when` predicate (language-value-boundary
//! envelope). The runtime itself lives in `DoTrySegment`.

use camel_api::{CamelError, Exchange, PredicateSource};

/// Matcher for a `doCatch` clause.
#[derive(Clone)]
pub enum CatchMatcher {
    /// Match by CamelError variant name (e.g. ["ProcessorError", "Io"]).
    /// `"*"` matches any variant — equivalent to Camel's `doCatch(Throwable.class)`.
    ByVariant(Vec<String>),
    /// Match by a fallible predicate over the Exchange.
    Predicate(PredicateSource),
}

impl CatchMatcher {
    /// Returns whether this matcher matches the given error and exchange.
    ///
    /// A failed predicate evaluation yields `Err` (language-value-boundary:
    /// the caller chains the original error and fails the scope).
    pub async fn matches(&self, err: &CamelError, ex: &Exchange) -> Result<bool, CamelError> {
        match self {
            CatchMatcher::ByVariant(names) => {
                if names.iter().any(|n| n == "*") {
                    return Ok(true);
                }
                Ok(names.iter().any(|n| n == err.variant_name()))
            }
            CatchMatcher::Predicate(source) => source.matches(ex).await,
        }
    }
}

/// Chain the original try error into a failed catch-predicate error
/// (language-value-boundary: catch `when` / `on_when` failure envelope).
///
/// The copy keeps every field unchanged except `cause`, which holds the
/// original error so the chain stays retrievable. Two kinds of cause are
/// replaced: an empty slot (`cause: None`) and a mapper-generated diagnostic
/// cause identified by the camel-language-api provenance marker (never by
/// variant or string shape). Genuine caller-supplied chains — including
/// hand-built `ProcessorError`/`ProcessorErrorWithSource` causes — pass
/// through untouched. Errors that carry no `cause` slot pass through
/// untouched as well.
pub(crate) fn chain_predicate_error(
    mut predicate_err: CamelError,
    original: CamelError,
) -> CamelError {
    let should_chain = match &predicate_err {
        CamelError::ExpressionFailed { cause, .. } => {
            cause.is_none() || camel_language_api::is_expression_detail_cause(&predicate_err)
        }
        _ => false,
    };
    if should_chain && let CamelError::ExpressionFailed { cause, .. } = &mut predicate_err {
        *cause = Some(Box::new(original));
    }
    predicate_err
}
