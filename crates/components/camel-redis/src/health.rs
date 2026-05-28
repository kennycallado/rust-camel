use crate::RedisEndpointConfig;
use async_trait::async_trait;
use camel_api::{AsyncHealthCheck, CheckResult};
use camel_component_api::CamelError;
use redis::aio::MultiplexedConnection;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

type PingFuture = Pin<Box<dyn Future<Output = Result<String, CamelError>> + Send>>;

trait RedisPingProbe: Send + Sync {
    fn ping(&self) -> PingFuture;
}

struct RedisClientPingProbe {
    client: redis::Client,
    endpoint: String,
}

impl RedisClientPingProbe {
    fn new(config: &RedisEndpointConfig) -> Result<Self, CamelError> {
        let client = redis::Client::open(config.redis_url().as_str()).map_err(|e| {
            CamelError::ProcessorError(format!(
                "Failed to create Redis client for health check '{}': {}",
                config.redis_url_safe(),
                e
            ))
        })?;

        Ok(Self {
            client,
            endpoint: config.safe_endpoint(),
        })
    }
}

impl RedisPingProbe for RedisClientPingProbe {
    fn ping(&self) -> PingFuture {
        let client = self.client.clone();
        let endpoint = self.endpoint.clone();

        Box::pin(async move {
            let mut conn: MultiplexedConnection = client
                .get_multiplexed_async_connection()
                .await
                .map_err(|e| {
                CamelError::ProcessorError(format!(
                    "Failed to connect to Redis for health check '{}': {}",
                    endpoint, e
                ))
            })?;

            redis::cmd("PING")
                .query_async::<String>(&mut conn)
                .await
                .map_err(|e| {
                    CamelError::ProcessorError(format!(
                        "Redis health check PING failed for '{}': {}",
                        endpoint, e
                    ))
                })
        })
    }
}

pub struct RedisHealthCheck {
    probe: Arc<dyn RedisPingProbe>,
    timeout: Duration,
}

impl RedisHealthCheck {
    pub fn new(config: &RedisEndpointConfig) -> Result<Self, CamelError> {
        let probe = RedisClientPingProbe::new(config)?;
        Ok(Self {
            probe: Arc::new(probe),
            timeout: Duration::from_secs(2),
        })
    }

    #[cfg(test)]
    fn with_probe_for_tests(probe: Arc<dyn RedisPingProbe>, timeout: Duration) -> Self {
        Self { probe, timeout }
    }
}

#[async_trait]
impl AsyncHealthCheck for RedisHealthCheck {
    fn name(&self) -> &str {
        "redis"
    }

    async fn check(&self) -> CheckResult {
        match tokio::time::timeout(self.timeout, self.probe.ping()).await {
            Ok(Ok(pong)) if pong.eq_ignore_ascii_case("PONG") => CheckResult::healthy(self.name()),
            Ok(Ok(other)) => {
                CheckResult::unhealthy(self.name(), &format!("Unexpected PING response: {}", other))
            }
            Ok(Err(err)) => CheckResult::unhealthy(self.name(), &err.to_string()),
            Err(_) => CheckResult::unhealthy(self.name(), "PING timed out"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_api::HealthStatus;

    struct MockPingProbe {
        responder: Arc<dyn Fn() -> PingFuture + Send + Sync>,
    }

    impl MockPingProbe {
        fn new<F>(f: F) -> Self
        where
            F: Fn() -> PingFuture + Send + Sync + 'static,
        {
            Self {
                responder: Arc::new(f),
            }
        }
    }

    impl RedisPingProbe for MockPingProbe {
        fn ping(&self) -> PingFuture {
            (self.responder)()
        }
    }

    #[tokio::test]
    async fn redis_health_check_healthy_on_pong() {
        let probe = Arc::new(MockPingProbe::new(|| {
            Box::pin(async { Ok("PONG".to_string()) })
        }));
        let check = RedisHealthCheck::with_probe_for_tests(probe, Duration::from_millis(50));

        let result = check.check().await;

        assert_eq!(result.name, "redis");
        assert_eq!(result.status, HealthStatus::Healthy);
        assert!(result.message.is_none());
    }

    #[tokio::test]
    async fn redis_health_check_unhealthy_on_error() {
        let probe = Arc::new(MockPingProbe::new(|| {
            Box::pin(async {
                Err(CamelError::ProcessorError(
                    "simulated redis error".to_string(),
                ))
            })
        }));
        let check = RedisHealthCheck::with_probe_for_tests(probe, Duration::from_millis(50));

        let result = check.check().await;

        assert_eq!(result.name, "redis");
        assert_eq!(result.status, HealthStatus::Unhealthy);
        assert!(
            result
                .message
                .as_deref()
                .is_some_and(|m| m.contains("simulated redis error"))
        );
    }

    #[tokio::test]
    async fn redis_health_check_unhealthy_on_timeout() {
        let probe = Arc::new(MockPingProbe::new(|| {
            Box::pin(async {
                tokio::time::sleep(Duration::from_millis(50)).await;
                Ok("PONG".to_string())
            })
        }));
        let check = RedisHealthCheck::with_probe_for_tests(probe, Duration::from_millis(5));

        let result = check.check().await;

        assert_eq!(result.name, "redis");
        assert_eq!(result.status, HealthStatus::Unhealthy);
        assert_eq!(result.message.as_deref(), Some("PING timed out"));
    }
}
