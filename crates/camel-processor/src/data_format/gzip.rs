use bytes::Bytes;
use camel_api::body::Body;
use camel_api::data_format::DataFormat;
use camel_api::error::CamelError;
use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use serde::Deserialize;
use std::io::{Read, Write};

const DEFAULT_MAX_DECOMPRESSED_SIZE: u64 = 1_073_741_824;
/// Default cap on the materialized input size of `marshal` (R3-L1). The eager
/// marshal collects the whole body into a `Vec<u8>` before compressing; this
/// bounds that allocation.
const DEFAULT_MAX_INPUT_SIZE: u64 = 64 * 1024 * 1024; // 64 MiB

/// Validates the raw level at serde parse time so an out-of-range value fails
/// closed (`RouteError` from the config factory) instead of reaching the
/// encoder. Shared with `tar_gz`, whose config carries the same field. The
/// public config exposes a primitive `Option<u8>` so `flate2` types do not
/// leak through the frozen API; the encoder level is derived internally.
pub(super) fn deserialize_compression_level<'de, D>(deserializer: D) -> Result<Option<u8>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw: Option<u32> = Option::deserialize(deserializer)?;
    match raw {
        None => Ok(None),
        Some(level) if level <= 9 => Ok(Some(level as u8)),
        Some(level) => Err(serde::de::Error::custom(format!(
            "compression_level must be between 0 and 9, got {level}"
        ))),
    }
}

/// Converts the public primitive level into a `flate2::Compression`. Serde
/// already rejects out-of-range values; this guards programmatic construction
/// and fails closed at `marshal` time (mirrors the zip format).
fn to_flate2_compression(level: Option<u8>) -> Result<Compression, CamelError> {
    match level {
        None => Ok(Compression::default()),
        Some(level) if level <= 9 => Ok(Compression::new(u32::from(level))),
        Some(level) => Err(CamelError::TypeConversionFailed(format!(
            "GzipDataFormat::marshal compression_level must be 0-9, got {level}"
        ))),
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GzipConfig {
    pub max_decompressed_size: u64,
    /// Maximum materialized input size accepted by `marshal` (DoS cap, R3-L1).
    pub max_input_size: u64,
    /// Deflate level 0-9; `None` uses the flate2 default (level 6).
    #[serde(deserialize_with = "deserialize_compression_level")]
    pub compression_level: Option<u8>,
}

impl Default for GzipConfig {
    fn default() -> Self {
        Self {
            max_decompressed_size: DEFAULT_MAX_DECOMPRESSED_SIZE,
            max_input_size: DEFAULT_MAX_INPUT_SIZE,
            compression_level: None,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct GzipDataFormat {
    config: GzipConfig,
}

impl GzipDataFormat {
    pub fn new(config: GzipConfig) -> Self {
        Self { config }
    }
}

impl DataFormat for GzipDataFormat {
    fn name(&self) -> &str {
        "gzip"
    }

    fn marshal(&self, body: Body) -> Result<Body, CamelError> {
        let content =
            super::materialize_marshal_input("GzipDataFormat", &body, self.config.max_input_size)?;

        let level = to_flate2_compression(self.config.compression_level)?;
        let mut encoder = GzEncoder::new(Vec::new(), level);
        encoder.write_all(&content).map_err(|e| {
            CamelError::TypeConversionFailed(format!(
                "GzipDataFormat::marshal failed to compress: {e}"
            ))
        })?;
        let compressed = encoder.finish().map_err(|e| {
            CamelError::TypeConversionFailed(format!(
                "GzipDataFormat::marshal failed to finalize gzip stream: {e}"
            ))
        })?;

        Ok(Body::Bytes(Bytes::from(compressed)))
    }

    fn unmarshal(&self, body: Body) -> Result<Body, CamelError> {
        let raw = super::raw_unmarshal_body("GzipDataFormat", "GZIP data", &body)?;

        // Read at most cap + 1 decompressed bytes so a decompression bomb
        // never materializes more than one byte past the limit, and so the
        // overshoot is detectable before returning.
        let limit = self.config.max_decompressed_size.saturating_add(1);
        let mut limited = GzDecoder::new(std::io::Cursor::new(&raw)).take(limit);
        let mut data = Vec::new();
        limited.read_to_end(&mut data).map_err(|e| {
            CamelError::TypeConversionFailed(format!(
                "GzipDataFormat::unmarshal invalid GZIP stream: {e}"
            ))
        })?;

        if data.len() as u64 > self.config.max_decompressed_size {
            return Err(CamelError::TypeConversionFailed(format!(
                "GzipDataFormat::unmarshal decompressed size exceeds max_decompressed_size {}",
                self.config.max_decompressed_size
            )));
        }

        Ok(Body::Bytes(Bytes::from(data)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;

    use super::super::test_util::{assert_bytes, stream_body_pair};

    #[test]
    fn test_name() {
        let df = GzipDataFormat::default();
        assert_eq!(df.name(), "gzip");
    }

    #[test]
    fn test_gzip_config_deserialize_from_json() {
        let json = serde_json::json!({
            "max_decompressed_size": 2147483648u64,
            "max_input_size": 134217728u64,
            "compression_level": 9
        });
        let cfg: GzipConfig = serde_json::from_value(json).unwrap();
        assert_eq!(cfg.max_decompressed_size, 2147483648);
        assert_eq!(cfg.max_input_size, 134217728);
        assert_eq!(cfg.compression_level, Some(9));
    }

    #[test]
    fn test_gzip_config_compression_level_boundaries() {
        for level in [0u8, 9] {
            let json = serde_json::json!({ "compression_level": level });
            let cfg: GzipConfig = serde_json::from_value(json).unwrap();
            assert_eq!(cfg.compression_level, Some(level));
        }
    }

    #[test]
    fn test_gzip_programmatic_out_of_range_level_fails_closed() {
        let df = GzipDataFormat::new(GzipConfig {
            compression_level: Some(10),
            ..Default::default()
        });
        let result = df.marshal(Body::Bytes(Bytes::from_static(b"payload")));
        let msg = format!("{}", result.unwrap_err());
        assert!(
            msg.contains("compression_level must be 0-9"),
            "error should mention the level bound: {msg}"
        );
    }

    #[test]
    fn gzip_round_trip_with_explicit_compression_levels() {
        for level in [0u8, 9] {
            let df = GzipDataFormat::new(GzipConfig {
                compression_level: Some(level),
                ..Default::default()
            });
            let original = Body::Bytes(Bytes::from_static(b"explicit level payload"));
            let restored = df.unmarshal(df.marshal(original.clone()).unwrap()).unwrap();
            assert_eq!(restored, original, "round trip failed for level {level}");
        }
    }

    #[test]
    fn test_gzip_config_deny_unknown_fields() {
        let json = serde_json::json!({"unknown_key": 42});
        let result: Result<GzipConfig, _> = serde_json::from_value(json);
        assert!(result.is_err());
    }

    #[test]
    fn test_gzip_config_invalid_compression_level_fails_closed() {
        let json = serde_json::json!({"compression_level": 10});
        let result: Result<GzipConfig, _> = serde_json::from_value(json);
        assert!(result.is_err());
    }

    #[test]
    fn gzip_round_trip_bytes() {
        let df = GzipDataFormat::default();
        let original = Body::Bytes(Bytes::from_static(b"\x00\x01\x02\xffbinary payload"));
        let compressed = df.marshal(original.clone()).unwrap();
        let compressed_bytes = match &compressed {
            Body::Bytes(b) => b.clone(),
            _ => panic!("expected Body::Bytes"),
        };
        assert_eq!(
            &compressed_bytes[..2],
            &[0x1f, 0x8b],
            "marshal must emit standalone gzip bytes"
        );
        let restored = df.unmarshal(compressed).unwrap();
        assert_eq!(restored, original);
    }

    #[test]
    fn gzip_tar_cross_format_round_trip() {
        // Composed `tar` then `gzip` must decode back through `gzip` then
        // `tar` to the original bytes (wire-compatibility contract).
        let tar_df = super::super::tar::TarDataFormat::default();
        let gzip_df = GzipDataFormat::default();
        let original = Body::Bytes(Bytes::from_static(b"\x00\x01\x02\xfftar.gz payload"));

        let archived = tar_df.marshal(original.clone()).unwrap();
        let compressed = gzip_df.marshal(archived.clone()).unwrap();
        let decompressed = gzip_df.unmarshal(compressed).unwrap();
        assert_eq!(
            decompressed, archived,
            "gzip layer must preserve the TAR archive exactly"
        );
        let restored = tar_df.unmarshal(decompressed).unwrap();
        assert_eq!(restored, original);
    }

    #[test]
    fn gzip_decompression_limit_covers_full_stream() {
        let config = GzipConfig {
            max_decompressed_size: 16,
            ..Default::default()
        };
        let df = GzipDataFormat::new(config);
        // 4 KiB of payload decompresses far past the 16-byte cap; the decoder
        // must stop at cap + 1 bytes instead of materializing the full stream.
        let compressed = GzipDataFormat::default()
            .marshal(Body::Bytes(Bytes::from(vec![b'A'; 4096])))
            .unwrap();
        let result = df.unmarshal(compressed);
        match result {
            Err(CamelError::TypeConversionFailed(msg)) => {
                assert!(
                    msg.contains("max_decompressed_size"),
                    "error should mention max_decompressed_size: {msg}"
                );
            }
            _ => panic!("decompressed stream beyond the cap must be rejected"),
        }
    }

    #[test]
    fn malformed_gzip_input_rejected() {
        let df = GzipDataFormat::default();

        // Garbage bytes fail the gzip magic/header check.
        let garbage = vec![b'G'; 64];
        let result = df.unmarshal(Body::Bytes(Bytes::from(garbage)));
        match result {
            Err(CamelError::TypeConversionFailed(_)) => {}
            _ => panic!("garbage input must yield TypeConversionFailed"),
        }

        // A stream truncated before the CRC/ISIZE trailer fails validation.
        let full = match GzipDataFormat::default()
            .marshal(Body::Bytes(Bytes::from_static(b"hello world")))
        {
            Ok(Body::Bytes(b)) => b.to_vec(),
            other => panic!("marshal must yield compressed bytes: {other:?}"),
        };
        let truncated = full[..full.len() - 4].to_vec();
        let result = df.unmarshal(Body::Bytes(Bytes::from(truncated)));
        match result {
            Err(CamelError::TypeConversionFailed(_)) => {}
            _ => panic!("truncated gzip input must yield TypeConversionFailed"),
        }
    }

    #[test]
    fn gzip_materialized_empty_and_stream_bodies() {
        let df = GzipDataFormat::default();

        // Zero-length materialized payloads are valid bodies: marshal yields
        // a well-formed gzip stream; unmarshal returns the empty payload.
        for body in [Body::Bytes(Bytes::new()), Body::Text(String::new())] {
            let compressed = df.marshal(body).unwrap();
            let restored = df.unmarshal(compressed).unwrap();
            assert_bytes(restored, b"");
        }

        // Body::Empty fails marshal.
        assert!(df.marshal(Body::Empty).is_err());

        // Body::Stream fails marshal and unmarshal without consuming the
        // stream.
        let (body, slot) = stream_body_pair();
        assert!(df.marshal(body).is_err());
        assert!(
            slot.blocking_lock().is_some(),
            "marshal must not consume the stream"
        );

        let (body, slot) = stream_body_pair();
        assert!(df.unmarshal(body).is_err());
        assert!(
            slot.blocking_lock().is_some(),
            "unmarshal must not consume the stream"
        );
    }

    #[test]
    fn test_marshal_input_size_cap() {
        let config = GzipConfig {
            max_input_size: 16,
            ..Default::default()
        };
        let df = GzipDataFormat::new(config);
        let result = df.marshal(Body::Text("x".repeat(64)));
        assert!(result.is_err());
        let msg = format!("{}", result.unwrap_err());
        assert!(
            msg.contains("max_input_size"),
            "error should mention max_input_size: {msg}"
        );
    }
}
