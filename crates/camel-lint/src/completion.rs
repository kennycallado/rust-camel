//! Completion types for the LSP integration.
//!
//! Defines [`CompletionItem`] — the type returned by
//! [`LintEngine::complete_at`](crate::engine::LintEngine::complete_at).

/// A single completion suggestion for an editor popup.
#[derive(Debug, Clone, PartialEq)]
pub struct CompletionItem {
    /// The text inserted when the user accepts this completion.
    pub label: String,
    /// Optional detail line shown below the label (e.g. option description).
    pub detail: Option<String>,
}
