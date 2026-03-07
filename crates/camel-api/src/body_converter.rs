use crate::body::Body;
use crate::error::CamelError;
use bytes::Bytes;

/// Target type for body conversion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyType {
    Text,
    Json,
    Bytes,
    Empty,
}

/// Convert a `Body` to the target `BodyType`.
///
/// `Body::Stream` is always an error — materialize with `into_bytes()` first.
/// Returns `CamelError::TypeConversionFailed` on any incompatible conversion.
pub fn convert(body: Body, target: BodyType) -> Result<Body, CamelError> {
    match (body, target) {
        // noop: same variant
        (b @ Body::Text(_), BodyType::Text) => Ok(b),
        (b @ Body::Json(_), BodyType::Json) => Ok(b),
        (b @ Body::Bytes(_), BodyType::Bytes) => Ok(b),
        (Body::Empty, BodyType::Empty) => Ok(Body::Empty),

        // Text conversions
        (Body::Text(s), BodyType::Json) => {
            let v = serde_json::from_str(&s).map_err(|e| {
                CamelError::TypeConversionFailed(format!("cannot convert Body::Text to Json: {e}"))
            })?;
            Ok(Body::Json(v))
        }
        (Body::Text(s), BodyType::Bytes) => Ok(Body::Bytes(Bytes::from(s.into_bytes()))),
        (Body::Text(_), BodyType::Empty) => Err(CamelError::TypeConversionFailed(
            "cannot convert Body::Text to Empty".to_string(),
        )),

        // Json conversions
        (Body::Json(v), BodyType::Text) => Ok(Body::Text(v.to_string())),
        (Body::Json(v), BodyType::Bytes) => {
            let b = serde_json::to_vec(&v).map_err(|e| {
                CamelError::TypeConversionFailed(format!("cannot convert Body::Json to Bytes: {e}"))
            })?;
            Ok(Body::Bytes(Bytes::from(b)))
        }
        (Body::Json(_), BodyType::Empty) => Err(CamelError::TypeConversionFailed(
            "cannot convert Body::Json to Empty".to_string(),
        )),

        // Bytes conversions
        (Body::Bytes(b), BodyType::Text) => {
            let s = String::from_utf8(b.to_vec()).map_err(|e| {
                CamelError::TypeConversionFailed(format!(
                    "cannot convert Body::Bytes to Text: invalid UTF-8 sequence: {e}"
                ))
            })?;
            Ok(Body::Text(s))
        }
        (Body::Bytes(b), BodyType::Json) => {
            let s = String::from_utf8(b.to_vec()).map_err(|e| {
                CamelError::TypeConversionFailed(format!(
                    "cannot convert Body::Bytes to Json (UTF-8 error): {e}"
                ))
            })?;
            let v = serde_json::from_str(&s).map_err(|e| {
                CamelError::TypeConversionFailed(format!("cannot convert Body::Bytes to Json: {e}"))
            })?;
            Ok(Body::Json(v))
        }
        (Body::Bytes(_), BodyType::Empty) => Err(CamelError::TypeConversionFailed(
            "cannot convert Body::Bytes to Empty".to_string(),
        )),

        // Empty conversions
        (Body::Empty, BodyType::Text) => Err(CamelError::TypeConversionFailed(
            "cannot convert Empty body to Text".to_string(),
        )),
        (Body::Empty, BodyType::Json) => Err(CamelError::TypeConversionFailed(
            "cannot convert Empty body to Json".to_string(),
        )),
        (Body::Empty, BodyType::Bytes) => Err(CamelError::TypeConversionFailed(
            "cannot convert Empty body to Bytes".to_string(),
        )),

        // Stream: always fails
        (Body::Stream(_), _) => Err(CamelError::TypeConversionFailed(
            "cannot convert Body::Stream: materialize first with into_bytes()".to_string(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn text_to_json_valid() {
        let body = Body::Text(r#"{"a":1}"#.to_string());
        let result = convert(body, BodyType::Json).unwrap();
        assert_eq!(result, Body::Json(json!({"a": 1})));
    }

    #[test]
    fn text_to_json_invalid() {
        let body = Body::Text("not json".to_string());
        let result = convert(body, BodyType::Json);
        assert!(matches!(result, Err(CamelError::TypeConversionFailed(_))));
    }

    #[test]
    fn json_to_text() {
        let body = Body::Json(json!({"a": 1}));
        let result = convert(body, BodyType::Text).unwrap();
        match result {
            Body::Text(s) => assert!(s.contains("\"a\"")),
            _ => panic!("expected Body::Text"),
        }
    }

    #[test]
    fn json_to_bytes() {
        let body = Body::Json(json!({"x": 2}));
        let result = convert(body, BodyType::Bytes).unwrap();
        assert!(matches!(result, Body::Bytes(_)));
    }

    #[test]
    fn bytes_to_text_valid() {
        let body = Body::Bytes(Bytes::from_static(b"hello"));
        let result = convert(body, BodyType::Text).unwrap();
        assert_eq!(result, Body::Text("hello".to_string()));
    }

    #[test]
    fn bytes_to_text_invalid_utf8() {
        let body = Body::Bytes(Bytes::from_static(&[0xFF, 0xFE]));
        let result = convert(body, BodyType::Text);
        assert!(matches!(result, Err(CamelError::TypeConversionFailed(_))));
    }

    #[test]
    fn text_to_bytes() {
        let body = Body::Text("hi".to_string());
        let result = convert(body, BodyType::Bytes).unwrap();
        assert_eq!(result, Body::Bytes(Bytes::from_static(b"hi")));
    }

    #[test]
    fn empty_to_text_fails() {
        let result = convert(Body::Empty, BodyType::Text);
        assert!(matches!(result, Err(CamelError::TypeConversionFailed(_))));
    }

    #[test]
    fn empty_to_empty_noop() {
        let result = convert(Body::Empty, BodyType::Empty).unwrap();
        assert!(matches!(result, Body::Empty));
    }

    #[test]
    fn noop_same_type_text() {
        let body = Body::Text("x".to_string());
        let result = convert(body, BodyType::Text).unwrap();
        assert!(matches!(result, Body::Text(_)));
    }

    #[test]
    fn noop_same_type_json() {
        let body = Body::Json(json!(1));
        let result = convert(body, BodyType::Json).unwrap();
        assert!(matches!(result, Body::Json(_)));
    }

    #[test]
    fn noop_same_type_bytes() {
        let body = Body::Bytes(Bytes::from_static(b"x"));
        let result = convert(body, BodyType::Bytes).unwrap();
        assert!(matches!(result, Body::Bytes(_)));
    }

    #[test]
    fn stream_to_any_fails() {
        use crate::body::{StreamBody, StreamMetadata};
        use futures::stream;
        use std::sync::Arc;
        use tokio::sync::Mutex;

        let stream = stream::iter(vec![Ok(Bytes::from_static(b"data"))]);
        let body = Body::Stream(StreamBody {
            stream: Arc::new(Mutex::new(Some(Box::pin(stream)))),
            metadata: StreamMetadata::default(),
        });
        let result = convert(body, BodyType::Text);
        assert!(matches!(result, Err(CamelError::TypeConversionFailed(_))));
    }

    #[test]
    fn bytes_to_json_valid() {
        let body = Body::Bytes(Bytes::from_static(b"{\"k\":1}"));
        let result = convert(body, BodyType::Json).unwrap();
        assert!(matches!(result, Body::Json(_)));
    }

    #[test]
    fn bytes_to_json_invalid_utf8() {
        let body = Body::Bytes(Bytes::from_static(&[0xFF, 0xFE]));
        let result = convert(body, BodyType::Json);
        assert!(matches!(result, Err(CamelError::TypeConversionFailed(_))));
    }

    #[test]
    fn to_empty_always_fails() {
        assert!(matches!(
            convert(Body::Text("x".into()), BodyType::Empty),
            Err(CamelError::TypeConversionFailed(_))
        ));
        assert!(matches!(
            convert(Body::Json(serde_json::json!(1)), BodyType::Empty),
            Err(CamelError::TypeConversionFailed(_))
        ));
        assert!(matches!(
            convert(Body::Bytes(Bytes::from_static(b"x")), BodyType::Empty),
            Err(CamelError::TypeConversionFailed(_))
        ));
    }
}
