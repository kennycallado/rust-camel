//! The document-level `logs:` assertion grammar (rc-p1x2a, split out
//! of the parent module; mirrors the `document/error.rs` pattern).
//!
//! `LogsAssertion` and `LogLevel` are re-exported at
//! `crate::document` and the crate root, so consumers keep the paths
//! they had before the split.

use noyalib::compat::serde_yaml;
use serde::Deserialize;

use super::DocError;

/// The document-level `logs:` assertion block (rc-tdgh5): log-content
/// expectations the runner evaluates against the capture window that
/// spans the document run. Conjunction across clauses — every entry of
/// every list must hold; `None`-valued clauses assert nothing.
#[derive(Debug, Clone, PartialEq)]
pub struct LogsAssertion {
    /// Substring markers: each entry must appear in at least one
    /// captured event's message.
    pub contains: Vec<String>,
    /// Unanchored patterns: each entry must match at least one
    /// captured event's message. Every pattern compiles at load time;
    /// a non-compiling pattern is a load error.
    pub regex: Vec<String>,
    /// Severity ceiling: no captured event may carry a level above
    /// this cap. `None` asserts nothing about levels.
    pub no_level_above: Option<LogLevel>,
}

/// A `noLevelAbove` severity. The grammar accepts exactly
/// `trace|debug|info|warn|error`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    /// Below `debug`.
    Trace,
    /// Below `info`.
    Debug,
    /// Below `warn`.
    Info,
    /// Below `error`.
    Warn,
    /// The most severe level.
    Error,
}

/// Raw `logs:` block (rc-tdgh5): keys and level stay raw so the
/// clause walk can name the offending entry; conversion happens during
/// validation, never at the serde layer.
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawLogs {
    contains: Option<Vec<String>>,
    regex: Option<Vec<String>>,
    no_level_above: Option<String>,
}

/// Converts the raw `logs:` node (rc-tdgh5). Malformed blocks are load
/// errors through [`DocError::LogsBlock`]: an unknown key, a level
/// outside `trace|debug|info|warn|error`, or a regex that does not
/// compile — each error names the offending clause.
pub(super) fn logs_from_raw(value: serde_yaml::Value) -> Result<LogsAssertion, DocError> {
    let block_error = |detail: String| DocError::LogsBlock { detail };
    let raw: RawLogs = serde_yaml::from_value(value).map_err(|e| block_error(e.to_string()))?;
    let no_level_above = raw
        .no_level_above
        .as_deref()
        .map(|raw_level| match raw_level {
            "trace" => Ok(LogLevel::Trace),
            "debug" => Ok(LogLevel::Debug),
            "info" => Ok(LogLevel::Info),
            "warn" => Ok(LogLevel::Warn),
            "error" => Ok(LogLevel::Error),
            other => Err(block_error(format!(
                "`logs.noLevelAbove` must be one of trace|debug|info|warn|error, got `{other}`"
            ))),
        })
        .transpose()?;
    for pattern in raw.regex.iter().flatten() {
        // Compile-time gate: the runner matches unanchored, so a
        // pattern that compiles here always compiles there.
        if let Err(error) = regex::Regex::new(pattern) {
            return Err(block_error(format!(
                "`logs.regex` entry `{pattern}` does not compile: {error}"
            )));
        }
    }
    Ok(LogsAssertion {
        contains: raw.contains.unwrap_or_default(),
        regex: raw.regex.unwrap_or_default(),
        no_level_above,
    })
}
