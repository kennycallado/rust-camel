use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use async_trait::async_trait;
use tower::Service;

use camel_api::{BoxProcessor, CamelError, Exchange};
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
// FileConsumer (stub)
// ---------------------------------------------------------------------------

struct FileConsumer {
    config: FileConfig,
}

#[async_trait]
impl Consumer for FileConsumer {
    async fn start(&mut self, _context: ConsumerContext) -> Result<(), CamelError> {
        todo!("File consumer not yet implemented")
    }

    async fn stop(&mut self) -> Result<(), CamelError> {
        Ok(())
    }
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
}
