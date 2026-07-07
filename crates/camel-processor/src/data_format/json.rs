use camel_api::body::Body;
use camel_api::data_format::DataFormat;
use camel_api::error::CamelError;

/// Maximum input size accepted by `JsonDataFormat::unmarshal` (DoS cap, R3-L2).
/// `serde_json` retains its built-in 128-level recursion limit; this cap bounds
/// the byte volume before parsing begins.
const MAX_JSON_BYTES: usize = 16 * 1024 * 1024; // 16 MiB

pub struct JsonDataFormat;

impl DataFormat for JsonDataFormat {
    fn name(&self) -> &str {
        "json"
    }

    fn marshal(&self, body: Body) -> Result<Body, CamelError> {
        match body {
            Body::Json(v) => {
                let s = serde_json::to_string(&v).map_err(|e| {
                    CamelError::TypeConversionFailed(format!(
                        "cannot marshal Body::Json to text: {e}"
                    ))
                })?;
                Ok(Body::Text(s))
            }
            Body::Text(_) => Ok(body),
            Body::Stream(_) => Err(CamelError::TypeConversionFailed(
                "cannot marshal Body::Stream — add 'stream_cache' or 'convert_body_to' before this step".to_string(),
            )),
            // Empty body: no-op. A REST handler that produces no body (e.g. a
            // 204 DELETE) must not 500 at marshal — leave the body empty so the
            // reply finaliser ships an empty response (spec §8.1).
            Body::Empty => Ok(Body::Empty),
            Body::Xml(_) => Err(CamelError::TypeConversionFailed(
                "JsonDataFormat::marshal only supports Body::Json, Body::Text, Body::Bytes, \
                 and Body::Empty"
                    .to_string(),
            )),
            Body::Bytes(b) => {
                let v: serde_json::Value = serde_json::from_slice(&b).map_err(|e| {
                    CamelError::TypeConversionFailed(format!(
                        "cannot marshal Body::Bytes as JSON: {e}"
                    ))
                })?;
                Ok(Body::Text(serde_json::to_string(&v).map_err(|e| {
                    CamelError::TypeConversionFailed(format!(
                        "cannot serialize JSON value to text: {e}"
                    ))
                })?))
            }
        }
    }

    fn unmarshal(&self, body: Body) -> Result<Body, CamelError> {
        match body {
            Body::Json(_) => Ok(body),
            Body::Text(s) => {
                if s.len() > MAX_JSON_BYTES {
                    return Err(CamelError::TypeConversionFailed(format!(
                        "JSON unmarshal input {} bytes exceeds max {MAX_JSON_BYTES}",
                        s.len()
                    )));
                }
                let v = serde_json::from_str(&s).map_err(|e| {
                    CamelError::TypeConversionFailed(format!(
                        "cannot unmarshal Body::Text as JSON: {e}"
                    ))
                })?;
                Ok(Body::Json(v))
            }
            Body::Bytes(b) => {
                if b.len() > MAX_JSON_BYTES {
                    return Err(CamelError::TypeConversionFailed(format!(
                        "JSON unmarshal input {} bytes exceeds max {MAX_JSON_BYTES}",
                        b.len()
                    )));
                }
                let v = serde_json::from_slice(&b).map_err(|e| {
                    CamelError::TypeConversionFailed(format!(
                        "cannot unmarshal Body::Bytes as JSON: {e}"
                    ))
                })?;
                Ok(Body::Json(v))
            }
            Body::Stream(_) => Err(CamelError::TypeConversionFailed(
                "cannot unmarshal Body::Stream directly — use UnmarshalService which auto-materializes"
                    .to_string(),
            )),
            // Empty body: no-op. A body-less request (e.g. POST with no body)
            // must skip unmarshal rather than 500 — spec §8.1: "if the body is
            // empty, skip unmarshal".
            Body::Empty => Ok(Body::Empty),
            Body::Xml(_) => Err(CamelError::TypeConversionFailed(
                "JsonDataFormat::unmarshal only supports Body::Json, Body::Text, Body::Bytes, \
                 and Body::Empty"
                    .to_string(),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use serde_json::json;

    #[test]
    fn test_name() {
        assert_eq!(JsonDataFormat.name(), "json");
    }

    #[test]
    fn test_unmarshal_text_to_json() {
        let body = Body::Text(r#"{"a":1}"#.to_string());
        let result = JsonDataFormat.unmarshal(body).unwrap();
        assert!(matches!(result, Body::Json(_)));
        if let Body::Json(v) = result {
            assert_eq!(v["a"], json!(1));
        }
    }

    #[test]
    fn test_unmarshal_bytes_to_json() {
        let body = Body::Bytes(Bytes::from_static(b"{\"b\":2}"));
        let result = JsonDataFormat.unmarshal(body).unwrap();
        assert!(matches!(result, Body::Json(_)));
    }

    #[test]
    fn test_unmarshal_invalid_json() {
        let body = Body::Text("not json".to_string());
        let result = JsonDataFormat.unmarshal(body);
        assert!(matches!(result, Err(CamelError::TypeConversionFailed(_))));
    }

    #[test]
    fn test_unmarshal_json_noop() {
        let body = Body::Json(json!({"x": 1}));
        let result = JsonDataFormat.unmarshal(body).unwrap();
        assert!(matches!(result, Body::Json(_)));
    }

    #[test]
    fn test_unmarshal_unsupported_variant() {
        let body = Body::Xml("<root/>".to_string());
        let result = JsonDataFormat.unmarshal(body);
        assert!(matches!(result, Err(CamelError::TypeConversionFailed(_))));
    }

    #[test]
    fn test_unmarshal_empty_is_noop() {
        // spec §8.1: an empty request body skips unmarshal instead of erroring.
        let result = JsonDataFormat.unmarshal(Body::Empty).unwrap();
        assert!(matches!(result, Body::Empty));
    }

    #[test]
    fn test_marshal_empty_is_noop() {
        // spec §8.1: an empty response body marshals to empty (e.g. 204 DELETE).
        let result = JsonDataFormat.marshal(Body::Empty).unwrap();
        assert!(matches!(result, Body::Empty));
    }

    #[test]
    fn test_unmarshal_stream_rejected() {
        use camel_api::body::{StreamBody, StreamMetadata};
        use futures::stream;
        use std::sync::Arc;
        use tokio::sync::Mutex;

        let stream = stream::iter(vec![Ok(Bytes::from_static(b"data"))]);
        let body = Body::Stream(StreamBody {
            stream: Arc::new(Mutex::new(Some(Box::pin(stream)))),
            metadata: StreamMetadata::default(),
        });
        let result = JsonDataFormat.unmarshal(body);
        assert!(matches!(result, Err(CamelError::TypeConversionFailed(_))));
    }

    #[test]
    fn test_marshal_json_to_text() {
        let body = Body::Json(json!({"key": "value"}));
        let result = JsonDataFormat.marshal(body).unwrap();
        match result {
            Body::Text(s) => assert!(s.contains("\"key\"")),
            _ => panic!("expected Body::Text"),
        }
    }

    #[test]
    fn test_marshal_text_noop() {
        let body = Body::Text("already text".to_string());
        let result = JsonDataFormat.marshal(body).unwrap();
        assert_eq!(result, Body::Text("already text".to_string()));
    }

    #[test]
    fn test_marshal_unsupported_variant() {
        let body = Body::Xml("<root/>".to_string());
        let result = JsonDataFormat.marshal(body);
        assert!(matches!(result, Err(CamelError::TypeConversionFailed(_))));
    }

    #[test]
    fn test_marshal_bytes_to_text() {
        let body = Body::Bytes(Bytes::from_static(b"{\"key\":\"val\"}"));
        let result = JsonDataFormat.marshal(body).unwrap();
        match result {
            Body::Text(s) => assert!(s.contains("\"key\"")),
            _ => panic!("expected Body::Text"),
        }
    }

    #[test]
    fn test_marshal_invalid_bytes_returns_error() {
        let body = Body::Bytes(Bytes::from_static(b"not json"));
        let result = JsonDataFormat.marshal(body);
        assert!(matches!(result, Err(CamelError::TypeConversionFailed(_))));
    }

    #[test]
    fn test_unmarshal_text_size_cap() {
        // Build a valid JSON string exceeding the 16 MiB cap.
        let big = format!(r#"{{"k":"{}"}}"#, "x".repeat(16 * 1024 * 1024 + 10));
        let body = Body::Text(big);
        let result = JsonDataFormat.unmarshal(body);
        assert!(result.is_err(), "expected error for oversized JSON text");
        let msg = format!("{}", result.unwrap_err());
        assert!(
            msg.contains("exceeds") && msg.contains("max"),
            "error should mention size cap: {msg}"
        );
    }

    #[test]
    fn test_unmarshal_bytes_size_cap() {
        let big = format!(r#"{{"k":"{}"}}"#, "x".repeat(16 * 1024 * 1024 + 10));
        let body = Body::Bytes(bytes::Bytes::from(big));
        let result = JsonDataFormat.unmarshal(body);
        assert!(result.is_err());
    }
}
