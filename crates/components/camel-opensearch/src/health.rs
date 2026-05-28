//! Async health check for OpenSearch using `cluster.health()` API.
//!
//! Probes the OpenSearch cluster by calling the cluster health endpoint.
//! Green or yellow status is considered healthy; red or any error is unhealthy.

use async_trait::async_trait;
use opensearch::OpenSearch;
use opensearch::auth::Credentials;
use opensearch::cluster::ClusterHealthParts;
use opensearch::http::transport::{SingleNodeConnectionPool, TransportBuilder};
use std::time::Duration;
use tracing::{debug, warn};

use crate::config::OpenSearchEndpointConfig;
use camel_api::{AsyncHealthCheck, CheckResult};
use camel_component_api::CamelError;

/// Per-probe timeout for the cluster health call.
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// OpenSearch health check that probes via `cluster().health()`.
pub struct OpenSearchHealthCheck {
    config: OpenSearchEndpointConfig,
}

impl OpenSearchHealthCheck {
    /// Create a new health check from the endpoint configuration.
    pub fn new(config: &OpenSearchEndpointConfig) -> Self {
        Self {
            config: config.clone(),
        }
    }

    /// Build an OpenSearch client from the endpoint configuration.
    pub(crate) fn build_client(&self) -> Result<OpenSearch, CamelError> {
        let url = self.config.base_url();
        let parsed_url = url::Url::parse(&url).map_err(|e| {
            CamelError::EndpointCreationFailed(format!("Invalid OpenSearch URL: {}", e))
        })?;
        let pool = SingleNodeConnectionPool::new(parsed_url);
        let mut builder = TransportBuilder::new(pool);
        if let (Some(username), Some(password)) = (&self.config.username, &self.config.password) {
            builder = builder.auth(Credentials::Basic(username.clone(), password.clone()));
        }
        let transport = builder.build().map_err(|e| {
            CamelError::EndpointCreationFailed(format!("Failed to build transport: {}", e))
        })?;
        Ok(OpenSearch::new(transport))
    }
}

#[async_trait]
impl AsyncHealthCheck for OpenSearchHealthCheck {
    fn name(&self) -> &str {
        "opensearch"
    }

    async fn check(&self) -> CheckResult {
        let client = match self.build_client() {
            Ok(c) => c,
            Err(e) => return CheckResult::unhealthy(self.name(), &e.to_string()),
        };
        let url = self.config.base_url();

        debug!(endpoint = %url, "probing OpenSearch cluster health");

        let result = tokio::time::timeout(
            PROBE_TIMEOUT,
            client.cluster().health(ClusterHealthParts::None).send(),
        )
        .await;

        match result {
            Ok(Ok(response)) => {
                let status = response.status_code().as_u16();
                if status >= 400 {
                    warn!(endpoint = %url, status, "OpenSearch cluster health returned error status");
                    return CheckResult::unhealthy(
                        self.name(),
                        &format!("OpenSearch cluster health returned HTTP {}", status),
                    );
                }

                let body = response.json::<serde_json::Value>().await.map_err(|e| {
                    CamelError::ProcessorError(format!(
                        "Failed to parse cluster health response: {}",
                        e
                    ))
                });

                let body = match body {
                    Ok(b) => b,
                    Err(e) => return CheckResult::unhealthy(self.name(), &e.to_string()),
                };

                let cluster_status = body
                    .get("status")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");

                match cluster_status {
                    "green" | "yellow" => {
                        debug!(endpoint = %url, status = cluster_status, "OpenSearch cluster is healthy");
                        CheckResult::healthy(self.name())
                    }
                    "red" => {
                        warn!(endpoint = %url, status = cluster_status, "OpenSearch cluster is red");
                        CheckResult::unhealthy(self.name(), "OpenSearch cluster status is red")
                    }
                    other => {
                        warn!(endpoint = %url, status = other, "OpenSearch cluster has unknown status");
                        CheckResult::unhealthy(
                            self.name(),
                            &format!("OpenSearch cluster has unknown status: {}", other),
                        )
                    }
                }
            }
            Ok(Err(e)) => {
                warn!(endpoint = %url, error = %e, "OpenSearch cluster health probe failed");
                CheckResult::unhealthy(
                    self.name(),
                    &format!("OpenSearch cluster health probe failed: {}", e),
                )
            }
            Err(_) => {
                warn!(endpoint = %url, "OpenSearch cluster health probe timed out");
                CheckResult::unhealthy(self.name(), "OpenSearch cluster health probe timed out")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_api::HealthStatus;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use tokio::sync::Mutex;

    /// Mock probe that returns a configurable result.
    struct MockHealthCheck {
        name: String,
        should_succeed: Arc<AtomicBool>,
        call_count: Arc<Mutex<usize>>,
    }

    impl MockHealthCheck {
        fn new(should_succeed: bool) -> (Self, Arc<AtomicBool>, Arc<Mutex<usize>>) {
            let flag = Arc::new(AtomicBool::new(should_succeed));
            let count = Arc::new(Mutex::new(0usize));
            let mock = Self {
                name: "mock".to_string(),
                should_succeed: Arc::clone(&flag),
                call_count: Arc::clone(&count),
            };
            (mock, flag, count)
        }
    }

    #[async_trait]
    impl AsyncHealthCheck for MockHealthCheck {
        fn name(&self) -> &str {
            &self.name
        }

        async fn check(&self) -> CheckResult {
            *self.call_count.lock().await += 1;
            if self.should_succeed.load(Ordering::SeqCst) {
                CheckResult::healthy(&self.name)
            } else {
                CheckResult::unhealthy(&self.name, "mock probe failed")
            }
        }
    }

    #[tokio::test]
    async fn test_mock_healthy_probe_succeeds() {
        let (mock, _flag, count) = MockHealthCheck::new(true);
        assert_eq!(mock.name(), "mock");
        let result = mock.check().await;
        assert_eq!(
            result.status,
            HealthStatus::Healthy,
            "healthy probe should succeed"
        );
        assert_eq!(*count.lock().await, 1);
    }

    #[tokio::test]
    async fn test_mock_unhealthy_probe_returns_error() {
        let (mock, _flag, count) = MockHealthCheck::new(false);
        let result = mock.check().await;
        assert_eq!(
            result.status,
            HealthStatus::Unhealthy,
            "unhealthy probe should return error"
        );
        assert_eq!(*count.lock().await, 1);
    }

    #[tokio::test]
    async fn test_mock_probe_timeout_simulation() {
        struct SlowHealthCheck;

        #[async_trait]
        impl AsyncHealthCheck for SlowHealthCheck {
            fn name(&self) -> &str {
                "slow"
            }

            async fn check(&self) -> CheckResult {
                tokio::time::sleep(Duration::from_secs(30)).await;
                CheckResult::healthy("slow")
            }
        }

        let slow = SlowHealthCheck;
        assert_eq!(slow.name(), "slow");

        let result = tokio::time::timeout(Duration::from_millis(50), slow.check()).await;
        assert!(result.is_err(), "slow probe should time out within 50ms");
    }

    #[test]
    fn test_opensearch_health_check_name() {
        let config =
            OpenSearchEndpointConfig::from_uri("opensearch://localhost:9200/myindex").unwrap();
        let check = OpenSearchHealthCheck::new(&config);
        assert_eq!(check.name(), "opensearch");
    }

    #[test]
    fn test_opensearch_health_check_build_client_valid_config() {
        let config = OpenSearchEndpointConfig::from_uri(
            "opensearch://localhost:9200/myindex?operation=INDEX",
        )
        .unwrap();
        let check = OpenSearchHealthCheck::new(&config);
        let result = check.build_client();
        assert!(result.is_ok(), "client should build from valid config");
    }

    #[test]
    fn test_opensearch_health_check_build_client_with_auth() {
        let config = OpenSearchEndpointConfig::from_uri(
            "opensearch://localhost:9200/myindex?username=admin&password=secret",
        )
        .unwrap();
        let check = OpenSearchHealthCheck::new(&config);
        let result = check.build_client();
        assert!(result.is_ok(), "client should build with auth config");
    }

    #[test]
    fn test_opensearch_health_check_build_client_with_tls() {
        let config =
            OpenSearchEndpointConfig::from_uri("opensearchs://localhost:443/myindex").unwrap();
        let check = OpenSearchHealthCheck::new(&config);
        let result = check.build_client();
        assert!(result.is_ok(), "client should build with TLS config");
    }

    #[tokio::test]
    async fn test_opensearch_probe_fails_without_live_server() {
        let config =
            OpenSearchEndpointConfig::from_uri("opensearch://localhost:9200/myindex").unwrap();
        let check = OpenSearchHealthCheck::new(&config);
        let result = check.check().await;
        assert_eq!(
            result.status,
            HealthStatus::Unhealthy,
            "probe should fail without live OpenSearch server"
        );
    }

    #[tokio::test]
    async fn test_opensearch_probe_timeout_on_unreachable_host() {
        let config =
            OpenSearchEndpointConfig::from_uri("opensearch://192.0.2.1:9200/myindex").unwrap();
        let check = OpenSearchHealthCheck::new(&config);

        let result = tokio::time::timeout(Duration::from_millis(200), check.check()).await;
        assert!(
            result.is_err()
                || result
                    .as_ref()
                    .is_ok_and(|r| r.status == HealthStatus::Unhealthy),
            "probe should fail or timeout on unreachable host"
        );
    }
}
