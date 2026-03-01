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
use camel_component::{Component, Consumer, ConsumerContext, Endpoint};
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

    fn create_producer(&self) -> Result<BoxProcessor, CamelError> {
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

        if let Some(ref target_name) = config.file_name {
            if file_name != *target_name {
                continue;
            }
        }

        if let Some(re) = include_re {
            if !re.is_match(&file_name) {
                continue;
            }
        }

        if let Some(re) = exclude_re {
            if re.is_match(&file_name) {
                continue;
            }
        }

        if let Some(ref move_dir) = config.move_to {
            if file_path.starts_with(base_path.join(move_dir)) {
                continue;
            }
        }

        let content = match fs::read(&file_path).await {
            Ok(c) => c,
            Err(e) => {
                warn!(file = %file_path.display(), error = %e, "Failed to read file");
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
        exchange.input.set_header("CamelFileName", serde_json::Value::String(relative_path));
        exchange.input.set_header("CamelFileNameOnly", serde_json::Value::String(file_name.clone()));
        exchange.input.set_header("CamelFileAbsolutePath", serde_json::Value::String(absolute_path));
        exchange.input.set_header("CamelFileLength", serde_json::Value::Number(file_len.into()));
        exchange.input.set_header("CamelFileLastModified", serde_json::Value::Number(last_modified.into()));

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
// FileProducer (stub)
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct FileProducer {
    config: FileConfig,
}

impl Service<Exchange> for FileProducer {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, _exchange: Exchange) -> Self::Future {
        Box::pin(async move {
            todo!("File producer not yet implemented")
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio_util::sync::CancellationToken;

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
            "file:/data/output?fileExist=Append&tempPrefix=.tmp&autoCreate=false&fileName=out.txt"
        ).unwrap();
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
            .create_endpoint(&format!("file:{dir_path}?noop=true&initialDelay=0&delay=100"))
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
        }).await;
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
            .create_endpoint(&format!("file:{dir_path}?noop=true&initialDelay=0&delay=100&include=.*\\.csv"))
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
        }).await;
        token.cancel();

        assert_eq!(received.len(), 1);
        let name = received[0].input.header("CamelFileNameOnly")
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
            .create_endpoint(&format!("file:{dir_path}?delete=true&initialDelay=0&delay=100"))
            .unwrap();
        let mut consumer = endpoint.create_consumer().unwrap();

        let (tx, mut rx) = tokio::sync::mpsc::channel(16);
        let token = CancellationToken::new();
        let ctx = ConsumerContext::new(tx, token.clone());

        tokio::spawn(async move {
            consumer.start(ctx).await.unwrap();
        });

        let _ = tokio::time::timeout(Duration::from_millis(500), async {
            rx.recv().await
        }).await;
        token.cancel();

        tokio::time::sleep(Duration::from_millis(100)).await;

        assert!(!dir.path().join("deleteme.txt").exists(), "File should be deleted");
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

        let _ = tokio::time::timeout(Duration::from_millis(500), async {
            rx.recv().await
        }).await;
        token.cancel();

        tokio::time::sleep(Duration::from_millis(100)).await;

        assert!(!dir.path().join("moveme.txt").exists(), "Original file should be gone");
        assert!(dir.path().join(".camel").join("moveme.txt").exists(), "File should be in .camel/");
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
        assert!(result.is_ok(), "Consumer should have stopped after cancellation");
    }
}
