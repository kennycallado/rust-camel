use bytes::Bytes;
use camel_api::body::Body;
use camel_api::data_format::DataFormat;
use camel_api::error::CamelError;
use serde::Deserialize;
use std::io::Read;

const DEFAULT_MAX_DECOMPRESSED_SIZE: u64 = 1_073_741_824;
/// Default cap on the materialized input size of `marshal` (R3-L1). The eager
/// marshal collects the whole body into a `Vec<u8>` before archiving; this
/// bounds that allocation.
const DEFAULT_MAX_INPUT_SIZE: u64 = 64 * 1024 * 1024; // 64 MiB
const ENTRY_NAME: &str = "payload";

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TarConfig {
    pub max_decompressed_size: u64,
    /// Maximum materialized input size accepted by `marshal` (DoS cap, R3-L1).
    pub max_input_size: u64,
    pub allow_multi_entry: bool,
}

impl Default for TarConfig {
    fn default() -> Self {
        Self {
            max_decompressed_size: DEFAULT_MAX_DECOMPRESSED_SIZE,
            max_input_size: DEFAULT_MAX_INPUT_SIZE,
            allow_multi_entry: false,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct TarDataFormat {
    config: TarConfig,
}

impl TarDataFormat {
    pub fn new(config: TarConfig) -> Self {
        Self { config }
    }
}

impl DataFormat for TarDataFormat {
    fn name(&self) -> &str {
        "tar"
    }

    fn marshal(&self, body: Body) -> Result<Body, CamelError> {
        let content =
            super::materialize_marshal_input("TarDataFormat", &body, self.config.max_input_size)?;

        let mut buf = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut buf);
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(tar::EntryType::Regular);
            header.set_size(content.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(&mut header, ENTRY_NAME, content.as_slice())
                .map_err(|e| {
                    CamelError::TypeConversionFailed(format!(
                        "TarDataFormat::marshal failed to write entry: {e}"
                    ))
                })?;
            builder.finish().map_err(|e| {
                CamelError::TypeConversionFailed(format!(
                    "TarDataFormat::marshal failed to finalize archive: {e}"
                ))
            })?;
        }

        Ok(Body::Bytes(Bytes::from(buf)))
    }

    fn unmarshal(&self, body: Body) -> Result<Body, CamelError> {
        let raw = super::raw_unmarshal_body("TarDataFormat", "TAR data", &body)?;

        // TAR has no central directory: walk entries in stream order. Names
        // are never used for I/O and non-regular entries are skipped without
        // materializing their payload, so archive paths cannot escape
        // (path-confinement precedent rc-0ks57; v1 performs no disk I/O).
        let mut archive = tar::Archive::new(std::io::Cursor::new(&raw));
        let mut first_regular: Option<Vec<u8>> = None;
        let mut regular_count: usize = 0;
        {
            let entries = archive.entries().map_err(|e| {
                CamelError::TypeConversionFailed(format!(
                    "TarDataFormat::unmarshal invalid TAR: {e}"
                ))
            })?;
            for entry in entries {
                let entry = entry.map_err(|e| {
                    CamelError::TypeConversionFailed(format!(
                        "TarDataFormat::unmarshal invalid TAR entry: {e}"
                    ))
                })?;
                if entry.header().entry_type() != tar::EntryType::Regular {
                    continue;
                }
                regular_count += 1;
                if first_regular.is_none() {
                    // Read at most cap + 1 bytes so oversize payloads are
                    // detected without unbounded materialization.
                    let limit = self.config.max_decompressed_size.saturating_add(1);
                    let mut limited = entry.take(limit);
                    let mut data = Vec::new();
                    limited.read_to_end(&mut data).map_err(|e| {
                        CamelError::TypeConversionFailed(format!(
                            "TarDataFormat::unmarshal failed to read entry: {e}"
                        ))
                    })?;
                    first_regular = Some(data);
                }
            }
        }

        let payload = first_regular.ok_or_else(|| {
            CamelError::TypeConversionFailed(
                "TarDataFormat::unmarshal TAR archive has no regular file".to_string(),
            )
        })?;

        if regular_count > 1 && !self.config.allow_multi_entry {
            return Err(CamelError::TypeConversionFailed(format!(
                "TarDataFormat::unmarshal TAR has {regular_count} regular files but allow_multi_entry is false"
            )));
        }

        if regular_count > 1 {
            tracing::warn!(
                regular_files = regular_count,
                "TAR archive has multiple regular files, returning first only"
            );
        }

        if payload.len() as u64 > self.config.max_decompressed_size {
            return Err(CamelError::TypeConversionFailed(format!(
                "TarDataFormat::unmarshal regular file size exceeds max_decompressed_size {}",
                self.config.max_decompressed_size
            )));
        }

        Ok(Body::Bytes(Bytes::from(payload)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;

    use super::super::test_util::{
        CHARACTER_DEVICE, DIRECTORY, HARDLINK, REGULAR, SYMLINK, assert_bytes, capture_warns,
        make_tar, make_tar_payload, stream_body_pair,
    };

    #[test]
    fn test_name() {
        let df = TarDataFormat::default();
        assert_eq!(df.name(), "tar");
    }

    #[test]
    fn test_tar_config_deserialize_from_json() {
        let json = serde_json::json!({
            "max_decompressed_size": 2147483648u64,
            "max_input_size": 134217728u64,
            "allow_multi_entry": true
        });
        let cfg: TarConfig = serde_json::from_value(json).unwrap();
        assert_eq!(cfg.max_decompressed_size, 2147483648);
        assert_eq!(cfg.max_input_size, 134217728);
        assert!(cfg.allow_multi_entry);
    }

    #[test]
    fn test_tar_config_deny_unknown_fields() {
        let json = serde_json::json!({"unknown_key": 42});
        let result: Result<TarConfig, _> = serde_json::from_value(json);
        assert!(result.is_err());
    }

    #[test]
    fn tar_round_trip_bytes() {
        let df = TarDataFormat::default();
        let original = Body::Bytes(Bytes::from_static(b"\x00\x01\x02\xffbinary payload"));
        let archived = df.marshal(original.clone()).unwrap();
        let archived_bytes = match &archived {
            Body::Bytes(b) => b.clone(),
            _ => panic!("expected Body::Bytes"),
        };
        let mut names = Vec::new();
        for entry in tar::Archive::new(std::io::Cursor::new(&archived_bytes[..]))
            .entries()
            .unwrap()
        {
            names.push(entry.unwrap().path().unwrap().to_path_buf());
        }
        assert_eq!(names.len(), 1, "marshal must write exactly one entry");
        assert_eq!(names[0], std::path::Path::new(ENTRY_NAME));
        let restored = df.unmarshal(archived).unwrap();
        assert_eq!(restored, original);
    }

    #[test]
    fn tar_regular_entry_policy() {
        let df_strict = TarDataFormat::default();
        let df_multi = TarDataFormat::new(TarConfig {
            allow_multi_entry: true,
            ..Default::default()
        });

        // Zero regular files (only directory + symlink) errors under both
        // configurations.
        let no_regular = make_tar(&[("only-dir/", DIRECTORY, b""), ("only-link", SYMLINK, b"")]);
        for df in [&df_strict, &df_multi] {
            let result = df.unmarshal(Body::Bytes(Bytes::from(no_regular.clone())));
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
            let restored = df
                .unmarshal(Body::Bytes(Bytes::from(one_regular.clone())))
                .unwrap();
            assert_bytes(restored, b"single payload");
        }

        // Two regular files: strict config errors mentioning the policy;
        // allow_multi_entry returns the first regular file and warns.
        let two_regular = make_tar(&[
            ("first.txt", REGULAR, b"first"),
            ("dir/", DIRECTORY, b""),
            ("second.txt", REGULAR, b"second"),
        ]);
        let err = df_strict
            .unmarshal(Body::Bytes(Bytes::from(two_regular)))
            .unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("allow_multi_entry"),
            "error should mention allow_multi_entry: {msg}"
        );

        let two_regular = make_tar(&[
            ("first.txt", REGULAR, b"first"),
            ("second.txt", REGULAR, b"second"),
        ]);
        let (result, warnings) =
            capture_warns(|| df_multi.unmarshal(Body::Bytes(Bytes::from(two_regular.clone()))));
        assert_bytes(result.unwrap(), b"first");
        assert!(
            warnings.iter().any(|w| w.contains("multiple regular")),
            "multi-entry path should warn, captured: {warnings:?}"
        );
    }

    #[test]
    fn tar_non_regular_entries_and_malicious_paths_are_ignored() {
        let tar_bytes = make_tar(&[
            ("../escape-dir/", DIRECTORY, b""),
            ("/etc/passwd", SYMLINK, b""),
            ("../escape-hardlink", HARDLINK, b""),
            ("/dev/tty", CHARACTER_DEVICE, b""),
            ("payload.txt", REGULAR, b"safe payload bytes"),
        ]);
        let df = TarDataFormat::default();
        // The archive is scanned in memory only: no extraction, no path-based
        // I/O, so the malicious names above are never resolved. Only the
        // regular file is selected.
        let restored = df.unmarshal(Body::Bytes(Bytes::from(tar_bytes))).unwrap();
        assert_bytes(restored, b"safe payload bytes");
    }

    #[test]
    fn malformed_tar_input_rejected() {
        let df = TarDataFormat::default();

        // Garbage bytes fail header validation (checksum/octal parse).
        let garbage = vec![b'G'; 1024];
        let result = df.unmarshal(Body::Bytes(Bytes::from(garbage)));
        match result {
            Err(CamelError::TypeConversionFailed(_)) => {}
            _ => panic!("garbage input must yield TypeConversionFailed"),
        }

        // Truncated header block fails before any entry is returned.
        let full = make_tar_payload(b"hello world");
        let truncated = full[..400].to_vec();
        let result = df.unmarshal(Body::Bytes(Bytes::from(truncated)));
        match result {
            Err(CamelError::TypeConversionFailed(_)) => {}
            _ => panic!("truncated header must yield TypeConversionFailed"),
        }
    }

    #[test]
    fn tar_materialized_empty_and_stream_bodies() {
        let df = TarDataFormat::default();

        // Zero-length materialized payloads follow TAR semantics: marshal
        // yields a valid archive with one empty regular entry; unmarshal
        // returns the empty payload.
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
        let config = TarConfig {
            max_input_size: 16,
            ..Default::default()
        };
        let df = TarDataFormat::new(config);
        let result = df.marshal(Body::Text("x".repeat(64)));
        assert!(result.is_err());
        let msg = format!("{}", result.unwrap_err());
        assert!(
            msg.contains("max_input_size"),
            "error should mention max_input_size: {msg}"
        );
    }

    #[test]
    fn test_max_decompressed_size_exceeded() {
        let config = TarConfig {
            max_decompressed_size: 10,
            ..Default::default()
        };
        let df = TarDataFormat::new(config);
        let tar_data = make_tar_payload(b"this content is way longer than 10 bytes");
        let result = df.unmarshal(Body::Bytes(Bytes::from(tar_data)));
        assert!(result.is_err());
    }
}
