use async_trait::async_trait;
use camel_component_api::{AsyncHealthCheck, CamelError, CheckResult};
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::time::Duration;

type ProbeFuture = Pin<Box<dyn Future<Output = Result<(), CamelError>> + Send>>;

trait FileHealthProbe: Send + Sync {
    fn probe(&self) -> ProbeFuture;
}

struct DirectoryMetadataProbe {
    path: PathBuf,
    timeout: Duration,
}

impl DirectoryMetadataProbe {
    fn new(path: PathBuf, timeout: Duration) -> Self {
        Self { path, timeout }
    }
}

impl FileHealthProbe for DirectoryMetadataProbe {
    fn probe(&self) -> ProbeFuture {
        let path = self.path.clone();
        let timeout = self.timeout;
        Box::pin(async move {
            tokio::time::timeout(timeout, tokio::fs::metadata(&path))
                .await
                .map_err(|_| {
                    CamelError::ProcessorError(format!(
                        "health probe timed out for directory: {}",
                        path.display()
                    ))
                })?
                .map_err(|e| CamelError::ProcessorError(format!("directory metadata error: {}", e)))
                .map(|_| ())
        })
    }
}

/// Health check for the file component.
///
/// Probes that the target directory exists via `tokio::fs::metadata()`.
/// Reports `Healthy` when the directory is accessible.
/// Reports `Degraded` if the directory is missing or the probe times out.
pub struct FileHealthCheck {
    probe: Option<Box<dyn FileHealthProbe>>,
    timeout: Duration,
}

impl FileHealthCheck {
    pub fn new(path: PathBuf) -> Self {
        let timeout = Duration::from_secs(5);
        Self {
            probe: Some(Box::new(DirectoryMetadataProbe::new(path, timeout))),
            timeout,
        }
    }

    #[cfg(test)]
    fn with_probe_for_tests(probe: Box<dyn FileHealthProbe>, timeout: Duration) -> Self {
        Self {
            probe: Some(probe),
            timeout,
        }
    }
}

#[async_trait]
impl AsyncHealthCheck for FileHealthCheck {
    fn name(&self) -> &str {
        "file"
    }

    async fn check(&self) -> CheckResult {
        let Some(probe) = self.probe.as_ref() else {
            return CheckResult::healthy(self.name());
        };

        match tokio::time::timeout(self.timeout, probe.probe()).await {
            Ok(Ok(())) => CheckResult::healthy(self.name()),
            Ok(Err(err)) => CheckResult::degraded(self.name(), &err.to_string()),
            Err(_) => CheckResult::degraded(self.name(), "directory probe timed out"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_component_api::HealthStatus;
    use std::sync::Arc;

    struct MockProbe {
        responder: Arc<dyn Fn() -> ProbeFuture + Send + Sync>,
    }

    impl MockProbe {
        fn new<F>(f: F) -> Self
        where
            F: Fn() -> ProbeFuture + Send + Sync + 'static,
        {
            Self {
                responder: Arc::new(f),
            }
        }
    }

    impl FileHealthProbe for MockProbe {
        fn probe(&self) -> ProbeFuture {
            (self.responder)()
        }
    }

    #[tokio::test]
    async fn file_health_check_healthy_when_directory_exists() {
        let dir = tempfile::tempdir().unwrap();
        let check = FileHealthCheck::new(dir.path().to_path_buf());
        let result = check.check().await;
        assert_eq!(result.name, "file");
        assert_eq!(result.status, HealthStatus::Healthy);
        assert!(result.message.is_none());
    }

    #[tokio::test]
    async fn file_health_check_degraded_when_directory_missing() {
        let check = FileHealthCheck::new(PathBuf::from("/tmp/nonexistent_dir_12345"));
        let result = check.check().await;
        assert_eq!(result.name, "file");
        assert_eq!(result.status, HealthStatus::Degraded);
        assert!(
            result
                .message
                .as_deref()
                .is_some_and(|m| m.contains("directory metadata error"))
        );
    }

    #[tokio::test]
    async fn file_health_check_degraded_on_timeout() {
        let probe = MockProbe::new(|| {
            Box::pin(async {
                tokio::time::sleep(Duration::from_millis(100)).await;
                Ok(())
            })
        });
        let check =
            FileHealthCheck::with_probe_for_tests(Box::new(probe), Duration::from_millis(5));
        let result = check.check().await;
        assert_eq!(result.name, "file");
        assert_eq!(result.status, HealthStatus::Degraded);
        assert_eq!(result.message.as_deref(), Some("directory probe timed out"));
    }
}
