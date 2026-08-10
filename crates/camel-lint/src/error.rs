//! Crate error type for camel-lint.
//!
//! [`LintError`] is the only error type the crate returns. Analysis
//! ([`crate::engine::LintEngine::lint`]) is infallible — it returns
//! `Vec<Diagnostic>`; parse failures flow through
//! [`crate::document::Document::parse_failure`] to the R-SYN rule. Only
//! [`crate::Document::apply_fix`] returns `Result<_, LintError>`.

/// The single error type returned by fallible camel-lint operations.
///
/// Today only [`crate::Document::apply_fix`] returns it: an edit that cannot be
/// applied, or that would leave the document un-parseable, yields
/// `LintError::Internal`. The string is a non-stable detail for diagnostics;
/// callers match on the variant, not the message.
#[derive(Debug, thiserror::Error)]
pub enum LintError {
    #[error("internal lint error: {0}")]
    Internal(String),
}
