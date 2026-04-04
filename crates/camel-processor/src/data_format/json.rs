use camel_api::body::Body;
use camel_api::data_format::DataFormat;
use camel_api::error::CamelError;

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
                "cannot marshal Body::Stream — materialize with into_bytes() first".to_string(),
            )),
            Body::Empty | Body::Xml(_) => Err(CamelError::TypeConversionFailed(
                "JsonDataFormat::marshal only supports Body::Json, Body::Text, and Body::Bytes"
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
                let v = serde_json::from_str(&s).map_err(|e| {
                    CamelError::TypeConversionFailed(format!(
                        "cannot unmarshal Body::Text as JSON: {e}"
                    ))
                })?;
                Ok(Body::Json(v))
            }
            Body::Bytes(b) => {
                let v = serde_json::from_slice(&b).map_err(|e| {
                    CamelError::TypeConversionFailed(format!(
                        "cannot unmarshal Body::Bytes as JSON: {e}"
                    ))
                })?;
                Ok(Body::Json(v))
            }
            Body::Stream(_) => Err(CamelError::TypeConversionFailed(
                "cannot unmarshal Body::Stream — materialize with into_bytes() first".to_string(),
            )),
            Body::Empty | Body::Xml(_) => Err(CamelError::TypeConversionFailed(
                "JsonDataFormat::unmarshal only supports Body::Json, Body::Text, and Body::Bytes"
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
        let body = Body::Empty;
        let result = JsonDataFormat.unmarshal(body);
        assert!(matches!(result, Err(CamelError::TypeConversionFailed(_))));
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
}
