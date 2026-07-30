//! URI parsing for the external template component (ADR-0047 Stage 2,
//! Task 2.2).
//!
//! The expected URI shape is `template:file:///<abs-path>`, a two-part scheme
//! similar to `jdbc:h2:...`. This module provides a manual string parser —
//! no `url` crate dependency is needed for the simple split-and-validate
//! logic required.

use std::path::{Component, Path, PathBuf};

use camel_api::CamelError;

use crate::config::ExternalTemplateLimitsConfig;

/// Parsed configuration extracted from a `template:file:///<abs-path>` URI.
///
/// Constructed by [`parse_template_uri`]; consumed by later tasks (endpoint
/// creation, template loading).
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)] // consumed by later tasks (Phase 4)
pub(crate) struct TemplateEndpointConfig {
    /// Absolute filesystem path to the entry template.
    pub(crate) entry_abs_path: PathBuf,
    /// Operator-configured resource limits carried from `Camel.toml`.
    pub(crate) limits: ExternalTemplateLimitsConfig,
}

/// Parse a `template:file:///<abs-path>` URI into a [`TemplateEndpointConfig`].
///
/// ## URI shape
///
/// ```text
/// template:file:///<abs-path>
/// ```
///
/// - The outer scheme MUST be `template`.
/// - The inner scheme MUST be `file`.
/// - The authority portion MUST be empty (i.e., `://` followed immediately by
///   the path root).
/// - The path MUST be non-empty, absolute, and free of `..` segments.
///
/// ## Errors
///
/// Returns [`CamelError::Config`] on any violation with a diagnostic message.
///
/// ## Filesystem
///
/// This function performs NO filesystem I/O — it validates the URI string
/// structure only.
#[allow(dead_code)] // consumed by later tasks (Phase 4)
pub(crate) fn parse_template_uri(
    uri: &str,
    limits: ExternalTemplateLimitsConfig,
) -> Result<TemplateEndpointConfig, CamelError> {
    let (outer_scheme, rest) = uri.split_once(':').ok_or_else(|| {
        CamelError::Config(
            "template URI must be file:///<abs-path>: missing scheme separator".into(),
        )
    })?;

    if outer_scheme != "template" {
        return Err(CamelError::Config(format!(
            "template URI must be file:///<abs-path>: unknown outer scheme '{outer_scheme}'",
        )));
    }

    let (inner_scheme, path_start) = rest.split_once(':').ok_or_else(|| {
        CamelError::Config("template URI must be file:///<abs-path>: missing inner scheme".into())
    })?;

    if inner_scheme != "file" {
        return Err(CamelError::Config(format!(
            "template URI must be file:///<abs-path>: unknown inner scheme '{inner_scheme}'",
        )));
    }

    // file:///<abs-path>  → path_start is "///<abs-path>"
    // Strip the "//" (empty authority — `://` without a host) so the
    // remaining leading `/` is the root of the absolute path.
    let path_str = path_start.strip_prefix("//").ok_or_else(|| {
        CamelError::Config(
            "template URI must be file:///<abs-path>: expected file:///<abs-path>".into(),
        )
    })?;

    if path_str.is_empty() {
        return Err(CamelError::Config(
            "template URI must be file:///<abs-path>: path is empty".into(),
        ));
    }

    let path = Path::new(path_str);

    if !path.is_absolute() {
        return Err(CamelError::Config(format!(
            "template URI must be file:///<abs-path>: path '{path_str}' is not absolute",
        )));
    }

    // Reject any ".." path segment (path-traversal guard).
    if path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(CamelError::Config(format!(
            "template URI must be file:///<abs-path>: path '{path_str}' contains '..' segments",
        )));
    }

    Ok(TemplateEndpointConfig {
        entry_abs_path: path.to_path_buf(),
        limits,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn parse_valid_file_uri() {
        let uri = "template:file:///srv/t/page.html";
        let limits = ExternalTemplateLimitsConfig::default();
        let result = parse_template_uri(uri, limits);
        let config = result.expect("valid file URI should parse");
        assert_eq!(config.entry_abs_path, PathBuf::from("/srv/t/page.html"));
    }

    #[test]
    fn parse_rejects_bare_path() {
        let uri = "template:/srv/t/page.html";
        let limits = ExternalTemplateLimitsConfig::default();
        let result = parse_template_uri(uri, limits);
        assert!(result.is_err());
        assert!(matches!(result, Err(CamelError::Config(_))));
    }

    #[test]
    fn parse_rejects_non_file_scheme() {
        let uri = "template:http://h/p";
        let limits = ExternalTemplateLimitsConfig::default();
        let result = parse_template_uri(uri, limits);
        assert!(result.is_err());
        assert!(matches!(result, Err(CamelError::Config(_))));
    }
}
