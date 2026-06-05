//! Stateless proto-conversion helpers.

use std::sync::OnceLock;

use bytes::BytesMut;
use camel_api::CamelError;
use camel_proto_compiler::ProtoCache;
use prost::Message as _;
use prost_reflect::{DynamicMessage, MessageDescriptor};
use tracing::warn;

static PROTO_CACHE: OnceLock<ProtoCache> = OnceLock::new();

pub(crate) fn proto_cache() -> &'static ProtoCache {
    PROTO_CACHE.get_or_init(ProtoCache::new)
}

pub(crate) fn json_to_protobuf(
    json: serde_json::Value,
    desc: MessageDescriptor,
) -> Result<Vec<u8>, CamelError> {
    let json_str = serde_json::to_string(&json)
        .map_err(|e| CamelError::TypeConversionFailed(format!("failed to serialize JSON: {e}")))?;
    let mut de = serde_json::Deserializer::from_str(&json_str);
    let dyn_msg = DynamicMessage::deserialize(desc, &mut de).map_err(|e| {
        warn!(error = %e, "proto conversion failed");
        CamelError::TypeConversionFailed(format!("failed to parse JSON into protobuf: {e}"))
    })?;
    let mut buf = BytesMut::new();
    dyn_msg.encode(&mut buf).map_err(|e| {
        warn!(error = %e, "proto conversion failed");
        CamelError::TypeConversionFailed(format!("failed to encode protobuf message: {e}"))
    })?;
    Ok(buf.to_vec())
}

pub(crate) fn protobuf_to_json(
    bytes: Vec<u8>,
    desc: MessageDescriptor,
) -> Result<serde_json::Value, CamelError> {
    let dyn_msg = DynamicMessage::decode(desc, bytes.as_slice()).map_err(|e| {
        warn!(error = %e, "proto conversion failed");
        CamelError::TypeConversionFailed(format!("failed to decode protobuf bytes: {e}"))
    })?;
    serde_json::to_value(&dyn_msg).map_err(|e| {
        warn!(error = %e, "proto conversion failed");
        CamelError::TypeConversionFailed(format!("failed to serialize protobuf to JSON: {e}"))
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{json_to_protobuf, proto_cache, protobuf_to_json};

    // ── protobuf roundtrip tests ───────────────────────────────────────

    #[test]
    fn test_json_to_protobuf_roundtrip() {
        let proto_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/helloworld.proto");
        let cache = proto_cache();
        let pool = cache
            .get_or_compile(&proto_path, std::iter::empty::<&std::path::Path>())
            .unwrap();
        let svc = pool.get_service_by_name("helloworld.Greeter").unwrap();
        let method = svc.methods().find(|m| m.name() == "SayHello").unwrap();
        let desc = method.input();

        let json = serde_json::json!({"name": "World"});
        let bytes = json_to_protobuf(json, desc.clone()).unwrap();
        assert!(!bytes.is_empty());

        let decoded = protobuf_to_json(bytes, desc).unwrap();
        assert_eq!(decoded["name"], "World");
    }

    #[test]
    fn test_json_to_protobuf_invalid_field_type() {
        let proto_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/helloworld.proto");
        let cache = proto_cache();
        let pool = cache
            .get_or_compile(&proto_path, std::iter::empty::<&std::path::Path>())
            .unwrap();
        let svc = pool.get_service_by_name("helloworld.Greeter").unwrap();
        let method = svc.methods().find(|m| m.name() == "SayHello").unwrap();
        let desc = method.input();

        let json = serde_json::json!({"name": 123});
        let result = json_to_protobuf(json, desc);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("failed to parse JSON into protobuf")
        );
    }

    #[test]
    fn test_protobuf_to_json_invalid_bytes() {
        let proto_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/helloworld.proto");
        let cache = proto_cache();
        let pool = cache
            .get_or_compile(&proto_path, std::iter::empty::<&std::path::Path>())
            .unwrap();
        let svc = pool.get_service_by_name("helloworld.Greeter").unwrap();
        let method = svc.methods().find(|m| m.name() == "SayHello").unwrap();
        let desc = method.input();

        let result = protobuf_to_json(vec![0xFF, 0xFE, 0xFD], desc);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("failed to decode protobuf bytes")
        );
    }

    #[test]
    fn test_proto_cache_returns_singleton() {
        let first = proto_cache();
        let second = proto_cache();
        assert!(std::ptr::eq(first, second));
    }
}
