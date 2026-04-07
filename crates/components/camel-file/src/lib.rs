use std::collections::HashSet;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::str::FromStr;
use std::task::{Context, Poll};
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use regex::Regex;
use tokio::fs;
use tokio::fs::OpenOptions;
use tokio::io;
use tokio::io::AsyncWriteExt;
use tokio::time;
use tokio_util::io::ReaderStream;
use tower::Service;
use tracing::{debug, warn};

use camel_component_api::{
    Body, BoxProcessor, CamelError, Exchange, Message, StreamBody, StreamMetadata,
};
use camel_component_api::{Component, Consumer, ConsumerContext, Endpoint, ProducerContext};
use camel_component_api::{UriConfig, parse_uri};

// ---------------------------------------------------------------------------
// TempFileGuard — RAII cleanup for temp files (panic-safe)
// ---------------------------------------------------------------------------

/// RAII guard that ensures temp file cleanup even on panic.
///
/// When dropped, removes the file at `path` unless `disarm` is set to true.
/// This protects against temp file leaks if `io::copy` panics mid-write.
struct TempFileGuard {
    path: PathBuf,
    disarm: bool,
}

impl TempFileGuard {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            disarm: false,
        }
    }

    /// Call after successful rename to prevent cleanup.
    fn disarm(&mut self) {
        self.disarm = true;
    }
}

impl Drop for TempFileGuard {
    fn drop(&mut self) {
        if !self.disarm {
            // Best-effort cleanup; ignore errors (file may not exist)
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

// ---------------------------------------------------------------------------
// FileExistStrategy
// ---------------------------------------------------------------------------

/// Strategy for handling existing files when writing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FileExistStrategy {
    /// Overwrite existing file (default).
    #[default]
    Override,
    /// Append to existing file.
    Append,
    /// Fail if file exists.
    Fail,
}

impl FromStr for FileExistStrategy {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Override" | "override" => Ok(FileExistStrategy::Override),
            "Append" | "append" => Ok(FileExistStrategy::Append),
            "Fail" | "fail" => Ok(FileExistStrategy::Fail),
            _ => Ok(FileExistStrategy::Override), // Default for unknown values
        }
    }
}

// ---------------------------------------------------------------------------
// FileGlobalConfig
// ---------------------------------------------------------------------------

/// Global configuration for File component.
/// Plain Rust, no serde, with Default impl and builder methods.
/// These are the fallback defaults when URI params are not set.
#[derive(Debug, Clone, PartialEq)]
pub struct FileGlobalConfig {
    pub delay_ms: u64,
    pub initial_delay_ms: u64,
    pub read_timeout_ms: u64,
    pub write_timeout_ms: u64,
}

impl Default for FileGlobalConfig {
    fn default() -> Self {
        Self {
            delay_ms: 500,
            initial_delay_ms: 1_000,
            read_timeout_ms: 30_000,
            write_timeout_ms: 30_000,
        }
    }
}

impl FileGlobalConfig {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn with_delay_ms(mut self, v: u64) -> Self {
        self.delay_ms = v;
        self
    }
    pub fn with_initial_delay_ms(mut self, v: u64) -> Self {
        self.initial_delay_ms = v;
        self
    }
    pub fn with_read_timeout_ms(mut self, v: u64) -> Self {
        self.read_timeout_ms = v;
        self
    }
    pub fn with_write_timeout_ms(mut self, v: u64) -> Self {
        self.write_timeout_ms = v;
        self
    }
}

// ---------------------------------------------------------------------------
// FileConfig
// ---------------------------------------------------------------------------

/// Configuration for file component endpoints.
///
/// # Streaming
///
/// Both the file consumer and producer use **native streaming** with no RAM
/// materialization:
///
/// - The **consumer** creates a `Body::Stream` backed by `tokio::fs::File` via
///   `ReaderStream`. Files of any size are handled without loading them into memory.
///
/// - The **producer** writes via `tokio::io::copy` directly to a `tokio::fs::File`
///   using `Body::into_async_read()`. Writes for the `Override` strategy are
///   **atomic**: data is written to a temporary file first and renamed only on
///   success, preventing partial files on failure.
///
/// # Write strategies (`fileExist` URI parameter)
///
/// | Value | Behavior |
/// |-------|----------|
/// | `Override` (default) | Atomic write via temp file + rename |
/// | `Append` | Appends to existing file; non-atomic by nature |
/// | `Fail` | Returns error if file already exists |
#[derive(Debug, Clone, UriConfig)]
#[uri_scheme = "file"]
#[uri_config(skip_impl, crate = "camel_component_api")]
pub struct FileConfig {
    /// Directory path to read from or write to.
    pub directory: String,

    /// Polling delay in milliseconds (companion field for `delay`).
    #[allow(dead_code)]
    #[uri_param(name = "delay", default = "500")]
    delay_ms: u64,

    /// Polling delay as Duration.
    pub delay: Duration,

    /// Initial delay in milliseconds (companion field for `initial_delay`).
    #[allow(dead_code)]
    #[uri_param(name = "initialDelay", default = "1000")]
    initial_delay_ms: u64,

    /// Initial delay as Duration.
    pub initial_delay: Duration,

    /// If true, don't delete or move files after processing.
    #[uri_param(default = "false")]
    pub noop: bool,

    /// If true, delete files after processing.
    #[uri_param(default = "false")]
    pub delete: bool,

    /// Directory to move processed files to (only if not noop/delete).
    /// Default is ".camel" when not specified and noop/delete are false.
    #[uri_param(name = "move")]
    move_to: Option<String>,

    /// Fixed filename for producer (optional).
    #[uri_param(name = "fileName")]
    pub file_name: Option<String>,

    /// Regex pattern for including files (consumer).
    #[uri_param]
    pub include: Option<String>,

    /// Regex pattern for excluding files (consumer).
    #[uri_param]
    pub exclude: Option<String>,

    /// Whether to scan directories recursively.
    #[uri_param(default = "false")]
    pub recursive: bool,

    /// Strategy for handling existing files when writing.
    #[uri_param(name = "fileExist", default = "Override")]
    pub file_exist: FileExistStrategy,

    /// Prefix for temporary files during atomic writes.
    #[uri_param(name = "tempPrefix")]
    pub temp_prefix: Option<String>,

    /// Whether to automatically create directories.
    #[uri_param(name = "autoCreate", default = "true")]
    pub auto_create: bool,

    /// Read timeout in milliseconds (companion field for `read_timeout`).
    #[allow(dead_code)]
    #[uri_param(name = "readTimeout", default = "30000")]
    read_timeout_ms: u64,

    /// Read timeout as Duration.
    pub read_timeout: Duration,

    /// Write timeout in milliseconds (companion field for `write_timeout`).
    #[allow(dead_code)]
    #[uri_param(name = "writeTimeout", default = "30000")]
    write_timeout_ms: u64,

    /// Write timeout as Duration.
    pub write_timeout: Duration,
}

impl UriConfig for FileConfig {
    fn scheme() -> &'static str {
        "file"
    }

    fn from_uri(uri: &str) -> Result<Self, CamelError> {
        let parts = parse_uri(uri)?;
        Self::from_components(parts)
    }

    fn from_components(parts: camel_component_api::UriComponents) -> Result<Self, CamelError> {
        Self::parse_uri_components(parts)?.validate()
    }

    fn validate(self) -> Result<Self, CamelError> {
        // Apply conditional logic for move_to:
        // - If noop or delete is true, move_to should be None
        // - Otherwise, if move_to is None, default to ".camel"
        let move_to = if self.noop || self.delete {
            None
        } else {
            Some(self.move_to.unwrap_or_else(|| ".camel".to_string()))
        };

        Ok(Self { move_to, ..self })
    }
}

impl FileConfig {
    /// Apply global config defaults. Since FileConfig uses a proc macro that bakes in
    /// defaults, we compare Duration values against the known macro defaults to detect
    /// "not explicitly set by user". Only overrides when current value == macro default.
    ///
    /// **Note**: If a user explicitly sets a URI param to its default value (e.g.,
    /// `?delay=500`), it is indistinguishable from "not set" and will be overridden
    /// by global config. This is a known limitation of the Duration comparison approach.
    pub fn apply_global_defaults(&mut self, global: &FileGlobalConfig) {
        if self.delay == Duration::from_millis(500) {
            self.delay = Duration::from_millis(global.delay_ms);
        }
        if self.initial_delay == Duration::from_millis(1_000) {
            self.initial_delay = Duration::from_millis(global.initial_delay_ms);
        }
        if self.read_timeout == Duration::from_millis(30_000) {
            self.read_timeout = Duration::from_millis(global.read_timeout_ms);
        }
        if self.write_timeout == Duration::from_millis(30_000) {
            self.write_timeout = Duration::from_millis(global.write_timeout_ms);
        }
    }
}

// ---------------------------------------------------------------------------
// FileComponent
// ---------------------------------------------------------------------------

pub struct FileComponent {
    config: Option<FileGlobalConfig>,
}

impl FileComponent {
    pub fn new() -> Self {
        Self { config: None }
    }

    pub fn with_config(config: FileGlobalConfig) -> Self {
        Self {
            config: Some(config),
        }
    }

    pub fn with_optional_config(config: Option<FileGlobalConfig>) -> Self {
        Self { config }
    }
}

impl Default for FileComponent {
    fn default() -> Self {
        Self::new()
    }
}

impl Component for FileComponent {
    fn scheme(&self) -> &str {
        "file"
    }

    fn create_endpoint(&self, uri: &str) -> Result<Box<dyn Endpoint>, CamelError> {
        let mut config = FileConfig::from_uri(uri)?;
        if let Some(ref global_config) = self.config {
            config.apply_global_defaults(global_config);
        }
        Ok(Box::new(FileEndpoint {
            uri: uri.to_string(),
            config,
        }))
    }
}

// ---------------------------------------------------------------------------
// FileEndpoint
// ---------------------------------------------------------------------------

struct FileEndpoint {
    uri: String,
    config: FileConfig,
}

impl Endpoint for FileEndpoint {
    fn uri(&self) -> &str {
        &self.uri
    }

    fn create_consumer(&self) -> Result<Box<dyn Consumer>, CamelError> {
        Ok(Box::new(FileConsumer {
            config: self.config.clone(),
            seen: HashSet::new(),
        }))
    }

    fn create_producer(&self, _ctx: &ProducerContext) -> Result<BoxProcessor, CamelError> {
        Ok(BoxProcessor::new(FileProducer {
            config: self.config.clone(),
        }))
    }
}

// ---------------------------------------------------------------------------
// FileConsumer
// ---------------------------------------------------------------------------

struct FileConsumer {
    config: FileConfig,
    seen: HashSet<PathBuf>,
}

#[async_trait]
impl Consumer for FileConsumer {
    async fn start(&mut self, context: ConsumerContext) -> Result<(), CamelError> {
        let config = self.config.clone();

        let include_re = config
            .include
            .as_ref()
            .map(|p| Regex::new(p))
            .transpose()
            .map_err(|e| CamelError::InvalidUri(format!("invalid include regex: {e}")))?;
        let exclude_re = config
            .exclude
            .as_ref()
            .map(|p| Regex::new(p))
            .transpose()
            .map_err(|e| CamelError::InvalidUri(format!("invalid exclude regex: {e}")))?;

        if !config.initial_delay.is_zero() {
            tokio::select! {
                _ = time::sleep(config.initial_delay) => {}
                _ = context.cancelled() => {
                    debug!(directory = config.directory, "File consumer cancelled during initial delay");
                    return Ok(());
                }
            }
        }

        let mut interval = time::interval(config.delay);

        loop {
            tokio::select! {
                _ = context.cancelled() => {
                    debug!(directory = config.directory, "File consumer received cancellation, stopping");
                    break;
                }
                _ = interval.tick() => {
                    if let Err(e) = poll_directory(
                        &config,
                        &context,
                        &include_re,
                        &exclude_re,
                        &mut self.seen,
                    ).await {
                        warn!(directory = config.directory, error = %e, "Error polling directory");
                    }
                }
            }
        }

        Ok(())
    }

    async fn stop(&mut self) -> Result<(), CamelError> {
        Ok(())
    }
}

async fn poll_directory(
    config: &FileConfig,
    context: &ConsumerContext,
    include_re: &Option<Regex>,
    exclude_re: &Option<Regex>,
    seen: &mut HashSet<PathBuf>,
) -> Result<(), CamelError> {
    let base_path = std::path::Path::new(&config.directory);

    let files = list_files(base_path, config.recursive).await?;

    for file_path in files {
        let file_name = file_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();

        if let Some(ref target_name) = config.file_name
            && file_name != *target_name
        {
            continue;
        }

        if let Some(re) = include_re
            && !re.is_match(&file_name)
        {
            continue;
        }

        if let Some(re) = exclude_re
            && re.is_match(&file_name)
        {
            continue;
        }

        if let Some(ref move_dir) = config.move_to
            && file_path.starts_with(base_path.join(move_dir))
        {
            continue;
        }

        // Idempotent consumer: skip already-seen files when noop=true
        if config.noop && seen.contains(&file_path) {
            continue;
        }

        let (file, metadata) = match tokio::time::timeout(config.read_timeout, async {
            let f = fs::File::open(&file_path).await?;
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
                continue;
            }
            Err(_) => {
                warn!(
                    file = %file_path.display(),
                    timeout_ms = config.read_timeout.as_millis(),
                    "Timeout opening file"
                );
                continue;
            }
        };

        let file_len = metadata.as_ref().map(|m| m.len()).unwrap_or(0);
        let stream = ReaderStream::new(file).map(|res| res.map_err(CamelError::from));

        let last_modified = metadata
            .as_ref()
            .and_then(|m| m.modified().ok())
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

        let body = Body::Stream(StreamBody {
            stream: std::sync::Arc::new(tokio::sync::Mutex::new(Some(Box::pin(stream)))),
            metadata: StreamMetadata {
                size_hint: Some(file_len),
                content_type: None,
                origin: Some(absolute_path.clone()),
            },
        });

        let mut exchange = Exchange::new(Message::new(body));
        exchange
            .input
            .set_header("CamelFileName", serde_json::Value::String(relative_path));
        exchange.input.set_header(
            "CamelFileNameOnly",
            serde_json::Value::String(file_name.clone()),
        );
        exchange.input.set_header(
            "CamelFileAbsolutePath",
            serde_json::Value::String(absolute_path),
        );
        exchange.input.set_header(
            "CamelFileLength",
            serde_json::Value::Number(file_len.into()),
        );
        exchange.input.set_header(
            "CamelFileLastModified",
            serde_json::Value::Number(last_modified.into()),
        );

        debug!(
            file = %file_path.display(),
            correlation_id = %exchange.correlation_id(),
            "Processing file"
        );

        if context.send(exchange).await.is_err() {
            break;
        }

        if config.noop {
            seen.insert(file_path.clone());
        }

        if config.noop {
            // Do nothing
        } else if config.delete {
            if let Err(e) = fs::remove_file(&file_path).await {
                warn!(file = %file_path.display(), error = %e, "Failed to delete file");
            }
        } else if let Some(ref move_dir) = config.move_to {
            let target_dir = base_path.join(move_dir);
            if let Err(e) = fs::create_dir_all(&target_dir).await {
                warn!(dir = %target_dir.display(), error = %e, "Failed to create move directory");
                continue;
            }
            let target_path = target_dir.join(&file_name);
            if let Err(e) = fs::rename(&file_path, &target_path).await {
                warn!(
                    from = %file_path.display(),
                    to = %target_path.display(),
                    error = %e,
                    "Failed to move file"
                );
            }
        }
    }

    Ok(())
}

async fn list_files(
    dir: &std::path::Path,
    recursive: bool,
) -> Result<Vec<std::path::PathBuf>, CamelError> {
    let mut files = Vec::new();
    let mut read_dir = fs::read_dir(dir).await.map_err(CamelError::from)?;

    while let Some(entry) = read_dir.next_entry().await.map_err(CamelError::from)? {
        let path = entry.path();
        if path.is_file() {
            files.push(path);
        } else if path.is_dir() && recursive {
            let mut sub_files = Box::pin(list_files(&path, true)).await?;
            files.append(&mut sub_files);
        }
    }

    files.sort();
    Ok(files)
}

// ---------------------------------------------------------------------------
// Path validation for security
// ---------------------------------------------------------------------------

fn validate_path_is_within_base(
    base_dir: &std::path::Path,
    target_path: &std::path::Path,
) -> Result<(), CamelError> {
    let canonical_base = base_dir.canonicalize().map_err(|e| {
        CamelError::ProcessorError(format!("Cannot canonicalize base directory: {}", e))
    })?;

    // For non-existent paths, canonicalize the parent and construct the full path
    let canonical_target = if target_path.exists() {
        target_path.canonicalize().map_err(|e| {
            CamelError::ProcessorError(format!("Cannot canonicalize target path: {}", e))
        })?
    } else if let Some(parent) = target_path.parent() {
        // Ensure parent exists (should have been created by auto_create)
        if !parent.exists() {
            return Err(CamelError::ProcessorError(format!(
                "Parent directory '{}' does not exist",
                parent.display()
            )));
        }
        let canonical_parent = parent.canonicalize().map_err(|e| {
            CamelError::ProcessorError(format!("Cannot canonicalize parent directory: {}", e))
        })?;
        // Reconstruct the full path with the filename
        if let Some(filename) = target_path.file_name() {
            canonical_parent.join(filename)
        } else {
            return Err(CamelError::ProcessorError(
                "Invalid target path: no filename".to_string(),
            ));
        }
    } else {
        return Err(CamelError::ProcessorError(
            "Invalid target path: no parent directory".to_string(),
        ));
    };

    if !canonical_target.starts_with(&canonical_base) {
        return Err(CamelError::ProcessorError(format!(
            "Path '{}' is outside base directory '{}'",
            canonical_target.display(),
            canonical_base.display()
        )));
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// FileProducer
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct FileProducer {
    config: FileConfig,
}

impl FileProducer {
    fn resolve_filename(exchange: &Exchange, config: &FileConfig) -> Result<String, CamelError> {
        if let Some(name) = exchange
            .input
            .header("CamelFileName")
            .and_then(|v| v.as_str())
        {
            return Ok(name.to_string());
        }
        if let Some(ref name) = config.file_name {
            return Ok(name.clone());
        }
        Err(CamelError::ProcessorError(
            "No filename specified: set CamelFileName header or fileName option".to_string(),
        ))
    }
}

impl Service<Exchange> for FileProducer {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, mut exchange: Exchange) -> Self::Future {
        let config = self.config.clone();

        Box::pin(async move {
            let file_name = FileProducer::resolve_filename(&exchange, &config)?;
            let body = exchange.input.body.clone();

            let dir_path = std::path::Path::new(&config.directory);
            let target_path = dir_path.join(&file_name);

            // 1. Auto-create directories
            if config.auto_create
                && let Some(parent) = target_path.parent()
            {
                tokio::time::timeout(config.write_timeout, fs::create_dir_all(parent))
                    .await
                    .map_err(|_| CamelError::ProcessorError("Timeout creating directories".into()))?
                    .map_err(CamelError::from)?;
            }

            // 2. Security: validate path is within base directory
            validate_path_is_within_base(dir_path, &target_path)?;

            // 3. Handle file-exist strategy
            match config.file_exist {
                FileExistStrategy::Fail if target_path.exists() => {
                    return Err(CamelError::ProcessorError(format!(
                        "File already exists: {}",
                        target_path.display()
                    )));
                }
                FileExistStrategy::Append => {
                    // Append: write directly without temp file (append is inherently non-atomic)
                    let mut file = tokio::time::timeout(
                        config.write_timeout,
                        OpenOptions::new()
                            .append(true)
                            .create(true)
                            .open(&target_path),
                    )
                    .await
                    .map_err(|_| {
                        CamelError::ProcessorError("Timeout opening file for append".into())
                    })?
                    .map_err(CamelError::from)?;

                    tokio::time::timeout(
                        config.write_timeout,
                        io::copy(&mut body.into_async_read(), &mut file),
                    )
                    .await
                    .map_err(|_| CamelError::ProcessorError("Timeout writing to file".into()))?
                    .map_err(|e| CamelError::ProcessorError(e.to_string()))?;

                    file.flush().await.map_err(CamelError::from)?;
                }
                _ => {
                    // Override (or Fail when file doesn't exist): always atomic via temp file
                    let temp_name = if let Some(ref prefix) = config.temp_prefix {
                        format!("{prefix}{file_name}")
                    } else {
                        format!(".tmp.{file_name}")
                    };
                    let temp_path = dir_path.join(&temp_name);

                    // RAII guard ensures cleanup even on panic
                    let mut guard = TempFileGuard::new(temp_path.clone());

                    // Write to temp file
                    let mut file =
                        tokio::time::timeout(config.write_timeout, fs::File::create(&temp_path))
                            .await
                            .map_err(|_| {
                                CamelError::ProcessorError("Timeout creating temp file".into())
                            })?
                            .map_err(CamelError::from)?;

                    let copy_result = tokio::time::timeout(
                        config.write_timeout,
                        io::copy(&mut body.into_async_read(), &mut file),
                    )
                    .await;

                    // Flush any kernel buffers (best-effort; actual write errors come from io::copy above)
                    let _ = file.flush().await;

                    match copy_result {
                        Ok(Ok(_)) => {}
                        Ok(Err(e)) => {
                            // Guard will clean up temp file on drop
                            return Err(CamelError::ProcessorError(e.to_string()));
                        }
                        Err(_) => {
                            // Guard will clean up temp file on drop
                            return Err(CamelError::ProcessorError("Timeout writing file".into()));
                        }
                    }

                    // Atomic rename: temp → target
                    let rename_result = tokio::time::timeout(
                        config.write_timeout,
                        fs::rename(&temp_path, &target_path),
                    )
                    .await;

                    match rename_result {
                        Ok(Ok(_)) => {
                            // Success — disarm guard so it doesn't delete the renamed file
                            guard.disarm();
                        }
                        Ok(Err(e)) => {
                            // Guard will clean up temp file on drop
                            return Err(CamelError::from(e));
                        }
                        Err(_) => {
                            // Guard will clean up temp file on drop
                            return Err(CamelError::ProcessorError("Timeout renaming file".into()));
                        }
                    }
                }
            }

            // 4. Set output header
            let abs_path = target_path
                .canonicalize()
                .unwrap_or_else(|_| target_path.clone())
                .to_string_lossy()
                .to_string();
            exchange
                .input
                .set_header("CamelFileNameProduced", serde_json::Value::String(abs_path));

            debug!(
                file = %target_path.display(),
                correlation_id = %exchange.correlation_id(),
                "File written"
            );

            Ok(exchange)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use std::time::Duration;
    use tokio_util::sync::CancellationToken;

    fn test_producer_ctx() -> ProducerContext {
        ProducerContext::new()
    }

    #[test]
    fn test_file_config_defaults() {
        let config = FileConfig::from_uri("file:/tmp/inbox").unwrap();
        assert_eq!(config.directory, "/tmp/inbox");
        assert_eq!(config.delay, Duration::from_millis(500));
        assert_eq!(config.initial_delay, Duration::from_millis(1000));
        assert!(!config.noop);
        assert!(!config.delete);
        assert_eq!(config.move_to, Some(".camel".to_string()));
        assert!(config.file_name.is_none());
        assert!(config.include.is_none());
        assert!(config.exclude.is_none());
        assert!(!config.recursive);
        assert_eq!(config.file_exist, FileExistStrategy::Override);
        assert!(config.temp_prefix.is_none());
        assert!(config.auto_create);
        // New timeout defaults
        assert_eq!(config.read_timeout, Duration::from_secs(30));
        assert_eq!(config.write_timeout, Duration::from_secs(30));
    }

    #[test]
    fn test_file_config_consumer_options() {
        let config = FileConfig::from_uri(
            "file:/data/input?delay=1000&initialDelay=2000&noop=true&recursive=true&include=.*\\.csv"
        ).unwrap();
        assert_eq!(config.directory, "/data/input");
        assert_eq!(config.delay, Duration::from_millis(1000));
        assert_eq!(config.initial_delay, Duration::from_millis(2000));
        assert!(config.noop);
        assert!(config.recursive);
        assert_eq!(config.include, Some(".*\\.csv".to_string()));
    }

    #[test]
    fn test_file_config_producer_options() {
        let config = FileConfig::from_uri(
            "file:/data/output?fileExist=Append&tempPrefix=.tmp&autoCreate=false&fileName=out.txt",
        )
        .unwrap();
        assert_eq!(config.file_exist, FileExistStrategy::Append);
        assert_eq!(config.temp_prefix, Some(".tmp".to_string()));
        assert!(!config.auto_create);
        assert_eq!(config.file_name, Some("out.txt".to_string()));
    }

    #[test]
    fn test_file_config_delete_mode() {
        let config = FileConfig::from_uri("file:/tmp/inbox?delete=true").unwrap();
        assert!(config.delete);
        assert!(config.move_to.is_none());
    }

    #[test]
    fn test_file_config_noop_mode() {
        let config = FileConfig::from_uri("file:/tmp/inbox?noop=true").unwrap();
        assert!(config.noop);
        assert!(config.move_to.is_none());
    }

    #[test]
    fn test_file_config_wrong_scheme() {
        let result = FileConfig::from_uri("timer:tick");
        assert!(result.is_err());
    }

    #[test]
    fn test_file_component_scheme() {
        let component = FileComponent::new();
        assert_eq!(component.scheme(), "file");
    }

    #[test]
    fn test_file_component_creates_endpoint() {
        let component = FileComponent::new();
        let endpoint = component.create_endpoint("file:/tmp/test");
        assert!(endpoint.is_ok());
    }

    // -----------------------------------------------------------------------
    // Consumer tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_file_consumer_reads_files() {
        let dir = tempfile::tempdir().unwrap();
        let dir_path = dir.path().to_str().unwrap();

        std::fs::write(dir.path().join("test1.txt"), "hello").unwrap();
        std::fs::write(dir.path().join("test2.txt"), "world").unwrap();

        let component = FileComponent::new();
        let endpoint = component
            .create_endpoint(&format!(
                "file:{dir_path}?noop=true&initialDelay=0&delay=100"
            ))
            .unwrap();
        let mut consumer = endpoint.create_consumer().unwrap();

        let (tx, mut rx) = tokio::sync::mpsc::channel(16);
        let token = CancellationToken::new();
        let ctx = ConsumerContext::new(tx, token.clone());

        tokio::spawn(async move {
            consumer.start(ctx).await.unwrap();
        });

        let mut received = Vec::new();
        let timeout = tokio::time::timeout(Duration::from_secs(2), async {
            while let Some(envelope) = rx.recv().await {
                received.push(envelope.exchange);
                if received.len() == 2 {
                    break;
                }
            }
        })
        .await;
        token.cancel();

        assert!(timeout.is_ok(), "Should have received 2 exchanges");
        assert_eq!(received.len(), 2);

        for ex in &received {
            assert!(ex.input.header("CamelFileName").is_some());
            assert!(ex.input.header("CamelFileNameOnly").is_some());
            assert!(ex.input.header("CamelFileAbsolutePath").is_some());
            assert!(ex.input.header("CamelFileLength").is_some());
            assert!(ex.input.header("CamelFileLastModified").is_some());
        }
    }

    #[tokio::test]
    async fn noop_second_poll_does_not_re_emit_seen_files() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        tokio::fs::write(&file_path, b"hello").await.unwrap();

        let uri = format!(
            "file:{}?noop=true&initialDelay=0&delay=50",
            dir.path().display()
        );
        let config = FileConfig::from_uri(&uri).unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::channel(16);
        let token = CancellationToken::new();
        let ctx = ConsumerContext::new(tx, token);

        let include_re = None;
        let exclude_re = None;
        let mut seen = std::collections::HashSet::new();

        poll_directory(&config, &ctx, &include_re, &exclude_re, &mut seen)
            .await
            .unwrap();
        assert!(rx.try_recv().is_ok(), "first poll should emit file");
        assert!(rx.try_recv().is_err(), "should only emit once");

        poll_directory(&config, &ctx, &include_re, &exclude_re, &mut seen)
            .await
            .unwrap();
        assert!(
            rx.try_recv().is_err(),
            "second poll should not re-emit seen file"
        );
    }

    #[tokio::test]
    async fn noop_new_files_picked_up_after_first_poll() {
        let dir = tempfile::tempdir().unwrap();
        let file1 = dir.path().join("a.txt");
        tokio::fs::write(&file1, b"a").await.unwrap();

        let uri = format!(
            "file:{}?noop=true&initialDelay=0&delay=50",
            dir.path().display()
        );
        let config = FileConfig::from_uri(&uri).unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::channel(16);
        let token = CancellationToken::new();
        let ctx = ConsumerContext::new(tx, token);

        let include_re = None;
        let exclude_re = None;
        let mut seen = std::collections::HashSet::new();

        poll_directory(&config, &ctx, &include_re, &exclude_re, &mut seen)
            .await
            .unwrap();
        let _ = rx.try_recv();

        let file2 = dir.path().join("b.txt");
        tokio::fs::write(&file2, b"b").await.unwrap();

        poll_directory(&config, &ctx, &include_re, &exclude_re, &mut seen)
            .await
            .unwrap();
        assert!(
            rx.try_recv().is_ok(),
            "b.txt should be emitted on second poll"
        );
        assert!(rx.try_recv().is_err(), "a.txt should not be re-emitted");
    }

    #[tokio::test]
    async fn test_file_consumer_include_filter() {
        let dir = tempfile::tempdir().unwrap();
        let dir_path = dir.path().to_str().unwrap();

        std::fs::write(dir.path().join("data.csv"), "a,b,c").unwrap();
        std::fs::write(dir.path().join("readme.txt"), "hello").unwrap();

        let component = FileComponent::new();
        let endpoint = component
            .create_endpoint(&format!(
                "file:{dir_path}?noop=true&initialDelay=0&delay=100&include=.*\\.csv"
            ))
            .unwrap();
        let mut consumer = endpoint.create_consumer().unwrap();

        let (tx, mut rx) = tokio::sync::mpsc::channel(16);
        let token = CancellationToken::new();
        let ctx = ConsumerContext::new(tx, token.clone());

        tokio::spawn(async move {
            consumer.start(ctx).await.unwrap();
        });

        let mut received = Vec::new();
        let _ = tokio::time::timeout(Duration::from_millis(500), async {
            while let Some(envelope) = rx.recv().await {
                received.push(envelope.exchange);
                if received.len() == 1 {
                    break;
                }
            }
        })
        .await;
        token.cancel();

        assert_eq!(received.len(), 1);
        let name = received[0]
            .input
            .header("CamelFileNameOnly")
            .and_then(|v| v.as_str())
            .unwrap();
        assert_eq!(name, "data.csv");
    }

    #[tokio::test]
    async fn test_file_consumer_delete_mode() {
        let dir = tempfile::tempdir().unwrap();
        let dir_path = dir.path().to_str().unwrap();

        std::fs::write(dir.path().join("deleteme.txt"), "bye").unwrap();

        let component = FileComponent::new();
        let endpoint = component
            .create_endpoint(&format!(
                "file:{dir_path}?delete=true&initialDelay=0&delay=100"
            ))
            .unwrap();
        let mut consumer = endpoint.create_consumer().unwrap();

        let (tx, mut rx) = tokio::sync::mpsc::channel(16);
        let token = CancellationToken::new();
        let ctx = ConsumerContext::new(tx, token.clone());

        tokio::spawn(async move {
            consumer.start(ctx).await.unwrap();
        });

        let _ = tokio::time::timeout(Duration::from_millis(500), async { rx.recv().await }).await;
        token.cancel();

        tokio::time::sleep(Duration::from_millis(100)).await;

        assert!(
            !dir.path().join("deleteme.txt").exists(),
            "File should be deleted"
        );
    }

    #[tokio::test]
    async fn test_file_consumer_move_mode() {
        let dir = tempfile::tempdir().unwrap();
        let dir_path = dir.path().to_str().unwrap();

        std::fs::write(dir.path().join("moveme.txt"), "data").unwrap();

        let component = FileComponent::new();
        let endpoint = component
            .create_endpoint(&format!("file:{dir_path}?initialDelay=0&delay=100"))
            .unwrap();
        let mut consumer = endpoint.create_consumer().unwrap();

        let (tx, mut rx) = tokio::sync::mpsc::channel(16);
        let token = CancellationToken::new();
        let ctx = ConsumerContext::new(tx, token.clone());

        tokio::spawn(async move {
            consumer.start(ctx).await.unwrap();
        });

        let _ = tokio::time::timeout(Duration::from_millis(500), async { rx.recv().await }).await;
        token.cancel();

        tokio::time::sleep(Duration::from_millis(100)).await;

        assert!(
            !dir.path().join("moveme.txt").exists(),
            "Original file should be gone"
        );
        assert!(
            dir.path().join(".camel").join("moveme.txt").exists(),
            "File should be in .camel/"
        );
    }

    #[tokio::test]
    async fn test_file_consumer_respects_cancellation() {
        let dir = tempfile::tempdir().unwrap();
        let dir_path = dir.path().to_str().unwrap();

        let component = FileComponent::new();
        let endpoint = component
            .create_endpoint(&format!("file:{dir_path}?initialDelay=0&delay=50"))
            .unwrap();
        let mut consumer = endpoint.create_consumer().unwrap();

        let (tx, _rx) = tokio::sync::mpsc::channel(16);
        let token = CancellationToken::new();
        let ctx = ConsumerContext::new(tx, token.clone());

        let handle = tokio::spawn(async move {
            consumer.start(ctx).await.unwrap();
        });

        tokio::time::sleep(Duration::from_millis(150)).await;
        token.cancel();

        let result = tokio::time::timeout(Duration::from_secs(1), handle).await;
        assert!(
            result.is_ok(),
            "Consumer should have stopped after cancellation"
        );
    }

    // -----------------------------------------------------------------------
    // Producer tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_file_producer_writes_file() {
        use tower::ServiceExt;

        let dir = tempfile::tempdir().unwrap();
        let dir_path = dir.path().to_str().unwrap();

        let component = FileComponent::new();
        let endpoint = component
            .create_endpoint(&format!("file:{dir_path}"))
            .unwrap();
        let ctx = test_producer_ctx();
        let producer = endpoint.create_producer(&ctx).unwrap();

        let mut exchange = Exchange::new(Message::new("file content"));
        exchange.input.set_header(
            "CamelFileName",
            serde_json::Value::String("output.txt".to_string()),
        );

        let result = producer.oneshot(exchange).await.unwrap();

        let content = std::fs::read_to_string(dir.path().join("output.txt")).unwrap();
        assert_eq!(content, "file content");

        assert!(result.input.header("CamelFileNameProduced").is_some());
    }

    #[tokio::test]
    async fn test_file_producer_auto_create_dirs() {
        use tower::ServiceExt;

        let dir = tempfile::tempdir().unwrap();
        let dir_path = dir.path().to_str().unwrap();

        let component = FileComponent::new();
        let endpoint = component
            .create_endpoint(&format!("file:{dir_path}/sub/dir"))
            .unwrap();
        let ctx = test_producer_ctx();
        let producer = endpoint.create_producer(&ctx).unwrap();

        let mut exchange = Exchange::new(Message::new("nested"));
        exchange.input.set_header(
            "CamelFileName",
            serde_json::Value::String("file.txt".to_string()),
        );

        producer.oneshot(exchange).await.unwrap();

        assert!(dir.path().join("sub/dir/file.txt").exists());
    }

    #[tokio::test]
    async fn test_file_producer_file_exist_fail() {
        use tower::ServiceExt;

        let dir = tempfile::tempdir().unwrap();
        let dir_path = dir.path().to_str().unwrap();

        std::fs::write(dir.path().join("existing.txt"), "old").unwrap();

        let component = FileComponent::new();
        let endpoint = component
            .create_endpoint(&format!("file:{dir_path}?fileExist=Fail"))
            .unwrap();
        let ctx = test_producer_ctx();
        let producer = endpoint.create_producer(&ctx).unwrap();

        let mut exchange = Exchange::new(Message::new("new"));
        exchange.input.set_header(
            "CamelFileName",
            serde_json::Value::String("existing.txt".to_string()),
        );

        let result = producer.oneshot(exchange).await;
        assert!(
            result.is_err(),
            "Should fail when file exists with Fail strategy"
        );
    }

    #[tokio::test]
    async fn test_file_producer_file_exist_append() {
        use tower::ServiceExt;

        let dir = tempfile::tempdir().unwrap();
        let dir_path = dir.path().to_str().unwrap();

        std::fs::write(dir.path().join("append.txt"), "old").unwrap();

        let component = FileComponent::new();
        let endpoint = component
            .create_endpoint(&format!("file:{dir_path}?fileExist=Append"))
            .unwrap();
        let ctx = test_producer_ctx();
        let producer = endpoint.create_producer(&ctx).unwrap();

        let mut exchange = Exchange::new(Message::new("new"));
        exchange.input.set_header(
            "CamelFileName",
            serde_json::Value::String("append.txt".to_string()),
        );

        producer.oneshot(exchange).await.unwrap();

        let content = std::fs::read_to_string(dir.path().join("append.txt")).unwrap();
        assert_eq!(content, "oldnew");
    }

    #[tokio::test]
    async fn test_file_producer_temp_prefix() {
        use tower::ServiceExt;

        let dir = tempfile::tempdir().unwrap();
        let dir_path = dir.path().to_str().unwrap();

        let component = FileComponent::new();
        let endpoint = component
            .create_endpoint(&format!("file:{dir_path}?tempPrefix=.tmp"))
            .unwrap();
        let ctx = test_producer_ctx();
        let producer = endpoint.create_producer(&ctx).unwrap();

        let mut exchange = Exchange::new(Message::new("atomic write"));
        exchange.input.set_header(
            "CamelFileName",
            serde_json::Value::String("final.txt".to_string()),
        );

        producer.oneshot(exchange).await.unwrap();

        assert!(dir.path().join("final.txt").exists());
        assert!(!dir.path().join(".tmpfinal.txt").exists());
        let content = std::fs::read_to_string(dir.path().join("final.txt")).unwrap();
        assert_eq!(content, "atomic write");
    }

    #[tokio::test]
    async fn test_file_producer_uses_filename_option() {
        use tower::ServiceExt;

        let dir = tempfile::tempdir().unwrap();
        let dir_path = dir.path().to_str().unwrap();

        let component = FileComponent::new();
        let endpoint = component
            .create_endpoint(&format!("file:{dir_path}?fileName=fixed.txt"))
            .unwrap();
        let ctx = test_producer_ctx();
        let producer = endpoint.create_producer(&ctx).unwrap();

        let exchange = Exchange::new(Message::new("content"));

        producer.oneshot(exchange).await.unwrap();
        assert!(dir.path().join("fixed.txt").exists());
    }

    #[tokio::test]
    async fn test_file_producer_no_filename_errors() {
        use tower::ServiceExt;

        let dir = tempfile::tempdir().unwrap();
        let dir_path = dir.path().to_str().unwrap();

        let component = FileComponent::new();
        let endpoint = component
            .create_endpoint(&format!("file:{dir_path}"))
            .unwrap();
        let ctx = test_producer_ctx();
        let producer = endpoint.create_producer(&ctx).unwrap();

        let exchange = Exchange::new(Message::new("content"));

        let result = producer.oneshot(exchange).await;
        assert!(result.is_err(), "Should error when no filename is provided");
    }

    // -----------------------------------------------------------------------
    // Security tests - Path traversal protection
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_file_producer_rejects_path_traversal_parent_directory() {
        use tower::ServiceExt;

        let dir = tempfile::tempdir().unwrap();
        let dir_path = dir.path().to_str().unwrap();

        // Create a subdirectory
        std::fs::create_dir(dir.path().join("subdir")).unwrap();
        std::fs::write(dir.path().join("secret.txt"), "secret").unwrap();

        let component = FileComponent::new();
        let endpoint = component
            .create_endpoint(&format!("file:{dir_path}/subdir"))
            .unwrap();
        let ctx = test_producer_ctx();
        let producer = endpoint.create_producer(&ctx).unwrap();

        let mut exchange = Exchange::new(Message::new("malicious"));
        exchange.input.set_header(
            "CamelFileName",
            serde_json::Value::String("../secret.txt".to_string()),
        );

        let result = producer.oneshot(exchange).await;
        assert!(result.is_err(), "Should reject path traversal attempt");

        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("outside"),
            "Error should mention path is outside base directory"
        );
    }

    #[tokio::test]
    async fn test_file_producer_rejects_absolute_path_outside_base() {
        use tower::ServiceExt;

        let dir = tempfile::tempdir().unwrap();
        let dir_path = dir.path().to_str().unwrap();

        let component = FileComponent::new();
        let endpoint = component
            .create_endpoint(&format!("file:{dir_path}"))
            .unwrap();
        let ctx = test_producer_ctx();
        let producer = endpoint.create_producer(&ctx).unwrap();

        let mut exchange = Exchange::new(Message::new("malicious"));
        exchange.input.set_header(
            "CamelFileName",
            serde_json::Value::String("/etc/passwd".to_string()),
        );

        let result = producer.oneshot(exchange).await;
        assert!(result.is_err(), "Should reject absolute path outside base");
    }

    // -----------------------------------------------------------------------
    // Large file streaming tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    #[ignore] // Slow test - run with --ignored flag
    async fn test_large_file_streaming_constant_memory() {
        use std::io::Write;
        use tempfile::NamedTempFile;

        // Create a 150MB file (larger than 100MB limit)
        let mut temp_file = NamedTempFile::new().unwrap();
        let file_size = 150 * 1024 * 1024; // 150MB
        let chunk = vec![b'X'; 1024 * 1024]; // 1MB chunk

        for _ in 0..150 {
            temp_file.write_all(&chunk).unwrap();
        }
        temp_file.flush().unwrap();

        let dir = temp_file.path().parent().unwrap();
        let dir_path = dir.to_str().unwrap();
        let file_name = temp_file
            .path()
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();

        // Read file as stream (should succeed with lazy evaluation)
        let component = FileComponent::new();
        let endpoint = component
            .create_endpoint(&format!(
                "file:{dir_path}?noop=true&initialDelay=0&delay=100&fileName={file_name}"
            ))
            .unwrap();
        let mut consumer = endpoint.create_consumer().unwrap();

        let (tx, mut rx) = tokio::sync::mpsc::channel(16);
        let token = CancellationToken::new();
        let ctx = ConsumerContext::new(tx, token.clone());

        tokio::spawn(async move {
            let _ = consumer.start(ctx).await;
        });

        let exchange = tokio::time::timeout(Duration::from_secs(5), async {
            rx.recv().await.unwrap().exchange
        })
        .await
        .expect("Should receive exchange");
        token.cancel();

        // Verify body is a stream (not materialized)
        assert!(matches!(exchange.input.body, Body::Stream(_)));

        // Verify we can read metadata without consuming
        if let Body::Stream(ref stream_body) = exchange.input.body {
            assert!(stream_body.metadata.size_hint.is_some());
            let size = stream_body.metadata.size_hint.unwrap();
            assert_eq!(size, file_size as u64);
        }

        // Materializing should fail (exceeds 100MB limit)
        if let Body::Stream(stream_body) = exchange.input.body {
            let body = Body::Stream(stream_body);
            let result = body.into_bytes(100 * 1024 * 1024).await;
            assert!(result.is_err());
        }

        // But we CAN read chunks one at a time (simulating line-by-line processing)
        // This demonstrates lazy evaluation - we don't need to load entire file
        let component2 = FileComponent::new();
        let endpoint2 = component2
            .create_endpoint(&format!(
                "file:{dir_path}?noop=true&initialDelay=0&delay=100&fileName={file_name}"
            ))
            .unwrap();
        let mut consumer2 = endpoint2.create_consumer().unwrap();

        let (tx2, mut rx2) = tokio::sync::mpsc::channel(16);
        let token2 = CancellationToken::new();
        let ctx2 = ConsumerContext::new(tx2, token2.clone());

        tokio::spawn(async move {
            let _ = consumer2.start(ctx2).await;
        });

        let exchange2 = tokio::time::timeout(Duration::from_secs(5), async {
            rx2.recv().await.unwrap().exchange
        })
        .await
        .expect("Should receive exchange");
        token2.cancel();

        if let Body::Stream(stream_body) = exchange2.input.body {
            let mut stream_lock = stream_body.stream.lock().await;
            let mut stream = stream_lock.take().unwrap();

            // Read first chunk (size varies based on ReaderStream's buffer)
            if let Some(chunk_result) = stream.next().await {
                let chunk = chunk_result.unwrap();
                assert!(!chunk.is_empty());
                assert!(chunk.len() < file_size);
                // Memory usage is constant - we only have this chunk in memory, not 150MB
            }
        }
    }

    // -----------------------------------------------------------------------
    // Streaming producer tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_producer_writes_stream_body() {
        let dir = tempfile::tempdir().unwrap();
        let dir_path = dir.path().to_str().unwrap();
        let uri = format!("file:{dir_path}?fileName=out.txt");

        let component = FileComponent::new();
        let endpoint = component.create_endpoint(&uri).unwrap();
        let producer = endpoint.create_producer(&test_producer_ctx()).unwrap();

        let chunks: Vec<Result<Bytes, CamelError>> = vec![
            Ok(Bytes::from("hello ")),
            Ok(Bytes::from("streaming ")),
            Ok(Bytes::from("world")),
        ];
        let stream = futures::stream::iter(chunks);
        let body = Body::Stream(StreamBody {
            stream: std::sync::Arc::new(tokio::sync::Mutex::new(Some(Box::pin(stream)))),
            metadata: StreamMetadata {
                size_hint: None,
                content_type: None,
                origin: None,
            },
        });

        let exchange = Exchange::new(Message::new(body));
        tower::ServiceExt::oneshot(producer, exchange)
            .await
            .unwrap();

        let content = tokio::fs::read_to_string(format!("{dir_path}/out.txt"))
            .await
            .unwrap();
        assert_eq!(content, "hello streaming world");
    }

    #[tokio::test]
    async fn test_producer_stream_atomic_no_partial_on_error() {
        // If the stream errors mid-write, no file should exist at the target path
        let dir = tempfile::tempdir().unwrap();
        let dir_path = dir.path().to_str().unwrap();
        let uri = format!("file:{dir_path}?fileName=out.txt");

        let component = FileComponent::new();
        let endpoint = component.create_endpoint(&uri).unwrap();
        let producer = endpoint.create_producer(&test_producer_ctx()).unwrap();

        let chunks: Vec<Result<Bytes, CamelError>> = vec![
            Ok(Bytes::from("partial")),
            Err(CamelError::ProcessorError(
                "simulated stream error".to_string(),
            )),
        ];
        let stream = futures::stream::iter(chunks);
        let body = Body::Stream(StreamBody {
            stream: std::sync::Arc::new(tokio::sync::Mutex::new(Some(Box::pin(stream)))),
            metadata: StreamMetadata {
                size_hint: None,
                content_type: None,
                origin: None,
            },
        });

        let exchange = Exchange::new(Message::new(body));
        let result = tower::ServiceExt::oneshot(producer, exchange).await;
        assert!(
            result.is_err(),
            "expected error when stream fails mid-write"
        );

        // Target file must NOT exist — write was aborted and temp file cleaned up
        assert!(
            !std::path::Path::new(&format!("{dir_path}/out.txt")).exists(),
            "partial file must not exist after failed write"
        );

        // Temp file must also be cleaned up
        assert!(
            !std::path::Path::new(&format!("{dir_path}/.tmp.out.txt")).exists(),
            "temp file must be cleaned up after failed write"
        );
    }

    #[tokio::test]
    async fn test_producer_stream_append() {
        let dir = tempfile::tempdir().unwrap();
        let dir_path = dir.path().to_str().unwrap();
        let target = format!("{dir_path}/out.txt");

        // Pre-create file with initial content
        tokio::fs::write(&target, b"line1\n").await.unwrap();

        let uri = format!("file:{dir_path}?fileName=out.txt&fileExist=Append");
        let component = FileComponent::new();
        let endpoint = component.create_endpoint(&uri).unwrap();
        let producer = endpoint.create_producer(&test_producer_ctx()).unwrap();

        let chunks: Vec<Result<Bytes, CamelError>> = vec![Ok(Bytes::from("line2\n"))];
        let stream = futures::stream::iter(chunks);
        let body = Body::Stream(StreamBody {
            stream: std::sync::Arc::new(tokio::sync::Mutex::new(Some(Box::pin(stream)))),
            metadata: StreamMetadata {
                size_hint: None,
                content_type: None,
                origin: None,
            },
        });

        let exchange = Exchange::new(Message::new(body));
        tower::ServiceExt::oneshot(producer, exchange)
            .await
            .unwrap();

        let content = tokio::fs::read_to_string(&target).await.unwrap();
        assert_eq!(content, "line1\nline2\n");
    }

    #[tokio::test]
    async fn test_producer_stream_append_partial_on_error() {
        // Append is inherently non-atomic: if the stream errors mid-write,
        // the file will contain partial data. This test documents that behavior.
        let dir = tempfile::tempdir().unwrap();
        let dir_path = dir.path().to_str().unwrap();
        let target = format!("{dir_path}/out.txt");

        // Pre-create file with initial content
        tokio::fs::write(&target, b"initial\n").await.unwrap();

        let uri = format!("file:{dir_path}?fileName=out.txt&fileExist=Append");
        let component = FileComponent::new();
        let endpoint = component.create_endpoint(&uri).unwrap();
        let producer = endpoint.create_producer(&test_producer_ctx()).unwrap();

        // Stream with an error in the middle
        let chunks: Vec<Result<Bytes, CamelError>> = vec![
            Ok(Bytes::from("partial-")), // This will be written
            Err(CamelError::ProcessorError("stream error".to_string())), // This causes failure
            Ok(Bytes::from("never-written")), // This won't be reached
        ];
        let stream = futures::stream::iter(chunks);
        let body = Body::Stream(StreamBody {
            stream: std::sync::Arc::new(tokio::sync::Mutex::new(Some(Box::pin(stream)))),
            metadata: StreamMetadata {
                size_hint: None,
                content_type: None,
                origin: None,
            },
        });

        let exchange = Exchange::new(Message::new(body));
        let result = tower::ServiceExt::oneshot(producer, exchange).await;

        // 1. Producer must return an error
        assert!(
            result.is_err(),
            "expected error when stream fails during append"
        );

        // 2. File must contain initial content + partial data written before the error
        let content = tokio::fs::read_to_string(&target).await.unwrap();
        assert_eq!(
            content, "initial\npartial-",
            "append leaves partial data on stream error (non-atomic by nature)"
        );
    }

    #[tokio::test]
    async fn test_producer_stream_already_consumed_errors() {
        let dir = tempfile::tempdir().unwrap();
        let dir_path = dir.path().to_str().unwrap();
        let uri = format!("file:{dir_path}?fileName=out.txt");

        let component = FileComponent::new();
        let endpoint = component.create_endpoint(&uri).unwrap();
        let producer = endpoint.create_producer(&test_producer_ctx()).unwrap();

        // Mutex holds None -> stream already consumed
        type MaybeStream = std::sync::Arc<
            tokio::sync::Mutex<
                Option<
                    std::pin::Pin<
                        Box<dyn futures::Stream<Item = Result<Bytes, CamelError>> + Send>,
                    >,
                >,
            >,
        >;
        let arc: MaybeStream = std::sync::Arc::new(tokio::sync::Mutex::new(None));
        let body = Body::Stream(StreamBody {
            stream: arc,
            metadata: StreamMetadata {
                size_hint: None,
                content_type: None,
                origin: None,
            },
        });

        let exchange = Exchange::new(Message::new(body));
        let result = tower::ServiceExt::oneshot(producer, exchange).await;
        assert!(
            result.is_err(),
            "expected error for already-consumed stream"
        );
    }

    // -----------------------------------------------------------------------
    // GlobalConfig tests - apply_global_defaults behavior
    // -----------------------------------------------------------------------

    #[test]
    fn test_global_config_applied_to_endpoint() {
        // Global config with non-default values
        let global = FileGlobalConfig::default()
            .with_delay_ms(2000)
            .with_initial_delay_ms(5000)
            .with_read_timeout_ms(60_000)
            .with_write_timeout_ms(45_000);
        let component = FileComponent::with_config(global);
        // URI uses no explicit delay/timeout params → macro defaults apply
        let endpoint = component.create_endpoint("file:/tmp/inbox").unwrap();
        // We cannot call endpoint.config directly (FileEndpoint is private),
        // but we can test apply_global_defaults on FileConfig directly:
        let mut config = FileConfig::from_uri("file:/tmp/inbox").unwrap();
        let global2 = FileGlobalConfig::default()
            .with_delay_ms(2000)
            .with_initial_delay_ms(5000)
            .with_read_timeout_ms(60_000)
            .with_write_timeout_ms(45_000);
        config.apply_global_defaults(&global2);
        assert_eq!(config.delay, Duration::from_millis(2000));
        assert_eq!(config.initial_delay, Duration::from_millis(5000));
        assert_eq!(config.read_timeout, Duration::from_millis(60_000));
        assert_eq!(config.write_timeout, Duration::from_millis(45_000));
        // endpoint creation succeeds too
        let _ = endpoint; // just verify create_endpoint didn't fail
    }

    #[test]
    fn test_uri_param_wins_over_global_config() {
        // URI explicitly sets delay=1000 (NOT the 500ms macro default)
        let mut config =
            FileConfig::from_uri("file:/tmp/inbox?delay=1000&initialDelay=2000").unwrap();
        // Global config would want 3000ms delay
        let global = FileGlobalConfig::default()
            .with_delay_ms(3000)
            .with_initial_delay_ms(4000);
        config.apply_global_defaults(&global);
        // URI value of 1000ms must be preserved (not replaced by 3000ms)
        assert_eq!(config.delay, Duration::from_millis(1000));
        // URI value of 2000ms must be preserved (not replaced by 4000ms)
        assert_eq!(config.initial_delay, Duration::from_millis(2000));
        // read_timeout was not set by URI → macro default (30000) → global wins if different
        // (read_timeout stays at 30000 since global has same default = 30000)
        assert_eq!(config.read_timeout, Duration::from_millis(30_000));
    }
}
