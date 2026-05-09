use base64::Engine;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// RegisterRequest
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RegisterRequest {
    pub function_id: String,
    pub runtime: String,
    pub source: String,
    pub timeout_ms: u64,
}

// ---------------------------------------------------------------------------
// BodyWire
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum BodyWire {
    Empty,
    Text(String),
    Json(serde_json::Value),
    Bytes(String),
    Xml(String),
}

impl BodyWire {
    pub fn from_body(body: &camel_api::Body) -> Self {
        match body {
            camel_api::Body::Empty => BodyWire::Empty,
            camel_api::Body::Text(s) => BodyWire::Text(s.clone()),
            camel_api::Body::Json(v) => BodyWire::Json(v.clone()),
            camel_api::Body::Bytes(b) => {
                BodyWire::Bytes(base64::engine::general_purpose::STANDARD.encode(b))
            }
            camel_api::Body::Xml(s) => BodyWire::Xml(s.clone()),
            camel_api::Body::Stream(_) => {
                tracing::debug!("stream body cannot cross process boundary, mapping to Empty");
                BodyWire::Empty
            }
        }
    }

    pub fn to_body(&self) -> camel_api::Body {
        match self {
            BodyWire::Empty => camel_api::Body::Empty,
            BodyWire::Text(s) => camel_api::Body::Text(s.clone()),
            BodyWire::Json(v) => camel_api::Body::Json(v.clone()),
            BodyWire::Bytes(b64) => {
                match base64::engine::general_purpose::STANDARD.decode(b64) {
                    Ok(bytes) => camel_api::Body::Bytes(bytes::Bytes::from(bytes)),
                    Err(e) => {
                        tracing::warn!(error = %e, "invalid base64 in wire body, falling back to Empty");
                        camel_api::Body::Empty
                    }
                }
            }
            BodyWire::Xml(s) => camel_api::Body::Xml(s.clone()),
        }
    }
}

// ---------------------------------------------------------------------------
// ExchangeWire
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExchangeWire {
    pub function_id: String,
    pub correlation_id: String,
    pub body: BodyWire,
    pub headers: HashMap<String, serde_json::Value>,
    pub properties: HashMap<String, serde_json::Value>,
    pub timeout_ms: u64,
}

// ---------------------------------------------------------------------------
// InvokeResponse
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InvokeResponse {
    pub ok: bool,
    pub body: Option<BodyWire>,
    pub headers_set: Vec<(String, serde_json::Value)>,
    pub headers_removed: Vec<String>,
    pub properties_set: Vec<(String, serde_json::Value)>,
    pub error: Option<ErrorWire>,
}

// ---------------------------------------------------------------------------
// PatchWire
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PatchWire {
    pub body: Option<BodyWire>,
    pub headers_set: Vec<(String, serde_json::Value)>,
    pub headers_removed: Vec<String>,
    pub properties_set: Vec<(String, serde_json::Value)>,
}

// ---------------------------------------------------------------------------
// ErrorWire
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ErrorWire {
    pub kind: String,
    pub message: String,
    pub stack: Option<String>,
}

// ---------------------------------------------------------------------------
// HealthResponse
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HealthResponse {
    pub status: String,
    pub registered: Vec<String>,
}

// ---------------------------------------------------------------------------
// ErrorResponse
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ErrorResponse {
    pub error: String,
    pub kind: String,
}

pub mod client;

pub use client::ProtocolClient;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_register_request_roundtrip() {
        let req = RegisterRequest {
            function_id: "fn-123".into(),
            runtime: "deno".into(),
            source: "export default function(ex) { return ex; }".into(),
            timeout_ms: 5000,
        };
        let json = serde_json::to_string(&req).unwrap();
        let decoded: RegisterRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(req, decoded);
    }

    fn make_exchange_wire(body: BodyWire) -> ExchangeWire {
        let mut headers = HashMap::new();
        headers.insert("content-type".into(), serde_json::json!("text/plain"));
        let mut properties = HashMap::new();
        properties.insert("retry-count".into(), serde_json::json!(3));
        ExchangeWire {
            function_id: "fn-abc".into(),
            correlation_id: "corr-001".into(),
            body,
            headers,
            properties,
            timeout_ms: 3000,
        }
    }

    #[test]
    fn test_exchange_wire_roundtrip_text() {
        let wire = make_exchange_wire(BodyWire::Text("hello world".into()));
        let json = serde_json::to_string(&wire).unwrap();
        let decoded: ExchangeWire = serde_json::from_str(&json).unwrap();
        assert_eq!(wire, decoded);
    }

    #[test]
    fn test_exchange_wire_roundtrip_json() {
        let wire = make_exchange_wire(BodyWire::Json(serde_json::json!({"key": "value"})));
        let json = serde_json::to_string(&wire).unwrap();
        let decoded: ExchangeWire = serde_json::from_str(&json).unwrap();
        assert_eq!(wire, decoded);
    }

    #[test]
    fn test_exchange_wire_roundtrip_bytes() {
        let original = b"binary data here";
        let encoded = base64::engine::general_purpose::STANDARD.encode(original);
        let wire = make_exchange_wire(BodyWire::Bytes(encoded));
        let json = serde_json::to_string(&wire).unwrap();
        let decoded: ExchangeWire = serde_json::from_str(&json).unwrap();
        assert_eq!(wire, decoded);
        // Verify base64 roundtrip
        if let BodyWire::Bytes(b64) = &decoded.body {
            let decoded_bytes = base64::engine::general_purpose::STANDARD.decode(b64).unwrap();
            assert_eq!(decoded_bytes, original);
        } else {
            panic!("expected Bytes variant");
        }
    }

    #[test]
    fn test_exchange_wire_roundtrip_xml() {
        let wire = make_exchange_wire(BodyWire::Xml("<root><item>1</item></root>".into()));
        let json = serde_json::to_string(&wire).unwrap();
        let decoded: ExchangeWire = serde_json::from_str(&json).unwrap();
        assert_eq!(wire, decoded);
    }

    #[test]
    fn test_exchange_wire_roundtrip_empty() {
        let wire = make_exchange_wire(BodyWire::Empty);
        let json = serde_json::to_string(&wire).unwrap();
        let decoded: ExchangeWire = serde_json::from_str(&json).unwrap();
        assert_eq!(wire, decoded);
    }

    #[test]
    fn test_invoke_response_ok() {
        let resp = InvokeResponse {
            ok: true,
            body: Some(BodyWire::Text("processed".into())),
            headers_set: vec![("x-custom".into(), serde_json::json!("added"))],
            headers_removed: vec!["x-old".into()],
            properties_set: vec![("status".into(), serde_json::json!("done"))],
            error: None,
        };
        let json = serde_json::to_string(&resp).unwrap();
        let decoded: InvokeResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(resp, decoded);
        assert!(decoded.ok);
        assert!(decoded.body.is_some());
    }

    #[test]
    fn test_invoke_response_error() {
        let resp = InvokeResponse {
            ok: false,
            body: None,
            headers_set: vec![],
            headers_removed: vec![],
            properties_set: vec![],
            error: Some(ErrorWire {
                kind: "user_error".into(),
                message: "ReferenceError: x is not defined".into(),
                stack: Some("at main (file:///fn.ts:3:1)".into()),
            }),
        };
        let json = serde_json::to_string(&resp).unwrap();
        let decoded: InvokeResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(resp, decoded);
        assert!(!decoded.ok);
        let err = decoded.error.unwrap();
        assert_eq!(err.kind, "user_error");
        assert!(err.stack.is_some());
    }

    #[test]
    fn test_health_response() {
        let resp = HealthResponse {
            status: "ok".into(),
            registered: vec!["fn-a".into(), "fn-b".into()],
        };
        let json = serde_json::to_string(&resp).unwrap();
        let decoded: HealthResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(resp, decoded);
        assert_eq!(decoded.registered.len(), 2);
    }

    #[test]
    fn test_error_response() {
        let resp = ErrorResponse {
            error: "function not found".into(),
            kind: "not_registered".into(),
        };
        let json = serde_json::to_string(&resp).unwrap();
        let decoded: ErrorResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(resp, decoded);
    }

    #[test]
    fn test_patch_wire() {
        let patch = PatchWire {
            body: Some(BodyWire::Json(serde_json::json!({"updated": true}))),
            headers_set: vec![("x-new".into(), serde_json::json!("val"))],
            headers_removed: vec!["x-old".into()],
            properties_set: vec![("key".into(), serde_json::json!(42))],
        };
        let json = serde_json::to_string(&patch).unwrap();
        let decoded: PatchWire = serde_json::from_str(&json).unwrap();
        assert_eq!(patch, decoded);
    }

    #[test]
    fn test_body_wire_serde_lowercase() {
        let wire = BodyWire::Text("hello".into());
        let json = serde_json::to_string(&wire).unwrap();
        assert!(json.contains("\"text\""), "expected lowercase variant name, got: {json}");
        assert!(!json.contains("\"Text\""), "should not have UpperCamelCase variant");
        let decoded: BodyWire = serde_json::from_str(&json).unwrap();
        assert_eq!(wire, decoded);
    }

    #[test]
    fn test_body_wire_bytes_base64_roundtrip() {
        let original_bytes = vec![0xDE, 0xAD, 0xBE, 0xEF, 0xCA, 0xFE];
        let encoded = base64::engine::general_purpose::STANDARD.encode(&original_bytes);
        let wire = BodyWire::Bytes(encoded.clone());

        let json = serde_json::to_string(&wire).unwrap();
        let decoded: BodyWire = serde_json::from_str(&json).unwrap();

        if let BodyWire::Bytes(b64) = &decoded {
            let roundtrip = base64::engine::general_purpose::STANDARD.decode(b64).unwrap();
            assert_eq!(roundtrip, original_bytes);
        } else {
            panic!("expected Bytes variant after roundtrip");
        }

        // Also verify to_body conversion
        let body = wire.to_body();
        if let camel_api::Body::Bytes(b) = body {
            assert_eq!(b.to_vec(), original_bytes);
        } else {
            panic!("expected Body::Bytes from to_body()");
        }
    }

    #[test]
    fn test_body_wire_from_body_roundtrip() {
        let bodies = vec![
            ("Empty", camel_api::Body::Empty),
            ("Text", camel_api::Body::Text("hello world".into())),
            ("Json", camel_api::Body::Json(serde_json::json!({"key": "value"}))),
            ("Xml", camel_api::Body::Xml("<root><item>1</item></root>".into())),
        ];

        for (name, body) in bodies {
            let wire = BodyWire::from_body(&body);
            let roundtripped = wire.to_body();
            assert_eq!(body, roundtripped, "roundtrip failed for {name}");
        }

        // Bytes need special handling since from_body base64-encodes
        let original_bytes = vec![0xDE, 0xAD, 0xBE, 0xEF];
        let body = camel_api::Body::Bytes(bytes::Bytes::from(original_bytes.clone()));
        let wire = BodyWire::from_body(&body);
        let roundtripped = wire.to_body();
        if let camel_api::Body::Bytes(b) = roundtripped {
            assert_eq!(b.to_vec(), original_bytes);
        } else {
            panic!("expected Body::Bytes after Bytes roundtrip");
        }
    }

    #[test]
    fn test_body_wire_from_body_stream_maps_to_empty() {
        use camel_api::{StreamBody, StreamMetadata};
        use futures::stream;

        let chunks = vec![Ok(bytes::Bytes::from("stream data"))];
        let stream_body = camel_api::Body::Stream(StreamBody {
            stream: std::sync::Arc::new(tokio::sync::Mutex::new(Some(Box::pin(stream::iter(chunks))))),
            metadata: StreamMetadata::default(),
        });

        let wire = BodyWire::from_body(&stream_body);
        assert!(matches!(wire, BodyWire::Empty));
    }

    #[test]
    fn test_body_wire_to_body_from_body_text() {
        let wire = BodyWire::Text("hello world".into());
        let body = wire.to_body();
        let wire2 = BodyWire::from_body(&body);

        assert!(matches!(wire2, BodyWire::Text(ref s) if s == "hello world"));
    }
}
