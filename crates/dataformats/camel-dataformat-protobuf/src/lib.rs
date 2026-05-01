use std::path::Path;

use bytes::BytesMut;
use camel_api::body::Body;
use camel_api::data_format::DataFormat;
use camel_api::error::CamelError;
use camel_proto_compiler::{ProtoCache, compile_proto};
use prost::Message;
use prost_reflect::{DynamicMessage, MessageDescriptor};

pub struct ProtobufDataFormat {
    descriptor: MessageDescriptor,
}

impl ProtobufDataFormat {
    pub fn new<P: AsRef<Path>>(proto_path: P, message_name: &str) -> Result<Self, CamelError> {
        let pool =
            compile_proto(proto_path.as_ref(), std::iter::empty::<&Path>()).map_err(|e| {
                CamelError::TypeConversionFailed(format!("failed to compile proto: {e}"))
            })?;
        let descriptor = pool.get_message_by_name(message_name).ok_or_else(|| {
            CamelError::TypeConversionFailed(format!(
                "message descriptor not found: {message_name}"
            ))
        })?;
        Ok(Self { descriptor })
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
            CamelError::TypeConversionFailed(format!(
                "message descriptor not found: {message_name}"
            ))
        })?;
        Ok(Self { descriptor })
    }

    pub fn descriptor(&self) -> &MessageDescriptor {
        &self.descriptor
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
            Body::Bytes(bytes) => Ok(Body::Bytes(bytes)),
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
    use serde_json::json;

    use super::ProtobufDataFormat;

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
        let out = df.marshal(body).expect("marshal should pass through bytes");
        assert_eq!(out, Body::Bytes(Bytes::from_static(b"raw")));
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
        assert!(format!("{err}").contains("message descriptor not found"));
    }
}
