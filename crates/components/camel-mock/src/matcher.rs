//! Assertion matcher vocabulary for the mock testkit.
//!
//! [`BodyMatcher`] and [`HeaderMatcher`] describe expected received bodies
//! and header values. They are assertion-side only: they never change
//! producer behavior (the producer stays a sink per the component identity
//! ruling).

use std::fmt;

use camel_component_api::Body;
use camel_matchers::{Expectation, expectation_matches};
use regex::Regex;
use serde_json::Value;

use crate::assert::body_eq;

/// A matcher over a received [`Body`].
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum BodyMatcher {
    /// The body equals the given value (variant-tagged structural equality).
    Equals(Body),
    /// The body text matches the given regular expression.
    Regex(String),
    /// The body text contains the given substring.
    Contains(String),
    /// The body text starts with the given prefix.
    StartsWith(String),
    /// The body text ends with the given suffix.
    EndsWith(String),
    /// The body is present (any variant except `Empty`).
    Exists,
    /// The body is a JSON object that is a superset of the given object.
    JsonSubset(Value),
}

impl BodyMatcher {
    /// Evaluate this matcher against a received body.
    pub fn matches(&self, actual: &Body) -> bool {
        match self {
            BodyMatcher::Equals(expected) => body_eq(expected, actual),
            BodyMatcher::Regex(pattern) => text_only(actual).is_some_and(|value| {
                expectation_matches(&Expectation::Regex(pattern.clone()), &value)
            }),
            BodyMatcher::Contains(needle) => text_only(actual).is_some_and(|value| {
                expectation_matches(&Expectation::Contains(needle.clone()), &value)
            }),
            BodyMatcher::StartsWith(prefix) => text_only(actual).is_some_and(|value| {
                expectation_matches(&Expectation::StartsWith(prefix.clone()), &value)
            }),
            BodyMatcher::EndsWith(suffix) => text_only(actual).is_some_and(|value| {
                expectation_matches(&Expectation::EndsWith(suffix.clone()), &value)
            }),
            BodyMatcher::Exists => !matches!(actual, Body::Empty),
            BodyMatcher::JsonSubset(pattern) => {
                pattern.is_object()
                    && json_value(actual).is_some_and(|received| {
                        expectation_matches(&Expectation::JsonSubset(pattern.clone()), &received)
                    })
            }
        }
    }

    /// The regex pattern, if this is a [`BodyMatcher::Regex`].
    pub fn regex_pattern(&self) -> Option<&str> {
        match self {
            BodyMatcher::Regex(pattern) => Some(pattern),
            _ => None,
        }
    }

    /// A short note explaining why a non-matching body failed, when the
    /// failure is a shape mismatch rather than a value mismatch.
    pub fn mismatch_note(&self, actual: &Body) -> Option<&'static str> {
        match self {
            BodyMatcher::Regex(_)
            | BodyMatcher::Contains(_)
            | BodyMatcher::StartsWith(_)
            | BodyMatcher::EndsWith(_) => match actual {
                Body::Text(_) => None,
                _ => Some("body is not text"),
            },
            BodyMatcher::JsonSubset(pattern) => {
                if !pattern.is_object() {
                    return Some("body is not JSON");
                }
                match json_value(actual) {
                    None => Some("body is not JSON"),
                    Some(received) => {
                        if received.is_object() {
                            None
                        } else {
                            Some("body is not a JSON object")
                        }
                    }
                }
            }
            BodyMatcher::Equals(_) | BodyMatcher::Exists => None,
        }
    }
}

impl fmt::Display for BodyMatcher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BodyMatcher::Equals(v) => write!(f, "equals {}", compact_body(v)),
            BodyMatcher::Regex(p) => write!(f, "regex {p}"),
            BodyMatcher::Contains(n) => write!(f, "contains {n}"),
            BodyMatcher::StartsWith(p) => write!(f, "startsWith {p}"),
            BodyMatcher::EndsWith(s) => write!(f, "endsWith {s}"),
            BodyMatcher::Exists => write!(f, "exists"),
            BodyMatcher::JsonSubset(v) => write!(f, "jsonSubset {}", compact(v)),
        }
    }
}

/// A matcher over a received header value.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum HeaderMatcher {
    /// The header value equals the given JSON value.
    Equals(Value),
    /// The header value (a string) matches the given regular expression.
    Regex(String),
    /// The header key is present (any value, including JSON null).
    Exists,
}

impl HeaderMatcher {
    /// Evaluate this matcher against a received header value.
    pub fn matches(&self, actual: Option<&Value>) -> bool {
        match self {
            HeaderMatcher::Exists => actual.is_some(),
            HeaderMatcher::Equals(expected) => match actual {
                Some(a) => a == expected,
                None => false,
            },
            HeaderMatcher::Regex(pattern) => match actual {
                Some(Value::String(s)) => compile(pattern).is_some_and(|re| re.is_match(s)),
                _ => false,
            },
        }
    }

    /// The regex pattern, if this is a [`HeaderMatcher::Regex`].
    pub fn regex_pattern(&self) -> Option<&str> {
        match self {
            HeaderMatcher::Regex(pattern) => Some(pattern),
            _ => None,
        }
    }

    /// A short note explaining why a non-matching header failed, when the
    /// failure is a shape mismatch rather than a value mismatch.
    pub fn mismatch_note(&self, actual: Option<&Value>) -> Option<&'static str> {
        match self {
            HeaderMatcher::Regex(_) => match actual {
                Some(Value::String(_)) => None,
                _ => Some("value is not a string"),
            },
            HeaderMatcher::Equals(_) | HeaderMatcher::Exists => None,
        }
    }
}

impl fmt::Display for HeaderMatcher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HeaderMatcher::Equals(v) => write!(f, "equals {}", compact(v)),
            HeaderMatcher::Regex(p) => write!(f, "regex {p}"),
            HeaderMatcher::Exists => write!(f, "exists"),
        }
    }
}

/// Compile a regex pattern, returning `None` for an invalid pattern.
fn compile(pattern: &str) -> Option<Regex> {
    Regex::new(pattern).ok()
}

/// Render a JSON value compactly.
fn compact(v: &Value) -> String {
    serde_json::to_string(v).unwrap_or_else(|_| String::new())
}

/// Render a body compactly for display.
pub(crate) fn compact_body(body: &Body) -> String {
    match body {
        Body::Json(v) => compact(v),
        Body::Text(s) => s.clone(),
        other => format!("{other:?}"),
    }
}

/// The body text as a JSON string value, if the body is text. Every
/// other body variant projects to `None` so the string verbs fail
/// closed on non-text bodies.
fn text_only(body: &Body) -> Option<Value> {
    match body {
        Body::Text(text) => Some(Value::String(text.clone())),
        _ => None,
    }
}

/// Extract the JSON value from a body, if it is JSON or parseable text.
fn json_value(body: &Body) -> Option<Value> {
    match body {
        Body::Json(v) => Some(v.clone()),
        Body::Text(text) => serde_json::from_str(text).ok(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn regex_body_pass_and_fail() {
        assert!(
            BodyMatcher::Regex("^order-[0-9]+$".into()).matches(&Body::Text("order-42".into()))
        );
        assert!(
            !BodyMatcher::Regex("^order-[0-9]+$".into()).matches(&Body::Text("refunded-42".into()))
        );
    }

    #[test]
    fn substring_and_anchor_matchers() {
        let body = Body::Text("order-total-42".into());
        assert!(BodyMatcher::Contains("total".into()).matches(&body));
        assert!(BodyMatcher::StartsWith("order-".into()).matches(&body));
        assert!(BodyMatcher::EndsWith("-42".into()).matches(&body));
    }

    #[test]
    fn exists_body_variants() {
        assert!(BodyMatcher::Exists.matches(&Body::Text("x".into())));
        assert!(!BodyMatcher::Exists.matches(&Body::Empty));
    }

    #[test]
    fn string_matchers_fail_non_text() {
        let json_body = Body::Json(json!({"a": 1}));
        let bytes_body = Body::Bytes(vec![97u8].into());
        assert!(!BodyMatcher::Contains("a".into()).matches(&json_body));
        assert!(!BodyMatcher::Contains("a".into()).matches(&bytes_body));
        assert_eq!(
            BodyMatcher::Contains("a".into()).mismatch_note(&json_body),
            Some("body is not text")
        );
        assert_eq!(
            BodyMatcher::Contains("a".into()).mismatch_note(&bytes_body),
            Some("body is not text")
        );
    }

    #[test]
    fn string_verbs_delegate_through_text_projection() {
        let pattern = "^order-[0-9]+$".to_string();
        let body = Body::Text("order-42".into());
        assert!(BodyMatcher::Regex(pattern.clone()).matches(&body));
        // Same verdict as the shared algebra applied to the projected
        // `Value::String`: the matcher is a thin projection plus
        // delegation.
        assert_eq!(
            BodyMatcher::Regex(pattern.clone()).matches(&body),
            expectation_matches(
                &Expectation::Regex(pattern.clone()),
                &Value::String("order-42".into())
            )
        );
        // A JSON body projects no text, so `contains` fails even though
        // the serialized JSON would contain the needle.
        let json_body = Body::Json(json!({"total": 42}));
        assert!(!BodyMatcher::Contains("total".into()).matches(&json_body));
    }

    #[test]
    fn non_text_bodies_fail_closed_for_string_verbs() {
        let matchers = [
            BodyMatcher::Regex("x".into()),
            BodyMatcher::Contains("x".into()),
            BodyMatcher::StartsWith("x".into()),
            BodyMatcher::EndsWith("x".into()),
        ];
        let json_body = Body::Json(json!({"x": 1}));
        let bytes_body = Body::Bytes(vec![120u8].into());
        for matcher in &matchers {
            assert!(!matcher.matches(&json_body), "{matcher:?} over json");
            assert!(!matcher.matches(&bytes_body), "{matcher:?} over bytes");
            assert!(!matcher.matches(&Body::Empty), "{matcher:?} over empty");
            assert_eq!(matcher.mismatch_note(&json_body), Some("body is not text"));
            assert_eq!(matcher.mismatch_note(&bytes_body), Some("body is not text"));
        }
    }

    #[test]
    fn json_subset_local_duplicate_deleted() {
        // Grep oracle (ADR-0072 step 2): the local recursive-subset
        // implementation was deleted in favor of the shared algebra in
        // `camel-matchers`; this file must not define it anymore. The
        // needle is the deleted definition's signature, assembled from
        // split literals so this file's own source text does not contain
        // the contiguous needle (test fn names such as
        // `json_subset_arrays_exact` share the prefix and must not trip
        // the oracle).
        let source = include_str!("matcher.rs");
        let needle = concat!("fn json_", "subset(pattern");
        assert!(!source.contains(needle));
    }

    #[test]
    fn json_subset_delegation_preserves_verdicts() {
        let matcher = BodyMatcher::JsonSubset(json!({"status": "ok", "meta": {"seq": 3}}));
        let superset = Body::Json(json!({"id": 7, "status": "ok", "meta": {"seq": 3, "ts": 9}}));
        assert!(matcher.matches(&superset));
        let mismatched = Body::Json(json!({"status": "ok", "meta": {"seq": 4}}));
        assert!(!matcher.matches(&mismatched));
        // A scalar pattern fails regardless of the body: the
        // object-pattern guard rejects it before delegation.
        assert!(!BodyMatcher::JsonSubset(json!(5)).matches(&Body::Json(json!(5))));
        assert!(!BodyMatcher::JsonSubset(json!(5)).matches(&Body::Text("5".into())));
    }

    #[test]
    fn json_subset_recursive_ignores_extra() {
        let matcher = BodyMatcher::JsonSubset(json!({"status": "ok", "meta": {"seq": 3}}));
        let body = Body::Json(json!({"id": 7, "status": "ok", "meta": {"seq": 3, "ts": 9}}));
        assert!(matcher.matches(&body));
    }

    #[test]
    fn json_subset_arrays_exact() {
        let matcher = BodyMatcher::JsonSubset(json!({"tags": ["a", "b"]}));
        assert!(!matcher.matches(&Body::Json(json!({"tags": ["b", "a"]}))));
        assert!(matcher.matches(&Body::Json(json!({"tags": ["a", "b"]}))));
    }

    #[test]
    fn json_subset_parses_text() {
        let matcher = BodyMatcher::JsonSubset(json!({"status": "ok"}));
        assert!(matcher.matches(&Body::Text("{\"status\": \"ok\"}".into())));
        let bad = Body::Text("ok".into());
        assert!(!matcher.matches(&bad));
        assert_eq!(matcher.mismatch_note(&bad), Some("body is not JSON"));
        assert!(!BodyMatcher::JsonSubset(json!(null)).matches(&Body::Json(json!(null))));
        assert!(!BodyMatcher::JsonSubset(json!([1, 2])).matches(&Body::Json(json!([1, 2]))));
    }

    #[test]
    fn json_subset_null_requires_null() {
        let matcher = BodyMatcher::JsonSubset(json!({"err": null}));
        assert!(matcher.matches(&Body::Json(json!({"err": null}))));
        assert!(!matcher.matches(&Body::Json(json!({"err": 0}))));
    }

    #[test]
    fn header_null_and_missing() {
        assert!(HeaderMatcher::Exists.matches(Some(&Value::Null)));
        assert!(!HeaderMatcher::Exists.matches(None));
        assert!(HeaderMatcher::Equals(Value::Null).matches(Some(&Value::Null)));
        let regex = HeaderMatcher::Regex("^a$".into());
        assert!(!regex.matches(Some(&Value::Null)));
        assert_eq!(
            regex.mismatch_note(Some(&Value::Null)),
            Some("value is not a string")
        );
    }
}
