//! TAR and TAR.GZ stream splitters: metadata constants, bounded
//! configuration, and the TAR-specific error surface.
//!
//! Splitting itself (bounded sequential TAR reading over materialized bytes)
//! is layered on this contract. Entry names are validated through the shared
//! archive path validator, so TAR and ZIP reject the same unsafe shapes with
//! format-prefixed error text. TAR.GZ runs the same reader over the output
//! of the bounded single-member GZIP decoder shared with
//! `GzipDataFormat`; concatenated multi-member GZIP input is rejected
//! instead of silently split.

use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;

use bytes::Bytes;
use futures::Stream;
use serde::Deserialize;
use tokio::sync::mpsc;

use camel_api::{Body, CamelError, Exchange, Message, StreamingSplitExpression, Value};

use crate::archive_splitter::{
    DEFAULT_MAX_PATH_LENGTH, DuplicatePolicy, next_free_indexed_name, validate_entry_path,
};
use crate::data_format::gzip::decode_first_member;

pub const CAMEL_TAR_ENTRY_NAME: &str = "CamelTarEntryName";
pub const CAMEL_TAR_ENTRY_PATH: &str = "CamelTarEntryPath";
pub const CAMEL_TAR_ENTRY_INDEX: &str = "CamelTarEntryIndex";
pub const CAMEL_TAR_ENTRY_SIZE: &str = "CamelTarEntrySize";
pub const CAMEL_TAR_ENTRY_IS_DIRECTORY: &str = "CamelTarEntryIsDirectory";

const DEFAULT_MAX_ENTRIES: usize = 10_000;
const DEFAULT_MAX_TOTAL_DECODED_SIZE: u64 = 1_073_741_824;
const DEFAULT_MAX_PER_ENTRY_SIZE: u64 = 512 * 1024 * 1024;
const DEFAULT_MAX_COMPRESSED_SIZE: u64 = 1_073_741_824;
const DEFAULT_CHANNEL_CAPACITY: usize = 2;

/// Bounded configuration for TAR and TAR.GZ stream splitting.
///
/// Unknown keys are rejected (`deny_unknown_fields`) and every cap has an
/// explicit default mirroring the ZIP splitter, so deserialized
/// configurations are always fully bounded.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct TarSplitConfig {
    /// Maximum number of emitted entries before the split fails.
    pub max_entries: usize,
    /// Maximum aggregate decoded bytes across all regular-entry payloads.
    /// TAR framing (headers and padding) is not counted against this cap;
    /// on TAR.GZ the decode step is bounded separately by this cap plus a
    /// framing allowance derived from `max_entries`.
    pub max_total_decoded_size: u64,
    /// Maximum decoded size of a single entry.
    pub max_per_entry_size: u64,
    /// Maximum compressed input size accepted before decoding (TAR.GZ).
    pub max_compressed_size: u64,
    /// Maximum validated entry path length.
    pub max_path_length: usize,
    /// Duplicate entry-name policy from the shared archive vocabulary:
    /// `Reject` fails the split on the first duplicate, `AllowWithIndex`
    /// emits deterministic collision-free indexed names. TAR applies this
    /// policy directly; ZIP's reader-level duplicate collapse is pinned
    /// historical behavior, not a parity target.
    pub duplicate_names_policy: DuplicatePolicy,
    /// Accept archives that contain zero regular-file entries.
    pub allow_empty_archive: bool,
}

impl Default for TarSplitConfig {
    fn default() -> Self {
        Self {
            max_entries: DEFAULT_MAX_ENTRIES,
            max_total_decoded_size: DEFAULT_MAX_TOTAL_DECODED_SIZE,
            max_per_entry_size: DEFAULT_MAX_PER_ENTRY_SIZE,
            max_compressed_size: DEFAULT_MAX_COMPRESSED_SIZE,
            max_path_length: DEFAULT_MAX_PATH_LENGTH,
            duplicate_names_policy: DuplicatePolicy::default(),
            allow_empty_archive: false,
        }
    }
}

/// Validate a TAR entry name against the shared archive path rules
/// (length, NUL, absolute paths, traversal, backslash, drive prefix) with
/// TAR-prefixed error text.
fn validate_tar_entry_path(name: &str, max_length: usize) -> Result<String, CamelError> {
    validate_entry_path(name, max_length, "TAR")
}

/// Malformed archive (bad header, truncated stream, checksum mismatch).
fn err_malformed_archive(detail: &str) -> CamelError {
    CamelError::TypeConversionFailed(format!("Invalid TAR archive: {detail}"))
}

/// Duplicate entry name under [`DuplicatePolicy::Reject`].
fn err_duplicate_entry_name(name: &str) -> CamelError {
    CamelError::TypeConversionFailed(format!("Duplicate TAR entry name: {name}"))
}

/// Entry-count cap violation.
fn err_max_entries(limit: usize) -> CamelError {
    CamelError::TypeConversionFailed(format!("TAR exceeds max entries: {limit}"))
}

/// Per-entry decoded-size cap violation.
fn err_entry_too_large(name: &str, size: u64, limit: u64) -> CamelError {
    CamelError::TypeConversionFailed(format!(
        "TAR entry '{name}' size {size} exceeds max {limit}"
    ))
}

/// Aggregate decoded-size cap violation.
fn err_total_decoded_exceeded(limit: u64) -> CamelError {
    CamelError::TypeConversionFailed(format!("TAR total decoded size exceeds max {limit}"))
}

/// Decoded-archive budget violation (TAR.GZ): the decompressed TAR stream
/// (payloads plus framing) exceeded the derived decode bound, so it cannot
/// be a payload-cap-respecting archive.
fn err_decoded_archive_budget(limit: u64) -> CamelError {
    CamelError::TypeConversionFailed(format!(
        "TAR.GZ decoded archive stream exceeds bounded decode budget {limit}"
    ))
}

/// Absolute fail-closed ceiling for the entry-count-derived part of the
/// TAR.GZ framing allowance. Without it, a huge `max_entries` would
/// saturate the allowance (and the budget) to `u64::MAX`, reopening an
/// unbounded decode. 64 MiB is generous for realistic PAX/GNU archives:
/// a default-cap archive (10,000 entries, each carrying a maximum-length
/// 4,096-byte name through a GNU longname or PAX extension block) needs
/// ~58.6 MiB of framing and still fits under the ceiling.
const MAX_TAR_FRAMING_ALLOWANCE: u64 = 64 * 1024 * 1024;

/// Constant base of the TAR.GZ framing allowance: the two end-of-archive
/// blocks, a PAX global ('g') header block, and slack. Per-entry
/// extension-block framing is covered by the per-entry term below.
const BASE_TAR_FRAMING_ALLOWANCE: u64 = 64 * 1024;

/// TAR block size in bytes.
const TAR_BLOCK_SIZE: u64 = 512;

/// Worst-case legit framing bytes per regular entry, derived from the
/// validated path cap (`max_path_length`):
///
/// - entry header block: 512
/// - GNU longname ('L') or PAX ('x') extension header block: 512 (tar
///   emits at most one of the two per entry)
/// - extension data block: the recorded path (bounded by the path cap,
///   plus record overhead — the `path=` key, length digits, and NUL),
///   padded to the 512-byte block boundary
/// - payload padding: up to 511 bytes
///
/// With the default 4,096-byte path cap this is
/// 512 + 512 + 4,608 + 511 = 6,143 bytes (~6 KiB) per entry.
fn tar_per_entry_framing(path_cap: usize) -> u64 {
    let extension_data = (path_cap as u64 + 32).div_ceil(TAR_BLOCK_SIZE) * TAR_BLOCK_SIZE;
    TAR_BLOCK_SIZE + TAR_BLOCK_SIZE + extension_data + (TAR_BLOCK_SIZE - 1)
}

/// Decode bound for a TAR.GZ stream whose entry payloads must stay within
/// `max_total_decoded_size`: the payload cap plus a dual-bounded TAR
/// framing allowance — `min(max_entries x per-entry framing, ceiling) +
/// base`. The per-entry term is sized from the validated path cap so a
/// maximum-length-name archive is never falsely rejected; the ceiling
/// keeps the allowance fail closed under absurd entry caps; the base
/// covers end-of-archive and global-header blocks that are not
/// proportional to the entry count. Payload accounting stays
/// authoritative in the parse; this bound only keeps the inflate itself
/// bounded.
///
/// Saturating arithmetic is contract, not accident: the budget derives
/// from the operator's payload cap, so a `max_total_decoded_size` of
/// `u64::MAX` is the operator explicitly opting out of decode bounding.
/// The derived framing term is always finite under its ceiling, so only
/// the operator-declared cap can saturate the sum; `checked_add` would
/// not change that contract, only rename the opt-out.
fn tar_gz_decode_budget(config: &TarSplitConfig) -> u64 {
    let per_entry =
        (config.max_entries as u64).saturating_mul(tar_per_entry_framing(config.max_path_length));
    let framing = per_entry.min(MAX_TAR_FRAMING_ALLOWANCE) + BASE_TAR_FRAMING_ALLOWANCE;
    config.max_total_decoded_size.saturating_add(framing)
}

/// Compressed-input cap violation (TAR.GZ), checked before decoding.
fn err_compressed_input_exceeded(size: u64, limit: u64) -> CamelError {
    CamelError::TypeConversionFailed(format!("TAR compressed size {size} exceeds max {limit}"))
}

/// A concatenated multi-member GZIP stream, which TAR.GZ splitting does not
/// support in v1.
fn err_multi_member_gzip() -> CamelError {
    CamelError::TypeConversionFailed(
        "TAR.GZ input contains multiple GZIP members; only a single-member \
         GZIP stream is supported"
            .to_string(),
    )
}

/// Zero regular-file entries while `allow_empty_archive` is false.
fn err_empty_archive() -> CamelError {
    CamelError::TypeConversionFailed(
        "TAR archive contains no regular entries; enable allow_empty_archive \
         to accept it"
            .to_string(),
    )
}

/// One emitted regular-file TAR entry, carried from the blocking reader
/// thread to the fragment-exchange builder.
struct TarEntryData {
    index: usize,
    path: String,
    size: u64,
    data: Vec<u8>,
}

/// Split a TAR archive's materialized bytes into a stream of Exchanges, one
/// per regular-file entry in header order.
///
/// Takes owned `Bytes` (for `'static` lifetime), a parent `Exchange` whose
/// headers and properties are cloned into each entry's exchange, and a
/// `TarSplitConfig` controlling limits and policy. Entries are read
/// sequentially from the in-memory archive: names are validated before any
/// body is read, directories/symlinks/hard links/devices are skipped (link
/// targets are never read), and each regular body is materialized through a
/// bounded `take(max_per_entry_size + 1)` read so an oversized entry is
/// rejected before its bytes are retained.
pub fn split_tar_bytes(
    parent: Exchange,
    bytes: Bytes,
    config: TarSplitConfig,
) -> Pin<Box<dyn Stream<Item = Result<Exchange, CamelError>> + Send>> {
    Box::pin(async_stream::stream! {
        // Compressed-input cap: applies to the materialized input bytes
        // before any entry is decoded.
        if bytes.len() as u64 > config.max_compressed_size {
            yield Err(err_compressed_input_exceeded(
                bytes.len() as u64,
                config.max_compressed_size,
            ));
            return;
        }

        let entries = tar_entry_stream(parent, bytes, config);
        for await result in entries {
            yield result;
        }
    })
}

/// Split a single-member TAR.GZ stream into a stream of Exchanges, one per
/// regular-file entry in header order.
///
/// The compressed-input cap is checked before decompression is accepted, the
/// input is then decoded through the bounded single-member GZIP decoder
/// shared with `GzipDataFormat`. The decode budget is the payload cap
/// (`max_total_decoded_size`) plus a bounded TAR framing allowance, so the
/// inflate is bounded without counting framing against the payload cap; the
/// resulting TAR bytes run through the same bounded entry reader as plain
/// TAR splitting, which enforces the payload accounting authoritatively. A
/// concatenated multi-member GZIP stream is rejected instead of silently
/// splitting only the first member.
pub fn split_tar_gz_bytes(
    parent: Exchange,
    bytes: Bytes,
    config: TarSplitConfig,
) -> Pin<Box<dyn Stream<Item = Result<Exchange, CamelError>> + Send>> {
    Box::pin(async_stream::stream! {
        // Compressed-input cap: enforced on the compressed bytes before any
        // decompression is accepted.
        if bytes.len() as u64 > config.max_compressed_size {
            yield Err(err_compressed_input_exceeded(
                bytes.len() as u64,
                config.max_compressed_size,
            ));
            return;
        }

        // Bounded single-member GZIP decode, run on the blocking pool so the
        // inflate never executes on the async runtime. The decompressed TAR
        // stream is budgeted from the payload cap plus the TAR framing
        // allowance; entry-level caps and payload accounting are still
        // enforced during the parse. If the fragment stream is dropped
        // before the decode finishes, the detached blocking task still
        // terminates on its own: the input is fully materialized and the
        // decode is capped at `take_limit` bytes.
        let decode_budget = tar_gz_decode_budget(&config);
        let take_limit = decode_budget.saturating_add(1);
        let decode_input = bytes.clone();
        let first = match tokio::task::spawn_blocking(move || {
            decode_first_member(&decode_input, take_limit)
        })
        .await
        {
            Ok(Ok(first)) => first,
            Ok(Err(e)) => {
                yield Err(err_malformed_archive(&format!(
                    "failed to decode GZIP stream: {e}"
                )));
                return;
            }
            Err(e) => {
                yield Err(err_malformed_archive(&format!(
                    "GZIP decode task failed: {e}"
                )));
                return;
            }
        };

        // `take_limit` is one past the budget, so a full buffer means the
        // decompressed stream exceeded the budget (a framing-heavy
        // decompression bomb that would never satisfy the payload cap).
        if first.data.len() as u64 >= take_limit {
            yield Err(err_decoded_archive_budget(decode_budget));
            return;
        }

        // v1 TAR.GZ is single-member: trailing input past the first member is
        // rejected rather than silently split.
        if first.has_trailing_input {
            yield Err(err_multi_member_gzip());
            return;
        }

        let entries = tar_entry_stream(parent, Bytes::from(first.data), config);
        for await result in entries {
            yield result;
        }
    })
}

/// Sequential bounded TAR parse over materialized (already decompressed)
/// bytes. Runs the reader on a blocking thread and drains the bounded
/// channel into the fragment stream; callers own any input-level cap checks.
fn tar_entry_stream(
    parent: Exchange,
    bytes: Bytes,
    config: TarSplitConfig,
) -> Pin<Box<dyn Stream<Item = Result<Exchange, CamelError>> + Send>> {
    Box::pin(async_stream::stream! {
        let (tx, mut rx) = mpsc::channel::<Result<TarEntryData, CamelError>>(
            DEFAULT_CHANNEL_CAPACITY,
        );

        let max_entries = config.max_entries;
        let max_per_entry = config.max_per_entry_size;
        let max_total = config.max_total_decoded_size;
        let max_path_len = config.max_path_length;
        let allow_empty = config.allow_empty_archive;
        let dup_policy = config.duplicate_names_policy;

        // The `tar` reader state is not `Send`, so the sequential parse runs
        // on a blocking thread, keeping blocking reads off the async runtime.
        // The bounded channel backpressures the reader; dropping the stream
        // drops the receiver, so `blocking_send` fails and the task exits.
        tokio::task::spawn_blocking(move || {
            let mut archive = tar::Archive::new(std::io::Cursor::new(bytes));
            let entries = match archive.entries() {
                Ok(entries) => entries,
                Err(e) => {
                    let _ = tx.blocking_send(Err(err_malformed_archive(&e.to_string())));
                    return;
                }
            };

            let mut total_decoded: u64 = 0;
            let mut emitted: usize = 0;
            let mut emitted_names: HashSet<String> = HashSet::new();
            let mut name_occurrences: HashMap<String, usize> = HashMap::new();

            for entry in entries {
                let mut entry = match entry {
                    Ok(e) => e,
                    Err(e) => {
                        let _ = tx.blocking_send(Err(err_malformed_archive(&e.to_string())));
                        return;
                    }
                };

                // Validate the entry name before any body use; the name is
                // never handed to the filesystem.
                let raw_name = match entry.path() {
                    Ok(p) => match p.to_str() {
                        Some(name) => name.to_string(),
                        None => {
                            let _ = tx.blocking_send(Err(err_malformed_archive(
                                "entry name is not valid UTF-8",
                            )));
                            return;
                        }
                    },
                    Err(e) => {
                        let _ = tx.blocking_send(Err(err_malformed_archive(&e.to_string())));
                        return;
                    }
                };
                let mut validated = match validate_tar_entry_path(&raw_name, max_path_len) {
                    Ok(path) => path,
                    Err(e) => {
                        let _ = tx.blocking_send(Err(e));
                        return;
                    }
                };

                // Skip directories, symlinks, hard links, and device nodes;
                // link targets are never read and nothing touches the
                // filesystem.
                if !matches!(entry.header().entry_type(), tar::EntryType::Regular) {
                    continue;
                }

                // Bounded read: one extra byte detects overflow before the
                // body is retained.
                let mut data = Vec::new();
                let mut limited = Read::take(&mut entry, max_per_entry.saturating_add(1));
                if let Err(e) = limited.read_to_end(&mut data) {
                    let _ = tx.blocking_send(Err(err_malformed_archive(&format!(
                        "failed to read TAR entry '{raw_name}': {e}"
                    ))));
                    return;
                }

                if data.len() as u64 > max_per_entry {
                    // Report the header-declared size: the bounded read only
                    // pulled through max + 1 bytes before rejecting.
                    let declared = entry.header().size().unwrap_or(data.len() as u64);
                    let _ = tx.blocking_send(Err(err_entry_too_large(
                        &raw_name,
                        declared,
                        max_per_entry,
                    )));
                    return;
                }

                let new_total = total_decoded.saturating_add(data.len() as u64);
                if new_total > max_total {
                    let _ = tx.blocking_send(Err(err_total_decoded_exceeded(max_total)));
                    return;
                }
                total_decoded = new_total;

                let index = emitted;
                if index >= max_entries {
                    let _ = tx.blocking_send(Err(err_max_entries(max_entries)));
                    return;
                }
                emitted += 1;

                match dup_policy {
                    DuplicatePolicy::Reject => {
                        if !emitted_names.insert(validated.clone()) {
                            let _ = tx.blocking_send(Err(err_duplicate_entry_name(&validated)));
                            return;
                        }
                    }
                    DuplicatePolicy::AllowWithIndex => {
                        // The occurrence counter alone cannot guarantee
                        // uniqueness: a literal entry can occupy an indexed
                        // name first (a.txt, a.1.txt, a.txt would derive
                        // a.1.txt twice), so the emitted-name set is the
                        // authority and the index bumps until the candidate
                        // is free.
                        let occurrences =
                            name_occurrences.entry(validated.clone()).or_insert(0);
                        let start = if *occurrences > 0 {
                            *occurrences
                        } else if emitted_names.contains(&validated) {
                            1
                        } else {
                            0
                        };
                        if start > 0 {
                            let (candidate, used) =
                                next_free_indexed_name(&validated, start, &emitted_names);
                            // The suffix grows the name, so the path-length
                            // cap stays authoritative for indexed names too.
                            validated = match validate_tar_entry_path(&candidate, max_path_len)
                            {
                                Ok(path) => path,
                                Err(e) => {
                                    let _ = tx.blocking_send(Err(e));
                                    return;
                                }
                            };
                            *occurrences = (*occurrences).max(used + 1);
                        }
                        emitted_names.insert(validated.clone());
                    }
                }

                if tx
                    .blocking_send(Ok(TarEntryData {
                        index,
                        path: validated,
                        size: data.len() as u64,
                        data,
                    }))
                    .is_err()
                {
                    return;
                }
            }

            if emitted == 0 && !allow_empty {
                let _ = tx.blocking_send(Err(err_empty_archive()));
            }
        });

        while let Some(result) = rx.recv().await {
            match result {
                Ok(entry) => {
                    let TarEntryData {
                        index,
                        path,
                        size,
                        data,
                    } = entry;
                    let msg = Message {
                        headers: parent.input.headers.clone(),
                        body: Body::Bytes(Bytes::from(data)),
                    };
                    let mut ex = Exchange::new(msg);
                    // Strip parent-level content headers that are stale for
                    // individual TAR entries.
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
                        CAMEL_TAR_ENTRY_NAME.to_string(),
                        Value::String(entry_name),
                    );
                    ex.input
                        .headers
                        .insert(CAMEL_TAR_ENTRY_PATH.to_string(), Value::String(path));
                    ex.input.headers.insert(
                        CAMEL_TAR_ENTRY_INDEX.to_string(),
                        Value::from(index as u64),
                    );
                    ex.input
                        .headers
                        .insert(CAMEL_TAR_ENTRY_SIZE.to_string(), Value::from(size));
                    ex.input.headers.insert(
                        CAMEL_TAR_ENTRY_IS_DIRECTORY.to_string(),
                        Value::Bool(false),
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

/// Build a [`StreamingSplitExpression`] that splits TAR archive bodies into
/// per-entry Exchanges.
pub fn tar_splitter(config: TarSplitConfig) -> StreamingSplitExpression {
    Arc::new(move |exchange: Exchange| {
        let config = config.clone();
        match exchange.input.body.clone() {
            Body::Bytes(b) => split_tar_bytes(exchange, b, config),
            Body::Text(s) => split_tar_bytes(exchange, Bytes::from(s.as_bytes().to_vec()), config),
            _ => Box::pin(async_stream::stream! {
                yield Err(CamelError::TypeConversionFailed(
                    "TarSplitter requires Body::Bytes or Body::Text".to_string(),
                ));
            }),
        }
    })
}

/// Build a [`StreamingSplitExpression`] that splits single-member TAR.GZ
/// bodies into per-entry Exchanges after bounded GZIP decoding.
pub fn tar_gz_splitter(config: TarSplitConfig) -> StreamingSplitExpression {
    Arc::new(move |exchange: Exchange| {
        let config = config.clone();
        match exchange.input.body.clone() {
            Body::Bytes(b) => split_tar_gz_bytes(exchange, b, config),
            Body::Text(s) => {
                split_tar_gz_bytes(exchange, Bytes::from(s.as_bytes().to_vec()), config)
            }
            _ => Box::pin(async_stream::stream! {
                yield Err(CamelError::TypeConversionFailed(
                    "TarGzSplitter requires Body::Bytes or Body::Text".to_string(),
                ));
            }),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive_splitter::test_util::make_zip_raw;
    use crate::zip_splitter::{CAMEL_ZIP_ENTRY_PATH, ZipSplitConfig, zip_splitter};
    use futures::StreamExt;

    /// Build one 512-byte TAR header block with a valid checksum.
    ///
    /// Hand-rolled so tests can create entry shapes the `tar` writer refuses
    /// to produce (absolute names, traversal names, device nodes).
    fn tar_header(name: &str, size: u64, typeflag: u8) -> [u8; 512] {
        let mut h = [0u8; 512];
        h[..name.len()].copy_from_slice(name.as_bytes());
        h[124..136].copy_from_slice(format!("{size:011o}\0").as_bytes());
        // Checksum field must be spaces while the checksum is computed.
        h[148..156].copy_from_slice(b"        ");
        h[156] = typeflag;
        h[257..263].copy_from_slice(b"ustar\0");
        h[263..265].copy_from_slice(b"00");
        let sum: u32 = h.iter().map(|&b| u32::from(b)).sum();
        h[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
        h
    }

    /// Assemble headers, padded data blocks, and the two-block end marker
    /// into a complete TAR archive.
    fn tar_archive(entries: &[(&str, u8, &[u8])]) -> Vec<u8> {
        let mut out = Vec::new();
        for &(name, typeflag, data) in entries {
            out.extend_from_slice(&tar_header(name, data.len() as u64, typeflag));
            if !data.is_empty() {
                let mut block = data.to_vec();
                let rem = block.len() % 512;
                if rem != 0 {
                    block.extend_from_slice(&vec![0u8; 512 - rem]);
                }
                out.extend_from_slice(&block);
            }
        }
        out.extend_from_slice(&[0u8; 1024]);
        out
    }

    async fn collect(
        config: TarSplitConfig,
        tar_data: Vec<u8>,
    ) -> Vec<Result<Exchange, CamelError>> {
        let expr = tar_splitter(config);
        let exchange = Exchange::new(Message {
            headers: Default::default(),
            body: Body::Bytes(Bytes::from(tar_data)),
        });
        expr(exchange).collect().await
    }

    #[test]
    fn tar_split_config_rejects_unknown_fields() {
        // Known keys deserialize; missing keys fall back to the bounded defaults.
        let cfg: TarSplitConfig =
            serde_json::from_str(r#"{"max_entries": 5, "allow_empty_archive": true}"#)
                .expect("valid config must deserialize");
        assert_eq!(cfg.max_entries, 5);
        assert!(cfg.allow_empty_archive);
        assert_eq!(cfg.max_per_entry_size, DEFAULT_MAX_PER_ENTRY_SIZE);
        assert_eq!(cfg.max_path_length, DEFAULT_MAX_PATH_LENGTH);

        // Unknown keys fail closed instead of being ignored.
        let err = serde_json::from_str::<TarSplitConfig>(r#"{"unknown_key": 1}"#)
            .expect_err("unknown config key must fail");
        assert!(
            err.to_string().contains("unknown field `unknown_key`"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn tar_split_emits_regular_files_in_header_order() {
        // Header order: file, dir, symlink, hard link, char device, file.
        // Only the two regular files may surface as fragments.
        let tar_data = tar_archive(&[
            ("first.txt", b'0', b"alpha".as_slice()),
            ("docs", b'5', b""),
            ("link.txt", b'2', b""),
            ("hard.txt", b'1', b""),
            ("dev-zero", b'3', b""),
            ("second.txt", b'0', b"beta".as_slice()),
        ]);
        let results = collect(TarSplitConfig::default(), tar_data).await;
        assert_eq!(results.len(), 2, "non-regular entries must be omitted");

        let first = results[0].as_ref().expect("first fragment ok");
        let second = results[1].as_ref().expect("second fragment ok");

        assert_eq!(
            first.input.headers.get(CAMEL_TAR_ENTRY_NAME),
            Some(&Value::String("first.txt".to_string()))
        );
        assert_eq!(
            second.input.headers.get(CAMEL_TAR_ENTRY_NAME),
            Some(&Value::String("second.txt".to_string()))
        );
        assert_eq!(
            first.input.headers.get(CAMEL_TAR_ENTRY_INDEX),
            Some(&Value::from(0u64))
        );
        assert_eq!(
            second.input.headers.get(CAMEL_TAR_ENTRY_INDEX),
            Some(&Value::from(1u64))
        );
        match &first.input.body {
            Body::Bytes(b) => assert_eq!(b.as_ref(), b"alpha"),
            other => panic!("expected Body::Bytes, got {other:?}"),
        }
        match &second.input.body {
            Body::Bytes(b) => assert_eq!(b.as_ref(), b"beta"),
            other => panic!("expected Body::Bytes, got {other:?}"),
        }
        assert_eq!(
            first.input.headers.get(CAMEL_TAR_ENTRY_PATH),
            Some(&Value::String("first.txt".to_string()))
        );
        assert_eq!(
            first.input.headers.get(CAMEL_TAR_ENTRY_SIZE),
            Some(&Value::from(5u64))
        );
        assert_eq!(
            first.input.headers.get(CAMEL_TAR_ENTRY_IS_DIRECTORY),
            Some(&Value::Bool(false))
        );
    }

    #[tokio::test]
    async fn tar_split_rejects_traversal_and_absolute_names() {
        let cases = [
            (
                "../escape",
                "TAR entry path contains '..' traversal: ../escape",
            ),
            ("/absolute", "TAR entry path is absolute: /absolute"),
        ];
        for (name, expected) in cases {
            let tar_data = tar_archive(&[(name, b'0', b"oops".as_slice())]);
            let results = collect(TarSplitConfig::default(), tar_data).await;
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

    #[tokio::test]
    async fn tar_split_enforces_all_bounds() {
        // Entry-count cap: three regular entries against max_entries: 2.
        let tar_data = tar_archive(&[
            ("a.txt", b'0', b"1".as_slice()),
            ("b.txt", b'0', b"2".as_slice()),
            ("c.txt", b'0', b"3".as_slice()),
        ]);
        let config = TarSplitConfig {
            max_entries: 2,
            ..Default::default()
        };
        let results = collect(config, tar_data).await;
        assert!(
            results.iter().any(|r| r
                .as_ref()
                .is_err_and(|e| e.to_string().contains("TAR exceeds max entries: 2"))),
            "expected entry-count cap error, got {results:?}"
        );

        // Per-entry cap: one 200-byte entry against max_per_entry_size: 100.
        let tar_data = tar_archive(&[("big.bin", b'0', &[b'x'; 200])]);
        let config = TarSplitConfig {
            max_per_entry_size: 100,
            ..Default::default()
        };
        let results = collect(config, tar_data).await;
        assert_eq!(results.len(), 1);
        assert!(
            results[0]
                .as_ref()
                .expect_err("per-entry cap")
                .to_string()
                .contains("TAR entry 'big.bin' size 200 exceeds max 100"),
            "expected per-entry cap error"
        );

        // Total-decoded cap: two 10-byte entries against max 15.
        let tar_data = tar_archive(&[
            ("a.txt", b'0', b"0123456789".as_slice()),
            ("b.txt", b'0', b"9876543210".as_slice()),
        ]);
        let config = TarSplitConfig {
            max_total_decoded_size: 15,
            ..Default::default()
        };
        let results = collect(config, tar_data).await;
        assert!(
            results.iter().any(|r| r.as_ref().is_err_and(|e| e
                .to_string()
                .contains("TAR total decoded size exceeds max 15"))),
            "expected total-decoded cap error, got {results:?}"
        );

        // Compressed-input cap: the materialized input itself against max 512.
        let tar_data = tar_archive(&[("a.txt", b'0', b"x".as_slice())]);
        let config = TarSplitConfig {
            max_compressed_size: 512,
            ..Default::default()
        };
        let results = collect(config, tar_data).await;
        assert_eq!(results.len(), 1);
        assert!(
            results[0]
                .as_ref()
                .expect_err("compressed-input cap")
                .to_string()
                .contains("exceeds max 512"),
            "expected compressed-input cap error"
        );

        // Path-length cap: a 26-character name against max_path_length: 10.
        let long_name = "a-very-long-entry-name.bin";
        let tar_data = tar_archive(&[(long_name, b'0', b"x".as_slice())]);
        let config = TarSplitConfig {
            max_path_length: 10,
            ..Default::default()
        };
        let results = collect(config, tar_data).await;
        assert_eq!(results.len(), 1);
        assert!(
            results[0]
                .as_ref()
                .expect_err("path-length cap")
                .to_string()
                .contains(&format!(
                    "TAR entry path exceeds max length: {} > 10",
                    long_name.len()
                )),
            "expected path-length cap error"
        );
    }

    #[tokio::test]
    async fn tar_split_empty_and_directory_only_archives_emit_zero() {
        let config = TarSplitConfig {
            allow_empty_archive: true,
            ..Default::default()
        };
        let results = collect(config.clone(), tar_archive(&[])).await;
        assert!(
            results.is_empty(),
            "empty archive must emit zero fragments: {results:?}"
        );

        let results = collect(
            config,
            tar_archive(&[("only-dir", b'5', b""), ("nested", b'5', b"")]),
        )
        .await;
        assert!(
            results.is_empty(),
            "directory-only archive must emit zero fragments: {results:?}"
        );
    }

    /// Compress raw bytes into a single-member GZIP stream for TAR.GZ setups.
    fn gzip_bytes(raw: &[u8]) -> Vec<u8> {
        use std::io::Write as _;
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(raw).expect("gzip write");
        encoder.finish().expect("gzip finish")
    }

    async fn collect_gz(
        config: TarSplitConfig,
        gz_data: Vec<u8>,
    ) -> Vec<Result<Exchange, CamelError>> {
        let expr = tar_gz_splitter(config);
        let exchange = Exchange::new(Message {
            headers: Default::default(),
            body: Body::Bytes(Bytes::from(gz_data)),
        });
        expr(exchange).collect().await
    }

    fn header_string(exchange: &Exchange, key: &str) -> String {
        match exchange.input.headers.get(key) {
            Some(Value::String(s)) => s.clone(),
            other => panic!("header {key} must be a string, got {other:?}"),
        }
    }

    fn body_bytes(exchange: &Exchange) -> Vec<u8> {
        match &exchange.input.body {
            Body::Bytes(b) => b.to_vec(),
            other => panic!("expected Body::Bytes, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn tar_gz_split_matches_tar_metadata() {
        let tar_data = tar_archive(&[
            ("first.txt", b'0', b"alpha".as_slice()),
            ("docs", b'5', b""),
            ("second.txt", b'0', b"beta".as_slice()),
        ]);

        let tar_results = collect(TarSplitConfig::default(), tar_data.clone()).await;
        let gz_results = collect_gz(TarSplitConfig::default(), gzip_bytes(&tar_data)).await;

        assert_eq!(gz_results.len(), tar_results.len(), "same entries emitted");
        for (t, g) in tar_results.iter().zip(gz_results.iter()) {
            let t = t.as_ref().expect("TAR fragment ok");
            let g = g.as_ref().expect("TAR.GZ fragment ok");
            for header in [
                CAMEL_TAR_ENTRY_NAME,
                CAMEL_TAR_ENTRY_PATH,
                CAMEL_TAR_ENTRY_INDEX,
                CAMEL_TAR_ENTRY_SIZE,
                CAMEL_TAR_ENTRY_IS_DIRECTORY,
            ] {
                assert_eq!(
                    t.input.headers.get(header),
                    g.input.headers.get(header),
                    "header {header} must match TAR output"
                );
            }
            assert_eq!(t.input.body, g.input.body, "body must match TAR output");
        }
    }

    /// TAR applies the shared archive duplicate-name policy directly
    /// (per the narrowed spec this is TAR's own contract, not ZIP
    /// parity): `Reject` fails on the first duplicate; `AllowWithIndex`
    /// emits every entry with a deterministic indexed name for later
    /// duplicates.
    #[tokio::test]
    async fn tar_split_applies_shared_duplicate_policy() {
        let tar_data = tar_archive(&[
            ("dup.txt", b'0', b"one".as_slice()),
            ("other.txt", b'0', b"mid".as_slice()),
            ("dup.txt", b'0', b"two".as_slice()),
        ]);

        // Reject policy: the split fails on the first duplicate name.
        let reject_config = TarSplitConfig {
            duplicate_names_policy: DuplicatePolicy::Reject,
            ..Default::default()
        };
        let results = collect(reject_config, tar_data.clone()).await;
        assert!(
            results.iter().any(|r| r
                .as_ref()
                .is_err_and(|e| e.to_string().contains("Duplicate TAR entry name: dup.txt"))),
            "reject policy must fail on the duplicate: {results:?}"
        );

        // AllowWithIndex: every entry is emitted and the later duplicate gets
        // a deterministic index inserted before its extension.
        let index_config = TarSplitConfig {
            duplicate_names_policy: DuplicatePolicy::AllowWithIndex,
            ..Default::default()
        };
        let first_run = collect(index_config.clone(), tar_data.clone()).await;
        let second_run = collect(index_config, tar_data).await;

        assert_eq!(first_run.len(), 3, "every entry must be emitted");
        let snapshot: Vec<(String, String, Vec<u8>)> = first_run
            .iter()
            .map(|r| {
                let ex = r.as_ref().expect("allow-with-index must not fail");
                (
                    header_string(ex, CAMEL_TAR_ENTRY_PATH),
                    header_string(ex, CAMEL_TAR_ENTRY_NAME),
                    body_bytes(ex),
                )
            })
            .collect();
        assert_eq!(
            snapshot,
            [
                (
                    "dup.txt".to_string(),
                    "dup.txt".to_string(),
                    b"one".to_vec()
                ),
                (
                    "other.txt".to_string(),
                    "other.txt".to_string(),
                    b"mid".to_vec()
                ),
                (
                    "dup.1.txt".to_string(),
                    "dup.1.txt".to_string(),
                    b"two".to_vec()
                ),
            ],
            "indexed names must be deterministic"
        );

        let second_paths: Vec<String> = second_run
            .iter()
            .map(|r| {
                let ex = r.as_ref().expect("second run must not fail");
                header_string(ex, CAMEL_TAR_ENTRY_PATH)
            })
            .collect();
        let first_paths: Vec<String> = snapshot.into_iter().map(|(p, _, _)| p).collect();
        assert_eq!(
            first_paths, second_paths,
            "naming must be deterministic across runs"
        );
    }

    /// `DuplicatePolicy` ownership, per the narrowed spec: TAR applies the
    /// shared archive duplicate-name vocabulary (`Reject` fails the split;
    /// `AllowWithIndex` emits deterministic collision-free indexed names via
    /// the shared [`crate::archive_splitter::indexed_duplicate_name`] /
    /// [`crate::archive_splitter::next_free_indexed_name`] helpers). ZIP's
    /// historical behavior is pinned, not claimed as parity: the `zip`
    /// reader indexes the central directory by name, so duplicate entries
    /// collapse (last data wins) before the splitter observes them. That
    /// boundary is pinned here so a reader change that starts surfacing
    /// duplicates forces the ZIP mangling/reject branches (shared with TAR)
    /// to be revisited instead of silently changing behavior. ZIP runtime
    /// behavior is unchanged by this change.
    #[tokio::test]
    async fn tar_duplicate_policy_is_shared_zip_collapse_is_pinned() {
        let tar_data = tar_archive(&[
            ("dup.txt", b'0', b"one".as_slice()),
            ("dup.txt", b'0', b"two".as_slice()),
        ]);
        let zip_data = make_zip_raw(&[
            ("dup.txt", b"one".as_slice()),
            ("dup.txt", b"two".as_slice()),
        ]);

        // AllowWithIndex on TAR: every entry is emitted and the later
        // duplicate carries the shared deterministic indexed suffix.
        let index_config = TarSplitConfig {
            duplicate_names_policy: DuplicatePolicy::AllowWithIndex,
            ..Default::default()
        };
        let tar_results = collect(index_config, tar_data.clone()).await;
        let tar_paths: Vec<String> = tar_results
            .iter()
            .map(|r| header_string(r.as_ref().expect("TAR fragment ok"), CAMEL_TAR_ENTRY_PATH))
            .collect();
        assert_eq!(
            tar_paths,
            ["dup.txt".to_string(), "dup.1.txt".to_string()],
            "TAR indexed names must come from the shared mangling helper"
        );

        // AllowWithIndex on ZIP over the same logical entries: the reader
        // collapses the duplicates, so exactly one unmangled fragment with
        // the surviving (last) body is observable.
        let zip_expr = zip_splitter(ZipSplitConfig {
            duplicate_names_policy: DuplicatePolicy::AllowWithIndex,
            ..Default::default()
        });
        let zip_exchange = Exchange::new(Message {
            headers: Default::default(),
            body: Body::Bytes(Bytes::from(zip_data.clone())),
        });
        let zip_results: Vec<Result<Exchange, CamelError>> = zip_expr(zip_exchange).collect().await;
        assert_eq!(
            zip_results.len(),
            1,
            "the zip reader must collapse duplicate names for now: {zip_results:?}"
        );
        let zip_path = header_string(
            zip_results[0].as_ref().expect("ZIP fragment ok"),
            CAMEL_ZIP_ENTRY_PATH,
        );
        assert_eq!(
            zip_path, "dup.txt",
            "unique observable names stay unmangled"
        );

        // Reject on TAR: the split fails on the first duplicate name.
        let reject_config = TarSplitConfig {
            duplicate_names_policy: DuplicatePolicy::Reject,
            ..Default::default()
        };
        let tar_err = collect(reject_config, tar_data).await;
        let tar_msg = tar_err
            .iter()
            .find_map(|r| r.as_ref().err().map(|e| e.to_string()))
            .expect("TAR reject must fail");
        assert!(
            tar_msg.contains("Duplicate TAR entry name: dup.txt"),
            "{tar_msg}"
        );

        // Reject on ZIP over the same logical entries: with duplicates
        // collapsed away the names are unique, so the split succeeds — the
        // reject branch stays reserved for a reader that surfaces them.
        let zip_expr = zip_splitter(ZipSplitConfig {
            duplicate_names_policy: DuplicatePolicy::Reject,
            ..Default::default()
        });
        let zip_exchange = Exchange::new(Message {
            headers: Default::default(),
            body: Body::Bytes(Bytes::from(zip_data)),
        });
        let zip_results: Vec<Result<Exchange, CamelError>> = zip_expr(zip_exchange).collect().await;
        assert!(
            zip_results.iter().all(|r| r.is_ok()),
            "collapsed names are unique, so reject must not fire: {zip_results:?}"
        );
    }

    #[tokio::test]
    async fn tar_gz_compressed_input_limit_is_checked() {
        let gz = gzip_bytes(&tar_archive(&[("a.txt", b'0', b"payload".as_slice())]));
        let config = TarSplitConfig {
            max_compressed_size: gz.len() as u64 - 1,
            ..Default::default()
        };
        let results = collect_gz(config, gz.clone()).await;
        assert_eq!(results.len(), 1);
        let err = results[0]
            .as_ref()
            .expect_err("compressed input over the cap must fail before decompression")
            .to_string();
        assert!(
            err.contains(&format!(
                "TAR compressed size {} exceeds max {}",
                gz.len(),
                gz.len() - 1
            )),
            "expected the compressed-input cap error: {err}"
        );
    }

    #[tokio::test]
    async fn tar_gz_multi_member_is_rejected() {
        let member_one = gzip_bytes(&tar_archive(&[("a.txt", b'0', b"one".as_slice())]));
        let member_two = gzip_bytes(&tar_archive(&[("b.txt", b'0', b"two".as_slice())]));
        let mut concatenated = member_one;
        concatenated.extend_from_slice(&member_two);

        let results = collect_gz(TarSplitConfig::default(), concatenated).await;
        assert_eq!(results.len(), 1);
        let err = results[0]
            .as_ref()
            .expect_err("multi-member input must be rejected, not split silently")
            .to_string();
        assert!(
            err.contains("multiple GZIP members"),
            "expected the unsupported-multi-member error: {err}"
        );
    }

    /// `max_total_decoded_size` is a payload-byte cap. TAR framing
    /// (512-byte headers, padding, end-of-archive blocks) must not count
    /// against it on TAR.GZ: the decode step is bounded by the payload cap
    /// plus a framing allowance derived from `max_entries`, and the parse
    /// enforces the payload accounting authoritatively.
    #[tokio::test]
    async fn tar_gz_total_decoded_cap_counts_payload_not_framing() {
        // Three tiny entries: payload total (30 bytes) well under the
        // 64-byte cap, decoded stream (~4 KiB with framing) well over it.
        // A framing-inclusive decode gate would reject this archive.
        let payload: &[u8] = b"0123456789";
        let gz = gzip_bytes(&tar_archive(&[
            ("a.txt", b'0', payload),
            ("b.txt", b'0', payload),
            ("c.txt", b'0', payload),
        ]));
        let results = collect_gz(
            TarSplitConfig {
                max_total_decoded_size: 64,
                ..Default::default()
            },
            gz,
        )
        .await;
        assert_eq!(
            results.len(),
            3,
            "framing must not count against the payload cap"
        );
        for r in &results {
            assert!(r.is_ok(), "fragment must succeed: {r:?}");
        }

        // A payload that itself exceeds the cap still fails, at the parse,
        // with the payload-cap error.
        let big = vec![b'x'; 100];
        let gz_big = gzip_bytes(&tar_archive(&[("big.txt", b'0', big.as_slice())]));
        let over = collect_gz(
            TarSplitConfig {
                max_total_decoded_size: 64,
                ..Default::default()
            },
            gz_big,
        )
        .await;
        assert_eq!(over.len(), 1);
        let err = over[0]
            .as_ref()
            .expect_err("payload over the cap must fail at the parse")
            .to_string();
        assert!(
            err.contains("TAR total decoded size exceeds max 64"),
            "expected the payload-cap error: {err}"
        );
    }

    /// The entry-count term of the framing allowance must be ceilinged,
    /// not saturating: an absurd `max_entries` must not inflate the decode
    /// budget toward `u64::MAX` (fail-open decode bomb).
    #[test]
    fn tar_gz_decode_budget_is_fail_closed_under_absurd_entry_caps() {
        // Per-entry derivation at the default path cap: 512 (header)
        // + 512 (extension header) + 4,608 (padded extension data for a
        // 4,096-byte name) + 511 (payload padding) = 6,143.
        assert_eq!(tar_per_entry_framing(4096), 6143);
        // Default config: 10,000 entries x 6,143 (~58.6 MiB) stays under
        // the 64 MiB ceiling, so the ceiling does not bite by default.
        assert_eq!(
            tar_gz_decode_budget(&TarSplitConfig::default()),
            1024 * 1024 * 1024 + 61_430_000 + 64 * 1024
        );

        let config = TarSplitConfig {
            max_entries: usize::MAX,
            max_total_decoded_size: 1024,
            ..Default::default()
        };
        let budget = tar_gz_decode_budget(&config);
        assert_eq!(
            budget,
            1024 + MAX_TAR_FRAMING_ALLOWANCE + BASE_TAR_FRAMING_ALLOWANCE,
            "the entry-count term must hit the absolute ceiling, not saturate"
        );
        assert!(budget < u64::MAX, "the budget must stay fail-closed finite");
    }

    /// GNU longname ('L') and PAX ('x') extension blocks are physical
    /// stream framing without being separate entries: the constant base
    /// of the framing allowance must keep single-entry long-name archives
    /// inside the decode budget instead of falsely rejecting them.
    #[tokio::test]
    async fn tar_gz_long_name_extension_blocks_stay_within_budget() {
        let long_path = format!("{}file.txt", "very/long/directory/prefix/".repeat(12));
        assert!(long_path.len() > 100, "the path must exceed the name field");

        // GNU: `append_data` emits a 'L' longname extension block before
        // the real header when the path does not fit the name field.
        let mut builder = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        header.set_size(10);
        header.set_mode(0o644);
        header.set_cksum();
        builder
            .append_data(&mut header, long_path.as_str(), b"0123456789".as_slice())
            .expect("append GNU long-name entry");
        let gnu_archive = builder.into_inner().expect("finish GNU archive");

        // PAX: a hand-crafted 'x' extended header whose record overrides
        // the short header name of the following entry.
        let pax_record = {
            let body = format!(" path={long_path}\n");
            // The record's declared length includes its own digits.
            let mut total = body.len() + 1;
            while total.to_string().len() + body.len() != total {
                total += 1;
            }
            format!("{total}{body}").into_bytes()
        };
        let pax_archive = tar_archive(&[
            ("./PaxHeaders.0/f", b'x', pax_record.as_slice()),
            ("file.txt", b'0', b"0123456789".as_slice()),
        ]);

        for (label, archive) in [("gnu", gnu_archive), ("pax", pax_archive)] {
            let gz = gzip_bytes(&archive);
            let results = collect_gz(
                TarSplitConfig {
                    // Tight payload cap: 10 payload bytes plus margin, so
                    // only the allowance keeps the decode accepted.
                    max_total_decoded_size: 32,
                    ..Default::default()
                },
                gz,
            )
            .await;
            assert_eq!(results.len(), 1, "{label}: one fragment expected");
            let ex = results[0]
                .as_ref()
                .unwrap_or_else(|e| panic!("{label}: long-name archive must split: {e}"));
            let path = header_string(ex, CAMEL_TAR_ENTRY_PATH);
            assert!(
                path.ends_with("file.txt") && path.len() > 100,
                "{label}: the long path must survive: {path}"
            );
        }
    }

    /// The occurrence counter alone cannot guarantee unique emitted names:
    /// `a.txt`, `a.1.txt`, `a.txt` would derive `a.1.txt` twice. The
    /// emitted-name set is the collision authority and the index bumps
    /// until the candidate is free.
    #[tokio::test]
    async fn tar_split_indexed_names_never_collide_with_emitted() {
        let data = tar_archive(&[
            ("a.txt", b'0', b"first".as_slice()),
            ("a.1.txt", b'0', b"literal".as_slice()),
            ("a.txt", b'0', b"second".as_slice()),
        ]);
        let results = collect(
            TarSplitConfig {
                duplicate_names_policy: DuplicatePolicy::AllowWithIndex,
                ..Default::default()
            },
            data,
        )
        .await;
        let paths: Vec<String> = results
            .iter()
            .map(|r| header_string(r.as_ref().expect("fragment ok"), CAMEL_TAR_ENTRY_PATH))
            .collect();
        assert_eq!(
            paths,
            [
                "a.txt".to_string(),
                "a.1.txt".to_string(),
                "a.2.txt".to_string()
            ],
            "the second a.txt must skip the occupied a.1.txt"
        );

        // Reverse order: a literal name colliding with an already-emitted
        // indexed name is itself re-indexed, keeping every emitted name
        // unique.
        let data = tar_archive(&[
            ("a.txt", b'0', b"first".as_slice()),
            ("a.txt", b'0', b"second".as_slice()),
            ("a.1.txt", b'0', b"literal".as_slice()),
        ]);
        let results = collect(
            TarSplitConfig {
                duplicate_names_policy: DuplicatePolicy::AllowWithIndex,
                ..Default::default()
            },
            data,
        )
        .await;
        let paths: Vec<String> = results
            .iter()
            .map(|r| header_string(r.as_ref().expect("fragment ok"), CAMEL_TAR_ENTRY_PATH))
            .collect();
        assert_eq!(
            paths,
            [
                "a.txt".to_string(),
                "a.1.txt".to_string(),
                "a.1.1.txt".to_string()
            ],
            "the literal a.1.txt must skip the emitted indexed a.1.txt"
        );
    }

    #[tokio::test]
    async fn tar_split_empty_default_is_rejected() {
        let results = collect(TarSplitConfig::default(), tar_archive(&[])).await;
        assert_eq!(results.len(), 1);
        let err = results[0]
            .as_ref()
            .expect_err("empty archive must fail closed by default")
            .to_string();
        assert!(
            err.contains("no regular entries") && err.contains("allow_empty_archive"),
            "expected the fail-closed empty-archive error: {err}"
        );
    }
}
