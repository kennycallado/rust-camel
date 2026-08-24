//! Assertion matcher vocabulary for the mock testkit.
//!
//! [`BodyMatcher`] and [`HeaderMatcher`] describe expected received bodies
//! and header values. They are assertion-side only: they never change
//! producer behavior (the producer stays a sink per the component identity
//! ruling).

use std::fmt;

use camel_component_api::Body;
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
            BodyMatcher::Regex(pattern) => match actual {
                Body::Text(text) => compile(pattern).is_some_and(|re| re.is_match(text)),
                _ => false,
            },
            BodyMatcher::Contains(needle) => match actual {
                Body::Text(text) => text.contains(needle),
                _ => false,
            },
            BodyMatcher::StartsWith(prefix) => match actual {
                Body::Text(text) => text.starts_with(prefix),
                _ => false,
            },
            BodyMatcher::EndsWith(suffix) => match actual {
                Body::Text(text) => text.ends_with(suffix),
                _ => false,
            },
            BodyMatcher::Exists => !matches!(actual, Body::Empty),
            BodyMatcher::JsonSubset(pattern) => {
                pattern.is_object()
                    && json_value(actual).is_some_and(|received| json_subset(pattern, &received))
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

/// Extract the JSON value from a body, if it is JSON or parseable text.
fn json_value(body: &Body) -> Option<Value> {
    match body {
        Body::Json(v) => Some(v.clone()),
        Body::Text(text) => serde_json::from_str(text).ok(),
        _ => None,
    }
}

/// Recursive JSON subset match: every pattern key must exist in `received`
/// with a value that is JSON-equal or, for objects, a recursive subset.
fn json_subset(pattern: &Value, received: &Value) -> bool {
    match (pattern, received) {
        (Value::Object(p), Value::Object(r)) => p
            .iter()
            .all(|(key, pv)| r.get(key).is_some_and(|rv| json_subset(pv, rv))),
        (Value::Array(p), Value::Array(r)) => {
            p.len() == r.len() && p.iter().zip(r.iter()).all(|(a, b)| a == b)
        }
        _ => pattern == received,
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
