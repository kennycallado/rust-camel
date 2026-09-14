use bytes::Bytes;
use std::collections::HashMap;
use std::io::Read;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use tokio::sync::mpsc;

use camel_api::{Body, CamelError, Exchange, Message, StreamingSplitExpression, Value};
use futures::Stream;

use crate::archive_splitter::{
    DEFAULT_MAX_PATH_LENGTH, next_free_indexed_name, validate_entry_path,
};

/// Duplicate-name policy shared with the TAR splitter; re-exported to
/// preserve the historical public path
/// `camel_processor::zip_splitter::DuplicatePolicy`.
pub use crate::archive_splitter::DuplicatePolicy;

const DEFAULT_MAX_ENTRIES: usize = 10000;
const DEFAULT_MAX_TOTAL_DECOMPRESSED_SIZE: u64 = 1_073_741_824;
const DEFAULT_MAX_PER_ENTRY_SIZE: u64 = 512 * 1024 * 1024;
const DEFAULT_MAX_COMPRESSED_SIZE: u64 = 1_073_741_824;
const DEFAULT_CHANNEL_CAPACITY: usize = 2;

pub const CAMEL_ZIP_ENTRY_NAME: &str = "CamelZipEntryName";
pub const CAMEL_ZIP_ENTRY_PATH: &str = "CamelZipEntryPath";
pub const CAMEL_ZIP_ENTRY_INDEX: &str = "CamelZipEntryIndex";
pub const CAMEL_ZIP_ENTRY_SIZE: &str = "CamelZipEntrySize";
pub const CAMEL_ZIP_ENTRY_COMPRESSED_SIZE: &str = "CamelZipEntryCompressedSize";
pub const CAMEL_ZIP_ENTRY_CRC32: &str = "CamelZipEntryCrc32";
pub const CAMEL_ZIP_ENTRY_IS_DIRECTORY: &str = "CamelZipEntryIsDirectory";
pub const CAMEL_ZIP_ENTRY_COMPRESSION: &str = "CamelZipEntryCompression";

#[derive(Debug, Clone)]
pub struct ZipSplitConfig {
    pub max_entries: usize,
    pub max_total_decompressed_size: u64,
    pub max_per_entry_size: u64,
    pub max_compressed_size: u64,
    pub max_path_length: usize,
    pub allow_empty_directories: bool,
    pub duplicate_names_policy: DuplicatePolicy,
    pub channel_capacity: usize,
}

impl Default for ZipSplitConfig {
    fn default() -> Self {
        Self {
            max_entries: DEFAULT_MAX_ENTRIES,
            max_total_decompressed_size: DEFAULT_MAX_TOTAL_DECOMPRESSED_SIZE,
            max_per_entry_size: DEFAULT_MAX_PER_ENTRY_SIZE,
            max_compressed_size: DEFAULT_MAX_COMPRESSED_SIZE,
            max_path_length: DEFAULT_MAX_PATH_LENGTH,
            allow_empty_directories: false,
            duplicate_names_policy: DuplicatePolicy::AllowWithIndex,
            channel_capacity: DEFAULT_CHANNEL_CAPACITY,
        }
    }
}

struct ZipEntryData {
    index: usize,
    path: String,
    size: u64,
    compressed_size: u64,
    crc32: Option<u32>,
    is_dir: bool,
    compression: String,
    data: Vec<u8>,
}

/// Split a ZIP archive's bytes into a stream of Exchanges, one per entry.
///
/// Takes owned `Bytes` (for `'static` lifetime), a parent `Exchange` whose headers
/// and properties are cloned into each entry's exchange, and a `ZipSplitConfig`
/// controlling limits and policy.
///
/// This is the core extraction — callers such as `zip_splitter()` or `camel-core`
/// component code can invoke it directly with already-acquired bytes.
pub fn split_zip_bytes(
    parent: Exchange,
    bytes: Bytes,
    config: ZipSplitConfig,
) -> Pin<Box<dyn Stream<Item = Result<Exchange, CamelError>> + Send>> {
    Box::pin(async_stream::stream! {
        if config.channel_capacity == 0 {
            yield Err(CamelError::Config(
                "ZipSplitConfig.channel_capacity must be > 0".into(),
            ));
            return;
        }

        if bytes.len() as u64 > config.max_compressed_size {
            yield Err(CamelError::TypeConversionFailed(format!(
                "ZIP compressed size {} exceeds max {}",
                bytes.len(),
                config.max_compressed_size
            )));
            return;
        }

        let (tx, mut rx) = mpsc::channel::<Result<ZipEntryData, CamelError>>(config.channel_capacity);

        let total_decompressed = Arc::new(AtomicU64::new(0));
        let entry_count = Arc::new(AtomicUsize::new(0));
        // Entry name -> occurrence count; drives both duplicate policies and
        // mirrors the TAR splitter's bookkeeping exactly.
        let seen_names: Arc<std::sync::Mutex<HashMap<String, usize>>> =
            Arc::new(std::sync::Mutex::new(HashMap::new()));
        // Names already emitted (original or indexed). The occurrence
        // counter alone cannot guarantee uniqueness — a literal entry can
        // occupy an indexed name first — so this set is the collision
        // authority, mirroring the TAR splitter.
        let emitted_names: Arc<std::sync::Mutex<std::collections::HashSet<String>>> =
            Arc::new(std::sync::Mutex::new(std::collections::HashSet::new()));

        let max_entries = config.max_entries;
        let max_per_entry = config.max_per_entry_size;
        let max_total = config.max_total_decompressed_size;
        let max_path_len = config.max_path_length;
        let allow_dirs = config.allow_empty_directories;
        let dup_policy = config.duplicate_names_policy;

        tokio::task::spawn_blocking(move || {
            let reader = std::io::Cursor::new(bytes);
            let mut archive = match zip::ZipArchive::new(reader) {
                Ok(a) => a,
                Err(e) => {
                    let _ = tx.blocking_send(Err(CamelError::TypeConversionFailed(
                        format!("Invalid ZIP archive: {e}"),
                    )));
                    return;
                }
            };

            for i in 0..archive.len() {
                let mut entry = match archive.by_index(i) {
                    Ok(e) => e,
                    Err(e) => {
                        let _ = tx.blocking_send(Err(CamelError::TypeConversionFailed(
                            format!("Failed to read ZIP entry {i}: {e}"),
                        )));
                        return;
                    }
                };

                let raw_name = entry.name().to_string();
                let is_dir = entry.is_dir();

                let mut validated = match validate_entry_path(&raw_name, max_path_len, "ZIP") {
                    Ok(p) => p,
                    Err(e) => {
                        let _ = tx.blocking_send(Err(e));
                        return;
                    }
                };

                if is_dir {
                    if allow_dirs {
                        let count = entry_count.fetch_add(1, Ordering::SeqCst);
                        if count >= max_entries {
                            let _ = tx.blocking_send(Err(CamelError::TypeConversionFailed(
                                format!("ZIP exceeds max entries: {max_entries}"),
                            )));
                            return;
                        }
                        if tx.blocking_send(Ok(ZipEntryData {
                            index: count,
                            path: validated,
                            size: 0,
                            compressed_size: entry.compressed_size(),
                            crc32: Some(entry.crc32()),
                            is_dir: true,
                            compression: format!("{:?}", entry.compression()),
                            data: Vec::new(),
                        }))
                        .is_err()
                        {
                            return;
                        }
                    }
                    continue;
                }

                let compressed_size = entry.compressed_size();
                let crc32 = entry.crc32();

                let mut data = Vec::new();
                let mut limited =
                    std::io::Read::take(&mut entry, max_per_entry.saturating_add(1));
                if let Err(e) = limited.read_to_end(&mut data) {
                    let _ = tx.blocking_send(Err(CamelError::TypeConversionFailed(
                        format!("Failed to decompress ZIP entry '{raw_name}': {e}"),
                    )));
                    return;
                }

                if data.len() as u64 > max_per_entry {
                    let _ = tx.blocking_send(Err(CamelError::TypeConversionFailed(
                        format!(
                            "ZIP entry '{raw_name}' size {} exceeds max {}",
                            data.len(),
                            max_per_entry
                        ),
                    )));
                    return;
                }

                let entry_size = data.len() as u64;
                let prev_total = total_decompressed.load(Ordering::SeqCst);
                let new_total = prev_total.saturating_add(entry_size);
                if new_total > max_total {
                    let _ = tx.blocking_send(Err(CamelError::TypeConversionFailed(
                        format!("ZIP total decompressed size exceeds max {max_total}"),
                    )));
                    return;
                }
                total_decompressed.store(new_total, Ordering::SeqCst);

                let count = entry_count.fetch_add(1, Ordering::SeqCst);
                if count >= max_entries {
                    let _ = tx.blocking_send(Err(CamelError::TypeConversionFailed(
                        format!("ZIP exceeds max entries: {max_entries}"),
                    )));
                    return;
                }

                match &dup_policy {
                    DuplicatePolicy::Reject => {
                        let mut seen = seen_names.lock().unwrap_or_else(|e| e.into_inner());
                        if seen.contains_key(&validated) {
                            let _ = tx.blocking_send(Err(CamelError::TypeConversionFailed(
                                format!("Duplicate ZIP entry name: {validated}"),
                            )));
                            return;
                        }
                        seen.insert(validated.clone(), 0);
                        emitted_names
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .insert(validated.clone());
                    }
                    DuplicatePolicy::AllowWithIndex => {
                        let mut seen = seen_names.lock().unwrap_or_else(|e| e.into_inner());
                        let mut emitted_set =
                            emitted_names.lock().unwrap_or_else(|e| e.into_inner());
                        let occurrences = seen.entry(validated.clone()).or_insert(0);
                        let start = if *occurrences > 0 {
                            *occurrences
                        } else if emitted_set.contains(&validated) {
                            1
                        } else {
                            0
                        };
                        if start > 0 {
                            // Identical to the TAR splitter: later duplicates
                            // and literal collisions with already-emitted
                            // indexed names get a deterministic collision-free
                            // index inserted before the extension, and the
                            // suffix grows the name, so the path-length cap
                            // stays authoritative for indexed names too.
                            let (candidate, used) =
                                next_free_indexed_name(&validated, start, &emitted_set);
                            validated =
                                match validate_entry_path(&candidate, max_path_len, "ZIP") {
                                    Ok(p) => p,
                                    Err(e) => {
                                        let _ = tx.blocking_send(Err(e));
                                        return;
                                    }
                                };
                            *occurrences = (*occurrences).max(used + 1);
                        }
                        emitted_set.insert(validated.clone());
                    }
                }

                if tx
                    .blocking_send(Ok(ZipEntryData {
                        index: count,
                        path: validated,
                        size: data.len() as u64,
                        compressed_size,
                        crc32: Some(crc32),
                        is_dir: false,
                        compression: format!("{:?}", entry.compression()),
                        data,
                    }))
                    .is_err()
                {
                    return;
                }
            }
        });

        while let Some(result) = rx.recv().await {
            match result {
                Ok(entry) => {
                    let ZipEntryData {
                        index,
                        path,
                        size,
                        compressed_size,
                        crc32,
                        is_dir,
                        compression,
                        data,
                    } = entry;
                    let body = if is_dir {
                        Body::Empty
                    } else {
                        Body::Bytes(Bytes::from(data))
                    };
                    let msg = Message {
                        headers: parent.input.headers.clone(),
                        body,
                    };
                    let mut ex = Exchange::new(msg);
                    // Strip parent-level content headers that are stale for individual ZIP entries
                    ex.input.headers.remove("Content-Length");
                    ex.input.headers.remove("Content-Type");
                    ex.properties = parent.properties.clone();
                    ex.pattern = parent.pattern;
                    ex.otel_context = parent.otel_context.clone();

                    let entry_name = Path::new(&path)
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default();

                    ex.input.headers.insert(
                        CAMEL_ZIP_ENTRY_NAME.to_string(),
                        Value::String(entry_name),
                    );
                    ex.input.headers.insert(
                        CAMEL_ZIP_ENTRY_PATH.to_string(),
                        Value::String(path),
                    );
                    ex.input.headers.insert(
                        CAMEL_ZIP_ENTRY_INDEX.to_string(),
                        Value::from(index as u64),
                    );
                    ex.input.headers
                        .insert(CAMEL_ZIP_ENTRY_SIZE.to_string(), Value::from(size));
                    ex.input.headers.insert(
                        CAMEL_ZIP_ENTRY_COMPRESSED_SIZE.to_string(),
                        Value::from(compressed_size),
                    );
                    if let Some(crc) = crc32 {
                        ex.input
                            .headers
                            .insert(CAMEL_ZIP_ENTRY_CRC32.to_string(), Value::from(crc));
                    }
                    ex.input.headers.insert(
                        CAMEL_ZIP_ENTRY_IS_DIRECTORY.to_string(),
                        Value::Bool(is_dir),
                    );
                    ex.input.headers.insert(
                        CAMEL_ZIP_ENTRY_COMPRESSION.to_string(),
                        Value::String(compression),
                    );

                    yield Ok(ex);
                }
                Err(e) => {
                    yield Err(e);
                }
            }
        }
    })
}

pub fn zip_splitter(config: ZipSplitConfig) -> StreamingSplitExpression {
    Arc::new(move |exchange: Exchange| {
        let config = config.clone();
        match exchange.input.body.clone() {
            Body::Bytes(b) => split_zip_bytes(exchange, b, config),
            Body::Text(s) => split_zip_bytes(exchange, Bytes::from(s.as_bytes().to_vec()), config),
            _ => Box::pin(async_stream::stream! {
                yield Err(CamelError::TypeConversionFailed(
                    "ZipSplitter requires Body::Bytes or Body::Text".to_string(),
                ));
            }),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive_splitter::test_util::make_zip_raw;
    use futures::StreamExt;
    use std::io::Write;

    fn make_zip_with_files(files: Vec<(&str, &[u8])>) -> Vec<u8> {
        let mut buf = Vec::new();
        {
            let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            for (name, content) in &files {
                writer.start_file(*name, options).unwrap();
                writer.write_all(content).unwrap();
            }
            writer.finish().unwrap();
        }
        buf
    }

    fn make_zip_with_dirs(entries: Vec<(&str, bool)>) -> Vec<u8> {
        let mut buf = Vec::new();
        {
            let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            let options = zip::write::SimpleFileOptions::default();
            for (name, is_dir) in &entries {
                if *is_dir {
                    writer.add_directory(*name, options).unwrap();
                } else {
                    writer.start_file(name, options).unwrap();
                    writer.write_all(b"content").unwrap();
                }
            }
            writer.finish().unwrap();
        }
        buf
    }

    async fn collect_entries(
        config: ZipSplitConfig,
        zip_data: Vec<u8>,
    ) -> Vec<Result<Exchange, CamelError>> {
        let expr = zip_splitter(config);
        let exchange = Exchange::new(Message {
            headers: Default::default(),
            body: Body::Bytes(Bytes::from(zip_data)),
        });
        let stream = expr(exchange);
        stream.collect().await
    }

    #[tokio::test]
    async fn test_zip_split_single_file() {
        let zip_data = make_zip_with_files(vec![("hello.txt", b"hello world")]);
        let results = collect_entries(ZipSplitConfig::default(), zip_data).await;
        assert_eq!(results.len(), 1);
        let ex = results[0].as_ref().unwrap();
        match &ex.input.body {
            Body::Bytes(b) => assert_eq!(b.as_ref(), b"hello world"),
            _ => panic!("expected Body::Bytes"),
        }
        assert_eq!(
            ex.input.headers.get(CAMEL_ZIP_ENTRY_NAME),
            Some(&Value::String("hello.txt".to_string()))
        );
    }

    #[tokio::test]
    async fn test_zip_split_multiple_files() {
        let zip_data = make_zip_with_files(vec![("a.txt", b"aaa"), ("b.txt", b"bbb")]);
        let results = collect_entries(ZipSplitConfig::default(), zip_data).await;
        assert_eq!(results.len(), 2);
    }

    #[tokio::test]
    async fn test_zip_split_with_directories() {
        let zip_data = make_zip_with_dirs(vec![("subdir/", true), ("subdir/file.txt", false)]);
        let config = ZipSplitConfig {
            allow_empty_directories: true,
            ..Default::default()
        };
        let results = collect_entries(config, zip_data).await;
        assert_eq!(results.len(), 2);
        let dir_ex = results[0].as_ref().unwrap();
        assert!(dir_ex.input.body.is_empty());
        assert_eq!(
            dir_ex.input.headers.get(CAMEL_ZIP_ENTRY_IS_DIRECTORY),
            Some(&Value::Bool(true))
        );
    }

    #[tokio::test]
    async fn test_zip_split_preserves_paths() {
        let zip_data = make_zip_with_files(vec![("deep/nested/path/file.txt", b"deep")]);
        let results = collect_entries(ZipSplitConfig::default(), zip_data).await;
        assert_eq!(results.len(), 1);
        let ex = results[0].as_ref().unwrap();
        assert_eq!(
            ex.input.headers.get(CAMEL_ZIP_ENTRY_PATH),
            Some(&Value::String("deep/nested/path/file.txt".to_string()))
        );
    }

    #[tokio::test]
    async fn test_zip_split_max_entries_exceeded() {
        let files: Vec<(String, Vec<u8>)> = (0..5)
            .map(|i| (format!("f{i}.txt"), b"x".to_vec()))
            .collect();
        let zip_data = {
            let mut buf = Vec::new();
            {
                let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
                let options = zip::write::SimpleFileOptions::default();
                for (name, content) in &files {
                    writer.start_file(name, options).unwrap();
                    writer.write_all(content).unwrap();
                }
                writer.finish().unwrap();
            }
            buf
        };
        let config = ZipSplitConfig {
            max_entries: 3,
            ..Default::default()
        };
        let results = collect_entries(config, zip_data).await;
        let has_error = results.iter().any(|r| r.is_err());
        assert!(has_error);
    }

    #[tokio::test]
    async fn test_zip_split_path_traversal_rejected() {
        let mut buf = Vec::new();
        {
            let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            let options = zip::write::SimpleFileOptions::default();
            writer.start_file("../etc/passwd", options).unwrap();
            writer.write_all(b"oops").unwrap();
            writer.finish().unwrap();
        }
        let results = collect_entries(ZipSplitConfig::default(), buf).await;
        let has_error = results.iter().any(|r| r.is_err());
        assert!(has_error);
    }

    #[tokio::test]
    async fn test_zip_split_headers_set() {
        let zip_data = make_zip_with_files(vec![("test.txt", b"content")]);
        let results = collect_entries(ZipSplitConfig::default(), zip_data).await;
        let ex = results[0].as_ref().unwrap();
        assert!(ex.input.headers.contains_key(CAMEL_ZIP_ENTRY_NAME));
        assert!(ex.input.headers.contains_key(CAMEL_ZIP_ENTRY_PATH));
        assert!(ex.input.headers.contains_key(CAMEL_ZIP_ENTRY_INDEX));
        assert!(ex.input.headers.contains_key(CAMEL_ZIP_ENTRY_SIZE));
        assert!(
            ex.input
                .headers
                .contains_key(CAMEL_ZIP_ENTRY_COMPRESSED_SIZE)
        );
        assert!(ex.input.headers.contains_key(CAMEL_ZIP_ENTRY_IS_DIRECTORY));
        assert!(ex.input.headers.contains_key(CAMEL_ZIP_ENTRY_COMPRESSION));
    }

    #[tokio::test]
    async fn test_zip_split_empty_zip() {
        let mut buf = Vec::new();
        {
            let writer = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            writer.finish().unwrap();
        }
        let results = collect_entries(ZipSplitConfig::default(), buf).await;
        assert!(results.is_empty());
    }

    /// Under `Reject`, unique names always pass. True duplicate names never
    /// reach the splitter — the `zip` reader indexes the central directory by
    /// name and collapses them — so the reject branch is defense-in-depth
    /// shared with the TAR splitter (see
    /// `archive_splitter::indexed_duplicate_name` and the cross-format parity
    /// test in `tar_splitter`).
    #[tokio::test]
    async fn test_zip_split_duplicate_names_reject() {
        let files: Vec<(&str, &[u8])> = vec![("a.txt", b"first"), ("b.txt", b"second")];
        let zip_data = make_zip_with_files(files);
        let config = ZipSplitConfig {
            duplicate_names_policy: DuplicatePolicy::Reject,
            ..Default::default()
        };
        let results = collect_entries(config, zip_data).await;
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|r| r.is_ok()));
    }
    /// Boundary guard: a hand-built ZIP carrying true duplicate names has
    /// them collapsed by the `zip` reader (first position, last entry's data
    /// wins) before the splitter sees it, so `AllowWithIndex` observes only
    /// unique names and never mangles. If a reader upgrade starts surfacing
    /// duplicates, this test fails and the shared index-mangling branch
    /// becomes live end-to-end.
    #[tokio::test]
    async fn test_zip_split_raw_duplicate_entries_collapse_at_reader() {
        let zip_data = make_zip_raw(&[
            ("dup.txt", b"first"),
            ("other.txt", b"mid"),
            ("dup.txt", b"second"),
        ]);
        let results = collect_entries(ZipSplitConfig::default(), zip_data).await;
        assert_eq!(
            results.len(),
            2,
            "duplicate names must collapse at the reader: {results:?}"
        );
        let first = results[0].as_ref().unwrap();
        assert_eq!(
            first.input.headers.get(CAMEL_ZIP_ENTRY_PATH),
            Some(&Value::String("dup.txt".to_string())),
            "the first position wins with the surviving name unmangled"
        );
        match &first.input.body {
            Body::Bytes(b) => assert_eq!(b.as_ref(), b"second", "last entry's data wins"),
            other => panic!("expected Body::Bytes, got {other:?}"),
        }
        assert_eq!(
            results[1]
                .as_ref()
                .unwrap()
                .input
                .headers
                .get(CAMEL_ZIP_ENTRY_PATH),
            Some(&Value::String("other.txt".to_string()))
        );
    }

    #[tokio::test]
    async fn test_zip_split_max_per_entry_size_exceeded() {
        let zip_data = make_zip_with_files(vec![("big.txt", b"x".repeat(200).as_slice())]);
        let config = ZipSplitConfig {
            max_per_entry_size: 100,
            ..Default::default()
        };
        let results = collect_entries(config, zip_data).await;
        let has_error = results.iter().any(|r| r.is_err());
        assert!(has_error);
    }

    #[tokio::test]
    async fn test_zip_split_max_total_decompressed_size_exceeded() {
        let zip_data =
            make_zip_with_files(vec![("a.txt", b"aaaaaaaaaa"), ("b.txt", b"bbbbbbbbbb")]);
        let config = ZipSplitConfig {
            max_total_decompressed_size: 15,
            ..Default::default()
        };
        let results = collect_entries(config, zip_data).await;
        let has_error = results.iter().any(|r| r.is_err());
        assert!(has_error);
    }

    #[tokio::test]
    async fn test_zip_split_corrupt_zip() {
        let results = collect_entries(ZipSplitConfig::default(), b"not a zip file".to_vec()).await;
        let has_error = results.iter().any(|r| r.is_err());
        assert!(has_error);
    }

    #[tokio::test]
    async fn test_zip_split_backslash_rejected() {
        let mut buf = Vec::new();
        {
            let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            let options = zip::write::SimpleFileOptions::default();
            writer.start_file("sub\\file.txt", options).unwrap();
            writer.write_all(b"oops").unwrap();
            writer.finish().unwrap();
        }
        let results = collect_entries(ZipSplitConfig::default(), buf).await;
        let has_error = results.iter().any(|r| r.is_err());
        assert!(has_error);
    }

    /// Regression guard for the shared-validator extraction: safe names are
    /// accepted and unsafe names are rejected with the exact pre-refactor
    /// ZIP error text.
    #[tokio::test]
    async fn zip_path_validation_behavior_is_unchanged() {
        // Safe relative name is accepted with its path preserved.
        let safe = collect_entries(
            ZipSplitConfig::default(),
            make_zip_with_files(vec![("docs/readme.txt", b"ok")]),
        )
        .await;
        assert_eq!(safe.len(), 1);
        let ex = safe[0].as_ref().unwrap();
        assert_eq!(
            ex.input.headers.get(CAMEL_ZIP_ENTRY_PATH),
            Some(&Value::String("docs/readme.txt".to_string()))
        );

        // Absolute and traversal names are rejected with identical error text.
        let cases = [
            ("/etc/passwd", "ZIP entry path is absolute: /etc/passwd"),
            (
                "../etc/passwd",
                "ZIP entry path contains '..' traversal: ../etc/passwd",
            ),
        ];
        for (name, expected) in cases {
            let results = collect_entries(
                ZipSplitConfig::default(),
                make_zip_with_files(vec![(name, b"oops")]),
            )
            .await;
            assert_eq!(
                results.len(),
                1,
                "expected exactly the validation error for {name}"
            );
            let err = results[0]
                .as_ref()
                .expect_err(&format!("'{name}' must be rejected"))
                .to_string();
            assert!(
                err.contains(expected),
                "error text mismatch for {name}: {err}"
            );
        }
    }
}
