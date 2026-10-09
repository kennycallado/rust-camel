//! Bounded, faithful JSON serializer for the wrapper tree.
//!
//! Emits compact JSON from `JsonValue` in stored order: number tokens verbatim,
//! object keys in `IndexMap` order, raw UTF-8, `/` unescaped. The output bound is
//! the exact byte length, checked before every append so the result is either the
//! complete serialization or a redacted `Limit` error — never a truncated string.

use super::{JsonHostError, JsonKind, JsonValue};

/// Serialize `value` to compact JSON, never exceeding `max_size` output bytes.
///
/// `max_size == 0` means unlimited. The limit is checked before every append
/// against the exact accumulated byte length, so no partial string is returned.
pub(crate) fn to_json_string(value: &JsonValue, max_size: usize) -> Result<String, JsonHostError> {
    let mut out = String::new();
    write_value(&mut out, value, max_size)?;
    Ok(out)
}

/// Append `chunk` only when it keeps the output within `max_size` bytes.
fn push_bounded(out: &mut String, chunk: &str, max_size: usize) -> Result<(), JsonHostError> {
    if max_size != 0 && out.len() + chunk.len() > max_size {
        return Err(JsonHostError::Limit);
    }
    out.push_str(chunk);
    Ok(())
}

/// Append one `char`, charging its UTF-8 length against the bound.
fn push_char_bounded(out: &mut String, ch: char, max_size: usize) -> Result<(), JsonHostError> {
    if max_size != 0 && out.len() + ch.len_utf8() > max_size {
        return Err(JsonHostError::Limit);
    }
    out.push(ch);
    Ok(())
}

/// Write one value in compact form, dispatching on its kind.
fn write_value(out: &mut String, value: &JsonValue, max_size: usize) -> Result<(), JsonHostError> {
    match value.kind() {
        JsonKind::Null => push_bounded(out, "null", max_size),
        JsonKind::Bool(true) => push_bounded(out, "true", max_size),
        JsonKind::Bool(false) => push_bounded(out, "false", max_size),
        JsonKind::Number(number) => push_bounded(out, number.as_str(), max_size),
        JsonKind::String(text) => write_string(out, text, max_size),
        JsonKind::Array(items) => {
            push_bounded(out, "[", max_size)?;
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    push_bounded(out, ",", max_size)?;
                }
                write_value(out, item, max_size)?;
            }
            push_bounded(out, "]", max_size)
        }
        JsonKind::Object(entries) => {
            push_bounded(out, "{", max_size)?;
            for (index, (key, child)) in entries.iter().enumerate() {
                if index > 0 {
                    push_bounded(out, ",", max_size)?;
                }
                write_string(out, key, max_size)?;
                push_bounded(out, ":", max_size)?;
                write_value(out, child, max_size)?;
            }
            push_bounded(out, "}", max_size)
        }
    }
}

/// Write one JSON string: raw UTF-8, escaping only `"`, `\`, and controls.
fn write_string(out: &mut String, text: &str, max_size: usize) -> Result<(), JsonHostError> {
    push_bounded(out, "\"", max_size)?;
    for ch in text.chars() {
        match ch {
            '"' => push_bounded(out, "\\\"", max_size)?,
            '\\' => push_bounded(out, "\\\\", max_size)?,
            '\u{08}' => push_bounded(out, "\\b", max_size)?,
            '\u{0C}' => push_bounded(out, "\\f", max_size)?,
            '\n' => push_bounded(out, "\\n", max_size)?,
            '\r' => push_bounded(out, "\\r", max_size)?,
            '\t' => push_bounded(out, "\\t", max_size)?,
            control if (control as u32) < 0x20 => {
                push_control_escape(out, control as u32, max_size)?;
            }
            other => {
                let mut buf = [0u8; 4];
                push_bounded(out, other.encode_utf8(&mut buf), max_size)?;
            }
        }
    }
    push_bounded(out, "\"", max_size)
}

/// Write a `\u00XX` escape for a control code below `0x20`.
fn push_control_escape(out: &mut String, code: u32, max_size: usize) -> Result<(), JsonHostError> {
    const HEX: [char; 16] = [
        '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', 'a', 'b', 'c', 'd', 'e', 'f',
    ];
    push_bounded(out, "\\u00", max_size)?;
    push_char_bounded(out, HEX[((code >> 4) & 0xf) as usize], max_size)?;
    push_char_bounded(out, HEX[(code & 0xf) as usize], max_size)
}

#[cfg(test)]
mod tests {
    use super::to_json_string;
    use crate::json::parse::{JsonLimits, parse};
    use crate::json::{JsonHostError, JsonKind, JsonValue};
    use indexmap::IndexMap;

    fn unlimited() -> JsonLimits {
        JsonLimits {
            max_string_size: 0,
            max_array_size: 0,
            max_map_size: 0,
        }
    }

    fn obj(pairs: Vec<(&str, JsonValue)>) -> JsonValue {
        let entries: IndexMap<String, JsonValue> = pairs
            .into_iter()
            .map(|(key, value)| (key.to_string(), value))
            .collect();
        JsonValue::new(JsonKind::Object(entries))
    }

    fn arr(items: Vec<JsonValue>) -> JsonValue {
        JsonValue::new(JsonKind::Array(items))
    }

    #[test]
    fn serialize_authored_order() {
        let value = parse(r#"{"z":1,"a":2,"m":3}"#, &unlimited()).unwrap();
        assert_eq!(to_json_string(&value, 0).unwrap(), r#"{"z":1,"a":2,"m":3}"#);
    }

    #[test]
    fn serialize_duplicate_key_order_and_value() {
        let value = parse(r#"{"b":1,"a":2,"b":3}"#, &unlimited()).unwrap();
        assert_eq!(to_json_string(&value, 0).unwrap(), r#"{"b":3,"a":2}"#);
    }

    #[test]
    fn serialize_remove_keeps_remaining_order() {
        let mut value = parse(r#"{"a":1,"b":2,"c":3}"#, &unlimited()).unwrap();
        value
            .mutate(|kind| {
                let JsonKind::Object(entries) = kind else {
                    return Err(JsonHostError::TypeMismatch);
                };
                entries.shift_remove("a");
                Ok(())
            })
            .unwrap();
        assert_eq!(to_json_string(&value, 0).unwrap(), r#"{"b":2,"c":3}"#);
    }

    #[test]
    fn serialize_large_magnitude_exact() {
        let value = parse(
            "[18446744073709551615,123456789012345678901234567890,1e400]",
            &unlimited(),
        )
        .unwrap();
        assert_eq!(
            to_json_string(&value, 0).unwrap(),
            "[18446744073709551615,123456789012345678901234567890,1e400]"
        );
    }

    #[test]
    fn serialize_raw_utf8_no_slash_escape() {
        let value = obj(vec![(
            "café",
            JsonValue::new(JsonKind::String("a/b".to_string())),
        )]);
        let out = to_json_string(&value, 0).unwrap();
        assert!(out.contains("café"));
        assert!(out.contains("a/b"));
        assert!(!out.contains("\\/"));
    }

    #[test]
    fn serialize_control_chars_escaped() {
        let value = JsonValue::new(JsonKind::String("\u{1}".to_string()));
        let out = to_json_string(&value, 0).unwrap();
        assert!(out.contains("\\u0001"));
        assert!(!out.contains('\u{1}'));
    }

    #[test]
    fn serialize_escapes_quote_and_backslash() {
        let original = "\"\\";
        let value = JsonValue::new(JsonKind::String(original.to_string()));
        let out = to_json_string(&value, 0).unwrap();
        assert_eq!(out, "\"\\\"\\\\\"");
        assert_eq!(parse(&out, &unlimited()).unwrap(), value);
    }

    #[test]
    fn serialize_bounded_stops_with_limit() {
        let value = arr(vec![
            JsonValue::new(JsonKind::Null),
            JsonValue::new(JsonKind::Null),
        ]);
        assert_eq!(to_json_string(&value, 4), Err(JsonHostError::Limit));
    }
}
