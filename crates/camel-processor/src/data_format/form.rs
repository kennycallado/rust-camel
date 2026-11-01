use camel_api::body::Body;
use camel_api::data_format::DataFormat;
use camel_api::error::CamelError;
use serde::Deserialize;
use serde_json::Map;
use serde_json::Value;
use std::str;

const DEFAULT_MAX_FIELDS: usize = 1_000;
const DEFAULT_MAX_BYTES: usize = 1_048_576;

/// Configuration for [`FormDataFormat`]. Both caps are on by default.
#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FormConfig {
    /// Maximum number of non-empty pairs accepted by `unmarshal`, duplicates
    /// included. Empty `&` segments do not count. Default 1_000.
    /// `None` disables the cap (not recommended for untrusted input).
    pub max_fields: Option<usize>,
    /// Maximum input length in bytes accepted by `unmarshal`. Default 1 MiB.
    /// `None` disables the cap.
    pub max_bytes: Option<usize>,
}

impl Default for FormConfig {
    fn default() -> Self {
        Self {
            max_fields: Some(DEFAULT_MAX_FIELDS),
            max_bytes: Some(DEFAULT_MAX_BYTES),
        }
    }
}

/// Built-in `application/x-www-form-urlencoded` data format.
#[derive(Clone, Debug, Default)]
pub struct FormDataFormat {
    cfg: FormConfig,
}

impl FormDataFormat {
    /// Creates a form data format with the given [`FormConfig`].
    pub fn new(cfg: FormConfig) -> Self {
        Self { cfg }
    }

    fn pairs_to_body(pairs: Vec<(String, String)>) -> Body {
        let mut map = Map::new();
        for (key, value) in pairs {
            match map.get_mut(&key) {
                Some(Value::Array(values)) => values.push(Value::String(value)),
                Some(existing) => {
                    let first = existing.take();
                    *existing = Value::Array(vec![first, Value::String(value)]);
                }
                None => {
                    map.insert(key, Value::String(value));
                }
            }
        }
        Body::Json(Value::Object(map))
    }
}

/// Parses an `application/x-www-form-urlencoded` payload into pairs in
/// encounter order. Strictly validates `%XX` escapes and UTF-8; never lossy.
pub(crate) fn parse_form_pairs(
    input: &[u8],
    cfg: &FormConfig,
) -> Result<Vec<(String, String)>, CamelError> {
    if let Some(max_bytes) = cfg.max_bytes
        && input.len() > max_bytes
    {
        return Err(CamelError::TypeConversionFailed(format!(
            "form unmarshal input {} bytes exceeds max_bytes {max_bytes}",
            input.len()
        )));
    }

    let mut pairs = Vec::new();
    let mut offset = 0usize;
    for segment in input.split(|&b| b == b'&') {
        let segment_offset = offset;
        offset += segment.len() + 1;
        if segment.is_empty() {
            continue;
        }
        if let Some(max_fields) = cfg.max_fields
            && pairs.len() >= max_fields
        {
            return Err(CamelError::TypeConversionFailed(format!(
                "form unmarshal exceeds max_fields {max_fields}"
            )));
        }
        let (raw_key, raw_value, value_offset) = match segment.iter().position(|&b| b == b'=') {
            Some(eq) => (&segment[..eq], &segment[eq + 1..], segment_offset + eq + 1),
            None => (segment, &[][..], segment_offset + segment.len()),
        };
        let key = decode_component(raw_key, segment_offset)?;
        let value = decode_component(raw_value, value_offset)?;
        pairs.push((key, value));
    }
    Ok(pairs)
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn decode_component(raw: &[u8], base_offset: usize) -> Result<String, CamelError> {
    let mut decoded = Vec::with_capacity(raw.len());
    let mut i = 0usize;
    while i < raw.len() {
        match raw[i] {
            b'+' => {
                decoded.push(b' ');
                i += 1;
            }
            b'%' => {
                let escape_offset = base_offset + i;
                let end = (i + 3).min(raw.len());
                let escape = String::from_utf8_lossy(&raw[i..end]);
                let (Some(high), Some(low)) = (
                    raw.get(i + 1).copied().and_then(hex_nibble),
                    raw.get(i + 2).copied().and_then(hex_nibble),
                ) else {
                    return Err(CamelError::TypeConversionFailed(format!(
                        "form malformed percent escape '{escape}' at byte offset {escape_offset}"
                    )));
                };
                decoded.push(high * 16 + low);
                i += 3;
            }
            byte => {
                decoded.push(byte);
                i += 1;
            }
        }
    }
    str::from_utf8(&decoded).map(str::to_owned).map_err(|err| {
        CamelError::TypeConversionFailed(format!(
            "form decoded component is not valid UTF-8 at byte offset {base_offset}: {err}"
        ))
    })
}

/// WHATWG `application/x-www-form-urlencoded` serializer for one component.
pub(crate) fn encode_form_component(s: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(s.len());
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'*' | b'-' | b'.' | b'_' => {
                out.push(char::from(byte));
            }
            b' ' => out.push('+'),
            _ => {
                out.push('%');
                out.push(char::from(HEX[usize::from(byte >> 4)]));
                out.push(char::from(HEX[usize::from(byte & 0x0F)]));
            }
        }
    }
    out
}

fn push_form_pair(out: &mut String, key: &str, value: &str) {
    if !out.is_empty() {
        out.push('&');
    }
    out.push_str(&encode_form_component(key));
    out.push('=');
    out.push_str(&encode_form_component(value));
}

fn scalar_to_form_string(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.clone()),
        Value::Number(_) | Value::Bool(_) | Value::Null => Some(value.to_string()),
        Value::Array(_) | Value::Object(_) => None,
    }
}

fn json_shape(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

impl DataFormat for FormDataFormat {
    fn name(&self) -> &str {
        "form"
    }

    fn unmarshal(&self, body: Body) -> Result<Body, CamelError> {
        match body {
            Body::Text(s) => Ok(Self::pairs_to_body(parse_form_pairs(
                s.as_bytes(),
                &self.cfg,
            )?)),
            Body::Bytes(b) => Ok(Self::pairs_to_body(parse_form_pairs(&b, &self.cfg)?)),
            Body::Empty => Ok(Body::Empty),
            Body::Stream(_) => Err(CamelError::TypeConversionFailed(
                "form cannot unmarshal Body::Stream directly — use UnmarshalService which auto-materializes"
                    .to_string(),
            )),
            _ => Err(CamelError::TypeConversionFailed(
                "form: FormDataFormat::unmarshal only supports Body::Text, Body::Bytes, and Body::Empty"
                    .to_string(),
            )),
        }
    }

    fn marshal(&self, body: Body) -> Result<Body, CamelError> {
        match body {
            Body::Empty => Ok(Body::Empty),
            Body::Stream(_) => Err(CamelError::TypeConversionFailed(
                "form: cannot marshal Body::Stream — add 'stream_cache' or 'convert_body_to' before this step"
                    .to_string(),
            )),
            Body::Json(Value::Object(map)) => {
                let mut encoded = String::new();
                for (key, value) in &map {
                    if let Value::Array(items) = value {
                        for item in items {
                            let Some(text) = scalar_to_form_string(item) else {
                                return Err(CamelError::TypeConversionFailed(format!(
                                    "form marshal cannot encode nested value under key '{key}'"
                                )));
                            };
                            push_form_pair(&mut encoded, key, &text);
                        }
                    } else {
                        let Some(text) = scalar_to_form_string(value) else {
                            return Err(CamelError::TypeConversionFailed(format!(
                                "form marshal cannot encode nested value under key '{key}'"
                            )));
                        };
                        push_form_pair(&mut encoded, key, &text);
                    }
                }
                Ok(Body::Text(encoded))
            }
            Body::Json(other) => Err(CamelError::TypeConversionFailed(format!(
                "form marshal expects a JSON object, got {}",
                json_shape(&other)
            ))),
            _ => Err(CamelError::TypeConversionFailed(
                "form: FormDataFormat::marshal only supports Body::Json, Body::Empty".to_string(),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_cfg() -> FormConfig {
        FormConfig::default()
    }

    fn message(err: CamelError) -> String {
        match err {
            CamelError::TypeConversionFailed(msg) => msg,
            other => panic!("expected TypeConversionFailed, got {other:?}"),
        }
    }

    #[test]
    fn parse_single_pair() {
        let pairs = parse_form_pairs(b"name=camel", &default_cfg()).expect("parse form");
        assert_eq!(pairs, vec![("name".to_string(), "camel".to_string())]);
    }

    #[test]
    fn parse_plus_and_percent() {
        let pairs =
            parse_form_pairs(b"a=one+two&b=one%20two&c=1%2B1", &default_cfg()).expect("parse form");
        assert_eq!(
            pairs,
            vec![
                ("a".to_string(), "one two".to_string()),
                ("b".to_string(), "one two".to_string()),
                ("c".to_string(), "1+1".to_string()),
            ]
        );
    }

    #[test]
    fn parse_missing_equals_empty_value() {
        let pairs = parse_form_pairs(b"flag&x=", &default_cfg()).expect("parse form");
        assert_eq!(
            pairs,
            vec![
                ("flag".to_string(), String::new()),
                ("x".to_string(), String::new()),
            ]
        );
    }

    #[test]
    fn parse_empty_key() {
        let pairs = parse_form_pairs(b"=v", &default_cfg()).expect("parse form");
        assert_eq!(pairs, vec![(String::new(), "v".to_string())]);
    }

    #[test]
    fn parse_empty_segments_skipped() {
        let pairs = parse_form_pairs(b"a=1&&b=2", &default_cfg()).expect("parse form");
        assert_eq!(
            pairs,
            vec![
                ("a".to_string(), "1".to_string()),
                ("b".to_string(), "2".to_string()),
            ]
        );

        let pairs = parse_form_pairs(b"&a=1&", &default_cfg()).expect("parse form");
        assert_eq!(pairs, vec![("a".to_string(), "1".to_string())]);

        let cfg = FormConfig {
            max_fields: Some(1),
            ..FormConfig::default()
        };
        assert!(parse_form_pairs(b"a=1&&", &cfg).is_ok());
    }

    #[test]
    fn parse_malformed_percent() {
        let cases: [(&[u8], &str); 2] = [(b"a=100%2", "%2"), (b"a=%zz", "%zz")];
        for (input, escape) in cases {
            let err = parse_form_pairs(input, &default_cfg()).expect_err("malformed escape");
            let msg = message(err);
            assert!(msg.starts_with("form"), "message was: {msg}");
            assert!(msg.contains(escape), "message was: {msg}");
        }
    }

    #[test]
    fn parse_invalid_utf8() {
        let cases: [&[u8]; 2] = [b"a=%FF", b"a=%E9"];
        for input in cases {
            let err = parse_form_pairs(input, &default_cfg()).expect_err("invalid utf-8");
            let msg = message(err);
            assert!(msg.starts_with("form"), "message was: {msg}");
        }
    }

    #[test]
    fn parse_max_fields_exceeded() {
        let cfg = FormConfig {
            max_fields: Some(2),
            ..FormConfig::default()
        };
        let err = parse_form_pairs(b"a=1&b=2&c=3", &cfg).expect_err("field cap");
        let msg = message(err);
        assert!(msg.starts_with("form"), "message was: {msg}");
        assert!(msg.contains("max_fields"), "message was: {msg}");
    }

    #[test]
    fn parse_max_fields_exact_passes() {
        let cfg = FormConfig {
            max_fields: Some(3),
            ..FormConfig::default()
        };
        let pairs = parse_form_pairs(b"a=1&b=2&c=3", &cfg).expect("exact cap passes");
        assert_eq!(pairs.len(), 3);
    }

    #[test]
    fn parse_max_bytes_before_parsing() {
        let cfg = FormConfig {
            max_bytes: Some(4),
            ..FormConfig::default()
        };

        let err = parse_form_pairs(b"a=%zz&b=2", &cfg).expect_err("byte cap");
        let msg = message(err);
        assert!(msg.starts_with("form"), "message was: {msg}");
        assert!(msg.contains("max_bytes"), "message was: {msg}");
        assert!(!msg.contains("%zz"), "message was: {msg}");

        let err = parse_form_pairs(b"a=1&b=2", &cfg).expect_err("byte cap");
        let msg = message(err);
        assert!(msg.contains("max_bytes"), "message was: {msg}");
        assert!(!msg.contains("a=1"), "message was: {msg}");
    }

    #[test]
    fn encode_whatwg_matrix() {
        assert_eq!(encode_form_component("a b&c=d+é"), "a+b%26c%3Dd%2B%C3%A9");
        assert_eq!(encode_form_component("AZaz09*-._"), "AZaz09*-._");
        assert_eq!(encode_form_component("café"), "caf%C3%A9");
    }

    fn unmarshal_json(body: Body) -> Value {
        let result = FormDataFormat::new(FormConfig::default())
            .unmarshal(body)
            .expect("unmarshal form");
        match result {
            Body::Json(value) => value,
            other => panic!("expected Body::Json, got {other:?}"),
        }
    }

    #[test]
    fn unmarshal_text_to_object() {
        let value = unmarshal_json(Body::Text("name=camel".into()));
        assert_eq!(value, serde_json::json!({"name": "camel"}));
    }

    #[test]
    fn unmarshal_duplicates_array_encounter_order() {
        let value = unmarshal_json(Body::Text("tag=a&tag=b&tag=c".into()));
        assert_eq!(value, serde_json::json!({"tag": ["a", "b", "c"]}));
    }

    #[test]
    fn unmarshal_empty_input_object() {
        let value = unmarshal_json(Body::Text(String::new()));
        assert_eq!(value, serde_json::json!({}));
    }

    #[test]
    fn unmarshal_bytes_path() {
        use bytes::Bytes;

        let value = unmarshal_json(Body::Bytes(Bytes::from_static(b"a=1")));
        assert_eq!(value, serde_json::json!({"a": "1"}));
    }

    #[test]
    fn unmarshal_empty_noop() {
        let result = FormDataFormat::new(FormConfig::default())
            .unmarshal(Body::Empty)
            .expect("empty unmarshal");
        assert!(matches!(result, Body::Empty));
    }

    #[test]
    fn unmarshal_stream_fail_closed() {
        use camel_api::body::{StreamBody, StreamMetadata};
        use futures::stream;
        use std::sync::Arc;
        use tokio::sync::Mutex;

        let stream = stream::iter(vec![Ok(bytes::Bytes::from_static(b"a=1"))]);
        let body = Body::Stream(StreamBody {
            stream: Arc::new(Mutex::new(Some(Box::pin(stream)))),
            metadata: StreamMetadata::default(),
        });
        let err = FormDataFormat::new(FormConfig::default())
            .unmarshal(body)
            .expect_err("stream must be rejected");
        let msg = message(err);
        assert!(msg.starts_with("form"), "message was: {msg}");
    }

    #[test]
    fn unmarshal_utf8_payload() {
        let value = unmarshal_json(Body::Text("nome=café".into()));
        assert_eq!(value, serde_json::json!({"nome": "café"}));
    }

    fn marshal_text(body: Body) -> String {
        let result = FormDataFormat::new(FormConfig::default())
            .marshal(body)
            .expect("marshal form");
        match result {
            Body::Text(text) => text,
            other => panic!("expected Body::Text, got {other:?}"),
        }
    }

    fn marshal_err(body: Body) -> String {
        let err = FormDataFormat::new(FormConfig::default())
            .marshal(body)
            .expect_err("marshal must fail");
        message(err)
    }

    fn stream_body() -> Body {
        use camel_api::body::{StreamBody, StreamMetadata};
        use futures::stream;
        use std::sync::Arc;
        use tokio::sync::Mutex;

        let stream = stream::iter(vec![Ok(bytes::Bytes::from_static(b"a=1"))]);
        Body::Stream(StreamBody {
            stream: Arc::new(Mutex::new(Some(Box::pin(stream)))),
            metadata: StreamMetadata::default(),
        })
    }

    #[test]
    fn marshal_flat_object_pairset() {
        let text = marshal_text(Body::Json(serde_json::json!({"a": "1", "b": "2"})));
        let pairs: std::collections::BTreeSet<String> =
            text.split('&').map(str::to_string).collect();
        assert_eq!(
            pairs,
            std::collections::BTreeSet::from(["a=1".to_string(), "b=2".to_string()])
        );
    }

    #[test]
    fn marshal_array_values_repeat_key() {
        let text = marshal_text(Body::Json(serde_json::json!({"tag": ["a", "b"]})));
        assert_eq!(text, "tag=a&tag=b");
    }

    #[test]
    fn marshal_scalars_lexical() {
        let text = marshal_text(Body::Json(serde_json::json!({
            "n": 1,
            "b": true,
            "z": null,
        })));
        let pairs: std::collections::BTreeSet<String> =
            text.split('&').map(str::to_string).collect();
        assert_eq!(
            pairs,
            std::collections::BTreeSet::from([
                "n=1".to_string(),
                "b=true".to_string(),
                "z=null".to_string(),
            ])
        );
    }

    #[test]
    fn marshal_whatwg_encoding() {
        let text = marshal_text(Body::Json(serde_json::json!({"q": "a b&c=d+é"})));
        assert_eq!(text, "q=a+b%26c%3Dd%2B%C3%A9");
    }

    #[test]
    fn marshal_utf8_roundtrip() {
        let text = marshal_text(Body::Json(serde_json::json!({"nome": "café"})));
        assert_eq!(text, "nome=caf%C3%A9");
    }

    #[test]
    fn marshal_empty_object() {
        let text = marshal_text(Body::Json(serde_json::json!({})));
        assert_eq!(text, "");
    }

    #[test]
    fn marshal_empty_noop() {
        let result = FormDataFormat::new(FormConfig::default())
            .marshal(Body::Empty)
            .expect("empty marshal");
        assert!(matches!(result, Body::Empty));
    }

    #[test]
    fn marshal_stream_fail_closed() {
        let msg = marshal_err(stream_body());
        assert!(msg.starts_with("form"), "message was: {msg}");
    }

    #[test]
    fn marshal_rejects_nested_object() {
        let msg = marshal_err(Body::Json(serde_json::json!({"a": {"b": "1"}})));
        assert!(msg.starts_with("form"), "message was: {msg}");
    }

    #[test]
    fn marshal_rejects_nested_array() {
        let msg = marshal_err(Body::Json(serde_json::json!({"a": [["1"]]})));
        assert!(msg.starts_with("form"), "message was: {msg}");
    }

    #[test]
    fn marshal_rejects_non_object() {
        for value in [serde_json::json!([1]), serde_json::json!(true)] {
            let msg = marshal_err(Body::Json(value));
            assert!(msg.starts_with("form"), "message was: {msg}");
        }
    }

    #[test]
    fn registry_resolves_form() {
        let df = crate::data_format::builtin_data_format("form").expect("form registered");
        assert_eq!(df.name(), "form");
    }

    #[test]
    fn registry_config_denies_unknown() {
        let result = crate::data_format::builtin_data_format_with_config(
            "form",
            &serde_json::json!({"bogus": 1}),
        );
        let CamelError::RouteError(msg) = (match result {
            Err(err) => err,
            Ok(_) => panic!("unknown key must be rejected"),
        }) else {
            panic!("expected RouteError");
        };
        assert!(msg.contains("bogus"), "message was: {msg}");
    }
}
