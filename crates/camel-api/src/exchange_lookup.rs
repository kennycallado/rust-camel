//! Exchange-scoped lookup path grammar shared by SQL `:#` placeholders and
//! Simple `${...}` interpolation. See ADR-0016 (strict rejection) and the
//! rc-o6o Phase 2 spec §3.2.
//!
//! Grammar (e_gpt decision Q2):
//! - `body.a.b.c`        → `Body([Key("a"), Key("b"), Key("c")])` — walks JSON.
//! - `body.items.0`      → `Body([Key("items"), Index(0)])`        — array index.
//! - `header.some.name`  → `Header("some.name")`                   — flat key.
//! - `property.x`        → `Property("x")`                         — flat key.
//! - `exchangeProperty.x`→ `Property("x")`                         — alias.
//! - `foo`               → `Unscoped("foo")`                       — try body JSON key, then header, then property.

use crate::{Body, Exchange, Value};

/// A single segment in a body JSON path.
///
/// Lives in `camel-api` so both SQL (`ExchangeLookupPath`) and Simple
/// (`Expr::BodyField`) share the same segment type without cyclic deps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathSegment {
    /// Named object field: `"name"`, `"user"`.
    Key(String),
    /// Array index: `0`, `1`. Leading-zero strings (`"01"`) are NOT indexes —
    /// they are `Key("01")` — matching the existing Simple language rule.
    Index(usize),
}

/// A parsed Exchange lookup path. See module docs for the grammar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExchangeLookupPath {
    /// `body.a.b.c` — walk JSON tree via path segments. Empty vec means
    /// "whole body" (corresponds to Simple `${body}`).
    Body(Vec<PathSegment>),
    /// `header.some.name` — flat key `"some.name"` (headers are flat maps).
    Header(String),
    /// `property.some.name` / `exchangeProperty.some.name` — flat key.
    Property(String),
    /// `foo` — unscoped: try body JSON key (flat), then header (flat), then
    /// property (flat). The full token is the key in each scope.
    Unscoped(String),
}

/// Error raised by [`ExchangeLookupPath::parse`]. Aligns with ADR-0016 strict
/// rejection: ambiguous / malformed paths are reported, never silently coerced.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LookupPathError {
    /// The whole input was empty.
    #[error("empty lookup path")]
    Empty,
    /// A path segment between dots was empty (e.g. `body..name`).
    #[error("empty path segment in {input:?}")]
    EmptySegment { input: String },
    /// The path ended with a trailing dot (e.g. `body.`).
    #[error("trailing dot in {input:?}")]
    TrailingDot { input: String },
    /// The scope prefix was given but no key followed (e.g. `header.`).
    #[error("scope prefix {scope:?} requires a non-empty key")]
    EmptyScopedKey { scope: String, input: String },
}

impl ExchangeLookupPath {
    pub fn parse(s: &str) -> Result<Self, LookupPathError> {
        if s.is_empty() {
            return Err(LookupPathError::Empty);
        }

        // Scope detection: split on the FIRST dot only. The remainder stays
        // verbatim (header keys and property keys may contain dots themselves;
        // only `body.` walks segments).
        let (head, rest_opt) = match s.split_once('.') {
            Some((h, r)) => (h, Some(r)),
            None => (s, None),
        };

        match head {
            "body" => {
                let Some(rest) = rest_opt else {
                    // `body` alone → whole body.
                    return Ok(ExchangeLookupPath::Body(Vec::new()));
                };
                if rest.is_empty() {
                    return Err(LookupPathError::TrailingDot { input: s.into() });
                }
                let segments = parse_body_segments(rest, s)?;
                Ok(ExchangeLookupPath::Body(segments))
            }
            "header" => {
                let Some(rest) = rest_opt else {
                    // `header` with no key is invalid (a header lookup needs a key).
                    return Err(LookupPathError::EmptyScopedKey {
                        scope: "header".into(),
                        input: s.into(),
                    });
                };
                if rest.is_empty() {
                    return Err(LookupPathError::EmptyScopedKey {
                        scope: "header".into(),
                        input: s.into(),
                    });
                }
                Ok(ExchangeLookupPath::Header(rest.into()))
            }
            "property" | "exchangeProperty" => {
                let scope = head;
                let Some(rest) = rest_opt else {
                    return Err(LookupPathError::EmptyScopedKey {
                        scope: scope.into(),
                        input: s.into(),
                    });
                };
                if rest.is_empty() {
                    return Err(LookupPathError::EmptyScopedKey {
                        scope: scope.into(),
                        input: s.into(),
                    });
                }
                Ok(ExchangeLookupPath::Property(rest.into()))
            }
            _ => {
                // No reserved prefix → unscoped. The full token is the flat key
                // tried against body / header / property in that order.
                Ok(ExchangeLookupPath::Unscoped(s.into()))
            }
        }
    }

    /// Resolve this path against an Exchange. Returns `None` when the path
    /// does not match (caller decides whether that is an error).
    pub fn lookup(&self, exchange: &Exchange) -> Option<Value> {
        match self {
            ExchangeLookupPath::Body(segments) => lookup_body(exchange, segments),
            ExchangeLookupPath::Header(key) => exchange.input.header(key).cloned(),
            ExchangeLookupPath::Property(key) => exchange.property(key).cloned(),
            ExchangeLookupPath::Unscoped(token) => {
                // 1. Body JSON object flat key.
                if let Some(value) = body_json_object(exchange).and_then(|obj| obj.get(token)) {
                    return Some(value.clone());
                }
                // 2. Header flat key.
                if let Some(value) = exchange.input.header(token) {
                    return Some(value.clone());
                }
                // 3. Property flat key.
                exchange.property(token).cloned()
            }
        }
    }
}

fn body_json_object(exchange: &Exchange) -> Option<&serde_json::Map<String, Value>> {
    match &exchange.input.body {
        Body::Json(value) => value.as_object(),
        _ => None,
    }
}

fn lookup_body(exchange: &Exchange, segments: &[PathSegment]) -> Option<Value> {
    let Body::Json(value) = &exchange.input.body else {
        return None;
    };
    if segments.is_empty() {
        // `${body}` — whole body value.
        return Some(value.clone());
    }
    let mut current = value;
    for seg in segments {
        current = match seg {
            PathSegment::Key(k) => current.as_object().and_then(|obj| obj.get(k))?,
            PathSegment::Index(i) => current.as_array().and_then(|arr| arr.get(*i))?,
        };
    }
    Some(current.clone())
}

/// Parse the dotted segment list AFTER `body.`. Each segment is either a
/// `Key(string)` or, when it parses as `usize` with no leading zero (except
/// "0" itself), an `Index(n)`. Mirrors the existing Simple language rule so
/// Simple's `parse_body_path` regression tests pass unchanged.
fn parse_body_segments(path: &str, full_input: &str) -> Result<Vec<PathSegment>, LookupPathError> {
    let mut segments = Vec::new();
    for seg in path.split('.') {
        if seg.is_empty() {
            return Err(LookupPathError::EmptySegment {
                input: full_input.into(),
            });
        }
        let is_index = seg.parse::<usize>().is_ok() && (seg == "0" || !seg.starts_with('0'));
        if is_index {
            // seg already validated as a usize above.
            segments.push(PathSegment::Index(seg.parse::<usize>().expect(
                "invariant: seg is usize-parseable because is_index checked it",
            ))); // allow-unwrap
        } else {
            segments.push(PathSegment::Key(seg.into()));
        }
    }
    Ok(segments)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_unscoped_token() {
        assert_eq!(
            ExchangeLookupPath::parse("my-param"),
            Ok(ExchangeLookupPath::Unscoped("my-param".into()))
        );
        assert_eq!(
            ExchangeLookupPath::parse("foo.bar"),
            Ok(ExchangeLookupPath::Unscoped("foo.bar".into()))
        );
    }

    #[test]
    fn parse_body_scope_walks_segments() {
        assert_eq!(
            ExchangeLookupPath::parse("body.user.address.city"),
            Ok(ExchangeLookupPath::Body(vec![
                PathSegment::Key("user".into()),
                PathSegment::Key("address".into()),
                PathSegment::Key("city".into()),
            ]))
        );
    }

    #[test]
    fn parse_body_scope_with_numeric_index() {
        assert_eq!(
            ExchangeLookupPath::parse("body.items.0"),
            Ok(ExchangeLookupPath::Body(vec![
                PathSegment::Key("items".into()),
                PathSegment::Index(0),
            ]))
        );
    }

    #[test]
    fn parse_body_scope_leading_zero_is_key_not_index() {
        // Matches existing Simple language rule: "01" parses as Key("01"), not Index(1).
        assert_eq!(
            ExchangeLookupPath::parse("body.01"),
            Ok(ExchangeLookupPath::Body(vec![PathSegment::Key(
                "01".into()
            )]))
        );
    }

    #[test]
    fn parse_body_scope_bare_is_empty_segments() {
        // `${body}` in Simple means "whole body". Represent as Body(vec![]).
        assert_eq!(
            ExchangeLookupPath::parse("body"),
            Ok(ExchangeLookupPath::Body(vec![]))
        );
    }

    #[test]
    fn parse_header_scope_flat_key() {
        assert_eq!(
            ExchangeLookupPath::parse("header.some.name"),
            Ok(ExchangeLookupPath::Header("some.name".into()))
        );
    }

    #[test]
    fn parse_property_scope_flat_key() {
        assert_eq!(
            ExchangeLookupPath::parse("property.some.name"),
            Ok(ExchangeLookupPath::Property("some.name".into()))
        );
    }

    #[test]
    fn parse_exchange_property_alias() {
        assert_eq!(
            ExchangeLookupPath::parse("exchangeProperty.myKey"),
            Ok(ExchangeLookupPath::Property("myKey".into()))
        );
    }

    #[test]
    fn parse_rejects_empty_input() {
        assert_eq!(ExchangeLookupPath::parse(""), Err(LookupPathError::Empty));
    }

    #[test]
    fn parse_rejects_trailing_dot() {
        let err = ExchangeLookupPath::parse("body.").unwrap_err();
        assert!(
            matches!(err, LookupPathError::TrailingDot { .. }),
            "{err:?}"
        );
    }

    #[test]
    fn parse_rejects_empty_segment_in_body_path() {
        let err = ExchangeLookupPath::parse("body..name").unwrap_err();
        assert!(
            matches!(err, LookupPathError::EmptySegment { .. }),
            "{err:?}"
        );
    }

    #[test]
    fn parse_rejects_empty_scoped_key_for_header() {
        // `header.` with nothing after is a trailing dot but reported as
        // EmptyScopedKey because the scope prefix was explicit.
        let err = ExchangeLookupPath::parse("header.").unwrap_err();
        assert!(
            matches!(err, LookupPathError::EmptyScopedKey { .. }),
            "{err:?}"
        );
    }

    #[test]
    fn lookup_walks_nested_body_json() {
        use crate::{Body, Exchange, Message};
        let msg = Message::new(Body::Json(serde_json::json!({
            "user": { "address": { "city": "Berlin" } }
        })));
        let ex = Exchange::new(msg);

        let path = ExchangeLookupPath::parse("body.user.address.city").unwrap();
        assert_eq!(path.lookup(&ex), Some(serde_json::json!("Berlin")));
    }

    #[test]
    fn lookup_walks_body_array_index() {
        use crate::{Body, Exchange, Message};
        let msg = Message::new(Body::Json(serde_json::json!({
            "items": [10, 20, 30]
        })));
        let ex = Exchange::new(msg);

        let path = ExchangeLookupPath::parse("body.items.1").unwrap();
        assert_eq!(path.lookup(&ex), Some(serde_json::json!(20)));
    }

    #[test]
    fn lookup_body_whole_returns_full_body_value() {
        use crate::{Body, Exchange, Message};
        let msg = Message::new(Body::Json(serde_json::json!({"a": 1})));
        let ex = Exchange::new(msg);

        let path = ExchangeLookupPath::parse("body").unwrap();
        assert_eq!(path.lookup(&ex), Some(serde_json::json!({"a": 1})));
    }

    #[test]
    fn lookup_body_returns_none_when_not_json() {
        use crate::{Body, Exchange, Message};
        let msg = Message::new(Body::Text("hello".into()));
        let ex = Exchange::new(msg);

        let path = ExchangeLookupPath::parse("body.user").unwrap();
        assert_eq!(path.lookup(&ex), None);
    }

    #[test]
    fn lookup_body_returns_none_when_path_misses() {
        use crate::{Body, Exchange, Message};
        let msg = Message::new(Body::Json(serde_json::json!({"a": 1})));
        let ex = Exchange::new(msg);

        let path = ExchangeLookupPath::parse("body.b.c").unwrap();
        assert_eq!(path.lookup(&ex), None);
    }

    #[test]
    fn lookup_header_flat_dotted_key() {
        use crate::{Exchange, Message};
        let mut msg = Message::default();
        msg.set_header("some.name", serde_json::json!(42));
        let ex = Exchange::new(msg);

        let path = ExchangeLookupPath::parse("header.some.name").unwrap();
        assert_eq!(path.lookup(&ex), Some(serde_json::json!(42)));
    }

    #[test]
    fn lookup_property_flat_dotted_key() {
        use crate::{Exchange, Message};
        let mut ex = Exchange::new(Message::default());
        ex.set_property("config.key", serde_json::json!("v"));

        let path = ExchangeLookupPath::parse("property.config.key").unwrap();
        assert_eq!(path.lookup(&ex), Some(serde_json::json!("v")));
    }

    #[test]
    fn lookup_unscoped_fallback_body_then_header_then_property() {
        use crate::{Body, Exchange, Message};
        // Body wins over header.
        let mut msg = Message::new(Body::Json(serde_json::json!({"id": 1})));
        msg.set_header("id", serde_json::json!(2));
        let ex = Exchange::new(msg);
        let path = ExchangeLookupPath::parse("id").unwrap();
        assert_eq!(path.lookup(&ex), Some(serde_json::json!(1)));
    }

    #[test]
    fn lookup_unscoped_falls_through_to_header() {
        use crate::{Exchange, Message};
        let mut msg = Message::default();
        msg.set_header("token", serde_json::json!("abc"));
        let ex = Exchange::new(msg);
        let path = ExchangeLookupPath::parse("token").unwrap();
        assert_eq!(path.lookup(&ex), Some(serde_json::json!("abc")));
    }

    #[test]
    fn lookup_unscoped_falls_through_to_property() {
        use crate::{Exchange, Message};
        let mut ex = Exchange::new(Message::default());
        ex.set_property("tenant", serde_json::json!("acme"));
        let path = ExchangeLookupPath::parse("tenant").unwrap();
        assert_eq!(path.lookup(&ex), Some(serde_json::json!("acme")));
    }

    #[test]
    fn lookup_unscoped_returns_none_when_missing_everywhere() {
        use crate::{Exchange, Message};
        let ex = Exchange::new(Message::default());
        let path = ExchangeLookupPath::parse("nope").unwrap();
        assert_eq!(path.lookup(&ex), None);
    }
}
