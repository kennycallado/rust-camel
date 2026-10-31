//! URI parsing for the external template component (ADR-0047 Stage 2,
//! Task 2.2).
//!
//! Two URI shapes are accepted, both with a two-part scheme similar to
//! `jdbc:h2:...`:
//!
//! - `template:file:///<abs-path>` — an absolute entry path.
//! - `template:file:<rel-path>` — the canonical relative form resolved against
//!   the configured root.
//!
//! `./`-prefixed and `..`-bearing relative spellings are configuration errors,
//! as are empty path components. This module provides a manual string parser —
//! no `url` crate dependency is needed for the simple split-and-validate
//! logic required.

use std::path::{Component, Path, PathBuf};

use camel_api::CamelError;

use crate::config::ExternalTemplateLimitsConfig;

/// How the entry template path was spelled in the URI.
///
/// Consumed by endpoint creation: a [`TemplateEntry::Relative`] path is
/// resolved against the configured root, while a [`TemplateEntry::Absolute`]
/// path is used as-is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TemplateEntry {
    /// `template:file:///<abs-path>`.
    Absolute(PathBuf),
    /// Canonical relative form `template:file:<rel-path>`.
    Relative(PathBuf),
}

/// Parsed configuration extracted from a `template:file:<path>` URI.
///
/// Constructed by [`parse_template_uri`]; consumed by later tasks (endpoint
/// creation, template loading).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TemplateEndpointConfig {
    /// Entry template path, tagged with its URI spelling.
    pub(crate) entry: TemplateEntry,
    /// Operator-configured resource limits carried from `Camel.toml`.
    pub(crate) limits: ExternalTemplateLimitsConfig,
}

/// Parse a `template:file:<path>` URI into a [`TemplateEndpointConfig`].
///
/// ## URI shape
///
/// ```text
/// template:file:///<abs-path>
/// template:file:<rel-path>
/// ```
///
/// - The outer scheme MUST be `template`.
/// - The inner scheme MUST be `file`.
/// - The absolute form has an empty authority (i.e., `://` followed
///   immediately by the path root); the path MUST be non-empty, absolute, and
///   free of `..` segments.
/// - The relative form is the canonical `<rel-path>` spelling. Its path MUST be
///   non-empty, contain no empty segments, and contain no `.` or `..`
///   segments.
///
/// ## Errors
///
/// Returns [`CamelError::Config`] on any violation with a diagnostic message.
///
/// ## Filesystem
///
/// This function performs NO filesystem I/O — it validates the URI string
/// structure only.
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

    let entry = if let Some(path_str) = path_start.strip_prefix("//") {
        // file:///<abs-path>  → path_start is "///<abs-path>". Strip the "//"
        // (empty authority — `://` without a host) so the remaining leading
        // `/` is the root of the absolute path.
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

        TemplateEntry::Absolute(path.to_path_buf())
    } else {
        // Canonical relative form: `template:file:<rel-path>`.
        if path_start.is_empty() {
            return Err(CamelError::Config(
                "template URI relative form must be file:<rel-path>: path is empty".into(),
            ));
        }

        for component in path_start.split('/') {
            if component.is_empty() {
                return Err(CamelError::Config(format!(
                    "template URI relative form must be file:<rel-path>: path '{path_start}' contains an empty component",
                )));
            }
            if component == "." {
                return Err(CamelError::Config(format!(
                    "template URI relative form must be file:<rel-path>: path '{path_start}' contains a '.' component",
                )));
            }
            if component == ".." {
                return Err(CamelError::Config(format!(
                    "template URI relative form must be file:<rel-path>: path '{path_start}' contains a '..' component",
                )));
            }
        }

        TemplateEntry::Relative(PathBuf::from(path_start))
    };

    Ok(TemplateEndpointConfig { entry, limits })
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
        assert_eq!(
            config.entry,
            TemplateEntry::Absolute(PathBuf::from("/srv/t/page.html"))
        );
    }

    #[test]
    fn parse_valid_relative_uri() {
        let uri = "template:file:templates/page.html";
        let limits = ExternalTemplateLimitsConfig::default();
        let result = parse_template_uri(uri, limits);
        let config = result.expect("valid relative URI should parse");
        assert_eq!(
            config.entry,
            TemplateEntry::Relative(PathBuf::from("templates/page.html"))
        );
    }

    #[test]
    fn parse_relative_rejects_cur_dir() {
        let uri = "template:file:./t.html";
        let limits = ExternalTemplateLimitsConfig::default();
        let result = parse_template_uri(uri, limits);
        assert!(result.is_err());
        assert!(matches!(result, Err(CamelError::Config(_))));
    }

    #[test]
    fn parse_relative_rejects_parent_dir() {
        let uri = "template:file:../t.html";
        let limits = ExternalTemplateLimitsConfig::default();
        let result = parse_template_uri(uri, limits);
        assert!(result.is_err());
        assert!(matches!(result, Err(CamelError::Config(_))));
    }

    #[test]
    fn parse_relative_rejects_parent_mid_path() {
        let uri = "template:file:a/../b.html";
        let limits = ExternalTemplateLimitsConfig::default();
        let result = parse_template_uri(uri, limits);
        assert!(result.is_err());
        assert!(matches!(result, Err(CamelError::Config(_))));
    }

    #[test]
    fn parse_relative_rejects_empty() {
        let uri = "template:file:";
        let limits = ExternalTemplateLimitsConfig::default();
        let result = parse_template_uri(uri, limits);
        assert!(result.is_err());
        assert!(matches!(result, Err(CamelError::Config(_))));
    }

    #[test]
    fn parse_relative_rejects_empty_component() {
        let uri = "template:file:a//b.html";
        let limits = ExternalTemplateLimitsConfig::default();
        let result = parse_template_uri(uri, limits);
        assert!(result.is_err());
        assert!(matches!(result, Err(CamelError::Config(_))));
    }

    #[test]
    fn parse_relative_rejects_trailing_slash() {
        let uri = "template:file:t/";
        let limits = ExternalTemplateLimitsConfig::default();
        let result = parse_template_uri(uri, limits);
        assert!(result.is_err());
        assert!(matches!(result, Err(CamelError::Config(_))));
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
