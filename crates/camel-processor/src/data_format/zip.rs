use bytes::Bytes;
use camel_api::body::Body;
use camel_api::data_format::DataFormat;
use camel_api::error::CamelError;
use serde::Deserialize;
use std::io::Read;
use std::io::Write;
use zip::ZipArchive;

const DEFAULT_MAX_DECOMPRESSED_SIZE: u64 = 1_073_741_824;
/// Default cap on the materialized input size of `marshal` (R3-L1). The eager
/// marshal collects the whole body into a `Vec<u8>` before compression; this
/// bounds that allocation.
const DEFAULT_MAX_INPUT_SIZE: u64 = 64 * 1024 * 1024; // 64 MiB
const ENTRY_NAME: &str = "payload";

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ZipConfig {
    pub max_decompressed_size: u64,
    /// Maximum materialized input size accepted by `marshal` (DoS cap, R3-L1).
    pub max_input_size: u64,
    pub compression_level: Option<i32>,
    pub allow_multi_entry: bool,
}

impl Default for ZipConfig {
    fn default() -> Self {
        Self {
            max_decompressed_size: DEFAULT_MAX_DECOMPRESSED_SIZE,
            max_input_size: DEFAULT_MAX_INPUT_SIZE,
            compression_level: None,
            allow_multi_entry: false,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ZipDataFormat {
    config: ZipConfig,
}

impl ZipDataFormat {
    pub fn new(config: ZipConfig) -> Self {
        Self { config }
    }
}

impl DataFormat for ZipDataFormat {
    fn name(&self) -> &str {
        "zip"
    }

    fn marshal(&self, body: Body) -> Result<Body, CamelError> {
        let content: Vec<u8> = match &body {
            Body::Text(s) => s.as_bytes().to_vec(),
            Body::Json(v) => serde_json::to_vec(v).map_err(|e| {
                CamelError::TypeConversionFailed(format!(
                    "ZipDataFormat::marshal cannot serialize JSON: {e}"
                ))
            })?,
            Body::Bytes(b) => b.to_vec(),
            Body::Xml(s) => s.as_bytes().to_vec(),
            Body::Empty => {
                return Err(CamelError::TypeConversionFailed(
                    "ZipDataFormat::marshal requires non-empty body".to_string(),
                ));
            }
            Body::Stream(_) => {
                return Err(CamelError::TypeConversionFailed(
                    "cannot marshal Body::Stream — add 'stream_cache' or 'convert_body_to' before this step".to_string(),
                ));
            }
        };

        if content.len() as u64 > self.config.max_input_size {
            return Err(CamelError::TypeConversionFailed(format!(
                "ZipDataFormat::marshal input {} bytes exceeds max_input_size {}",
                content.len(),
                self.config.max_input_size
            )));
        }

        let mut buf = Vec::new();
        {
            let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            let mut options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            if let Some(level) = self.config.compression_level {
                if !(0..=9).contains(&level) {
                    return Err(CamelError::TypeConversionFailed(format!(
                        "ZipDataFormat::marshal compression_level must be 0-9, got {level}"
                    )));
                }
                options = options.compression_level(Some(level as i64));
            }
            writer.start_file(ENTRY_NAME, options).map_err(|e| {
                CamelError::TypeConversionFailed(format!(
                    "ZipDataFormat::marshal failed to start entry: {e}"
                ))
            })?;
            writer.write_all(&content).map_err(|e| {
                CamelError::TypeConversionFailed(format!(
                    "ZipDataFormat::marshal failed to write entry: {e}"
                ))
            })?;
            writer.finish().map_err(|e| {
                CamelError::TypeConversionFailed(format!(
                    "ZipDataFormat::marshal failed to finalize archive: {e}"
                ))
            })?;
        }

        Ok(Body::Bytes(Bytes::from(buf)))
    }

    fn unmarshal(&self, body: Body) -> Result<Body, CamelError> {
        let raw: Vec<u8> = match &body {
            Body::Bytes(b) => b.to_vec(),
            Body::Text(s) => s.as_bytes().to_vec(),
            Body::Empty => {
                return Err(CamelError::TypeConversionFailed(
                    "ZipDataFormat::unmarshal requires non-empty body".to_string(),
                ));
            }
            Body::Stream(_) => {
                return Err(CamelError::TypeConversionFailed(
                    "cannot unmarshal Body::Stream — use UnmarshalService which auto-materializes"
                        .to_string(),
                ));
            }
            Body::Json(_) | Body::Xml(_) => {
                return Err(CamelError::TypeConversionFailed(
                    "ZipDataFormat::unmarshal only supports Body::Bytes and Body::Text (ZIP data)"
                        .to_string(),
                ));
            }
        };

        let reader = std::io::Cursor::new(&raw);
        let mut archive = ZipArchive::new(reader).map_err(|e| {
            CamelError::TypeConversionFailed(format!("ZipDataFormat::unmarshal invalid ZIP: {e}"))
        })?;

        if archive.is_empty() {
            return Err(CamelError::TypeConversionFailed(
                "ZipDataFormat::unmarshal ZIP archive has no entries".to_string(),
            ));
        }

        if archive.len() > 1 && !self.config.allow_multi_entry {
            return Err(CamelError::TypeConversionFailed(format!(
                "ZipDataFormat::unmarshal ZIP has {} entries but allow_multi_entry is false",
                archive.len()
            )));
        }

        if archive.len() > 1 {
            tracing::warn!(
                entries = archive.len(),
                "ZIP archive has multiple entries, extracting first only"
            );
        }

        let mut entry = archive.by_index(0).map_err(|e| {
            CamelError::TypeConversionFailed(format!(
                "ZipDataFormat::unmarshal failed to read entry: {e}"
            ))
        })?;

        let mut decompressed = Vec::new();
        let limit = self.config.max_decompressed_size.saturating_add(1);
        let mut limited = std::io::Read::take(&mut entry, limit);
        limited.read_to_end(&mut decompressed).map_err(|e| {
            CamelError::TypeConversionFailed(format!(
                "ZipDataFormat::unmarshal failed to decompress: {e}"
            ))
        })?;

        if decompressed.len() as u64 > self.config.max_decompressed_size {
            return Err(CamelError::TypeConversionFailed(format!(
                "ZipDataFormat::unmarshal decompressed size exceeds max {}",
                self.config.max_decompressed_size
            )));
        }

        Ok(Body::Bytes(Bytes::from(decompressed)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use serde_json::json;
    use std::io::Cursor;
    use std::io::Read;
    use zip::ZipArchive;

    fn extract_single_entry(zip_bytes: &[u8]) -> Vec<u8> {
        let reader = Cursor::new(zip_bytes);
        let mut archive = ZipArchive::new(reader).unwrap();
        let mut entry = archive.by_index(0).unwrap();
        let name = entry.name().to_string();
        assert_eq!(name, "payload");
        let mut buf = Vec::new();
        entry.read_to_end(&mut buf).unwrap();
        buf
    }

    #[test]
    fn test_name() {
        let df = ZipDataFormat::default();
        assert_eq!(df.name(), "zip");
    }

    #[test]
    fn test_marshal_text_to_zip() {
        let df = ZipDataFormat::default();
        let body = Body::Text("hello world".to_string());
        let result = df.marshal(body).unwrap();
        match result {
            Body::Bytes(b) => {
                let decompressed = extract_single_entry(&b);
                assert_eq!(decompressed, b"hello world");
            }
            _ => panic!("expected Body::Bytes"),
        }
    }

    #[test]
    fn test_marshal_json_to_zip() {
        let df = ZipDataFormat::default();
        let body = Body::Json(json!({"key": "value"}));
        let result = df.marshal(body).unwrap();
        match result {
            Body::Bytes(b) => {
                let decompressed = extract_single_entry(&b);
                let original = serde_json::to_vec(&json!({"key": "value"})).unwrap();
                assert_eq!(decompressed, original);
            }
            _ => panic!("expected Body::Bytes"),
        }
    }

    #[test]
    fn test_marshal_bytes_to_zip() {
        let df = ZipDataFormat::default();
        let body = Body::Bytes(Bytes::from_static(b"raw bytes"));
        let result = df.marshal(body).unwrap();
        match result {
            Body::Bytes(b) => {
                let decompressed = extract_single_entry(&b);
                assert_eq!(decompressed, b"raw bytes");
            }
            _ => panic!("expected Body::Bytes"),
        }
    }

    #[test]
    fn test_marshal_xml_to_zip() {
        let df = ZipDataFormat::default();
        let body = Body::Xml("<root><item>1</item></root>".to_string());
        let result = df.marshal(body).unwrap();
        match result {
            Body::Bytes(b) => {
                let decompressed = extract_single_entry(&b);
                assert_eq!(decompressed, b"<root><item>1</item></root>");
            }
            _ => panic!("expected Body::Bytes"),
        }
    }

    #[test]
    fn test_marshal_empty_error() {
        let df = ZipDataFormat::default();
        let result = df.marshal(Body::Empty);
        assert!(result.is_err());
    }

    #[test]
    fn test_marshal_stream_error() {
        use camel_api::body::{StreamBody, StreamMetadata};
        use futures::stream;
        use std::sync::Arc;
        use tokio::sync::Mutex;

        let stream = stream::iter(vec![Ok(Bytes::from_static(b"data"))]);
        let body = Body::Stream(StreamBody {
            stream: Arc::new(Mutex::new(Some(Box::pin(stream)))),
            metadata: StreamMetadata::default(),
        });
        let df = ZipDataFormat::default();
        let result = df.marshal(body);
        assert!(result.is_err());
    }

    fn make_zip(content: &[u8]) -> Vec<u8> {
        let mut buf = Vec::new();
        {
            let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            writer.start_file("payload", options).unwrap();
            writer.write_all(content).unwrap();
            writer.finish().unwrap();
        }
        buf
    }

    #[test]
    fn test_unmarshal_zip_bytes() {
        let df = ZipDataFormat::default();
        let zip_data = make_zip(b"decompressed content");
        let body = Body::Bytes(Bytes::from(zip_data));
        let result = df.unmarshal(body).unwrap();
        match result {
            Body::Bytes(b) => assert_eq!(b.as_ref(), b"decompressed content"),
            _ => panic!("expected Body::Bytes"),
        }
    }

    #[test]
    fn test_unmarshal_zip_text() {
        let df = ZipDataFormat::default();
        let content = b"text from text body";
        let zip_data = make_zip(content);
        let text_body = unsafe { String::from_utf8_unchecked(zip_data) };
        let body = Body::Text(text_body);
        let result = df.unmarshal(body).unwrap();
        match result {
            Body::Bytes(b) => assert_eq!(b.as_ref(), content),
            _ => panic!("expected Body::Bytes"),
        }
    }

    #[test]
    fn test_unmarshal_invalid_zip_error() {
        let df = ZipDataFormat::default();
        let body = Body::Bytes(Bytes::from_static(b"not a zip file"));
        let result = df.unmarshal(body);
        assert!(result.is_err());
    }

    #[test]
    fn test_unmarshal_empty_zip_error() {
        let mut buf = Vec::new();
        {
            let writer = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            writer.finish().unwrap();
        }
        let df = ZipDataFormat::default();
        let body = Body::Bytes(Bytes::from(buf));
        let result = df.unmarshal(body);
        assert!(result.is_err());
    }

    #[test]
    fn test_unmarshal_json_error() {
        let df = ZipDataFormat::default();
        let body = Body::Json(json!({"not": "zip"}));
        let result = df.unmarshal(body);
        assert!(result.is_err());
    }

    #[test]
    fn test_unmarshal_xml_error() {
        let df = ZipDataFormat::default();
        let body = Body::Xml("<root/>".to_string());
        let result = df.unmarshal(body);
        assert!(result.is_err());
    }

    #[test]
    fn test_unmarshal_multi_entry_error() {
        let mut buf = Vec::new();
        {
            let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            let options = zip::write::SimpleFileOptions::default();
            writer.start_file("file1.txt", options).unwrap();
            writer.write_all(b"one").unwrap();
            writer.start_file("file2.txt", options).unwrap();
            writer.write_all(b"two").unwrap();
            writer.finish().unwrap();
        }
        let df = ZipDataFormat::default();
        let body = Body::Bytes(Bytes::from(buf));
        let result = df.unmarshal(body);
        assert!(result.is_err());
    }

    #[test]
    fn test_unmarshal_multi_entry_allowed() {
        let mut buf = Vec::new();
        {
            let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            let options = zip::write::SimpleFileOptions::default();
            writer.start_file("file1.txt", options).unwrap();
            writer.write_all(b"first").unwrap();
            writer.start_file("file2.txt", options).unwrap();
            writer.write_all(b"second").unwrap();
            writer.finish().unwrap();
        }
        let config = ZipConfig {
            allow_multi_entry: true,
            ..Default::default()
        };
        let df = ZipDataFormat::new(config);
        let body = Body::Bytes(Bytes::from(buf));
        let result = df.unmarshal(body).unwrap();
        match result {
            Body::Bytes(b) => assert_eq!(b.as_ref(), b"first"),
            _ => panic!("expected Body::Bytes"),
        }
    }

    #[test]
    fn test_roundtrip_text() {
        let df = ZipDataFormat::default();
        let original = Body::Text("roundtrip text content".to_string());
        let compressed = df.marshal(original).unwrap();
        let decompressed = df.unmarshal(compressed).unwrap();
        match decompressed {
            Body::Bytes(b) => assert_eq!(b.as_ref(), b"roundtrip text content"),
            _ => panic!("expected Body::Bytes"),
        }
    }

    #[test]
    fn test_roundtrip_json() {
        let df = ZipDataFormat::default();
        let original = Body::Json(json!({"round": "trip"}));
        let compressed = df.marshal(original).unwrap();
        let decompressed = df.unmarshal(compressed).unwrap();
        match decompressed {
            Body::Bytes(b) => {
                let v: serde_json::Value = serde_json::from_slice(&b).unwrap();
                assert_eq!(v, json!({"round": "trip"}));
            }
            _ => panic!("expected Body::Bytes"),
        }
    }

    #[test]
    fn test_roundtrip_bytes() {
        let df = ZipDataFormat::default();
        let original = Body::Bytes(Bytes::from_static(b"\x00\x01\x02\xff"));
        let compressed = df.marshal(original).unwrap();
        let decompressed = df.unmarshal(compressed).unwrap();
        match decompressed {
            Body::Bytes(b) => assert_eq!(b.as_ref(), b"\x00\x01\x02\xff"),
            _ => panic!("expected Body::Bytes"),
        }
    }

    #[test]
    fn test_max_decompressed_size_exceeded() {
        let config = ZipConfig {
            max_decompressed_size: 10,
            ..Default::default()
        };
        let df = ZipDataFormat::new(config);
        let zip_data = make_zip(b"this content is way longer than 10 bytes");
        let body = Body::Bytes(Bytes::from(zip_data));
        let result = df.unmarshal(body);
        assert!(result.is_err());
    }

    #[test]
    fn test_unmarshal_empty_error() {
        let df = ZipDataFormat::default();
        let result = df.unmarshal(Body::Empty);
        assert!(result.is_err());
    }

    #[test]
    fn test_unmarshal_stream_error() {
        use camel_api::body::{StreamBody, StreamMetadata};
        use futures::stream;
        use std::sync::Arc;
        use tokio::sync::Mutex;

        let stream = stream::iter(vec![Ok(Bytes::from_static(b"data"))]);
        let body = Body::Stream(StreamBody {
            stream: Arc::new(Mutex::new(Some(Box::pin(stream)))),
            metadata: StreamMetadata::default(),
        });
        let df = ZipDataFormat::default();
        let result = df.unmarshal(body);
        assert!(result.is_err());
    }

    #[test]
    fn test_marshal_invalid_compression_level() {
        let config = ZipConfig {
            compression_level: Some(42),
            ..Default::default()
        };
        let df = ZipDataFormat::new(config);
        let result = df.marshal(Body::Text("test".to_string()));
        assert!(result.is_err());
    }

    #[test]
    fn test_marshal_input_size_cap() {
        let config = ZipConfig {
            max_input_size: 16,
            ..Default::default()
        };
        let df = ZipDataFormat::new(config);
        let body = Body::Text("x".repeat(64));
        let result = df.marshal(body);
        assert!(result.is_err());
        let msg = format!("{}", result.unwrap_err());
        assert!(
            msg.contains("max_input_size"),
            "error should mention max_input_size: {msg}"
        );
    }

    #[test]
    fn test_marshal_default_cap_accepts_small_input() {
        let df = ZipDataFormat::default();
        let body = Body::Text("hello".to_string());
        let result = df.marshal(body).unwrap();
        assert!(matches!(result, Body::Bytes(_)));
    }

    #[test]
    fn test_builtin_zip_registered() {
        let df = super::super::builtin_data_format("zip");
        assert!(df.is_some());
        assert_eq!(df.unwrap().name(), "zip");
    }

    #[test]
    fn test_zip_config_deserialize_from_json() {
        let json = serde_json::json!({
            "max_decompressed_size": 2147483648u64,
            "max_input_size": 134217728u64,
            "compression_level": 6,
            "allow_multi_entry": true
        });
        let cfg: ZipConfig = serde_json::from_value(json).unwrap();
        assert_eq!(cfg.max_decompressed_size, 2147483648);
        assert_eq!(cfg.compression_level, Some(6));
        assert!(cfg.allow_multi_entry);
    }

    #[test]
    fn test_zip_config_deny_unknown_fields() {
        let json = serde_json::json!({"unknown_key": 42});
        let result: Result<ZipConfig, _> = serde_json::from_value(json);
        assert!(result.is_err());
    }
}
