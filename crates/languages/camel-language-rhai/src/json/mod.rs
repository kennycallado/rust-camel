//! Private JSON data model for the Rhai host helpers.
//!
//! The wrapper tree and parser are built here, one level at a time, from exact
//! source tokens. The `host` module builds the shared process-wide Rhai module;
//! `RhaiLanguage::create_base_engine` registers it on every sandbox engine, so
//! the crate-local surface is live rather than test-only.

pub(crate) mod host;
pub(crate) mod parse;
pub(crate) mod serialize;

use indexmap::IndexMap;
use std::sync::Arc;

/// Stable Rhai type name of the JSON wrapper. Never a Rust type path.
pub(crate) const JSON_VALUE_TYPE_NAME: &str = "json value";
/// Stable Rhai type name of the number wrapper.
pub(crate) const JSON_NUMBER_TYPE_NAME: &str = "json number";
/// Owned depth cap: depth 128 is accepted, 129 is refused.
pub(crate) const MAX_JSON_DEPTH: usize = 128;

/// Redacted failure classes produced inside the JSON module.
///
/// None carries input text, parsed values, or parser positions; the host layer
/// attaches the script call position when it maps these to Rhai errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum JsonHostError {
    /// Strict RFC 8259 parse failure.
    Parse,
    /// A sandbox bound (depth, string, array, map, size) was exceeded.
    Limit,
    /// A value of an unsupported or incorrect type was supplied.
    TypeMismatch,
    /// An explicit numeric conversion produced a non-finite result.
    Arithmetic,
}

/// Immutable, reference-counted JSON tree.
///
/// Cloning is O(1); mutation goes through `Arc::make_mut` (copy-on-write).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct JsonValue(Arc<JsonNode>);

/// One tree node: the payload kind plus cached structural metrics.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct JsonNode {
    pub(crate) kind: JsonKind,
    /// Cached node depth: scalar 0, container `1 + max(child depth)`.
    pub(crate) depth: usize,
    /// Cached approximate serialized size in bytes.
    pub(crate) size: usize,
}

/// JSON value payload.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum JsonKind {
    Null,
    Bool(bool),
    Number(JsonNumber),
    String(String),
    Array(Vec<JsonValue>),
    Object(IndexMap<String, JsonValue>),
}

/// Exact source token of a JSON number.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct JsonNumber(Arc<str>);

impl JsonValue {
    /// Build a value, caching its `depth` and `size`.
    pub(crate) fn new(kind: JsonKind) -> Self {
        let (depth, size) = metrics(&kind);
        JsonValue(Arc::new(JsonNode { kind, depth, size }))
    }

    pub(crate) fn kind(&self) -> &JsonKind {
        &self.0.kind
    }

    pub(crate) fn depth(&self) -> usize {
        self.0.depth
    }

    pub(crate) fn size(&self) -> usize {
        self.0.size
    }

    /// Copy-on-write mutation: apply `f` to the kind, then recompute the cached
    /// metrics on the same node. The closure must not mutate before returning `Err`.
    pub(crate) fn mutate<R>(
        &mut self,
        f: impl FnOnce(&mut JsonKind) -> Result<R, JsonHostError>,
    ) -> Result<R, JsonHostError> {
        let node = Arc::make_mut(&mut self.0);
        let result = f(&mut node.kind)?;
        let (depth, size) = metrics(&node.kind);
        node.depth = depth;
        node.size = size;
        Ok(result)
    }
}

impl JsonNumber {
    pub(crate) fn new(token: impl Into<Arc<str>>) -> Self {
        JsonNumber(token.into())
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }

    /// Explicit, checked conversion to `f64`; a non-finite result is refused.
    pub(crate) fn to_f64(&self) -> Result<f64, JsonHostError> {
        match self.0.parse::<f64>() {
            Ok(value) if value.is_finite() => Ok(value),
            _ => Err(JsonHostError::Arithmetic),
        }
    }
}

/// Cached `(depth, size)` for a kind.
///
/// A scalar contributes its token/UTF-8 byte length. A container contributes
/// `2` for its delimiters plus the sizes of its keys and children, and a depth
/// of `1 + max(child depth)`.
fn metrics(kind: &JsonKind) -> (usize, usize) {
    match kind {
        JsonKind::Null => (0, "null".len()),
        JsonKind::Bool(true) => (0, "true".len()),
        JsonKind::Bool(false) => (0, "false".len()),
        JsonKind::Number(number) => (0, number.as_str().len()),
        JsonKind::String(text) => (0, text.len()),
        JsonKind::Array(items) => {
            let depth = 1 + items.iter().map(JsonValue::depth).max().unwrap_or(0);
            let size = 2 + items.iter().map(JsonValue::size).sum::<usize>();
            (depth, size)
        }
        JsonKind::Object(entries) => {
            let depth = 1 + entries.values().map(JsonValue::depth).max().unwrap_or(0);
            let size = 2 + entries
                .iter()
                .map(|(key, value)| key.len() + value.size())
                .sum::<usize>();
            (depth, size)
        }
    }
}

/// Project a number token to a native `i64` only when it is an integer form
/// (no fraction or exponent) that fits. `-0` normalizes to `0`.
pub(crate) fn token_projects_to_i64(token: &str) -> Option<i64> {
    if token.bytes().any(|b| matches!(b, b'.' | b'e' | b'E')) {
        return None;
    }
    token.parse::<i64>().ok()
}

/// Project a stored value to a Rhai `Dynamic` using the shared projection:
/// containers become `JsonValue`, strings/bools/unit stay native, and only
/// integer-form tokens become native `INT` (other numbers stay `JsonNumber`).
pub(crate) fn project_dynamic(value: &JsonValue) -> rhai::Dynamic {
    match value.kind() {
        JsonKind::Null => rhai::Dynamic::UNIT,
        JsonKind::Bool(flag) => rhai::Dynamic::from(*flag),
        JsonKind::Number(number) => match token_projects_to_i64(number.as_str()) {
            Some(int) => rhai::Dynamic::from(int),
            None => rhai::Dynamic::from(number.clone()),
        },
        JsonKind::String(text) => rhai::Dynamic::from(text.clone()),
        JsonKind::Array(_) | JsonKind::Object(_) => rhai::Dynamic::from(value.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::{JsonHostError, JsonNumber, token_projects_to_i64};

    #[test]
    fn token_projects_integer_forms() {
        assert_eq!(token_projects_to_i64("9223372036854775807"), Some(i64::MAX));
        assert_eq!(token_projects_to_i64("-0"), Some(0));
        assert_eq!(token_projects_to_i64("1.0"), None);
        assert_eq!(token_projects_to_i64("1e2"), None);
        assert_eq!(token_projects_to_i64("18446744073709551615"), None);
    }

    #[test]
    fn parse_to_float_1e400_arithmetic() {
        assert_eq!(
            JsonNumber::new("1e400").to_f64(),
            Err(JsonHostError::Arithmetic)
        );
    }
}
