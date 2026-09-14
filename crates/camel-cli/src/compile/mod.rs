//! Compiled-artifact support for `camel compile` (openspec changes
//! `cli-compile` and `multidoc`).
//!
//! The artifact format appends a marked trailer to a copy of the current
//! executable: v1 is `payload || manifest || fixed footer` (68-byte footer),
//! v2 is `CAMELTR1 || content || index || manifest || footer` (76-byte
//! footer) bundling a multi-document virtual store. This module owns the
//! pieces:
//!
//! - [`trailer`] — deterministic EOF trailer codec (v1 and v2 framing).
//! - [`manifest`] — canonical operational manifest embedded next to the
//!   payload.
//! - [`sources`] — explicit multi-document source selection, resolution,
//!   and confinement (multidoc Task 1.2).
//! - [`store`] — re-export of the canonical virtual-document store model
//!   from `camel-dsl`.
//!
//! The artifact embeds authoring text, never `RouteDefinition` or compiled
//! steps: DSL stays responsible for parsing and interpolation, runtime for
//! lowering and lifecycle.

pub mod manifest;
pub mod policy;
pub mod runtime;
pub mod sources;
pub mod store;
pub mod trailer;

use std::fmt;

/// Compile-pipeline failure taxonomy for `camel compile`.
///
/// Named variants let the command surface exit-2 diagnostics that name the
/// rejected cause (invalid bytes, oversize payload, unsupported asset class,
/// unparsable document) instead of a generic error string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompileError {
    /// Document bytes are not valid UTF-8.
    InvalidUtf8,
    /// Normalized document exceeds the 16 MiB compile payload limit.
    PayloadTooLarge,
    /// Document requires a compile-time asset v1 cannot embed. The payload
    /// names the rejected asset class or field.
    UnsupportedAsset(String),
    /// Document could not be processed for the stated reason.
    InvalidDocument(String),
}

impl fmt::Display for CompileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidUtf8 => write!(f, "document is not valid UTF-8"),
            Self::PayloadTooLarge => {
                write!(f, "document exceeds the 16 MiB compile payload limit")
            }
            Self::UnsupportedAsset(asset) => write!(f, "unsupported compile-time asset: {asset}"),
            Self::InvalidDocument(reason) => write!(f, "invalid document: {reason}"),
        }
    }
}

impl std::error::Error for CompileError {}
