use camel_api::body::Body;
use camel_api::data_format::DataFormat;
use camel_api::error::CamelError;
use serde::Deserialize;

use super::gzip::{GzipConfig, GzipDataFormat};
use super::tar::{TarConfig, TarDataFormat};

const DEFAULT_MAX_DECOMPRESSED_SIZE: u64 = 1_073_741_824;
/// Default cap on the materialized input size of `marshal` (R3-L1). Enforced
/// by the inner TAR layer on the raw body before archiving; this bounds that
/// allocation.
const DEFAULT_MAX_INPUT_SIZE: u64 = 64 * 1024 * 1024; // 64 MiB

/// Worst-case TAR framing the inner gzip layer sees above the raw body: one
/// 512-byte header, up to 511 bytes of payload padding, and the 1024-byte
/// end-of-archive marker (rounded up to 2048). The inner gzip `max_input_size`
/// is widened by this bound so a body accepted by the outer cap always
/// produces an archive the gzip layer accepts — the effective raw-body bound
/// is unchanged because the TAR layer rejects oversized bodies first.
const TAR_FRAMING_BOUND: u64 = 512 + 512 + 1024;

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TarGzConfig {
    /// Cap on every byte the gzip decoder emits during `unmarshal` — headers,
    /// padding, and skipped entries included, not only the selected payload.
    pub max_decompressed_size: u64,
    /// Maximum materialized input size accepted by `marshal` (DoS cap, R3-L1).
    pub max_input_size: u64,
    /// TAR entry policy: when false, archives with more than one regular file
    /// are rejected instead of returning only the first.
    pub allow_multi_entry: bool,
    /// Deflate level 0-9; `None` uses the flate2 default (level 6).
    #[serde(deserialize_with = "super::gzip::deserialize_compression_level")]
    pub compression_level: Option<u8>,
}

impl Default for TarGzConfig {
    fn default() -> Self {
        Self {
            max_decompressed_size: DEFAULT_MAX_DECOMPRESSED_SIZE,
            max_input_size: DEFAULT_MAX_INPUT_SIZE,
            allow_multi_entry: false,
            compression_level: None,
        }
    }
}

/// Combined `tar.gz` format implemented by composing the existing
/// [`TarDataFormat`] and [`GzipDataFormat`]: `marshal = gzip(tar(body))` and
/// `unmarshal = tar(gzip(body))`. The inner configs carry the outer caps so
/// the composition is behaviorally identical to running the two formats in
/// sequence.
#[derive(Debug, Clone)]
pub struct TarGzDataFormat {
    tar: TarDataFormat,
    gzip: GzipDataFormat,
}

impl Default for TarGzDataFormat {
    fn default() -> Self {
        Self::new(TarGzConfig::default())
    }
}

impl TarGzDataFormat {
    pub fn new(config: TarGzConfig) -> Self {
        // The TAR layer materializes the raw body, so it enforces the outer
        // `max_input_size` exactly where the allocation happens. Its
        // `max_input_size` and per-entry output limits are irrelevant to
        // `unmarshal` (the gzip layer bounds the stream first) but map to the
        // same outer values so both directions share one bound.
        let tar = TarDataFormat::new(TarConfig {
            max_decompressed_size: config.max_decompressed_size,
            max_input_size: config.max_input_size,
            allow_multi_entry: config.allow_multi_entry,
        });
        // The gzip layer caps every decoded byte — TAR headers, padding, and
        // skipped entries included — before any TAR parsing happens. On
        // `marshal` it consumes the archive, whose size is the body plus
        // bounded framing (see `TAR_FRAMING_BOUND`).
        let gzip = GzipDataFormat::new(GzipConfig {
            max_decompressed_size: config.max_decompressed_size,
            max_input_size: config.max_input_size.saturating_add(TAR_FRAMING_BOUND),
            compression_level: config.compression_level,
        });
        Self { tar, gzip }
    }
}

impl DataFormat for TarGzDataFormat {
    fn name(&self) -> &str {
        "tar.gz"
    }

    /// `gzip(tar(body))`: the TAR layer writes one regular-file entry named
    /// `payload` — never an attacker-controlled path — and enforces the
    /// raw-body input cap; the gzip layer compresses the archive so `tar.gz`
    /// output decodes through `gzip` then `tar`.
    fn marshal(&self, body: Body) -> Result<Body, CamelError> {
        self.gzip.marshal(self.tar.marshal(body)?)
    }

    /// `tar(gzip(body))`: the gzip layer first caps the full decoded stream
    /// at `max_decompressed_size`, then the TAR layer performs regular-file
    /// selection and the `allow_multi_entry` policy. Names are never used
    /// for I/O and non-regular entries are skipped without materializing
    /// their payload (path-confinement precedent rc-0ks57; v1 performs no
    /// disk I/O).
    fn unmarshal(&self, body: Body) -> Result<Body, CamelError> {
        self.tar.unmarshal(self.gzip.unmarshal(body)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;

    use super::super::gzip::GzipDataFormat;
    use super::super::tar::TarDataFormat;
    use super::super::test_util::{
        CHARACTER_DEVICE, DIRECTORY, HARDLINK, REGULAR, SYMLINK, assert_bytes, capture_warns,
        make_tar, stream_body_pair,
    };

    fn gzip_bytes(raw: Vec<u8>) -> Body {
        GzipDataFormat::default()
            .marshal(Body::Bytes(Bytes::from(raw)))
            .unwrap()
    }

    #[test]
    fn test_name() {
        let df = TarGzDataFormat::default();
        assert_eq!(df.name(), "tar.gz");
    }

    #[test]
    fn test_tar_gz_config_deserialize_from_json() {
        let json = serde_json::json!({
            "max_decompressed_size": 2147483648u64,
            "max_input_size": 134217728u64,
            "allow_multi_entry": true,
            "compression_level": 9
        });
        let cfg: TarGzConfig = serde_json::from_value(json).unwrap();
        assert_eq!(cfg.max_decompressed_size, 2147483648);
        assert_eq!(cfg.max_input_size, 134217728);
        assert!(cfg.allow_multi_entry);
        assert_eq!(cfg.compression_level, Some(9));
    }

    #[test]
    fn tar_gz_round_trip_with_explicit_compression_level() {
        let df = TarGzDataFormat::new(TarGzConfig {
            compression_level: Some(9),
            ..Default::default()
        });
        let original = Body::Bytes(Bytes::from_static(b"level nine payload"));
        let restored = df.unmarshal(df.marshal(original.clone()).unwrap()).unwrap();
        assert_eq!(restored, original);
    }

    #[test]
    fn tar_gz_programmatic_out_of_range_level_fails_closed() {
        let df = TarGzDataFormat::new(TarGzConfig {
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
    fn test_tar_gz_config_deny_unknown_fields() {
        let json = serde_json::json!({"unknown_key": 42});
        let result: Result<TarGzConfig, _> = serde_json::from_value(json);
        assert!(result.is_err());
    }

    #[test]
    fn test_tar_gz_config_invalid_compression_level_fails_closed() {
        let json = serde_json::json!({"compression_level": 10});
        let result: Result<TarGzConfig, _> = serde_json::from_value(json);
        assert!(result.is_err());
    }

    #[test]
    fn tar_gz_round_trip_bytes() {
        let gzip_df = GzipDataFormat::default();
        let df = TarGzDataFormat::default();
        let original = Body::Bytes(Bytes::from_static(b"\x00\x01\x02\xffbinary payload"));
        let archived = df.marshal(original.clone()).unwrap();

        let combined_bytes = match &archived {
            Body::Bytes(b) => b.clone(),
            _ => panic!("expected Body::Bytes"),
        };
        assert_eq!(
            &combined_bytes[..2],
            &[0x1f, 0x8b],
            "marshal must emit gzip-wrapped bytes"
        );

        // Peeling the layers with the standalone formats must reveal exactly
        // one regular entry named `payload`.
        let tar_bytes = gzip_df.unmarshal(archived.clone()).unwrap();
        let tar_bytes = match &tar_bytes {
            Body::Bytes(b) => b.clone(),
            _ => panic!("expected Body::Bytes"),
        };
        let mut names = Vec::new();
        for entry in tar::Archive::new(std::io::Cursor::new(&tar_bytes[..]))
            .entries()
            .unwrap()
        {
            names.push(entry.unwrap().path().unwrap().to_path_buf());
        }
        assert_eq!(names.len(), 1, "marshal must write exactly one entry");
        assert_eq!(names[0], std::path::Path::new("payload"));

        let restored = df.unmarshal(archived).unwrap();
        assert_eq!(restored, original);
    }

    #[test]
    fn gzip_and_tar_gz_cross_decode() {
        let tar_df = TarDataFormat::default();
        let gzip_df = GzipDataFormat::default();
        let df = TarGzDataFormat::default();
        let original = Body::Bytes(Bytes::from_static(b"\x00\x01\x02\xffcross payload"));

        // Direction A: tar.gz output decodes through gzip then tar.
        let combined = df.marshal(original.clone()).unwrap();
        let combined_bytes = match &combined {
            Body::Bytes(b) => b.clone(),
            _ => panic!("expected Body::Bytes"),
        };
        assert_eq!(
            &combined_bytes[..2],
            &[0x1f, 0x8b],
            "tar.gz marshal must emit gzip-wrapped bytes"
        );
        let archived = gzip_df.unmarshal(combined.clone()).unwrap();
        let restored = tar_df.unmarshal(archived).unwrap();
        assert_eq!(restored, original);

        // Direction B: composed tar then gzip decodes through tar.gz.
        let composed = gzip_df
            .marshal(tar_df.marshal(original.clone()).unwrap())
            .unwrap();
        let restored = df.unmarshal(composed).unwrap();
        assert_eq!(restored, original);
    }

    #[test]
    fn tar_gz_decompression_limit_includes_tar_stream() {
        // The cap is set below the size of the TAR headers alone, while the
        // regular-file payload is tiny. The cap must trip on the decoded
        // headers/padding, proving it bounds the full gzip output — not just
        // the selected payload.
        let config = TarGzConfig {
            max_decompressed_size: 1024,
            ..Default::default()
        };
        let df = TarGzDataFormat::new(config);

        // Three 512-byte directory headers precede one small regular file.
        let tar_bytes = make_tar(&[
            ("d0/", DIRECTORY, b""),
            ("d1/", DIRECTORY, b""),
            ("d2/", DIRECTORY, b""),
            ("payload.txt", REGULAR, b"tiny"),
        ]);
        let compressed = gzip_bytes(tar_bytes);

        let result = df.unmarshal(compressed);
        match result {
            Err(CamelError::TypeConversionFailed(msg)) => {
                assert!(
                    msg.contains("max_decompressed_size"),
                    "error should mention max_decompressed_size: {msg}"
                );
            }
            _ => panic!("decoded stream beyond the cap must be rejected"),
        }
    }

    #[test]
    fn malformed_tar_gz_input_rejected() {
        let df = TarGzDataFormat::default();

        // Garbage bytes fail the gzip magic/header check.
        let garbage = vec![b'G'; 64];
        let result = df.unmarshal(Body::Bytes(Bytes::from(garbage)));
        match result {
            Err(CamelError::TypeConversionFailed(_)) => {}
            _ => panic!("garbage input must yield TypeConversionFailed"),
        }

        // A valid gzip stream that does not decode to a TAR fails TAR parsing.
        let not_tar = gzip_bytes(vec![b'X'; 512]);
        let result = df.unmarshal(not_tar);
        match result {
            Err(CamelError::TypeConversionFailed(_)) => {}
            _ => panic!("gzip of non-TAR input must yield TypeConversionFailed"),
        }

        // A stream truncated before the CRC/ISIZE trailer fails validation.
        let full = match df
            .marshal(Body::Bytes(Bytes::from_static(b"hello world")))
            .unwrap()
        {
            Body::Bytes(b) => b.to_vec(),
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
    fn tar_gz_non_regular_entries_and_malicious_paths_are_ignored() {
        let tar_bytes = make_tar(&[
            ("../escape-dir/", DIRECTORY, b""),
            ("/etc/passwd", SYMLINK, b""),
            ("../escape-hardlink", HARDLINK, b""),
            ("/dev/tty", CHARACTER_DEVICE, b""),
            ("payload.txt", REGULAR, b"safe payload bytes"),
        ]);
        let df = TarGzDataFormat::default();
        // The archive is scanned in memory only: no extraction, no path-based
        // I/O, so the malicious names above are never resolved. Only the
        // regular file is selected.
        let restored = df.unmarshal(gzip_bytes(tar_bytes)).unwrap();
        assert_bytes(restored, b"safe payload bytes");
    }

    #[test]
    fn tar_gz_regular_entry_policy() {
        let df_strict = TarGzDataFormat::default();
        let df_multi = TarGzDataFormat::new(TarGzConfig {
            allow_multi_entry: true,
            ..Default::default()
        });

        // Zero regular files (only directory + symlink) errors under both
        // configurations.
        let no_regular = make_tar(&[("only-dir/", DIRECTORY, b""), ("only-link", SYMLINK, b"")]);
        for df in [&df_strict, &df_multi] {
            let result = df.unmarshal(gzip_bytes(no_regular.clone()));
            match result {
                Err(CamelError::TypeConversionFailed(_)) => {}
                _ => panic!("archive without regular files must be rejected"),
            }
        }

        // Exactly one regular file (plus non-regular entries) succeeds under
        // both configurations.
        let one_regular = make_tar(&[
            ("dir/", DIRECTORY, b""),
            ("payload.txt", REGULAR, b"single payload"),
            ("link", HARDLINK, b""),
        ]);
        for df in [&df_strict, &df_multi] {
            let restored = df.unmarshal(gzip_bytes(one_regular.clone())).unwrap();
            assert_bytes(restored, b"single payload");
        }

        // Two regular files: strict config errors mentioning the policy;
        // allow_multi_entry returns the first regular file and warns.
        let two_regular = make_tar(&[
            ("first.txt", REGULAR, b"first"),
            ("second.txt", REGULAR, b"second"),
        ]);
        let err = df_strict
            .unmarshal(gzip_bytes(two_regular.clone()))
            .unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("allow_multi_entry"),
            "error should mention allow_multi_entry: {msg}"
        );

        let (result, warnings) =
            capture_warns(|| df_multi.unmarshal(gzip_bytes(two_regular.clone())));
        assert_bytes(result.unwrap(), b"first");
        assert!(
            warnings.iter().any(|w| w.contains("multiple regular")),
            "multi-entry path should warn, captured: {warnings:?}"
        );
    }

    #[test]
    fn tar_gz_materialized_empty_and_stream_bodies() {
        let df = TarGzDataFormat::default();

        // Zero-length materialized payloads are valid bodies: marshal yields
        // a well-formed archive; unmarshal returns the empty payload.
        for body in [Body::Bytes(Bytes::new()), Body::Text(String::new())] {
            let archived = df.marshal(body).unwrap();
            let restored = df.unmarshal(archived).unwrap();
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
        let config = TarGzConfig {
            max_input_size: 16,
            ..Default::default()
        };
        let df = TarGzDataFormat::new(config);
        let result = df.marshal(Body::Text("x".repeat(64)));
        assert!(result.is_err());
        let msg = format!("{}", result.unwrap_err());
        assert!(
            msg.contains("max_input_size"),
            "error should mention max_input_size: {msg}"
        );
    }

    #[test]
    fn tar_gz_marshal_accepts_body_at_exact_input_cap() {
        // A body exactly at `max_input_size` passes the outer cap; the inner
        // gzip layer must accept the archive too (TAR framing allowance) or
        // the composition would silently tighten the outer bound.
        let config = TarGzConfig {
            max_input_size: 64,
            ..Default::default()
        };
        let df = TarGzDataFormat::new(config);
        let original = Body::Bytes(Bytes::from(vec![b'x'; 64]));
        let restored = df.unmarshal(df.marshal(original.clone()).unwrap()).unwrap();
        assert_eq!(restored, original);
    }
}
