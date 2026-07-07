//! camel-dataformat-protobuf — Protobuf DataFormat for Apache Camel Rust.
//!
//! Provides marshal/unmarshal support for Protocol Buffers using dynamic message
//! descriptors compiled at runtime via `prost-reflect`. JSON ↔ binary protobuf
//! round-tripping is supported out of the box.
//!
//! TODO(PROTO-005): Schema registry integration (e.g. Confluent Schema Registry)
//! is not yet implemented. When available, this will allow automatic schema
//! lookup/registration by subject and version during marshal/unmarshal.

use std::path::Path;

use bytes::BytesMut;
use camel_api::body::Body;
use camel_api::data_format::DataFormat;
use camel_api::error::CamelError;
use camel_proto_compiler::{ProtoCache, compile_proto};
use prost::Message;
use prost_reflect::{DynamicMessage, MessageDescriptor};

/// Default maximum input size accepted by `unmarshal`/`marshal` before
/// `DynamicMessage::decode` (DoS cap, R5-M2 — closes the OOM vector).
///
/// Recursion-depth: prost 0.14 enforces a built-in RECURSION_LIMIT=100
/// (active by default; the `no-recursion-limit` feature is NOT enabled in
/// this workspace), and prost-reflect's decode routes nested messages through
/// `prost::encoding::message::merge` which checks it. So deeply-nested /
/// recursive-schema payloads return `Err(RecursionLimitReached)` at depth 100
/// — the spec's max-depth requirement is satisfied at the dependency level.
/// See `test_unmarshal_recursive_schema_hits_recursion_limit`.
const DEFAULT_MAX_DECODE_BYTES: usize = 64 * 1024 * 1024; // 64 MiB

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProtobufConfig {
    /// Optional content type format, e.g. `application/protobuf` or `application/json`.
    /// When unset, binary protobuf is assumed.
    pub content_type_format: Option<String>,
    /// Optional fully-qualified class/type name used for deserialization.
    pub instance_class: Option<String>,
}

impl ProtobufConfig {
    pub fn validate(&self) -> Result<(), CamelError> {
        if let Some(instance_class) = &self.instance_class
            && instance_class.trim().is_empty()
        {
            return Err(CamelError::TypeConversionFailed(
                "instance_class must be non-empty when set".to_string(),
            ));
        }

        Ok(())
    }
}

pub struct ProtobufDataFormat {
    descriptor: MessageDescriptor,
    max_decode_bytes: usize,
}

impl ProtobufDataFormat {
    pub fn new<P: AsRef<Path>>(proto_path: P, message_name: &str) -> Result<Self, CamelError> {
        let pool =
            compile_proto(proto_path.as_ref(), std::iter::empty::<&Path>()).map_err(|e| {
                CamelError::TypeConversionFailed(format!("failed to compile proto: {e}"))
            })?;
        let descriptor = pool.get_message_by_name(message_name).ok_or_else(|| {
            CamelError::Config(format!("message descriptor not found: {message_name}"))
        })?;
        Ok(Self {
            descriptor,
            max_decode_bytes: DEFAULT_MAX_DECODE_BYTES,
        })
    }

    pub fn new_with_cache<P: AsRef<Path>>(
        proto_path: P,
        message_name: &str,
        cache: &ProtoCache,
    ) -> Result<Self, CamelError> {
        let pool = cache
            .get_or_compile(proto_path.as_ref(), std::iter::empty::<&Path>())
            .map_err(|e| {
                CamelError::TypeConversionFailed(format!("failed to compile proto: {e}"))
            })?;
        let descriptor = pool.get_message_by_name(message_name).ok_or_else(|| {
            CamelError::Config(format!("message descriptor not found: {message_name}"))
        })?;
        Ok(Self {
            descriptor,
            max_decode_bytes: DEFAULT_MAX_DECODE_BYTES,
        })
    }

    pub fn descriptor(&self) -> &MessageDescriptor {
        &self.descriptor
    }

    /// Maximum input size accepted before decode (default 64 MiB).
    pub fn max_decode_bytes(&self) -> usize {
        self.max_decode_bytes
    }

    /// Override the decode byte-size cap (builder style).
    #[must_use]
    pub fn with_max_decode_bytes(mut self, max: usize) -> Self {
        self.max_decode_bytes = max;
        self
    }

    pub fn json_to_dynamic(
        &self,
        json_val: serde_json::Value,
    ) -> Result<DynamicMessage, CamelError> {
        let json_str = serde_json::to_string(&json_val).map_err(|e| {
            CamelError::TypeConversionFailed(format!("failed to serialize JSON: {e}"))
        })?;
        let mut de = serde_json::Deserializer::from_str(&json_str);
        DynamicMessage::deserialize(self.descriptor.clone(), &mut de).map_err(|e| {
            CamelError::TypeConversionFailed(format!("failed to parse JSON into protobuf: {e}"))
        })
    }

    pub fn dynamic_to_json(&self, msg: DynamicMessage) -> Result<serde_json::Value, CamelError> {
        serde_json::to_value(&msg).map_err(|e| {
            CamelError::TypeConversionFailed(format!("failed to serialize protobuf to JSON: {e}"))
        })
    }
}

impl DataFormat for ProtobufDataFormat {
    fn name(&self) -> &str {
        "protobuf"
    }

    fn marshal(&self, body: Body) -> Result<Body, CamelError> {
        match body {
            Body::Json(val) => {
                let msg = self.json_to_dynamic(val)?;
                let mut buf = BytesMut::new();
                msg.encode(&mut buf).map_err(|e| {
                    CamelError::TypeConversionFailed(format!(
                        "failed to encode protobuf message: {e}"
                    ))
                })?;
                Ok(Body::Bytes(buf.freeze()))
            }
            Body::Text(text) => {
                let val: serde_json::Value = serde_json::from_str(&text).map_err(|e| {
                    CamelError::TypeConversionFailed(format!(
                        "invalid JSON text for protobuf marshal: {e}"
                    ))
                })?;
                self.marshal(Body::Json(val))
            }
            Body::Bytes(bytes) => {
                if bytes.len() > self.max_decode_bytes {
                    return Err(CamelError::TypeConversionFailed(format!(
                        "protobuf marshal rejected: {} bytes exceeds max_decode_bytes {}",
                        bytes.len(),
                        self.max_decode_bytes
                    )));
                }
                DynamicMessage::decode(self.descriptor.clone(), bytes.as_ref()).map_err(|e| {
                    CamelError::ProcessorError(format!(
                        "protobuf marshal: invalid bytes for type {}: {e}",
                        self.descriptor.full_name()
                    ))
                })?;
                Ok(Body::Bytes(bytes))
            }
            Body::Empty => Err(CamelError::TypeConversionFailed(
                "protobuf marshal does not support empty body".to_string(),
            )),
            Body::Stream(_) => Err(CamelError::TypeConversionFailed(
                "protobuf marshal does not support stream body".to_string(),
            )),
            Body::Xml(_) => Err(CamelError::TypeConversionFailed(
                "protobuf marshal does not support XML body".to_string(),
            )),
        }
    }

    fn unmarshal(&self, body: Body) -> Result<Body, CamelError> {
        match body {
            Body::Bytes(bytes) => {
                if bytes.len() > self.max_decode_bytes {
                    return Err(CamelError::TypeConversionFailed(format!(
                        "protobuf unmarshal rejected: {} bytes exceeds max_decode_bytes {}",
                        bytes.len(),
                        self.max_decode_bytes
                    )));
                }
                let msg = DynamicMessage::decode(self.descriptor.clone(), bytes.as_ref()).map_err(
                    |e| {
                        CamelError::TypeConversionFailed(format!(
                            "failed to decode protobuf bytes: {e}"
                        ))
                    },
                )?;
                let json = self.dynamic_to_json(msg)?;
                Ok(Body::Json(json))
            }
            Body::Json(val) => Ok(Body::Json(val)),
            Body::Text(_) => Err(CamelError::TypeConversionFailed(
                "protobuf unmarshal does not support text body".to_string(),
            )),
            Body::Stream(_) => Err(CamelError::TypeConversionFailed(
                "protobuf unmarshal does not support stream body".to_string(),
            )),
            Body::Empty => Err(CamelError::TypeConversionFailed(
                "protobuf unmarshal does not support empty body".to_string(),
            )),
            Body::Xml(_) => Err(CamelError::TypeConversionFailed(
                "protobuf unmarshal does not support XML body".to_string(),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use bytes::Bytes;
    use camel_api::body::Body;
    use camel_api::data_format::DataFormat;
    use camel_api::error::CamelError;
    use serde_json::json;

    use super::{ProtobufConfig, ProtobufDataFormat};

    fn test_proto_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("helloworld.proto")
    }

    fn data_format() -> ProtobufDataFormat {
        ProtobufDataFormat::new(test_proto_path(), "helloworld.HelloRequest")
            .expect("should load descriptor")
    }

    #[test]
    fn test_name() {
        let df = data_format();
        assert_eq!(df.name(), "protobuf");
    }

    #[test]
    fn test_marshal_json_to_bytes() {
        let df = data_format();
        let body = Body::Json(json!({ "name": "Alice" }));
        let out = df.marshal(body).expect("marshal should succeed");
        match out {
            Body::Bytes(b) => assert!(!b.is_empty()),
            other => panic!("expected bytes, got {other:?}"),
        }
    }

    #[test]
    fn test_unmarshal_bytes_to_json() {
        let df = data_format();
        let bytes = match df
            .marshal(Body::Json(json!({ "name": "Alice" })))
            .expect("marshal should succeed")
        {
            Body::Bytes(b) => b,
            other => panic!("expected bytes, got {other:?}"),
        };

        let out = df
            .unmarshal(Body::Bytes(bytes))
            .expect("unmarshal should succeed");
        match out {
            Body::Json(v) => assert_eq!(v, json!({ "name": "Alice" })),
            other => panic!("expected json, got {other:?}"),
        }
    }

    #[test]
    fn test_roundtrip_json_bytes_json() {
        let df = data_format();
        let input = json!({ "name": "Bob" });
        let bytes = match df
            .marshal(Body::Json(input.clone()))
            .expect("marshal should succeed")
        {
            Body::Bytes(b) => b,
            other => panic!("expected bytes, got {other:?}"),
        };
        let output = match df
            .unmarshal(Body::Bytes(bytes))
            .expect("unmarshal should succeed")
        {
            Body::Json(v) => v,
            other => panic!("expected json, got {other:?}"),
        };
        assert_eq!(output, input);
    }

    #[test]
    fn test_marshal_bytes_passthrough() {
        let df = data_format();
        let body = Body::Bytes(Bytes::from_static(b"raw"));
        let err = df
            .marshal(body)
            .expect_err("invalid bytes should be rejected");
        assert!(
            err.to_string()
                .contains("protobuf marshal: invalid bytes for type")
        );
    }

    #[test]
    fn test_marshal_valid_bytes_accepted() {
        let df = data_format();
        let bytes = match df
            .marshal(Body::Json(json!({ "name": "Alice" })))
            .expect("marshal should succeed")
        {
            Body::Bytes(b) => b,
            other => panic!("expected bytes, got {other:?}"),
        };
        let out = df
            .marshal(Body::Bytes(bytes.clone()))
            .expect("valid protobuf bytes should be accepted");
        assert_eq!(out, Body::Bytes(bytes));
    }

    #[test]
    fn test_unmarshal_json_passthrough() {
        let df = data_format();
        let body = Body::Json(json!({ "name": "Passthrough" }));
        let out = df
            .unmarshal(body.clone())
            .expect("unmarshal should pass through JSON");
        assert_eq!(out, body);
    }

    #[test]
    fn test_marshal_empty_rejected() {
        let df = data_format();
        let err = df.marshal(Body::Empty).expect_err("empty must be rejected");
        assert!(format!("{err}").contains("empty"));
    }

    #[test]
    fn test_unmarshal_empty_rejected() {
        let df = data_format();
        let err = df
            .unmarshal(Body::Empty)
            .expect_err("empty must be rejected");
        assert!(format!("{err}").contains("empty"));
    }

    #[test]
    fn test_message_not_found_error() {
        let err = ProtobufDataFormat::new(test_proto_path(), "helloworld.DoesNotExist")
            .err()
            .expect("unknown message should fail");
        assert!(matches!(err, CamelError::Config(_)));
    }

    #[test]
    fn test_empty_instance_class_rejected() {
        let config = ProtobufConfig {
            instance_class: Some("".into()),
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_valid_instance_class_accepted() {
        let config = ProtobufConfig {
            instance_class: Some("com.example.MyMessage".into()),
            ..Default::default()
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_no_instance_class_valid() {
        let config = ProtobufConfig::default();
        assert!(config.validate().is_ok());
    }

    /// PROTO-003: Encode/decode roundtrip — marshal JSON to bytes, then unmarshal
    /// those bytes back to JSON and verify the values match the original input.
    #[test]
    fn test_encode_decode_roundtrip() {
        let df = data_format();
        let original = json!({ "name": "RoundtripCharlie" });

        // Encode: JSON → binary protobuf bytes
        let encoded = match df
            .marshal(Body::Json(original.clone()))
            .expect("marshal should succeed")
        {
            Body::Bytes(b) => b,
            other => panic!("expected bytes after marshal, got {other:?}"),
        };
        assert!(!encoded.is_empty(), "encoded bytes should not be empty");

        // Decode: binary protobuf bytes → JSON
        let decoded = match df
            .unmarshal(Body::Bytes(encoded))
            .expect("unmarshal should succeed")
        {
            Body::Json(v) => v,
            other => panic!("expected json after unmarshal, got {other:?}"),
        };

        assert_eq!(decoded, original, "roundtrip should preserve field values");
    }

    #[test]
    fn test_unmarshal_rejects_oversized_bytes() {
        let df = data_format().with_max_decode_bytes(16);
        // 64 raw bytes >> 16-byte cap.
        let body = Body::Bytes(Bytes::from_static(
            b"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        ));
        let err = df.unmarshal(body).unwrap_err(); // allow-unwrap
        assert!(
            format!("{err}").contains("max_decode_bytes"),
            "error should mention max_decode_bytes: {err}"
        );
    }

    #[test]
    fn test_unmarshal_default_cap_accepts_valid_bytes() {
        let df = data_format(); // default 64 MiB cap
        let bytes = match df
            .marshal(Body::Json(json!({ "name": "Alice" })))
            .unwrap() // allow-unwrap
        {
            Body::Bytes(b) => b,
            _ => panic!("expected bytes"),
        };
        let out = df.unmarshal(Body::Bytes(bytes)).unwrap(); // allow-unwrap
        assert!(matches!(out, Body::Json(_)));
    }

    #[test]
    fn test_with_max_decode_bytes_overrides_default() {
        let df = data_format().with_max_decode_bytes(1);
        assert_eq!(df.max_decode_bytes(), 1);
    }

    /// Build a ProtobufDataFormat over the recursive `test.Node` schema, for the
    /// recursion-limit regression test (R5-M2). Mirrors the existing `data_format()`
    /// helper's compile path but points at tests/fixtures/recursive.proto.
    fn recursive_node_data_format() -> ProtobufDataFormat {
        ProtobufDataFormat::new(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("tests")
                .join("fixtures")
                .join("recursive.proto"),
            "test.Node",
        )
        .expect("recursive.proto must compile and expose test.Node") // allow-unwrap
    }

    #[test]
    fn test_unmarshal_recursive_schema_hits_recursion_limit() {
        // R5-M2: prost 0.14 enforces RECURSION_LIMIT=100 (DecodeContext::limit_reached
        // → DecodeErrorKind::RecursionLimitReached), active because `no-recursion-limit`
        // is NOT enabled. prost-reflect routes nested Value::Message through
        // prost::encoding::message::merge, so the limit applies. This test PROVES the
        // guard fires for a recursive schema AND doubles as the guard against a future
        // `no-recursion-limit` feature flip (if flipped, the deep payload would decode
        // instead of erroring → test fails).
        //
        // Schema (tests/fixtures/recursive.proto): message Node { Node child = 1; }
        let df = recursive_node_data_format();
        // Build 200 levels of nesting (>100 limit); innermost = empty message.
        // Wire: field 1, wire type LEN(2) → tag 0x0a, then varint length, then inner.
        let mut payload: Vec<u8> = Vec::new();
        for _ in 0..200 {
            let mut wrapped = vec![0x0a]; // tag: field 1, wire type LEN
            let mut n = payload.len() as u64;
            loop {
                let mut byte = (n & 0x7f) as u8;
                n >>= 7;
                if n != 0 {
                    byte |= 0x80;
                }
                wrapped.push(byte);
                if n == 0 {
                    break;
                }
            }
            wrapped.extend_from_slice(&payload);
            payload = wrapped;
        }
        assert!(
            payload.len() < df.max_decode_bytes(),
            "fixture must sit under byte cap"
        );
        let err = df
            .unmarshal(Body::Bytes(Bytes::from(payload)))
            .expect_err("deeply-nested recursive payload must be rejected"); // allow-unwrap
        let msg = format!("{err}").to_lowercase();
        assert!(
            msg.contains("recursion") || msg.contains("limit"),
            "decode must surface prost's recursion-limit error, got: {msg}"
        );
    }
}
