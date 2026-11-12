//! Route-level mapping from business error payloads to HTTP statuses.
//!
//! [`HttpErrorMapping`] is a route-level field, deliberately separate from
//! [`crate::error_handler::ErrorHandlerConfig`]: the mapping decides the
//! reply status of a propagated `CamelError::Business`, while the error
//! handler owns retry/dead-letter policy. A route may carry both.
//!
//! Status resolution precedence (see [`business_status`]):
//!
//! 1. explicit `codes` entry (exact, case-sensitive match on the code),
//! 2. explicit `kinds` entry (exact, case-sensitive match on the kind),
//! 3. the built-in default kind map ([`default_kind_status`]),
//! 4. `500`.
//!
//! Values are restricted to the client/server error domain (`400..=599`).
//! Load-time validation ([`validate_status`], [`validate_key`], and
//! [`HttpErrorMapping::validate`]) rejects blank keys and out-of-domain
//! values. Programmatic construction can bypass that validation, so
//! resolution also defends: an out-of-domain entry is treated as absent and
//! the remaining precedence rules apply. A business error can never resolve
//! to a 2xx status.

use std::collections::BTreeMap;

use thiserror::Error;

/// Lowest status an error mapping entry may produce (client error).
pub const MIN_MAPPED_STATUS: u16 = 400;
/// Highest status an error mapping entry may produce (server error).
pub const MAX_MAPPED_STATUS: u16 = 599;
/// Status used when no mapping entry and no built-in default matches.
pub const DEFAULT_BUSINESS_STATUS: u16 = 500;

/// Built-in default kind map for business errors.
///
/// Kebab-case kinds match the error kind strings exposed by
/// `CamelError::classify()` and the `on_exceptions` matchers.
const DEFAULT_KIND_STATUSES: [(&str, u16); 5] = [
    ("invalid-argument", 400),
    ("conflict", 409),
    ("not-found", 404),
    ("forbidden", 403),
    ("failed", 500),
];

/// Which table of an [`HttpErrorMapping`] a key belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum MappingTable {
    /// Exact business error code to status (`codes` table).
    Codes,
    /// Business error kind to status (`kinds` table).
    Kinds,
}

impl MappingTable {
    /// Table name as it appears in mapping documents.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Codes => "codes",
            Self::Kinds => "kinds",
        }
    }
}

/// Typed load-time validation failure for an [`HttpErrorMapping`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum HttpErrorMappingError {
    /// A table key is empty or whitespace-only.
    #[error("blank `{table}` key in HTTP error mapping")]
    BlankKey {
        /// Name of the offending table (`codes` or `kinds`).
        table: &'static str,
    },
    /// A table value falls outside the client/server error domain.
    #[error("HTTP error mapping `{table}` value {status} is outside 400-599")]
    StatusOutOfDomain {
        /// Name of the offending table (`codes` or `kinds`).
        table: &'static str,
        /// Rejected status value.
        status: u16,
    },
}

/// Returns `true` when `status` is a client or server error status
/// (`400..=599`), the only domain a business error may map to.
#[must_use]
pub const fn is_mapped_status(status: u16) -> bool {
    status >= MIN_MAPPED_STATUS && status <= MAX_MAPPED_STATUS
}

/// Reject a blank table key.
///
/// # Errors
///
/// Returns [`HttpErrorMappingError::BlankKey`] when `key` is empty or
/// whitespace-only.
pub fn validate_key(table: MappingTable, key: &str) -> Result<(), HttpErrorMappingError> {
    if key.trim().is_empty() {
        return Err(HttpErrorMappingError::BlankKey {
            table: table.name(),
        });
    }
    Ok(())
}

/// Reject a status outside the client/server error domain.
///
/// # Errors
///
/// Returns [`HttpErrorMappingError::StatusOutOfDomain`] for any value below
/// 400 or above 599 (including informational, success, and redirect
/// statuses such as 101, 200, and 304).
pub fn validate_status(table: MappingTable, status: u16) -> Result<(), HttpErrorMappingError> {
    if is_mapped_status(status) {
        Ok(())
    } else {
        Err(HttpErrorMappingError::StatusOutOfDomain {
            table: table.name(),
            status,
        })
    }
}

/// Built-in default status for a business error kind, if the kind is mapped.
///
/// The default map is kebab-case and matches the existing error kind strings:
/// `invalid-argument` 400, `conflict` 409, `not-found` 404, `forbidden` 403,
/// `failed` 500.
#[must_use]
pub fn default_kind_status(kind: &str) -> Option<u16> {
    DEFAULT_KIND_STATUSES
        .iter()
        .find_map(|(name, status)| (*name == kind).then_some(*status))
}

/// Resolve the reply status for a business error.
///
/// Resolution order: explicit code map, explicit kind map, built-in default
/// kind map, [`DEFAULT_BUSINESS_STATUS`]. Out-of-domain explicit entries are
/// treated as absent. `None` applies the built-in default map and the 500
/// closure, so a route without a mapping still never yields 2xx.
#[must_use]
pub fn business_status(mapping: Option<&HttpErrorMapping>, code: &str, kind: &str) -> u16 {
    mapping
        .and_then(|mapping| mapping.explicit_status(code, kind))
        .or_else(|| default_kind_status(kind))
        .unwrap_or(DEFAULT_BUSINESS_STATUS)
}

/// Route-level business error to HTTP status mapping.
///
/// `codes` maps an exact business error code to a status; `kinds` maps an
/// exact business error kind to a status. Both tables are case-sensitive and
/// may be empty.
///
/// The mapping is a separate route-level field and never lives on
/// [`crate::error_handler::ErrorHandlerConfig`]. It carries no `message` or
/// `data`: the reply body for a business error is exactly
/// `{"error": <code>, "kind": <kind>}`, because runtime exchange data is
/// untrusted and no mapping authorizes its disclosure.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HttpErrorMapping {
    /// Exact business error code to HTTP status.
    pub codes: BTreeMap<String, u16>,
    /// Exact business error kind to HTTP status.
    pub kinds: BTreeMap<String, u16>,
}

impl HttpErrorMapping {
    /// Create an empty mapping (built-in defaults only).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a code entry, returning `self` for builder-style construction.
    #[must_use]
    pub fn with_code(mut self, code: impl Into<String>, status: u16) -> Self {
        self.codes.insert(code.into(), status);
        self
    }

    /// Add a kind entry, returning `self` for builder-style construction.
    #[must_use]
    pub fn with_kind(mut self, kind: impl Into<String>, status: u16) -> Self {
        self.kinds.insert(kind.into(), status);
        self
    }

    /// Returns `true` when both tables are empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.codes.is_empty() && self.kinds.is_empty()
    }

    /// Validate every entry: non-blank keys and statuses in `400..=599`.
    ///
    /// # Errors
    ///
    /// Returns the first [`HttpErrorMappingError`] found, checking the
    /// `codes` table before the `kinds` table.
    pub fn validate(&self) -> Result<(), HttpErrorMappingError> {
        for (code, status) in &self.codes {
            validate_key(MappingTable::Codes, code)?;
            validate_status(MappingTable::Codes, *status)?;
        }
        for (kind, status) in &self.kinds {
            validate_key(MappingTable::Kinds, kind)?;
            validate_status(MappingTable::Kinds, *status)?;
        }
        Ok(())
    }

    /// Explicit status for a business error, or `None` when no explicit
    /// entry applies.
    ///
    /// The code table wins over the kind table. An out-of-domain entry is
    /// treated as absent, so resolution continues with the remaining
    /// precedence rules.
    #[must_use]
    pub fn explicit_status(&self, code: &str, kind: &str) -> Option<u16> {
        self.codes
            .get(code)
            .copied()
            .filter(|status| is_mapped_status(*status))
            .or_else(|| {
                self.kinds
                    .get(kind)
                    .copied()
                    .filter(|status| is_mapped_status(*status))
            })
    }
}
