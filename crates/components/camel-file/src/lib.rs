use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use async_trait::async_trait;
use regex::Regex;
use tokio::fs;
use tokio::time;
use tower::Service;
use tracing::{debug, warn};

use camel_api::{BoxProcessor, CamelError, Exchange, Message, body::Body};
use camel_component::{Component, Consumer, ConsumerContext, Endpoint, ProducerContext};
use camel_endpoint::parse_uri;

// ---------------------------------------------------------------------------
// FileExistStrategy
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileExistStrategy {
    Override,
    Append,
    Fail,
}

impl FileExistStrategy {
    fn from_str(s: &str) -> Self {
        match s {
            "Append" | "append" => FileExistStrategy::Append,
            "Fail" | "fail" => FileExistStrategy::Fail,
            _ => FileExistStrategy::Override,
        }
    }
}

// ---------------------------------------------------------------------------
// FileConfig
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct FileConfig {
    pub directory: String,
    pub delay: Duration,
    pub initial_delay: Duration,
    pub noop: bool,
    pub delete: bool,
    pub move_to: Option<String>,
    pub file_name: Option<String>,
    pub include: Option<String>,
    pub exclude: Option<String>,
    pub recursive: bool,
    pub file_exist: FileExistStrategy,
    pub temp_prefix: Option<String>,
    pub auto_create: bool,
    // Timeout fields for preventing hanging on slow filesystems
    pub read_timeout: Duration,
    pub write_timeout: Duration,
}

impl FileConfig {
    pub fn from_uri(uri: &str) -> Result<Self, CamelError> {
        let parts = parse_uri(uri)?;
        if parts.scheme != "file" {
            return Err(CamelError::InvalidUri(format!(
                "expected scheme 'file', got '{}'",
                parts.scheme
            )));
        }

        let delay = parts
            .params
            .get("delay")
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(500);

        let initial_delay = parts
            .params
            .get("initialDelay")
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(1000);

        let noop = parts
            .params
            .get("noop")
            .map(|v| v == "true")
            .unwrap_or(false);

        let delete = parts
            .params
            .get("delete")
            .map(|v| v == "true")
            .unwrap_or(false);

        let move_to = if noop || delete {
            None
        } else {
            Some(
                parts
                    .params
                    .get("move")
                    .cloned()
                    .unwrap_or_else(|| ".camel".to_string()),
            )
        };

        let file_name = parts.params.get("fileName").cloned();
        let include = parts.params.get("include").cloned();
        let exclude = parts.params.get("exclude").cloned();

        let recursive = parts
            .params
            .get("recursive")
            .map(|v| v == "true")
            .unwrap_or(false);

        let file_exist = parts
            .params
            .get("fileExist")
            .map(|v| FileExistStrategy::from_str(v))
            .unwrap_or(FileExistStrategy::Override);

        let temp_prefix = parts.params.get("tempPrefix").cloned();

        let auto_create = parts
            .params
            .get("autoCreate")
            .map(|v| v != "false")
            .unwrap_or(true);

        let read_timeout = parts
            .params
            .get("readTimeout")
            .and_then(|v| v.parse::<u64>().ok())
            .map(Duration::from_millis)
            .unwrap_or(Duration::from_secs(30));

        let write_timeout = parts
            .params
            .get("writeTimeout")
            .and_then(|v| v.parse::<u64>().ok())
            .map(Duration::from_millis)
            .unwrap_or(Duration::from_secs(30));

        Ok(Self {
            directory: parts.path,
            delay: Duration::from_millis(delay),
            initial_delay: Duration::from_millis(initial_delay),
            noop,
            delete,
            move_to,
            file_name,
            include,
            exclude,
            recursive,
            file_exist,
            temp_prefix,
            auto_create,
            read_timeout,
            write_timeout,
        })
    }
}

// ---------------------------------------------------------------------------
// FileComponent
// ---------------------------------------------------------------------------

pub struct FileComponent;

impl FileComponent {
    pub fn new() -> Self {
        Self
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
        let config = FileConfig::from_uri(uri)?;
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

        let content = match tokio::time::timeout(config.read_timeout, fs::read(&file_path)).await {
            Ok(Ok(c)) => c,
            Ok(Err(e)) => {
                warn!(
                    file = %file_path.display(),
                    error = %e,
                    "Failed to read file"
                );
                continue;
            }
            Err(_) => {
                warn!(
                    file = %file_path.display(),
                    timeout_ms = config.read_timeout.as_millis(),
                    "Timeout reading file"
                );
                continue;
            }
        };

        let metadata = fs::metadata(&file_path).await.ok();
        let file_len = metadata.as_ref().map(|m| m.len()).unwrap_or(0);
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

        let mut exchange = Exchange::new(Message::new(Body::Bytes(bytes::Bytes::from(content))));
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
    fn body_to_bytes(body: &Body) -> Result<Vec<u8>, CamelError> {
        match body {
            Body::Empty => Err(CamelError::ProcessorError(
                "Cannot write empty body to file".to_string(),
            )),
            Body::Bytes(b) => Ok(b.to_vec()),
            Body::Text(s) => Ok(s.as_bytes().to_vec()),
            Body::Json(v) => Ok(v.to_string().into_bytes()),
        }
    }

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
            let data = FileProducer::body_to_bytes(&exchange.input.body)?;

            let dir_path = std::path::Path::new(&config.directory);
            let target_path = dir_path.join(&file_name);

            if config.auto_create
                && let Some(parent) = target_path.parent()
            {
                tokio::time::timeout(config.write_timeout, fs::create_dir_all(parent))
                    .await
                    .map_err(|_| CamelError::ProcessorError("Timeout creating directories".into()))?
                    .map_err(CamelError::from)?;
            }

            // SECURITY: Validate path is within base directory
            validate_path_is_within_base(dir_path, &target_path)?;

            if target_path.exists() {
                match config.file_exist {
                    FileExistStrategy::Fail => {
                        return Err(CamelError::ProcessorError(format!(
                            "File already exists: {}",
                            target_path.display()
                        )));
                    }
                    FileExistStrategy::Append => {
                        use tokio::io::AsyncWriteExt;
                        let mut file = tokio::time::timeout(
                            config.write_timeout,
                            fs::OpenOptions::new().append(true).open(&target_path),
                        )
                        .await
                        .map_err(|_| {
                            CamelError::ProcessorError("Timeout opening file for append".into())
                        })?
                        .map_err(CamelError::from)?;

                        tokio::time::timeout(config.write_timeout, async {
                            file.write_all(&data).await?;
                            file.flush().await?;
                            Ok::<_, std::io::Error>(())
                        })
                        .await
                        .map_err(|_| CamelError::ProcessorError("Timeout writing to file".into()))?
                        .map_err(CamelError::from)?;

                        let abs_path = target_path
                            .canonicalize()
                            .unwrap_or_else(|_| target_path.clone())
                            .to_string_lossy()
                            .to_string();
                        exchange.input.set_header(
                            "CamelFileNameProduced",
                            serde_json::Value::String(abs_path),
                        );
                        return Ok(exchange);
                    }
                    FileExistStrategy::Override => {}
                }
            }

            if let Some(ref prefix) = config.temp_prefix {
                let temp_name = format!("{prefix}{file_name}");
                let temp_path = dir_path.join(&temp_name);

                tokio::time::timeout(config.write_timeout, fs::write(&temp_path, &data))
                    .await
                    .map_err(|_| CamelError::ProcessorError("Timeout writing temp file".into()))?
                    .map_err(CamelError::from)?;

                tokio::time::timeout(config.write_timeout, fs::rename(&temp_path, &target_path))
                    .await
                    .map_err(|_| CamelError::ProcessorError("Timeout renaming file".into()))?
                    .map_err(CamelError::from)?;
            } else {
                tokio::time::timeout(config.write_timeout, fs::write(&target_path, &data))
                    .await
                    .map_err(|_| CamelError::ProcessorError("Timeout writing file".into()))?
                    .map_err(CamelError::from)?;
            }

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
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::sync::Mutex;
    use tokio_util::sync::CancellationToken;

    // NullRouteController for testing
    struct NullRouteController;

    #[async_trait::async_trait]
    impl camel_api::RouteController for NullRouteController {
        async fn start_route(&mut self, _: &str) -> Result<(), camel_api::CamelError> {
            Ok(())
        }
        async fn stop_route(&mut self, _: &str) -> Result<(), camel_api::CamelError> {
            Ok(())
        }
        async fn restart_route(&mut self, _: &str) -> Result<(), camel_api::CamelError> {
            Ok(())
        }
        async fn suspend_route(&mut self, _: &str) -> Result<(), camel_api::CamelError> {
            Ok(())
        }
        async fn resume_route(&mut self, _: &str) -> Result<(), camel_api::CamelError> {
            Ok(())
        }
        fn route_status(&self, _: &str) -> Option<camel_api::RouteStatus> {
            None
        }
        async fn start_all_routes(&mut self) -> Result<(), camel_api::CamelError> {
            Ok(())
        }
        async fn stop_all_routes(&mut self) -> Result<(), camel_api::CamelError> {
            Ok(())
        }
    }

    fn test_producer_ctx() -> ProducerContext {
        ProducerContext::new(Arc::new(Mutex::new(NullRouteController)))
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
}
