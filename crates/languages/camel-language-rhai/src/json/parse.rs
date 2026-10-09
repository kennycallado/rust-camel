//! Strict RFC 8259 parser for the bounded JSON tree.
//!
//! Parsing descends one container level at a time using
//! `serde_json::value::RawValue` over an owned work stack (never recursion),
//! with an owned per-level depth budget. Scalars are validated explicitly so
//! exact number tokens survive and failures stay redacted.

use super::{JsonHostError, JsonKind, JsonNumber, JsonValue, MAX_JSON_DEPTH};
use indexmap::IndexMap;
use serde::de::{MapAccess, Visitor};
use serde_json::value::RawValue;
use std::fmt;

/// Sandbox bounds applied while parsing. `0` means unlimited for every cap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct JsonLimits {
    pub(crate) max_string_size: usize,
    pub(crate) max_array_size: usize,
    pub(crate) max_map_size: usize,
}

/// Parse `input` into a `JsonValue`, enforcing `limits`.
///
/// The raw input byte length is checked against `max_string_size` before any
/// descent. Descent then stops before the first container that would exceed
/// `MAX_JSON_DEPTH`, so parse cost stays bounded by the depth cap times the
/// input length. Every failure is a payload-free `JsonHostError`.
pub(crate) fn parse(input: &str, limits: &JsonLimits) -> Result<JsonValue, JsonHostError> {
    if limits.max_string_size != 0 && input.len() > limits.max_string_size {
        return Err(JsonHostError::Limit);
    }

    let root: Box<RawValue> = serde_json::from_str(input).map_err(|_| JsonHostError::Parse)?;

    // Explicit depth-first work stack. A `Finish` task always pops after its
    // children have pushed their values onto `results`, in child order.
    let mut work: Vec<Work> = vec![Work::Value(root, MAX_JSON_DEPTH)];
    let mut results: Vec<JsonValue> = Vec::new();

    while let Some(task) = work.pop() {
        match task {
            Work::Value(raw, depth_budget) => match first_byte(raw.get()) {
                b'{' => {
                    if depth_budget == 0 {
                        return Err(JsonHostError::Limit);
                    }
                    let RawEntries(entries): RawEntries =
                        serde_json::from_str(raw.get()).map_err(|_| JsonHostError::Parse)?;
                    let count = entries.len();
                    let (keys, values): (Vec<String>, Vec<Box<RawValue>>) =
                        entries.into_iter().unzip();
                    work.push(Work::FinishObject {
                        keys,
                        count,
                        max_size: limits.max_map_size,
                    });
                    for value in values.into_iter().rev() {
                        work.push(Work::Value(value, depth_budget - 1));
                    }
                }
                b'[' => {
                    if depth_budget == 0 {
                        return Err(JsonHostError::Limit);
                    }
                    let items: Vec<Box<RawValue>> =
                        serde_json::from_str(raw.get()).map_err(|_| JsonHostError::Parse)?;
                    if limits.max_array_size != 0 && items.len() > limits.max_array_size {
                        return Err(JsonHostError::Limit);
                    }
                    let count = items.len();
                    work.push(Work::FinishArray { count });
                    for item in items.into_iter().rev() {
                        work.push(Work::Value(item, depth_budget - 1));
                    }
                }
                _ => results.push(parse_scalar(raw.get())?),
            },
            Work::FinishArray { count } => {
                let split = results
                    .len()
                    .checked_sub(count)
                    .ok_or(JsonHostError::Parse)?;
                let items = results.split_off(split);
                results.push(JsonValue::new(JsonKind::Array(items)));
            }
            Work::FinishObject {
                keys,
                count,
                max_size,
            } => {
                let split = results
                    .len()
                    .checked_sub(count)
                    .ok_or(JsonHostError::Parse)?;
                let values = results.split_off(split);
                let entries: IndexMap<String, JsonValue> = keys.into_iter().zip(values).collect();
                if max_size != 0 && entries.len() > max_size {
                    return Err(JsonHostError::Limit);
                }
                results.push(JsonValue::new(JsonKind::Object(entries)));
            }
        }
    }

    results.pop().ok_or(JsonHostError::Parse)
}

/// One step of the explicit traversal.
enum Work {
    /// Process a captured raw value with the remaining container depth budget.
    Value(Box<RawValue>, usize),
    /// Collect the last `count` results into an array.
    FinishArray { count: usize },
    /// Collect the last `count` results into an object keyed by `keys`, then
    /// enforce the distinct-key map cap (`max_size`, `0` = unlimited).
    FinishObject {
        keys: Vec<String>,
        count: usize,
        max_size: usize,
    },
}

/// Object entries retaining EVERY raw key/value occurrence in authored order.
///
/// `IndexMap`'s own `Deserialize` silently replaces a duplicate key, dropping
/// the earlier raw value before it is validated. This visitor keeps every
/// occurrence so each value still passes scalar and depth validation; the
/// parser dedups (first position, last value) only after all values validate.
struct RawEntries(Vec<(String, Box<RawValue>)>);

impl<'de> serde::Deserialize<'de> for RawEntries {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct RawEntriesVisitor;

        impl<'de> Visitor<'de> for RawEntriesVisitor {
            type Value = RawEntries;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a JSON object")
            }

            fn visit_map<A>(self, mut access: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut entries = Vec::with_capacity(access.size_hint().unwrap_or(0));
                while let Some((key, value)) = access.next_entry::<String, Box<RawValue>>()? {
                    entries.push((key, value));
                }
                Ok(RawEntries(entries))
            }
        }

        deserializer.deserialize_map(RawEntriesVisitor)
    }
}

/// First non-whitespace byte, used to pick the container arms.
fn first_byte(raw: &str) -> u8 {
    raw.trim_start().as_bytes().first().copied().unwrap_or(b' ')
}

/// Validate and convert one captured scalar token.
fn parse_scalar(raw: &str) -> Result<JsonValue, JsonHostError> {
    let token = raw.trim();
    match token.as_bytes().first() {
        Some(b'"') => {
            let text: String = serde_json::from_str(token).map_err(|_| JsonHostError::Parse)?;
            Ok(JsonValue::new(JsonKind::String(text)))
        }
        Some(b'-') | Some(b'0'..=b'9') => {
            if validate_number_token(token) {
                Ok(JsonValue::new(JsonKind::Number(JsonNumber::new(token))))
            } else {
                Err(JsonHostError::Parse)
            }
        }
        Some(b't') if token == "true" => Ok(JsonValue::new(JsonKind::Bool(true))),
        Some(b'f') if token == "false" => Ok(JsonValue::new(JsonKind::Bool(false))),
        Some(b'n') if token == "null" => Ok(JsonValue::new(JsonKind::Null)),
        _ => Err(JsonHostError::Parse),
    }
}

/// Strict number-token check: `-?(0|[1-9][0-9]*)(\.[0-9]+)?([eE][+-]?[0-9]+)?`.
///
/// A hand-written check is required because `serde_json::Number` rejects
/// magnitudes such as `1e400` that RFC 8259 admits as a token.
fn validate_number_token(token: &str) -> bool {
    let bytes = token.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    if i < len && bytes[i] == b'-' {
        i += 1;
    }
    if i >= len {
        return false;
    }
    if bytes[i] == b'0' {
        i += 1;
    } else if bytes[i].is_ascii_digit() {
        i += 1;
        while i < len && bytes[i].is_ascii_digit() {
            i += 1;
        }
    } else {
        return false;
    }

    if i < len && bytes[i] == b'.' {
        i += 1;
        let start = i;
        while i < len && bytes[i].is_ascii_digit() {
            i += 1;
        }
        if i == start {
            return false;
        }
    }

    if i < len && (bytes[i] == b'e' || bytes[i] == b'E') {
        i += 1;
        if i < len && (bytes[i] == b'+' || bytes[i] == b'-') {
            i += 1;
        }
        let start = i;
        while i < len && bytes[i].is_ascii_digit() {
            i += 1;
        }
        if i == start {
            return false;
        }
    }

    i == len
}

#[cfg(test)]
mod tests {
    use super::{JsonLimits, parse};
    use crate::json::{JsonHostError, JsonKind, JsonNumber, JsonValue, project_dynamic};

    fn unlimited() -> JsonLimits {
        JsonLimits {
            max_string_size: 0,
            max_array_size: 0,
            max_map_size: 0,
        }
    }

    fn object_keys(value: &JsonValue) -> Vec<String> {
        match value.kind() {
            JsonKind::Object(entries) => entries.keys().cloned().collect(),
            _ => panic!("expected object"),
        }
    }

    fn array_items(value: &JsonValue) -> &[JsonValue] {
        match value.kind() {
            JsonKind::Array(items) => items,
            _ => panic!("expected array"),
        }
    }

    fn number_token(value: &JsonValue) -> &str {
        match value.kind() {
            JsonKind::Number(number) => number.as_str(),
            _ => panic!("expected number"),
        }
    }

    #[test]
    fn depth_counts_scalars_zero_containers_one() {
        assert_eq!(parse("1", &unlimited()).unwrap().depth(), 0);
        assert_eq!(parse("[1]", &unlimited()).unwrap().depth(), 1);
        assert_eq!(parse("[[1]]", &unlimited()).unwrap().depth(), 2);
    }

    #[test]
    fn parse_escaped_solidus() {
        let value = parse(r#"{"path":"a\/b"}"#, &unlimited()).unwrap();
        match value.kind() {
            JsonKind::Object(entries) => match entries.get("path").map(JsonValue::kind) {
                Some(JsonKind::String(text)) => assert_eq!(text, "a/b"),
                other => panic!("unexpected path value: {other:?}"),
            },
            _ => panic!("expected object"),
        }
    }

    #[test]
    fn parse_unpaired_surrogate_refused() {
        assert_eq!(
            parse(r#""\uD800""#, &unlimited()),
            Err(JsonHostError::Parse)
        );
    }

    #[test]
    fn parse_integer_forms_project_decimals_wrap() {
        let value = parse(
            "[9223372036854775807,-0,1.0,1e2,18446744073709551615]",
            &unlimited(),
        )
        .unwrap();
        let items = array_items(&value);
        assert!(project_dynamic(&items[0]).is::<i64>());
        assert!(project_dynamic(&items[1]).is::<i64>());
        assert!(project_dynamic(&items[2]).is::<JsonNumber>());
        assert!(project_dynamic(&items[3]).is::<JsonNumber>());
        assert!(project_dynamic(&items[4]).is::<JsonNumber>());
    }

    #[test]
    fn parse_large_magnitude_exact_tokens() {
        let value = parse(
            "[18446744073709551615,123456789012345678901234567890,1e400]",
            &unlimited(),
        )
        .unwrap();
        let items = array_items(&value);
        assert_eq!(number_token(&items[0]), "18446744073709551615");
        assert_eq!(number_token(&items[1]), "123456789012345678901234567890");
        assert_eq!(number_token(&items[2]), "1e400");
        // The same tokens also parse at the root (scalar path).
        assert_eq!(
            number_token(&parse("18446744073709551615", &unlimited()).unwrap()),
            "18446744073709551615"
        );
        assert_eq!(
            number_token(&parse("123456789012345678901234567890", &unlimited()).unwrap()),
            "123456789012345678901234567890"
        );
        assert_eq!(
            number_token(&parse("1e400", &unlimited()).unwrap()),
            "1e400"
        );
    }

    #[test]
    fn parse_authored_order_retained() {
        let value = parse(r#"{"z":1,"a":2,"m":3}"#, &unlimited()).unwrap();
        assert_eq!(object_keys(&value), vec!["z", "a", "m"]);
    }

    #[test]
    fn parse_duplicate_key_first_position_last_value() {
        let value = parse(r#"{"b":1,"a":2,"b":3}"#, &unlimited()).unwrap();
        assert_eq!(object_keys(&value), vec!["b", "a"]);
        match value.kind() {
            JsonKind::Object(entries) => {
                assert_eq!(entries.get("b").map(number_token), Some("3"));
            }
            _ => panic!("expected object"),
        }
    }

    #[test]
    fn parse_overwritten_surrogates_refused() {
        let limits = unlimited();
        assert_eq!(
            parse(r#"{"a":"\uD800","a":1}"#, &limits),
            Err(JsonHostError::Parse)
        );
        assert_eq!(
            parse(r#"{"a":"\uDC00","a":1}"#, &limits),
            Err(JsonHostError::Parse)
        );
    }

    #[test]
    fn parse_overwritten_depth_limit() {
        let limits = unlimited();
        let deep = format!("{}{}", "[".repeat(128), "]".repeat(128));
        let refused = format!(r#"{{"a":{deep},"a":1}}"#);
        assert_eq!(parse(&refused, &limits), Err(JsonHostError::Limit));

        let shallow = format!("{}{}", "[".repeat(127), "]".repeat(127));
        let accepted = format!(r#"{{"a":{shallow},"a":1}}"#);
        let value = parse(&accepted, &limits).unwrap();
        match value.kind() {
            JsonKind::Object(entries) => {
                assert_eq!(entries.get("a").map(number_token), Some("1"));
            }
            _ => panic!("expected object"),
        }
    }

    #[test]
    fn parse_duplicate_map_cap_counts_stored_keys() {
        let limits = JsonLimits {
            max_map_size: 1,
            ..unlimited()
        };
        let value = parse(r#"{"a":1,"a":2}"#, &limits).unwrap();
        assert_eq!(object_keys(&value), vec!["a"]);
        match value.kind() {
            JsonKind::Object(entries) => {
                assert_eq!(entries.get("a").map(number_token), Some("2"));
            }
            _ => panic!("expected object"),
        }
        assert_eq!(
            parse(r#"{"a":1,"b":2}"#, &limits),
            Err(JsonHostError::Limit)
        );
    }

    #[test]
    fn parse_depth_128_accepted_129_limit() {
        let accepted = format!("{}{}", "[".repeat(128), "]".repeat(128));
        assert!(parse(&accepted, &unlimited()).is_ok());
        let refused = format!("{}{}", "[".repeat(129), "]".repeat(129));
        assert_eq!(parse(&refused, &unlimited()), Err(JsonHostError::Limit));
    }

    #[test]
    fn parse_max_string_size_before_descent() {
        let limits = JsonLimits {
            max_string_size: 4,
            ..unlimited()
        };
        assert_eq!(parse("[1,2,3]", &limits), Err(JsonHostError::Limit));
    }

    #[test]
    fn parse_array_and_map_caps_limit() {
        let array_limits = JsonLimits {
            max_array_size: 2,
            ..unlimited()
        };
        assert_eq!(parse("[1,2,3]", &array_limits), Err(JsonHostError::Limit));

        let map_limits = JsonLimits {
            max_map_size: 1,
            ..unlimited()
        };
        assert_eq!(
            parse(r#"{"a":1,"b":2}"#, &map_limits),
            Err(JsonHostError::Limit)
        );
    }

    #[test]
    fn parse_zero_caps_are_unlimited() {
        let limits = unlimited();
        assert!(parse("[1,2,3]", &limits).is_ok());
        let accepted = format!("{}{}", "[".repeat(128), "]".repeat(128));
        assert!(parse(&accepted, &limits).is_ok());
        let refused = format!("{}{}", "[".repeat(129), "]".repeat(129));
        assert_eq!(parse(&refused, &limits), Err(JsonHostError::Limit));
    }

    #[test]
    fn parse_malformed_redacted() {
        let err = parse("secret-token-xyz", &unlimited()).unwrap_err();
        assert_eq!(err, JsonHostError::Parse);
        let rendered = format!("{err:?}").to_lowercase();
        assert!(!rendered.contains("secret-token-xyz"));
        assert!(!rendered.contains("line"));
        assert!(!rendered.contains("column"));
    }
}
