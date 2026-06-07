//! File system component for rust-camel — polls directories for new or changed
//! files as consumer, writes exchange bodies to files as producer.
//!
//! Main types: `FileBundle`, `FileComponent`, `FileConsumer`, `FileProducer`.
//! Main modules: `bundle`.

pub mod bundle;
pub mod health;
mod poll_logic;
mod polling_consumer;

use poll_logic::poll_directory;

pub use bundle::FileBundle;
pub use health::FileHealthCheck;

use std::collections::HashSet;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use async_trait::async_trait;
use dashmap::DashMap;
use regex::Regex;
use tokio::fs;
use tokio::fs::OpenOptions;
use tokio::io;
use tokio::io::AsyncWriteExt;
use tokio::time;
use tower::Service;
use tracing::{debug, warn};

use camel_component_api::{Body, BoxProcessor, CamelError, Exchange};
use camel_component_api::{
    Component, Consumer, ConsumerContext, Endpoint, PollingConsumer, ProducerContext,
};
use camel_component_api::{UriConfig, parse_uri};
use camel_language_api::Language;
use camel_language_simple::SimpleLanguage;

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
    /// Skip write if file already exists.
    Ignore,
}

impl FromStr for FileExistStrategy {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Override" | "override" => Ok(FileExistStrategy::Override),
            "Append" | "append" => Ok(FileExistStrategy::Append),
            "Fail" | "fail" => Ok(FileExistStrategy::Fail),
            "Ignore" | "ignore" => Ok(FileExistStrategy::Ignore),
            other => Err(format!("unknown FileExistStrategy: {other}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReadLockStrategy {
    #[default]
    None,
    InProcess,
    Rename,
}

impl FromStr for ReadLockStrategy {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "None" | "none" => Ok(Self::None),
            "InProcess" | "inProcess" | "inprocess" => Ok(Self::InProcess),
            "Rename" | "rename" => Ok(Self::Rename),
            other => Err(format!("unknown ReadLockStrategy: {other}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum IdempotentKey {
    #[default]
    None,
    FileName,
    FilePath,
    FileSize,
    /// Lightweight fingerprint using file path + last-modified timestamp (not a cryptographic hash).
    Digest,
}

impl FromStr for IdempotentKey {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "None" | "none" => Ok(Self::None),
            "FileName" | "fileName" | "filename" => Ok(Self::FileName),
            "FilePath" | "filePath" | "filepath" => Ok(Self::FilePath),
            "FileSize" | "fileSize" | "filesize" => Ok(Self::FileSize),
            "Digest" | "digest" => Ok(Self::Digest),
            other => Err(format!("unknown IdempotentKey: {other}")),
        }
    }
}

// ---------------------------------------------------------------------------
// FileGlobalConfig
// ---------------------------------------------------------------------------

/// Global configuration for File component.
/// Supports serde deserialization with defaults and builder methods.
/// These are the fallback defaults when URI params are not set.
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
#[serde(default)]
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
/// | `Ignore` | Skip write if file already exists |
#[derive(Debug, Clone)]
pub struct FileConfig {
    /// Directory path to read from or write to.
    pub directory: String,

    /// Polling delay in milliseconds (companion field for `delay`).
    #[allow(dead_code)]
    delay_ms: u64,

    /// Polling delay as Duration.
    pub delay: Duration,

    /// Initial delay in milliseconds (companion field for `initial_delay`).
    #[allow(dead_code)]
    initial_delay_ms: u64,

    /// Initial delay as Duration.
    pub initial_delay: Duration,

    /// If true, don't delete or move files after processing.
    pub noop: bool,

    /// If true, delete files after processing.
    pub delete: bool,

    /// Directory to move processed files to (only if not noop/delete).
    /// Default is ".camel" when not specified and noop/delete are false.
    move_to: Option<String>,

    /// Fixed filename for producer (optional).
    pub file_name: Option<String>,

    /// Regex pattern for including files (consumer).
    pub include: Option<String>,

    /// Regex pattern for excluding files (consumer).
    pub exclude: Option<String>,

    /// Whether to scan directories recursively.
    pub recursive: bool,

    /// Strategy for handling existing files when writing.
    pub file_exist: FileExistStrategy,

    /// Read lock strategy for concurrent consumers.
    pub read_lock_strategy: ReadLockStrategy,

    /// In-memory idempotent key selector for consumer.
    pub idempotent_key: IdempotentKey,

    /// Done marker filename pattern created after successful write.
    pub done_file_name: Option<String>,

    /// Charset for string body encoding (UTF-8, ISO-8859-1).
    pub charset: Option<String>,

    /// Prefix for temporary files during atomic writes.
    pub temp_prefix: Option<String>,

    /// Whether to automatically create directories.
    pub auto_create: bool,

    /// If true, verify the starting directory exists at startup.
    pub starting_directory_must_exist: bool,

    /// Read timeout in milliseconds (companion field for `read_timeout`).
    #[allow(dead_code)]
    read_timeout_ms: u64,

    /// Read timeout as Duration.
    pub read_timeout: Duration,

    /// Write timeout in milliseconds (companion field for `write_timeout`).
    #[allow(dead_code)]
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
        if parts.scheme != Self::scheme() {
            return Err(CamelError::InvalidUri(format!(
                "unsupported scheme '{}', expected '{}'",
                parts.scheme,
                Self::scheme()
            )));
        }

        fn parse_bool_param(
            params: &std::collections::HashMap<String, String>,
            key: &str,
            default: bool,
        ) -> Result<bool, CamelError> {
            match params.get(key) {
                Some(v) => match v.to_lowercase().as_str() {
                    "true" | "1" | "yes" => Ok(true),
                    "false" | "0" | "no" => Ok(false),
                    _ => Err(CamelError::InvalidUri(format!(
                        "invalid value for {key}: invalid boolean value: '{v}'"
                    ))),
                },
                None => Ok(default),
            }
        }

        fn parse_u64_param(
            params: &std::collections::HashMap<String, String>,
            key: &str,
            default: u64,
        ) -> Result<u64, CamelError> {
            match params.get(key) {
                Some(v) => v
                    .parse::<u64>()
                    .map_err(|e| CamelError::InvalidUri(format!("invalid value for {key}: {e}"))),
                None => Ok(default),
            }
        }

        fn parse_enum_param<T: FromStr>(
            params: &std::collections::HashMap<String, String>,
            key: &str,
            default: &str,
        ) -> Result<T, CamelError>
        where
            T::Err: std::fmt::Display,
        {
            let raw = params.get(key).map(String::as_str).unwrap_or(default);
            raw.parse::<T>().map_err(|e| {
                CamelError::InvalidUri(format!("invalid value for parameter '{key}': {e}"))
            })
        }

        let params = &parts.params;
        let delay_ms = parse_u64_param(params, "delay", 500)?;
        let initial_delay_ms = parse_u64_param(params, "initialDelay", 1000)?;
        let read_timeout_ms = parse_u64_param(params, "readTimeout", 30_000)?;
        let write_timeout_ms = parse_u64_param(params, "writeTimeout", 30_000)?;

        let cfg = Self {
            directory: parts.path,
            delay_ms,
            delay: Duration::from_millis(delay_ms),
            initial_delay_ms,
            initial_delay: Duration::from_millis(initial_delay_ms),
            noop: parse_bool_param(params, "noop", false)?,
            delete: parse_bool_param(params, "delete", false)?,
            move_to: params.get("move").cloned(),
            file_name: params.get("fileName").cloned(),
            include: params.get("include").cloned(),
            exclude: params.get("exclude").cloned(),
            recursive: parse_bool_param(params, "recursive", false)?,
            file_exist: parse_enum_param(params, "fileExist", "Override")?,
            read_lock_strategy: parse_enum_param(params, "readLock", "None")?,
            idempotent_key: parse_enum_param(params, "idempotentKey", "None")?,
            done_file_name: params.get("doneFileName").cloned(),
            charset: params.get("charset").cloned(),
            temp_prefix: params.get("tempPrefix").cloned(),
            auto_create: parse_bool_param(params, "autoCreate", true)?,
            starting_directory_must_exist: parse_bool_param(
                params,
                "startingDirectoryMustExist",
                false,
            )?,
            read_timeout_ms,
            read_timeout: Duration::from_millis(read_timeout_ms),
            write_timeout_ms,
            write_timeout: Duration::from_millis(write_timeout_ms),
        };

        cfg.validate()
    }

    fn validate(self) -> Result<Self, CamelError> {
        // Reject path traversal in move_to
        if let Some(ref move_to) = self.move_to
            && path_contains_traversal(move_to)
        {
            return Err(CamelError::Config(format!(
                "move_to contains path traversal component: {move_to}"
            )));
        }

        if let Some(ref move_to) = self.move_to
            && std::path::Path::new(move_to).is_absolute()
        {
            return Err(CamelError::InvalidUri(format!(
                "move_to must be relative path within base directory: {move_to}"
            )));
        }

        // Reject path traversal in temp_prefix
        if let Some(ref temp_prefix) = self.temp_prefix
            && path_contains_traversal(temp_prefix)
        {
            return Err(CamelError::Config(format!(
                "temp_prefix contains path traversal component: {temp_prefix}"
            )));
        }

        if let Some(ref temp_prefix) = self.temp_prefix
            && !is_valid_temp_prefix(temp_prefix)
        {
            return Err(CamelError::Config(
                "temp_prefix must be plain filename prefix (no path separators, absolute paths, or null bytes)".into(),
            ));
        }

        // Reject empty file_name
        if let Some(ref file_name) = self.file_name {
            if file_name.is_empty() {
                return Err(CamelError::Config("file_name must not be empty".into()));
            }
            // Reject null bytes in file_name
            if file_name.contains('\0') {
                return Err(CamelError::Config(
                    "file_name must not contain null bytes".into(),
                ));
            }
        }

        // If starting_directory_must_exist, verify directory exists
        if self.starting_directory_must_exist {
            let dir_path = std::path::Path::new(&self.directory);
            if !dir_path.exists() {
                return Err(CamelError::Config(format!(
                    "starting directory does not exist: {}",
                    self.directory
                )));
            }
        }

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

    fn create_endpoint(
        &self,
        uri: &str,
        ctx: &dyn camel_component_api::ComponentContext,
    ) -> Result<Box<dyn Endpoint>, CamelError> {
        let mut config = FileConfig::from_uri(uri)?;
        if let Some(ref global_config) = self.config {
            config.apply_global_defaults(global_config);
        }
        let dir_path = std::path::PathBuf::from(&config.directory);
        let health_check = FileHealthCheck::new(dir_path.clone());
        ctx.register_current_route_health_check(std::sync::Arc::new(health_check));
        Ok(Box::new(FileEndpoint {
            uri: uri.to_string(),
            config,
            in_process_locks: std::sync::Arc::new(DashMap::new()),
            idempotent_repo: std::sync::Arc::new(tokio::sync::Mutex::new(HashSet::new())),
        }))
    }
}

// ---------------------------------------------------------------------------
// FileEndpoint
// ---------------------------------------------------------------------------

struct FileEndpoint {
    uri: String,
    config: FileConfig,
    in_process_locks: std::sync::Arc<DashMap<PathBuf, ()>>,
    idempotent_repo: std::sync::Arc<tokio::sync::Mutex<HashSet<String>>>,
}

impl Endpoint for FileEndpoint {
    fn uri(&self) -> &str {
        &self.uri
    }

    fn create_consumer(
        &self,
        rt: Arc<dyn camel_component_api::RuntimeObservability>,
    ) -> Result<Box<dyn Consumer>, CamelError> {
        Ok(Box::new(FileConsumer::new(
            self.config.clone(),
            self.in_process_locks.clone(),
            self.idempotent_repo.clone(),
            rt,
        )))
    }

    fn create_producer(
        &self,
        _rt: Arc<dyn camel_component_api::RuntimeObservability>,
        _ctx: &ProducerContext,
    ) -> Result<BoxProcessor, CamelError> {
        Ok(BoxProcessor::new(FileProducer {
            config: self.config.clone(),
        }))
    }

    fn polling_consumer(&self) -> Option<Box<dyn PollingConsumer>> {
        Some(Box::new(polling_consumer::FilePollingConsumer::new(
            self.config.clone(),
            self.in_process_locks.clone(),
            self.idempotent_repo.clone(),
        )))
    }
}

// ---------------------------------------------------------------------------
// FileConsumer
// ---------------------------------------------------------------------------

struct FileConsumer {
    config: FileConfig,
    seen: HashSet<PathBuf>,
    in_process_locks: std::sync::Arc<DashMap<PathBuf, ()>>,
    idempotent_repo: std::sync::Arc<tokio::sync::Mutex<HashSet<String>>>,
    /// Phase B will use this for `rt.metrics().increment_errors(...)` and
    /// `rt.health().force_unhealthy_for_route(...)` calls per ADR-0012.
    #[allow(dead_code)]
    runtime: Arc<dyn camel_component_api::RuntimeObservability>,
}

impl FileConsumer {
    fn new(
        config: FileConfig,
        in_process_locks: std::sync::Arc<DashMap<PathBuf, ()>>,
        idempotent_repo: std::sync::Arc<tokio::sync::Mutex<HashSet<String>>>,
        runtime: Arc<dyn camel_component_api::RuntimeObservability>,
    ) -> Self {
        Self {
            config,
            seen: HashSet::new(),
            in_process_locks,
            idempotent_repo,
            runtime,
        }
    }
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
                        &self.in_process_locks,
                        &self.idempotent_repo,
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

// ---------------------------------------------------------------------------
// Path validation for security
// ---------------------------------------------------------------------------

/// Returns true if the path string contains a `..` component (path traversal).
fn path_contains_traversal(path: &str) -> bool {
    std::path::Path::new(path)
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
}

fn is_valid_temp_prefix(prefix: &str) -> bool {
    !prefix.contains('\0')
        && !std::path::Path::new(prefix).is_absolute()
        && !prefix.contains(std::path::MAIN_SEPARATOR)
        && !prefix.contains('/')
        && !prefix.contains('\\')
}

fn validate_path_is_within_base(
    base_dir: &std::path::Path,
    target_path: &std::path::Path,
) -> Result<(), CamelError> {
    // If both base and target exist, use strict canonicalize comparison.
    // Otherwise, do lexical traversal check (sufficient since config-time
    // validation already rejects '..' in fileName).
    if base_dir.exists() {
        let canonical_base = base_dir.canonicalize().map_err(|e| {
            CamelError::ProcessorError(format!("Cannot canonicalize base directory: {}", e))
        })?;

        let canonical_target = if target_path.exists() {
            target_path.canonicalize().map_err(|e| {
                CamelError::ProcessorError(format!("Cannot canonicalize target path: {}", e))
            })?
        } else if let Some(parent) = target_path.parent() {
            if parent.exists() {
                let canonical_parent = parent.canonicalize().map_err(|e| {
                    CamelError::ProcessorError(format!(
                        "Cannot canonicalize parent directory: {}",
                        e
                    ))
                })?;
                if let Some(filename) = target_path.file_name() {
                    canonical_parent.join(filename)
                } else {
                    return Err(CamelError::ProcessorError(
                        "Invalid target path: no filename".to_string(),
                    ));
                }
            } else {
                // Neither target nor its parent exist — use lexical traversal check.
                let rel = target_path.strip_prefix(base_dir).map_err(|_| {
                    CamelError::ProcessorError(format!(
                        "Path '{}' is not under base '{}'",
                        target_path.display(),
                        base_dir.display()
                    ))
                })?;
                if path_contains_traversal(&rel.to_string_lossy()) {
                    return Err(CamelError::ProcessorError(format!(
                        "Path '{}' contains directory traversal",
                        target_path.display()
                    )));
                }
                return Ok(());
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
    } else {
        // Base dir doesn't exist yet (auto_create case).
        // Lexical check: ensure no traversal in the relative portion.
        let rel = target_path.strip_prefix(base_dir).map_err(|_| {
            CamelError::ProcessorError(format!(
                "Path '{}' is not under base '{}'",
                target_path.display(),
                base_dir.display()
            ))
        })?;
        let rel_str = rel.to_string_lossy();
        if path_contains_traversal(&rel_str) {
            return Err(CamelError::ProcessorError(format!(
                "Path '{}' contains directory traversal",
                target_path.display()
            )));
        }
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
    async fn resolve_filename(
        exchange: &Exchange,
        config: &FileConfig,
    ) -> Result<String, CamelError> {
        let raw = if let Some(name) = exchange
            .input
            .header("CamelFileName")
            .and_then(|v| v.as_str())
        {
            Some(name.to_string())
        } else {
            config.file_name.clone()
        };

        match raw {
            Some(name) if name.contains("${") => {
                let lang = SimpleLanguage::new();
                let expr = lang.create_expression(&name).map_err(|e| {
                    CamelError::ProcessorError(format!(
                        "cannot parse fileName expression '{}': {e}",
                        name
                    ))
                })?;
                let val = expr.evaluate(exchange).await.map_err(|e| {
                    CamelError::ProcessorError(format!(
                        "cannot evaluate fileName expression '{}': {e}",
                        name
                    ))
                })?;
                match val {
                    serde_json::Value::String(s) => Ok(s),
                    other => Ok(other.to_string()),
                }
            }
            Some(name) => Ok(name),
            None => Err(CamelError::ProcessorError(
                "No filename specified: set CamelFileName header or fileName option".to_string(),
            )),
        }
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
            let file_name = FileProducer::resolve_filename(&exchange, &config).await?;
            let body = exchange.input.body.clone();

            let dir_path = std::path::Path::new(&config.directory);
            let target_path = dir_path.join(&file_name);

            // 1. Security: validate path is within base directory
            validate_path_is_within_base(dir_path, &target_path)?;

            // 2. Auto-create directories (after path validation)
            if config.auto_create
                && let Some(parent) = target_path.parent()
            {
                tokio::time::timeout(config.write_timeout, fs::create_dir_all(parent))
                    .await
                    .map_err(|_| CamelError::ProcessorError("Timeout creating directories".into()))?
                    .map_err(CamelError::from)?;
            }

            // 3. Handle file-exist strategy
            match config.file_exist {
                FileExistStrategy::Fail => {
                    let mut file = tokio::time::timeout(
                        config.write_timeout,
                        OpenOptions::new()
                            .write(true)
                            .create_new(true)
                            .open(&target_path),
                    )
                    .await
                    .map_err(|_| {
                        CamelError::ProcessorError("Timeout opening file with create_new".into())
                    })?
                    .map_err(CamelError::from)?;

                    write_body_with_charset(body, &config.charset, &mut file, config.write_timeout)
                        .await?;
                    file.flush().await.map_err(CamelError::from)?;
                }
                FileExistStrategy::Ignore if target_path.exists() => return Ok(exchange),
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

                    write_body_with_charset(body, &config.charset, &mut file, config.write_timeout)
                        .await?;

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

                    let copy_result = write_body_with_charset(
                        body,
                        &config.charset,
                        &mut file,
                        config.write_timeout,
                    )
                    .await;

                    // Flush any kernel buffers (best-effort; actual write errors come from io::copy above)
                    let _ = file.flush().await;

                    copy_result?;

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

            if let Some(done_pattern) = &config.done_file_name {
                let done_name = done_pattern.replace("${file:name}", &file_name);
                tokio::time::timeout(
                    config.write_timeout,
                    fs::write(dir_path.join(done_name), []),
                )
                .await
                .map_err(|_| CamelError::ProcessorError("Timeout creating done file".into()))?
                .map_err(CamelError::from)?;
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

async fn write_body_with_charset(
    body: Body,
    charset: &Option<String>,
    file: &mut fs::File,
    timeout: Duration,
) -> Result<(), CamelError> {
    match body {
        Body::Text(text) | Body::Xml(text) => {
            let bytes = encode_text_by_charset(&text, charset)?;
            tokio::time::timeout(timeout, file.write_all(&bytes))
                .await
                .map_err(|_| CamelError::ProcessorError("Timeout writing file".into()))?
                .map_err(CamelError::from)?;
            Ok(())
        }
        other => {
            let mut reader = other.into_async_read()?;
            tokio::time::timeout(timeout, io::copy(&mut reader, file))
                .await
                .map_err(|_| CamelError::ProcessorError("Timeout writing to file".into()))?
                .map_err(|e| CamelError::ProcessorError(e.to_string()))?;
            Ok(())
        }
    }
}

fn encode_text_by_charset(text: &str, charset: &Option<String>) -> Result<Vec<u8>, CamelError> {
    let Some(charset) = charset.as_ref() else {
        return Ok(text.as_bytes().to_vec());
    };

    if charset.eq_ignore_ascii_case("utf-8") {
        return Ok(text.as_bytes().to_vec());
    }

    if charset.eq_ignore_ascii_case("iso-8859-1") {
        let mut out = Vec::with_capacity(text.len());
        for ch in text.chars() {
            let code = ch as u32;
            if code > 0xFF {
                return Err(CamelError::ProcessorError(format!(
                    "character '{ch}' cannot be encoded as ISO-8859-1"
                )));
            }
            out.push(code as u8);
        }
        return Ok(out);
    }

    Err(CamelError::Config(format!(
        "unsupported charset '{charset}', supported: UTF-8, ISO-8859-1"
    )))
}

#[cfg(test)]
mod tests {
    use camel_component_api::test_support::PanicRuntimeObservability;
    fn rt() -> std::sync::Arc<dyn camel_component_api::RuntimeObservability> {
        std::sync::Arc::new(PanicRuntimeObservability)
    }

    use super::*;
    use crate::poll_logic::{ModificationDetectingStream, list_files};
    use bytes::Bytes;
    use camel_component_api::{Message, NoOpComponentContext, StreamBody, StreamMetadata};
    use futures::StreamExt;
    use std::time::Duration;
    use tokio_util::io::ReaderStream;
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
        let ctx = NoOpComponentContext;
        let endpoint = component.create_endpoint("file:/tmp/test", &ctx);
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
        let ctx = NoOpComponentContext;
        let endpoint = component
            .create_endpoint(
                &format!("file:{dir_path}?noop=true&initialDelay=0&delay=100"),
                &ctx,
            )
            .unwrap();
        let mut consumer = endpoint.create_consumer(rt()).unwrap();

        let (tx, mut rx) = tokio::sync::mpsc::channel(16);
        let token = CancellationToken::new();
        let ctx = ConsumerContext::new(tx, token.clone(), "file-test-route".to_string());

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
        let ctx = ConsumerContext::new(tx, token, "file-test-route".to_string());

        let include_re = None;
        let exclude_re = None;
        let mut seen = std::collections::HashSet::new();
        let in_process_locks = std::sync::Arc::new(DashMap::new());
        let idempotent_repo = std::sync::Arc::new(tokio::sync::Mutex::new(HashSet::new()));

        poll_directory(
            &config,
            &ctx,
            &include_re,
            &exclude_re,
            &mut seen,
            &in_process_locks,
            &idempotent_repo,
        )
        .await
        .unwrap();
        assert!(rx.try_recv().is_ok(), "first poll should emit file");
        assert!(rx.try_recv().is_err(), "should only emit once");

        poll_directory(
            &config,
            &ctx,
            &include_re,
            &exclude_re,
            &mut seen,
            &in_process_locks,
            &idempotent_repo,
        )
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
        let ctx = ConsumerContext::new(tx, token, "file-test-route".to_string());

        let include_re = None;
        let exclude_re = None;
        let mut seen = std::collections::HashSet::new();
        let in_process_locks = std::sync::Arc::new(DashMap::new());
        let idempotent_repo = std::sync::Arc::new(tokio::sync::Mutex::new(HashSet::new()));

        poll_directory(
            &config,
            &ctx,
            &include_re,
            &exclude_re,
            &mut seen,
            &in_process_locks,
            &idempotent_repo,
        )
        .await
        .unwrap();
        let _ = rx.try_recv();

        let file2 = dir.path().join("b.txt");
        tokio::fs::write(&file2, b"b").await.unwrap();

        poll_directory(
            &config,
            &ctx,
            &include_re,
            &exclude_re,
            &mut seen,
            &in_process_locks,
            &idempotent_repo,
        )
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
        let ctx = NoOpComponentContext;
        let endpoint = component
            .create_endpoint(
                &format!("file:{dir_path}?noop=true&initialDelay=0&delay=100&include=.*\\.csv"),
                &ctx,
            )
            .unwrap();
        let mut consumer = endpoint.create_consumer(rt()).unwrap();

        let (tx, mut rx) = tokio::sync::mpsc::channel(16);
        let token = CancellationToken::new();
        let ctx = ConsumerContext::new(tx, token.clone(), "file-test-route".to_string());

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
        let ctx = NoOpComponentContext;
        let endpoint = component
            .create_endpoint(
                &format!("file:{dir_path}?delete=true&initialDelay=0&delay=100"),
                &ctx,
            )
            .unwrap();
        let mut consumer = endpoint.create_consumer(rt()).unwrap();

        let (tx, mut rx) = tokio::sync::mpsc::channel(16);
        let token = CancellationToken::new();
        let ctx = ConsumerContext::new(tx, token.clone(), "file-test-route".to_string());

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
        tokio::fs::write(dir.path().join("moveme.txt"), b"data")
            .await
            .unwrap();

        let uri = format!("file:{}?initialDelay=0&delay=50", dir.path().display());
        let config = FileConfig::from_uri(&uri).unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::channel(16);
        let token = CancellationToken::new();
        let ctx = ConsumerContext::new(tx, token, "file-test-route".to_string());

        let include_re = None;
        let exclude_re = None;
        let mut seen = std::collections::HashSet::new();
        let in_process_locks = std::sync::Arc::new(DashMap::new());
        let idempotent_repo = std::sync::Arc::new(tokio::sync::Mutex::new(HashSet::new()));

        poll_directory(
            &config,
            &ctx,
            &include_re,
            &exclude_re,
            &mut seen,
            &in_process_locks,
            &idempotent_repo,
        )
        .await
        .unwrap();

        let ex = rx.try_recv().expect("should receive exchange");
        drop(ex);

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
        let ctx = NoOpComponentContext;
        let endpoint = component
            .create_endpoint(&format!("file:{dir_path}?initialDelay=0&delay=50"), &ctx)
            .unwrap();
        let mut consumer = endpoint.create_consumer(rt()).unwrap();

        let (tx, _rx) = tokio::sync::mpsc::channel(16);
        let token = CancellationToken::new();
        let ctx = ConsumerContext::new(tx, token.clone(), "file-test-route".to_string());

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
        let ctx = NoOpComponentContext;
        let endpoint = component
            .create_endpoint(&format!("file:{dir_path}"), &ctx)
            .unwrap();
        let ctx = test_producer_ctx();
        let producer = endpoint.create_producer(rt(), &ctx).unwrap();

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
        let ctx = NoOpComponentContext;
        let endpoint = component
            .create_endpoint(&format!("file:{dir_path}/sub/dir"), &ctx)
            .unwrap();
        let ctx = test_producer_ctx();
        let producer = endpoint.create_producer(rt(), &ctx).unwrap();

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
        let ctx = NoOpComponentContext;
        let endpoint = component
            .create_endpoint(&format!("file:{dir_path}?fileExist=Fail"), &ctx)
            .unwrap();
        let ctx = test_producer_ctx();
        let producer = endpoint.create_producer(rt(), &ctx).unwrap();

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
        let ctx = NoOpComponentContext;
        let endpoint = component
            .create_endpoint(&format!("file:{dir_path}?fileExist=Append"), &ctx)
            .unwrap();
        let ctx = test_producer_ctx();
        let producer = endpoint.create_producer(rt(), &ctx).unwrap();

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
        let ctx = NoOpComponentContext;
        let endpoint = component
            .create_endpoint(&format!("file:{dir_path}?tempPrefix=.tmp"), &ctx)
            .unwrap();
        let ctx = test_producer_ctx();
        let producer = endpoint.create_producer(rt(), &ctx).unwrap();

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
        let ctx = NoOpComponentContext;
        let endpoint = component
            .create_endpoint(&format!("file:{dir_path}?fileName=fixed.txt"), &ctx)
            .unwrap();
        let ctx = test_producer_ctx();
        let producer = endpoint.create_producer(rt(), &ctx).unwrap();

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
        let ctx = NoOpComponentContext;
        let endpoint = component
            .create_endpoint(&format!("file:{dir_path}"), &ctx)
            .unwrap();
        let ctx = test_producer_ctx();
        let producer = endpoint.create_producer(rt(), &ctx).unwrap();

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
        let ctx = NoOpComponentContext;
        let endpoint = component
            .create_endpoint(&format!("file:{dir_path}/subdir"), &ctx)
            .unwrap();
        let ctx = test_producer_ctx();
        let producer = endpoint.create_producer(rt(), &ctx).unwrap();

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
        let ctx = NoOpComponentContext;
        let endpoint = component
            .create_endpoint(&format!("file:{dir_path}"), &ctx)
            .unwrap();
        let ctx = test_producer_ctx();
        let producer = endpoint.create_producer(rt(), &ctx).unwrap();

        let mut exchange = Exchange::new(Message::new("malicious"));
        exchange.input.set_header(
            "CamelFileName",
            serde_json::Value::String("/etc/passwd".to_string()),
        );

        let result = producer.oneshot(exchange).await;
        assert!(result.is_err(), "Should reject absolute path outside base");
    }

    #[tokio::test]
    async fn test_file_producer_does_not_create_dirs_before_path_validation() {
        use tower::ServiceExt;

        let dir = tempfile::tempdir().unwrap();
        let dir_path = dir.path().to_str().unwrap();

        let component = FileComponent::new();
        let ctx = NoOpComponentContext;
        let endpoint = component
            .create_endpoint(&format!("file:{dir_path}"), &ctx)
            .unwrap();
        let ctx = test_producer_ctx();
        let producer = endpoint.create_producer(rt(), &ctx).unwrap();

        let outside_parent = dir.path().parent().unwrap().join("escaped-create-dir");
        if outside_parent.exists() {
            std::fs::remove_dir_all(&outside_parent).unwrap();
        }

        let mut exchange = Exchange::new(Message::new("malicious"));
        exchange.input.set_header(
            "CamelFileName",
            serde_json::Value::String("../escaped-create-dir/file.txt".to_string()),
        );

        let result = producer.oneshot(exchange).await;
        assert!(result.is_err(), "Should reject traversal path");
        assert!(
            !outside_parent.exists(),
            "Must not create outside directories before path validation"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_list_files_skips_symlink_cycle() {
        use std::os::unix::fs as unix_fs;

        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("nested");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("a.txt"), "x").unwrap();
        unix_fs::symlink(dir.path(), nested.join("loop")).unwrap();

        let files = list_files(dir.path(), true).await.unwrap();
        assert_eq!(files.iter().filter(|p| p.ends_with("a.txt")).count(), 1);
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
        let component_ctx = NoOpComponentContext;
        let endpoint = component
            .create_endpoint(
                &format!("file:{dir_path}?noop=true&initialDelay=0&delay=100&fileName={file_name}"),
                &component_ctx,
            )
            .unwrap();
        let mut consumer = endpoint.create_consumer(rt()).unwrap();

        let (tx, mut rx) = tokio::sync::mpsc::channel(16);
        let token = CancellationToken::new();
        let ctx = ConsumerContext::new(tx, token.clone(), "file-test-route".to_string());

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
            .create_endpoint(
                &format!("file:{dir_path}?noop=true&initialDelay=0&delay=100&fileName={file_name}"),
                &component_ctx,
            )
            .unwrap();
        let mut consumer2 = endpoint2.create_consumer(rt()).unwrap();

        let (tx2, mut rx2) = tokio::sync::mpsc::channel(16);
        let token2 = CancellationToken::new();
        let ctx2 = ConsumerContext::new(tx2, token2.clone(), "file-test-route-2".to_string());

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
        let ctx = NoOpComponentContext;
        let endpoint = component.create_endpoint(&uri, &ctx).unwrap();
        let producer = endpoint
            .create_producer(rt(), &test_producer_ctx())
            .unwrap();

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
        let ctx = NoOpComponentContext;
        let endpoint = component.create_endpoint(&uri, &ctx).unwrap();
        let producer = endpoint
            .create_producer(rt(), &test_producer_ctx())
            .unwrap();

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
        let ctx = NoOpComponentContext;
        let endpoint = component.create_endpoint(&uri, &ctx).unwrap();
        let producer = endpoint
            .create_producer(rt(), &test_producer_ctx())
            .unwrap();

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
        let ctx = NoOpComponentContext;
        let endpoint = component.create_endpoint(&uri, &ctx).unwrap();
        let producer = endpoint
            .create_producer(rt(), &test_producer_ctx())
            .unwrap();

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
        let ctx = NoOpComponentContext;
        let endpoint = component.create_endpoint(&uri, &ctx).unwrap();
        let producer = endpoint
            .create_producer(rt(), &test_producer_ctx())
            .unwrap();

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
        let ctx = NoOpComponentContext;
        // URI uses no explicit delay/timeout params → macro defaults apply
        let endpoint = component.create_endpoint("file:/tmp/inbox", &ctx).unwrap();
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

    #[tokio::test]
    async fn test_file_producer_filename_simple_language_from_header() {
        use tower::ServiceExt;

        let dir = tempfile::tempdir().unwrap();
        let dir_path = dir.path().to_str().unwrap();

        let component = FileComponent::new();
        let ctx = NoOpComponentContext;
        let endpoint = component
            .create_endpoint(&format!("file:{dir_path}"), &ctx)
            .unwrap();
        let ctx = test_producer_ctx();
        let producer = endpoint.create_producer(rt(), &ctx).unwrap();

        let mut exchange = Exchange::new(Message::new("content"));
        exchange
            .input
            .set_header("CamelTimerCounter", serde_json::Value::Number(42.into()));
        exchange.input.set_header(
            "CamelFileName",
            serde_json::Value::String("test-${header.CamelTimerCounter}.txt".to_string()),
        );

        producer.oneshot(exchange).await.unwrap();

        assert!(
            dir.path().join("test-42.txt").exists(),
            "fileName should have been evaluated from Simple Language expression"
        );
        let content = std::fs::read_to_string(dir.path().join("test-42.txt")).unwrap();
        assert_eq!(content, "content");
    }

    #[tokio::test]
    async fn test_file_producer_filename_simple_language_from_uri_param() {
        use tower::ServiceExt;

        let dir = tempfile::tempdir().unwrap();
        let dir_path = dir.path().to_str().unwrap();

        let component = FileComponent::new();
        let ctx = NoOpComponentContext;
        let endpoint = component
            .create_endpoint(
                &format!("file:{dir_path}?fileName=msg-${{header.id}}.dat"),
                &ctx,
            )
            .unwrap();
        let ctx = test_producer_ctx();
        let producer = endpoint.create_producer(rt(), &ctx).unwrap();

        let mut exchange = Exchange::new(Message::new("data"));
        exchange
            .input
            .set_header("id", serde_json::Value::String("abc".to_string()));

        producer.oneshot(exchange).await.unwrap();

        assert!(
            dir.path().join("msg-abc.dat").exists(),
            "fileName URI param should have been evaluated from Simple Language expression"
        );
    }

    #[tokio::test]
    async fn test_file_producer_filename_literal_without_expression() {
        use tower::ServiceExt;

        let dir = tempfile::tempdir().unwrap();
        let dir_path = dir.path().to_str().unwrap();

        let component = FileComponent::new();
        let ctx = NoOpComponentContext;
        let endpoint = component
            .create_endpoint(&format!("file:{dir_path}?fileName=plain.txt"), &ctx)
            .unwrap();
        let ctx = test_producer_ctx();
        let producer = endpoint.create_producer(rt(), &ctx).unwrap();

        let exchange = Exchange::new(Message::new("data"));
        producer.oneshot(exchange).await.unwrap();

        assert!(
            dir.path().join("plain.txt").exists(),
            "literal fileName without expressions should still work"
        );
    }

    // -----------------------------------------------------------------------
    // Config validation tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_rejects_path_traversal_in_move_to() {
        let result = FileConfig::from_uri("file:/tmp/inbox?move=../etc/passwd");
        assert!(result.is_err(), "should reject path traversal in move_to");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("path traversal"),
            "error should mention path traversal: {err}"
        );
    }

    #[test]
    fn test_rejects_absolute_move_to() {
        let result = FileConfig::from_uri("file:/tmp/inbox?move=/tmp/outside");
        assert!(result.is_err(), "should reject absolute move_to");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("relative path") || err.contains("Invalid URI"),
            "error should mention invalid move_to path: {err}"
        );
    }

    #[test]
    fn test_rejects_path_traversal_in_temp_prefix() {
        let result = FileConfig::from_uri("file:/tmp/inbox?tempPrefix=../tmp");
        assert!(
            result.is_err(),
            "should reject path traversal in temp_prefix"
        );
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("path traversal"),
            "error should mention path traversal: {err}"
        );
    }

    #[test]
    fn test_rejects_temp_prefix_with_path_separator() {
        let result = FileConfig::from_uri("file:/tmp/inbox?tempPrefix=tmp/sub");
        assert!(result.is_err(), "should reject temp_prefix with separator");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("plain filename prefix"),
            "error should mention plain filename prefix restriction: {err}"
        );
    }

    #[test]
    fn test_rejects_absolute_temp_prefix() {
        let result = FileConfig::from_uri("file:/tmp/inbox?tempPrefix=/tmp/");
        assert!(result.is_err(), "should reject absolute temp_prefix");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("plain filename prefix"),
            "error should mention plain filename prefix restriction: {err}"
        );
    }

    #[test]
    fn test_rejects_null_byte_in_temp_prefix() {
        let config = FileConfig {
            directory: "/tmp/inbox".into(),
            delay: Duration::from_millis(500),
            delay_ms: 500,
            initial_delay: Duration::from_millis(1000),
            initial_delay_ms: 1000,
            noop: false,
            delete: false,
            move_to: None,
            file_name: Some("ok.txt".into()),
            include: None,
            exclude: None,
            recursive: false,
            file_exist: FileExistStrategy::Override,
            read_lock_strategy: ReadLockStrategy::None,
            idempotent_key: IdempotentKey::None,
            done_file_name: None,
            charset: None,
            temp_prefix: Some("tmp\0".into()),
            auto_create: true,
            starting_directory_must_exist: false,
            read_timeout: Duration::from_millis(30_000),
            read_timeout_ms: 30_000,
            write_timeout: Duration::from_millis(30_000),
            write_timeout_ms: 30_000,
        };

        let result = config.validate();
        assert!(result.is_err(), "should reject null byte in temp_prefix");
    }

    #[test]
    fn test_rejects_null_byte_in_filename() {
        // Null bytes in URI params are typically URL-encoded, so we test via
        // direct config construction to simulate the validation logic.
        let config = FileConfig {
            directory: "/tmp/inbox".into(),
            delay: Duration::from_millis(500),
            delay_ms: 500,
            initial_delay: Duration::from_millis(1000),
            initial_delay_ms: 1000,
            noop: false,
            delete: false,
            move_to: None,
            file_name: Some("foo\0bar".into()),
            include: None,
            exclude: None,
            recursive: false,
            file_exist: FileExistStrategy::Override,
            read_lock_strategy: ReadLockStrategy::None,
            idempotent_key: IdempotentKey::None,
            done_file_name: None,
            charset: None,
            temp_prefix: None,
            auto_create: true,
            starting_directory_must_exist: false,
            read_timeout: Duration::from_millis(30_000),
            read_timeout_ms: 30_000,
            write_timeout: Duration::from_millis(30_000),
            write_timeout_ms: 30_000,
        };
        let result = config.validate();
        assert!(result.is_err(), "should reject null byte in filename");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("null"),
            "error should mention null bytes: {err}"
        );
    }

    #[test]
    fn test_rejects_empty_filename() {
        let config = FileConfig {
            directory: "/tmp/inbox".into(),
            delay: Duration::from_millis(500),
            delay_ms: 500,
            initial_delay: Duration::from_millis(1000),
            initial_delay_ms: 1000,
            noop: false,
            delete: false,
            move_to: None,
            file_name: Some("".into()),
            include: None,
            exclude: None,
            recursive: false,
            file_exist: FileExistStrategy::Override,
            read_lock_strategy: ReadLockStrategy::None,
            idempotent_key: IdempotentKey::None,
            done_file_name: None,
            charset: None,
            temp_prefix: None,
            auto_create: true,
            starting_directory_must_exist: false,
            read_timeout: Duration::from_millis(30_000),
            read_timeout_ms: 30_000,
            write_timeout: Duration::from_millis(30_000),
            write_timeout_ms: 30_000,
        };
        let result = config.validate();
        assert!(result.is_err(), "should reject empty filename");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("empty") || err.contains("must not"),
            "error should mention empty: {err}"
        );
    }

    #[test]
    fn test_rejects_nonexistent_directory_when_starting_directory_must_exist() {
        let result =
            FileConfig::from_uri("file:/tmp/nonexistent_dir_12345?startingDirectoryMustExist=true");
        assert!(
            result.is_err(),
            "should reject non-existent directory when startingDirectoryMustExist=true"
        );
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("does not exist"),
            "error should mention directory does not exist: {err}"
        );
    }

    #[test]
    fn test_accepts_existing_directory_when_starting_directory_must_exist() {
        let dir = tempfile::tempdir().unwrap();
        let dir_path = dir.path().to_str().unwrap();
        let result =
            FileConfig::from_uri(&format!("file:{dir_path}?startingDirectoryMustExist=true"));
        assert!(
            result.is_ok(),
            "should accept existing directory when startingDirectoryMustExist=true: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_valid_config_passes() {
        let cfg = FileConfig::from_uri("file:/tmp/inbox").unwrap();
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn test_file_exist_strategy_rejects_unknown() {
        // Unknown strategy values should be rejected by from_str
        let result = FileExistStrategy::from_str("BogusValue");
        assert!(
            result.is_err(),
            "unknown FileExistStrategy should return Err"
        );
    }

    #[test]
    fn test_path_contains_traversal_detects_parent_dir() {
        assert!(path_contains_traversal("../etc/passwd"));
        assert!(path_contains_traversal("foo/../bar"));
        assert!(path_contains_traversal(".."));
        assert!(path_contains_traversal("a/b/../../../c"));
    }

    #[test]
    fn test_path_contains_traversal_accepts_safe_paths() {
        assert!(!path_contains_traversal("safe/path"));
        assert!(!path_contains_traversal("/absolute/path"));
        assert!(!path_contains_traversal("filename.txt"));
        assert!(!path_contains_traversal(".hidden"));
        assert!(!path_contains_traversal(""));
    }

    // -----------------------------------------------------------------------
    // File modification detection tests (C-20)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_stream_detects_file_modified_during_read() {
        use futures::StreamExt;

        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("mutable.txt");
        std::fs::write(&file_path, b"initial content here").unwrap();

        // Capture initial metadata
        let initial_meta = std::fs::metadata(&file_path).unwrap();
        let initial_size = initial_meta.len();
        let initial_mtime = initial_meta.modified().ok();

        // Create a raw file stream (simulating what the consumer does)
        let file = tokio::fs::File::open(&file_path).await.unwrap();
        let raw_stream = ReaderStream::new(file).map(|res| res.map_err(CamelError::from));

        let mut stream = ModificationDetectingStream::new(
            raw_stream,
            file_path.clone(),
            initial_size,
            initial_mtime,
        );

        // Read a chunk (file not modified yet — should succeed)
        let chunk = stream.next().await;
        assert!(chunk.is_some(), "should produce at least one chunk");
        assert!(chunk.as_ref().unwrap().is_ok(), "first chunk should be Ok");

        // Modify the file (change content/size)
        std::fs::write(&file_path, b"modified content - different size!!").unwrap();

        // Drain remaining chunks — the stream should produce an error after EOF
        let mut got_error = false;
        while let Some(result) = stream.next().await {
            if result.is_err() {
                let err = result.unwrap_err();
                let msg = err.to_string();
                assert!(
                    msg.contains("file modified during read"),
                    "expected modification error, got: {msg}"
                );
                got_error = true;
                break;
            }
        }

        assert!(
            got_error,
            "stream should detect file was modified during read"
        );
    }

    #[tokio::test]
    async fn test_stream_succeeds_when_file_not_modified() {
        use futures::StreamExt;

        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("stable.txt");
        std::fs::write(&file_path, b"stable content").unwrap();

        let initial_meta = std::fs::metadata(&file_path).unwrap();
        let initial_size = initial_meta.len();
        let initial_mtime = initial_meta.modified().ok();

        let file = tokio::fs::File::open(&file_path).await.unwrap();
        let raw_stream = ReaderStream::new(file).map(|res| res.map_err(CamelError::from));

        let mut stream = ModificationDetectingStream::new(
            raw_stream,
            file_path.clone(),
            initial_size,
            initial_mtime,
        );

        // Drain entire stream — should produce no errors
        let mut all_ok = true;
        while let Some(result) = stream.next().await {
            if result.is_err() {
                all_ok = false;
                break;
            }
        }

        assert!(
            all_ok,
            "stream should complete without errors when file is not modified"
        );
    }
}
