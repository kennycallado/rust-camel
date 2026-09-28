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
//! - [`materialize`] — confined per-boot materialization of
//!   substitution-targeted assets (r2embed Task 3.2).
//! - [`signature`] — detached Ed25519ph signature envelope sidecar for
//!   signed artifacts (r4sign).
//! - [`trust`] — deployment truststore codec for signature pinning
//!   (keypin Task 1.2).
//!
//! The artifact embeds authoring text, never `RouteDefinition` or compiled
//! steps: DSL stays responsible for parsing and interpolation, runtime for
//! lowering and lifecycle.

pub mod manifest;
pub mod materialize;
pub mod policy;
pub mod runtime;
pub mod signature;
pub mod sources;
pub mod store;
pub mod trailer;

mod trust;

use std::fmt;

/// One MiB, for the human-readable cap annotation in the oversize-payload
/// diagnostic.
const MIB: u64 = 1024 * 1024;

/// Compile-pipeline failure taxonomy for `camel compile`.
///
/// Named variants let the command surface exit-2 diagnostics that name the
/// rejected cause (invalid bytes, oversize payload, unsupported asset class,
/// unparsable document) instead of a generic error string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompileError {
    /// Document bytes are not valid UTF-8.
    InvalidUtf8,
    /// The aggregate compile payload — normalized document bytes plus
    /// verbatim asset bytes — exceeds the configured
    /// `--max-payload-bytes` cap. Carries the offending total and the
    /// configured cap for the named diagnostic.
    PayloadTooLarge {
        /// Aggregate embedded byte total that breached the cap.
        total: u64,
        /// The configured cap the total must fit under.
        cap: u64,
    },
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
            Self::PayloadTooLarge { total, cap } => {
                write!(
                    f,
                    "aggregate compile payload of {total} bytes exceeds the configured cap of \
                     {cap} bytes"
                )?;
                // MiB annotation for the default-sized cap so operators
                // see the familiar bound.
                if cap % MIB == 0 {
                    write!(f, " ({} MiB)", cap / MIB)?;
                }
                Ok(())
            }
            Self::UnsupportedAsset(asset) => write!(f, "unsupported compile-time asset: {asset}"),
            Self::InvalidDocument(reason) => write!(f, "invalid document: {reason}"),
        }
    }
}

impl std::error::Error for CompileError {}
