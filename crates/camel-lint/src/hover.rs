//! Hover types for the LSP integration.
//!
//! Defines [`HoverInfo`] — the type returned by
//! [`LintEngine::hover_at`](crate::engine::LintEngine::hover_at).

/// Metadata about a URI option shown in a hover tooltip.
#[derive(Debug, Clone, PartialEq)]
pub struct HoverInfo {
    /// Option description (omitted when empty).
    pub description: Option<String>,
    /// Deprecation reason (set when the option is deprecated).
    pub deprecated: Option<String>,
    /// Whether the option value is a secret that should be masked in UI.
    pub secret: bool,
}
