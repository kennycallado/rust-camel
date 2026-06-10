//! Shared polling logic for the file component.
//!
//! Used by:
//! - `FileConsumer` (event-driven poll loop) — via lib.rs
//! - `FilePollingConsumer` (on-demand pull) — via polling_consumer.rs (added in Task 4)

use std::collections::HashSet;
use std::fs::Metadata;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use bytes::Bytes;
use dashmap::DashMap;
use futures::StreamExt;
use regex::Regex;
use tokio::fs;
use tokio_util::io::ReaderStream;
use tracing::{debug, warn};

use crate::{SortField, SortSpec};

use camel_component_api::{
    Body, CamelError, ConsumerContext, Exchange, Message, StreamBody, StreamMetadata,
};

// ---------------------------------------------------------------------------
// ModificationDetectingStream — detects file changes during stream consumption
// ---------------------------------------------------------------------------

/// Wrapper stream that checks file metadata after the inner stream ends.
/// If the file's size or modification time changed between open and stream end,
/// the wrapper yields an error as its final item.
pub(crate) struct ModificationDetectingStream<S> {
    inner: S,
    file_path: PathBuf,
    initial_size: u64,
    initial_mtime: Option<SystemTime>,
    checked: bool,
}

impl<S> ModificationDetectingStream<S> {
    pub(crate) fn new(
        inner: S,
        file_path: PathBuf,
        initial_size: u64,
        initial_mtime: Option<SystemTime>,
    ) -> Self {
        Self {
            inner,
            file_path,
            initial_size,
            initial_mtime,
            checked: false,
        }
    }
}

impl<S> futures::Stream for ModificationDetectingStream<S>
where
    S: futures::Stream<Item = Result<Bytes, CamelError>> + Unpin,
{
    type Item = Result<Bytes, CamelError>;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        // Poll inner stream
        let inner_poll = std::pin::Pin::new(&mut self.inner).poll_next(cx);

        match inner_poll {
            std::task::Poll::Ready(None) => {
                // Inner stream ended — check metadata once
                if !self.checked {
                    self.checked = true;
                    if let Ok(current_meta) = std::fs::metadata(&self.file_path) {
                        let current_size = current_meta.len();
                        let current_mtime = current_meta.modified().ok();

                        if current_size != self.initial_size || current_mtime != self.initial_mtime
                        {
                            return std::task::Poll::Ready(Some(Err(CamelError::ProcessorError(
                                format!("file modified during read: {}", self.file_path.display()),
                            ))));
                        }
                    }
                }
                std::task::Poll::Ready(None)
            }
            other => other,
        }
    }
}

// ---------------------------------------------------------------------------
// Poll loop — one iteration of directory scanning
// ---------------------------------------------------------------------------

/// The main poll loop type for file locking strategies.
pub(crate) enum FileReadLock {
    None(PathBuf),
    InProcess(PathBuf),
    Rename { original: PathBuf, locked: PathBuf },
}

impl FileReadLock {
    pub(crate) fn active_path(&self) -> &Path {
        match self {
            Self::None(path) | Self::InProcess(path) => path,
            Self::Rename { locked, .. } => locked,
        }
    }
}

/// Build an idempotency key for a file based on the configured strategy.
pub(crate) fn build_idempotent_key(
    strategy: crate::IdempotentKey,
    active_path: &Path,
    file_name: &str,
    metadata: &std::fs::Metadata,
) -> Option<String> {
    match strategy {
        crate::IdempotentKey::None => None,
        crate::IdempotentKey::FileName => Some(file_name.to_string()),
        crate::IdempotentKey::FilePath => Some(active_path.to_string_lossy().to_string()),
        crate::IdempotentKey::FileSize => Some(metadata.len().to_string()),
        crate::IdempotentKey::Digest => metadata
            .modified()
            .ok()
            .and_then(|mtime| mtime.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|ts| format!("{}:{}", active_path.to_string_lossy(), ts.as_nanos())),
    }
}

/// Finalize a locked file: delete, move, or unlock based on config.
pub(crate) async fn finalize_locked_file(
    config: &crate::FileConfig,
    lock: &FileReadLock,
    base_path: &Path,
    file_name: &str,
) {
    match lock {
        FileReadLock::None(path) | FileReadLock::InProcess(path) => {
            if config.noop {
                return;
            }
            if config.delete {
                if let Err(e) = fs::remove_file(path).await {
                    warn!(file = %path.display(), error = %e, "Failed to delete file");
                }
            } else if let Some(ref move_dir) = config.move_to {
                let target_path = base_path.join(move_dir).join(file_name);
                if let Err(e) = fs::rename(path, &target_path).await {
                    warn!(from = %path.display(), to = %target_path.display(), error = %e, "Failed to move file");
                }
            }
        }
        FileReadLock::Rename { original, locked } => {
            if config.noop {
                if let Err(e) = fs::rename(locked, original).await {
                    warn!(from = %locked.display(), to = %original.display(), error = %e, "Failed to unlock file");
                }
            } else if config.delete {
                if let Err(e) = fs::remove_file(locked).await {
                    warn!(file = %locked.display(), error = %e, "Failed to delete file");
                }
            } else if let Some(ref move_dir) = config.move_to {
                let target_path = base_path.join(move_dir).join(file_name);
                if let Err(e) = fs::rename(locked, &target_path).await {
                    warn!(from = %locked.display(), to = %target_path.display(), error = %e, "Failed to move locked file");
                }
            }
        }
    }
}

/// List files in a directory, optionally recursive.
/// Symlinks are skipped to avoid cycles.
#[allow(dead_code)]
pub(crate) async fn list_files(dir: &Path, recursive: bool) -> Result<Vec<PathBuf>, CamelError> {
    async fn list_files_inner(
        dir: &Path,
        recursive: bool,
        visited: &mut HashSet<PathBuf>,
    ) -> Result<Vec<PathBuf>, CamelError> {
        let mut files = Vec::new();
        let canonical_dir = fs::canonicalize(dir).await.map_err(CamelError::from)?;

        if !visited.insert(canonical_dir) {
            return Ok(files);
        }

        let mut read_dir = fs::read_dir(dir).await.map_err(CamelError::from)?;
        while let Some(entry) = read_dir.next_entry().await.map_err(CamelError::from)? {
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path)
                .await
                .map_err(CamelError::from)?;
            let file_type = metadata.file_type();

            if file_type.is_file() {
                files.push(path);
            } else if file_type.is_dir() && recursive {
                let mut sub_files = Box::pin(list_files_inner(&path, true, visited)).await?;
                files.append(&mut sub_files);
            } else if file_type.is_symlink() {
                // Skip symlink entries to avoid symlink traversal and recursion cycles.
                continue;
            }
        }

        Ok(files)
    }

    let mut visited = HashSet::new();
    let mut files = list_files_inner(dir, recursive, &mut visited).await?;

    files.sort();
    Ok(files)
}

pub(crate) struct ScanResult {
    pub candidates: Vec<Candidate>,
    pub hit_limit: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct Candidate {
    pub path: PathBuf,
    pub metadata: Metadata,
    #[allow(dead_code)]
    pub depth: usize,
}

/// Process a single file from the poll directory listing.
///
/// Applies include/exclude filters, acquires a read lock, reads the file
/// content, builds an Exchange, and performs post-read lifecycle
/// (delete/move/finalize) **eagerly** before returning the exchange.
///
/// Returns `Ok(Some(exchange))` if the file was processed successfully.
/// Returns `Ok(None)` if the file was filtered out or skipped.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn poll_one_file(
    config: &crate::FileConfig,
    file_path: PathBuf,
    base_path: &Path,
    include_re: &Option<Regex>,
    exclude_re: &Option<Regex>,
    seen: &mut HashSet<PathBuf>,
    in_process_locks: &Arc<DashMap<PathBuf, ()>>,
    idempotent_repo: &Arc<tokio::sync::Mutex<HashSet<String>>>,
) -> Result<Option<Exchange>, CamelError> {
    let file_name = file_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_string();

    // --- Pre-read filters ---

    if let Some(ref target_name) = config.file_name
        && file_name != *target_name
    {
        return Ok(None);
    }

    if let Some(re) = include_re
        && !re.is_match(&file_name)
    {
        return Ok(None);
    }

    if let Some(re) = exclude_re
        && re.is_match(&file_name)
    {
        return Ok(None);
    }

    if let Some(ref move_dir) = config.move_to
        && file_path.starts_with(base_path.join(move_dir))
    {
        return Ok(None);
    }

    // Idempotent consumer: skip already-seen files when noop=true
    if config.noop && seen.contains(&file_path) {
        return Ok(None);
    }

    // --- Lock acquisition ---

    let lock = match config.read_lock_strategy {
        crate::ReadLockStrategy::None => FileReadLock::None(file_path.clone()),
        crate::ReadLockStrategy::InProcess => {
            if in_process_locks.insert(file_path.clone(), ()).is_some() {
                return Ok(None);
            }
            FileReadLock::InProcess(file_path.clone())
        }
        crate::ReadLockStrategy::Rename => {
            let lock_path = file_path.with_file_name(format!("{file_name}.camel-lock"));
            if fs::rename(&file_path, &lock_path).await.is_err() {
                return Ok(None);
            }
            FileReadLock::Rename {
                original: file_path.clone(),
                locked: lock_path,
            }
        }
    };

    let active_path = lock.active_path();

    // --- Read file with timeout ---

    let (file, metadata) = match tokio::time::timeout(config.read_timeout, async {
        let f = fs::File::open(active_path).await?;
        let m = f.metadata().await?;
        Ok::<_, std::io::Error>((f, m))
    })
    .await
    {
        Ok(Ok((f, m))) => (f, Some(m)),
        Ok(Err(e)) => {
            warn!(
                file = %file_path.display(),
                error = %e,
                "Failed to open file"
            );
            if let FileReadLock::Rename { original, locked } = &lock {
                let _ = fs::rename(locked, original).await;
            }
            if let FileReadLock::InProcess(path) = &lock {
                in_process_locks.remove(path);
            }
            return Ok(None);
        }
        Err(_) => {
            warn!(
                file = %file_path.display(),
                timeout_ms = config.read_timeout.as_millis(),
                "Timeout opening file"
            );
            if let FileReadLock::Rename { original, locked } = &lock {
                let _ = fs::rename(locked, original).await;
            }
            if let FileReadLock::InProcess(path) = &lock {
                in_process_locks.remove(path);
            }
            return Ok(None);
        }
    };

    // --- Idempotency check ---

    if let Some(key) = metadata
        .as_ref()
        .and_then(|m| build_idempotent_key(config.idempotent_key, active_path, &file_name, m))
    {
        let mut repo = idempotent_repo.lock().await;
        if repo.contains(&key) {
            if let FileReadLock::Rename { original, locked } = &lock {
                let _ = fs::rename(locked, original).await;
            }
            if let FileReadLock::InProcess(path) = &lock {
                in_process_locks.remove(path);
            }
            return Ok(None);
        }
        repo.insert(key);
    }

    // --- Build exchange ---

    let file_len = metadata.as_ref().map(|m| m.len()).unwrap_or(0);
    let initial_mtime = metadata.as_ref().and_then(|m| m.modified().ok());

    let raw_stream = ReaderStream::new(file).map(|res| res.map_err(CamelError::from));
    let stream = ModificationDetectingStream::new(
        raw_stream,
        active_path.to_path_buf(),
        file_len,
        initial_mtime,
    );

    let last_modified = initial_mtime
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    let relative_path = file_path
        .strip_prefix(base_path)
        .unwrap_or(&file_path)
        .to_string_lossy()
        .to_string();

    let absolute_path = file_path
        .canonicalize()
        .unwrap_or_else(|_| file_path.clone())
        .to_string_lossy()
        .to_string();

    let camel_file_canonical_path = match file_path.canonicalize() {
        Ok(p) => p.to_string_lossy().to_string(),
        Err(err) => {
            tracing::warn!("failed to canonicalize {}: {err}", file_path.display());
            absolute_path.clone()
        }
    };

    let body = Body::Stream(StreamBody {
        stream: std::sync::Arc::new(tokio::sync::Mutex::new(Some(Box::pin(stream)))),
        metadata: StreamMetadata {
            size_hint: Some(file_len),
            content_type: None,
            origin: Some(absolute_path.clone()),
        },
    });

    let mut exchange = Exchange::new(Message::new(body));
    exchange.input.set_header(
        "CamelFileName",
        serde_json::Value::String(relative_path.clone()),
    );
    exchange.input.set_header(
        "CamelFileNameOnly",
        serde_json::Value::String(file_name.clone()),
    );
    exchange.input.set_header(
        "CamelFileAbsolutePath",
        serde_json::Value::String(file_path.to_string_lossy().to_string()),
    );
    exchange.input.set_header(
        "CamelFileLength",
        serde_json::Value::Number(file_len.into()),
    );
    exchange.input.set_header(
        "CamelFileLastModified",
        serde_json::Value::Number(last_modified.into()),
    );
    exchange.input.set_header(
        "CamelFilePath",
        serde_json::Value::String(base_path.to_string_lossy().to_string()),
    );
    exchange.input.set_header(
        "CamelFileParent",
        serde_json::Value::String(
            file_path
                .parent()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default(),
        ),
    );
    exchange.input.set_header(
        "CamelFileCanonicalPath",
        serde_json::Value::String(camel_file_canonical_path),
    );
    exchange.input.set_header(
        "CamelFileRelativePath",
        serde_json::Value::String(relative_path.clone()),
    );

    debug!(
        file = %file_path.display(),
        correlation_id = %exchange.correlation_id(),
        "Processing file"
    );

    // --- Eager lifecycle (no context.send — caller handles that) ---

    if config.noop {
        seen.insert(file_path.clone());
    }

    if !config.noop
        && !config.delete
        && let Some(ref move_dir) = config.move_to
    {
        if Path::new(move_dir).is_absolute() {
            warn!(move_to = %move_dir, "Ignoring absolute move_to path outside base directory");
            if let FileReadLock::Rename { original, locked } = &lock {
                let _ = fs::rename(locked, original).await;
            }
            if let FileReadLock::InProcess(path) = &lock {
                in_process_locks.remove(path);
            }
            return Ok(None);
        }

        let canonical_base = match fs::canonicalize(base_path).await {
            Ok(path) => path,
            Err(e) => {
                warn!(base = %base_path.display(), error = %e, "Failed to canonicalize base path for move_to validation");
                if let FileReadLock::Rename { original, locked } = &lock {
                    let _ = fs::rename(locked, original).await;
                }
                if let FileReadLock::InProcess(path) = &lock {
                    in_process_locks.remove(path);
                }
                return Ok(None);
            }
        };

        let target_dir = base_path.join(move_dir);
        let resolved_target_dir = if target_dir.exists() {
            match fs::canonicalize(&target_dir).await {
                Ok(path) => path,
                Err(e) => {
                    warn!(dir = %target_dir.display(), error = %e, "Failed to canonicalize move target directory");
                    if let FileReadLock::Rename { original, locked } = &lock {
                        let _ = fs::rename(locked, original).await;
                    }
                    if let FileReadLock::InProcess(path) = &lock {
                        in_process_locks.remove(path);
                    }
                    return Ok(None);
                }
            }
        } else {
            canonical_base.join(move_dir)
        };

        if !resolved_target_dir.starts_with(&canonical_base) {
            warn!(dir = %target_dir.display(), base = %canonical_base.display(), "Skipping move_to outside base directory");
            if let FileReadLock::Rename { original, locked } = &lock {
                let _ = fs::rename(locked, original).await;
            }
            if let FileReadLock::InProcess(path) = &lock {
                in_process_locks.remove(path);
            }
            return Ok(None);
        }

        if let Err(e) = fs::create_dir_all(&target_dir).await {
            warn!(dir = %target_dir.display(), error = %e, "Failed to create move directory");
            if let FileReadLock::Rename { original, locked } = &lock {
                let _ = fs::rename(locked, original).await;
            }
            if let FileReadLock::InProcess(path) = &lock {
                in_process_locks.remove(path);
            }
            return Ok(None);
        }
    }

    finalize_locked_file(config, &lock, base_path, &file_name).await;

    if !config.noop
        && let Some(ref pattern) = config.done_file_name
        && pattern.contains("${")
    {
        let done_name = resolve_done_file_name(pattern, &file_name);
        let done_path = file_path.with_file_name(&done_name);
        let _ = fs::remove_file(&done_path).await;
    }

    if let FileReadLock::InProcess(path) = &lock {
        in_process_locks.remove(path);
    }

    Ok(Some(exchange))
}

/// Lists files in the configured directory and processes each one through
/// `poll_one_file`. Called repeatedly by `FileConsumer`'s event loop.
pub(crate) async fn poll_directory(
    config: &crate::FileConfig,
    context: &ConsumerContext,
    filters: &crate::CompiledFilters,
    seen: &mut HashSet<PathBuf>,
    in_process_locks: &Arc<DashMap<PathBuf, ()>>,
    idempotent_repo: &Arc<tokio::sync::Mutex<HashSet<String>>>,
) -> Result<(), CamelError> {
    let base_path = Path::new(&config.directory);
    let scan_result = scan_candidates(config, filters, base_path, seen).await?;

    let was_limited = scan_result.hit_limit
        || (!config.eager_max_messages_per_poll
            && config.max_messages_per_poll > 0
            && scan_result.candidates.len() as i64 > config.max_messages_per_poll);

    let mut candidates = scan_result.candidates;
    apply_sort_and_limit(&mut candidates, config);

    let candidate_count = candidates.len();
    let mut processed_ok = 0usize;
    let mut processed_paths: Vec<PathBuf> = Vec::new();

    for candidate in candidates {
        let candidate_path = candidate.path.clone();
        if let Some(exchange) = poll_one_file(
            config,
            candidate.path,
            base_path,
            &filters.include_re,
            &filters.exclude_re,
            seen,
            in_process_locks,
            idempotent_repo,
        )
        .await?
        {
            if context.send(exchange).await.is_err() {
                return Err(CamelError::ChannelClosed);
            }
            processed_ok += 1;
            processed_paths.push(candidate_path);
        }
    }

    if !config.noop
        && let Some(ref pattern) = config.done_file_name
        && !pattern.contains("${")
        && !was_limited
        && processed_ok == candidate_count
        && candidate_count > 0
    {
        for processed_path in &processed_paths {
            let done_path = processed_path.parent().unwrap_or(base_path).join(pattern);
            let _ = tokio::fs::remove_file(&done_path).await;
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Filter helpers
// ---------------------------------------------------------------------------

fn file_extension(filename: &str) -> Option<&str> {
    let dot_pos = filename.find('.')?;
    Some(&filename[dot_pos + 1..])
}

fn resolve_done_file_name(pattern: &str, file_name: &str) -> String {
    let noext = file_name
        .rsplit_once('.')
        .map(|(stem, _)| stem)
        .unwrap_or(file_name);
    pattern
        .replace("${file:name}", file_name)
        .replace("${file:name.noext}", noext)
}

async fn done_file_exists(file_path: &Path, pattern: &str) -> bool {
    let file_name = file_path
        .file_name()
        .unwrap_or_default()
        .to_str()
        .unwrap_or("");
    let done_name = resolve_done_file_name(pattern, file_name);
    let done_path = file_path.with_file_name(done_name);
    match tokio::fs::try_exists(&done_path).await {
        Ok(true) => true,
        Ok(false) => false,
        Err(err) => {
            tracing::warn!("failed to check done file {}: {err}", done_path.display());
            false
        }
    }
}

fn is_done_marker(file_name: &str, pattern: &str) -> bool {
    if !pattern.contains("${") {
        return file_name == pattern;
    }
    let resolved = resolve_done_file_name(pattern, "");
    if pattern.starts_with("${") {
        file_name.ends_with(&resolved)
    } else {
        file_name.starts_with(&resolved)
    }
}

async fn pass_all_filters(
    path: &Path,
    file_name: &str,
    base_path: &Path,
    config: &crate::FileConfig,
    filters: &crate::CompiledFilters,
) -> bool {
    if let Some(ref target) = config.file_name
        && file_name != target
    {
        return false;
    }

    if let Some(ref re) = filters.include_re
        && !re.is_match(file_name)
    {
        return false;
    }

    if let Some(ref re) = filters.exclude_re
        && re.is_match(file_name)
    {
        return false;
    }

    if let Some(ref patterns) = filters.ant_include_patterns {
        let rel = path
            .strip_prefix(base_path)
            .unwrap_or(path)
            .to_string_lossy();
        if !patterns.iter().any(|p| p.matches(&rel)) {
            return false;
        }
    }

    if let Some(ref patterns) = filters.ant_exclude_patterns {
        let rel = path
            .strip_prefix(base_path)
            .unwrap_or(path)
            .to_string_lossy();
        if patterns.iter().any(|p| p.matches(&rel)) {
            return false;
        }
    }

    if let Some(ref exts) = filters.include_exts {
        let ext = file_extension(file_name);
        match ext {
            Some(e) if exts.iter().any(|x| x.eq_ignore_ascii_case(e)) => {}
            _ => return false,
        }
    }

    if let Some(ref exts) = filters.exclude_exts {
        let ext = file_extension(file_name);
        if let Some(e) = ext
            && exts.iter().any(|x| x.eq_ignore_ascii_case(e))
        {
            return false;
        }
    }

    if let Some(ref move_dir) = config.move_to
        && path.starts_with(base_path.join(move_dir))
    {
        return false;
    }

    if let Some(ref pattern) = config.done_file_name {
        if !done_file_exists(path, pattern).await {
            return false;
        }
        if is_done_marker(file_name, pattern) {
            return false;
        }
    }

    true
}

// ---------------------------------------------------------------------------
// scan_candidates — integrated eager-limit walk with all filters
// ---------------------------------------------------------------------------

pub(crate) async fn scan_candidates(
    config: &crate::FileConfig,
    filters: &crate::CompiledFilters,
    base_path: &Path,
    seen: &mut HashSet<PathBuf>,
) -> Result<ScanResult, CamelError> {
    let eager_max = if config.eager_max_messages_per_poll && config.max_messages_per_poll > 0 {
        Some(config.max_messages_per_poll as usize)
    } else {
        None
    };

    let mut candidates: Vec<Candidate> = Vec::new();

    #[allow(clippy::too_many_arguments)]
    async fn walk(
        dir: &Path,
        base_path: &Path,
        config: &crate::FileConfig,
        filters: &crate::CompiledFilters,
        depth: usize,
        visited: &mut HashSet<PathBuf>,
        candidates: &mut Vec<Candidate>,
        eager_max: Option<usize>,
        seen: &mut HashSet<PathBuf>,
    ) -> Result<bool, CamelError> {
        let canonical_dir = fs::canonicalize(dir).await.map_err(CamelError::from)?;
        if !visited.insert(canonical_dir.clone()) {
            return Ok(false);
        }
        if depth > config.max_depth {
            return Ok(false);
        }

        let mut read_dir = fs::read_dir(dir).await.map_err(CamelError::from)?;
        while let Some(entry) = read_dir.next_entry().await.map_err(CamelError::from)? {
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path)
                .await
                .map_err(CamelError::from)?;
            let file_type = metadata.file_type();

            if file_type.is_file() {
                if eager_max.is_some_and(|m| candidates.len() >= m) {
                    return Ok(true);
                }

                let file_name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or_default();

                if depth < config.min_depth {
                    continue;
                }

                if !pass_all_filters(&path, file_name, base_path, config, filters).await {
                    continue;
                }

                if config.noop && seen.contains(&path) {
                    continue;
                }

                candidates.push(Candidate {
                    path,
                    metadata,
                    depth,
                });
            } else if file_type.is_dir() && config.recursive {
                let hit_limit = Box::pin(walk(
                    &path,
                    base_path,
                    config,
                    filters,
                    depth + 1,
                    visited,
                    candidates,
                    eager_max,
                    seen,
                ))
                .await?;
                if hit_limit {
                    return Ok(true);
                }
            }
        }

        Ok(false)
    }

    let mut visited = HashSet::new();
    let hit_limit = walk(
        base_path,
        base_path,
        config,
        filters,
        1,
        &mut visited,
        &mut candidates,
        eager_max,
        seen,
    )
    .await?;

    candidates.sort_by(|a, b| a.path.cmp(&b.path));

    Ok(ScanResult {
        candidates,
        hit_limit,
    })
}

// ---------------------------------------------------------------------------
// sort_candidates — apply SortSpec ordering to Candidate list
// ---------------------------------------------------------------------------

fn sort_candidates(candidates: &mut [Candidate], spec: &SortSpec) {
    candidates.sort_by(|a, b| {
        for group in &spec.groups {
            let ord = match group.field {
                SortField::Name => {
                    let na = a.path.file_name().unwrap_or_default().to_string_lossy();
                    let nb = b.path.file_name().unwrap_or_default().to_string_lossy();
                    if group.ignore_case {
                        na.to_lowercase().cmp(&nb.to_lowercase())
                    } else {
                        na.cmp(&nb)
                    }
                }
                SortField::Length => {
                    let la = a.metadata.len();
                    let lb = b.metadata.len();
                    la.cmp(&lb)
                }
                SortField::Modified => {
                    let ta = a
                        .metadata
                        .modified()
                        .ok()
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| d.as_millis())
                        .unwrap_or(0);
                    let tb = b
                        .metadata
                        .modified()
                        .ok()
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| d.as_millis())
                        .unwrap_or(0);
                    ta.cmp(&tb)
                }
            };
            let final_ord = if group.reverse { ord.reverse() } else { ord };
            if final_ord != std::cmp::Ordering::Equal {
                return final_ord;
            }
        }
        std::cmp::Ordering::Equal
    });
}

// ---------------------------------------------------------------------------
// apply_sort_and_limit — sort, shuffle, and optionally limit candidates
// ---------------------------------------------------------------------------

pub(crate) fn apply_sort_and_limit(candidates: &mut Vec<Candidate>, config: &crate::FileConfig) {
    if let Some(ref spec) = config.sort_spec {
        sort_candidates(candidates, spec);
    }

    if config.shuffle {
        use rand::SeedableRng;
        use rand::seq::SliceRandom;
        let mut rng = rand::rngs::StdRng::seed_from_u64(42);
        candidates.shuffle(&mut rng);
    }

    if !config.eager_max_messages_per_poll && config.max_messages_per_poll > 0 {
        candidates.truncate(config.max_messages_per_poll as usize);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_file_extension_simple() {
        assert_eq!(file_extension("file.txt"), Some("txt"));
    }

    #[test]
    fn test_file_extension_multi_dot() {
        assert_eq!(file_extension("archive.tar.gz"), Some("tar.gz"));
    }

    #[test]
    fn test_file_extension_data_tar_gz() {
        assert_eq!(file_extension("data.tar.gz"), Some("tar.gz"));
    }

    #[test]
    fn test_file_extension_no_dot() {
        assert_eq!(file_extension("noext"), None);
    }

    #[test]
    fn test_file_extension_hidden() {
        assert_eq!(file_extension(".gitignore"), Some("gitignore"));
    }

    #[test]
    fn test_is_done_marker_template() {
        assert!(is_done_marker("ready.txt.done", "${file:name}.done"));
    }

    #[test]
    fn test_is_done_marker_static_match() {
        assert!(is_done_marker("done", "done"));
    }

    #[test]
    fn test_is_done_marker_static_no_match() {
        assert!(!is_done_marker("other.txt", "done"));
    }

    #[test]
    fn test_is_done_marker_prefix_pattern() {
        assert!(is_done_marker("ready-data.txt", "ready-${file:name}"));
        assert!(!is_done_marker("data.txt", "ready-${file:name}"));
    }
}
