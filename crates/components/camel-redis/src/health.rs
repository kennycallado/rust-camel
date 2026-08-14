use crate::RedisEndpointConfig;
use crate::topology::{RedisTopology, ServerKind};
use async_trait::async_trait;
use camel_api::{AsyncHealthCheck, CheckResult};
use camel_component_api::CamelError;
use redis::Client;
use redis::aio::MultiplexedConnection;
use std::sync::Arc;
use std::time::Duration;

/// Injectable seam that connects to a resolved Redis client and PINGs it.
///
/// Kept separate from [`RedisTopology`] so the health check can be tested
/// without a broker: a fake probe records the client it was handed and returns
/// a programmable outcome.
#[async_trait]
pub(crate) trait HealthProbe: Send + Sync {
    /// Open a multiplexed connection to `client` (bounded by `timeout`) and
    /// run `PING`. Returns `Ok(())` on a successful PONG, `Err` otherwise.
    async fn connect_and_ping(&self, client: &Client, timeout: Duration) -> Result<(), CamelError>;
}

/// Real [`HealthProbe`] that opens a connection and PINGs the resolved node.
struct RedisHealthProbe {
    endpoint: String,
}

impl RedisHealthProbe {
    fn new(config: &RedisEndpointConfig) -> Self {
        Self {
            endpoint: config.safe_endpoint(),
        }
    }
}

#[async_trait]
impl HealthProbe for RedisHealthProbe {
    async fn connect_and_ping(&self, client: &Client, timeout: Duration) -> Result<(), CamelError> {
        let mut conn: MultiplexedConnection =
            tokio::time::timeout(timeout, client.get_multiplexed_async_connection())
                .await
                .map_err(|_| {
                    CamelError::ProcessorError(format!(
                        "Health check connection to '{}' timed out",
                        self.endpoint
                    ))
                })?
                .map_err(|e| {
                    CamelError::ProcessorError(format!(
                        "Failed to connect to Redis for health check '{}': {}",
                        self.endpoint, e
                    ))
                })?;

        redis::cmd("PING")
            .query_async::<String>(&mut conn)
            .await
            .map_err(|e| {
                CamelError::ProcessorError(format!(
                    "Redis health check PING failed for '{}': {}",
                    self.endpoint, e
                ))
            })?;
        Ok(())
    }
}

pub struct RedisHealthCheck {
    topology: Arc<dyn RedisTopology>,
    probe: Arc<dyn HealthProbe>,
    inner_timeout: Duration,
    timeout: Duration,
}

impl RedisHealthCheck {
    pub fn new(config: &RedisEndpointConfig) -> Result<Self, CamelError> {
        let topology = crate::topology::topology_from_config(config)?;
        let probe = Arc::new(RedisHealthProbe::new(config));
        let inner_timeout = Duration::from_secs(config.connection_timeout_secs);
        // Outer timeout must exceed the inner connection timeout so the inner
        // timeout fires first and produces a specific error message.
        let timeout = Duration::from_secs(config.connection_timeout_secs + 5);
        Ok(Self {
            topology,
            probe,
            inner_timeout,
            timeout,
        })
    }

    #[cfg(test)]
    fn with_probe_for_tests(
        topology: Arc<dyn RedisTopology>,
        probe: Arc<dyn HealthProbe>,
        inner_timeout: Duration,
        timeout: Duration,
    ) -> Self {
        Self {
            topology,
            probe,
            inner_timeout,
            timeout,
        }
    }
}

#[async_trait]
impl AsyncHealthCheck for RedisHealthCheck {
    fn name(&self) -> &str {
        "redis"
    }

    async fn check(&self) -> CheckResult {
        // Resolve the current master first. On failure we report Unhealthy and
        // do NOT fall back to probing the (potentially stale) configured node.
        // The outer timeout wraps resolve + probe; it exceeds the inner
        // connection timeout so the inner fires first with a specific message.
        let outcome = tokio::time::timeout(self.timeout, async {
            let client = self.topology.resolve(ServerKind::Master).await?;
            self.probe
                .connect_and_ping(&client, self.inner_timeout)
                .await
        })
        .await;

        match outcome {
            Ok(Ok(())) => CheckResult::healthy(self.name()),
            Ok(Err(err)) => CheckResult::unhealthy(self.name(), &err.to_string()),
            Err(_) => CheckResult::unhealthy(self.name(), "health check timed out"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::topology::FakeTopology;
    use camel_api::HealthStatus;
    use std::sync::Mutex;

    /// Fake [`HealthProbe`] that records the client it was probed with and
    /// returns a programmable outcome, optionally after a delay.
    struct FakeHealthProbe {
        probed_client: Arc<Mutex<Option<Client>>>,
        result: Result<(), CamelError>,
        delay: Duration,
    }

    impl FakeHealthProbe {
        fn new(result: Result<(), CamelError>) -> Self {
            Self {
                probed_client: Arc::new(Mutex::new(None)),
                result,
                delay: Duration::ZERO,
            }
        }

        fn with_delay(mut self, delay: Duration) -> Self {
            self.delay = delay;
            self
        }

        fn probed_client(&self) -> Option<Client> {
            self.probed_client.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl HealthProbe for FakeHealthProbe {
        async fn connect_and_ping(
            &self,
            client: &Client,
            _timeout: Duration,
        ) -> Result<(), CamelError> {
            if !self.delay.is_zero() {
                tokio::time::sleep(self.delay).await;
            }
            *self.probed_client.lock().unwrap() = Some(client.clone());
            self.result.clone()
        }
    }

    #[tokio::test]
    async fn health_probes_current_master_not_stale() {
        let topology = Arc::new(FakeTopology::addrs(vec!["redis://b:6379".into()]));
        let probe = Arc::new(FakeHealthProbe::new(Ok(())));
        let check = RedisHealthCheck::with_probe_for_tests(
            topology.clone(),
            probe.clone(),
            Duration::from_secs(1),
            Duration::from_secs(2),
        );

        let result = check.check().await;

        assert_eq!(result.name, "redis");
        assert_eq!(result.status, HealthStatus::Healthy);
        assert_eq!(topology.resolve_call_count(), 1);
        let probed = probe
            .probed_client()
            .expect("probe should have been invoked with the resolved client");
        assert_eq!(
            probed.get_connection_info().addr().to_string(),
            "b:6379",
            "probe must target the resolved master, not the stale configured node"
        );
    }

    #[tokio::test]
    async fn health_unhealthy_when_no_master_resolvable() {
        let topology = Arc::new(FakeTopology::new(vec![Err(CamelError::ProcessorError(
            "no master".into(),
        ))]));
        let probe = Arc::new(FakeHealthProbe::new(Ok(())));
        let check = RedisHealthCheck::with_probe_for_tests(
            topology.clone(),
            probe.clone(),
            Duration::from_secs(1),
            Duration::from_secs(2),
        );

        let result = check.check().await;

        assert_eq!(result.status, HealthStatus::Unhealthy);
        assert!(
            result
                .message
                .as_deref()
                .is_some_and(|m| m.contains("no master"))
        );
        assert!(
            probe.probed_client().is_none(),
            "probe must not be invoked when no master is resolvable"
        );
    }

    #[tokio::test]
    async fn health_unhealthy_when_ping_fails() {
        let topology = Arc::new(FakeTopology::addrs(vec!["redis://b:6379".into()]));
        let probe = Arc::new(FakeHealthProbe::new(Err(CamelError::ProcessorError(
            "ping failed".into(),
        ))));
        let check = RedisHealthCheck::with_probe_for_tests(
            topology.clone(),
            probe.clone(),
            Duration::from_secs(1),
            Duration::from_secs(2),
        );

        let result = check.check().await;

        assert_eq!(result.status, HealthStatus::Unhealthy);
        assert!(
            result
                .message
                .as_deref()
                .is_some_and(|m| m.contains("ping failed"))
        );
    }

    #[tokio::test]
    async fn health_unhealthy_when_outer_timeout_fires() {
        let topology = Arc::new(FakeTopology::addrs(vec!["redis://b:6379".into()]));
        let probe = Arc::new(FakeHealthProbe::new(Ok(())).with_delay(Duration::from_millis(50)));
        let check = RedisHealthCheck::with_probe_for_tests(
            topology.clone(),
            probe.clone(),
            Duration::from_secs(1),
            Duration::from_millis(5),
        );

        let result = check.check().await;

        assert_eq!(result.status, HealthStatus::Unhealthy);
        assert_eq!(result.message.as_deref(), Some("health check timed out"));
    }
}
