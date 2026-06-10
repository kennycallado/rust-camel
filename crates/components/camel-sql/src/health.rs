use async_trait::async_trait;
use camel_api::{AsyncHealthCheck, CheckResult};
use camel_component_api::CamelError;
use sqlx::AnyPool;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::OnceCell;

type ProbeFuture = Pin<Box<dyn Future<Output = Result<(), CamelError>> + Send>>;

trait SqlHealthProbe: Send + Sync {
    fn probe(&self) -> ProbeFuture;
}

struct SqlPoolProbe {
    pool: Arc<OnceCell<Arc<AnyPool>>>,
}

impl SqlPoolProbe {
    fn new(pool: Arc<OnceCell<Arc<AnyPool>>>) -> Self {
        Self { pool }
    }
}

impl SqlHealthProbe for SqlPoolProbe {
    fn probe(&self) -> ProbeFuture {
        let pool = Arc::clone(&self.pool);
        Box::pin(async move {
            let pool = pool.get().ok_or_else(|| {
                CamelError::ProcessorError("SQL connection pool not initialized".to_string())
            })?;

            sqlx::query("SELECT 1")
                .execute(pool.as_ref())
                .await
                .map_err(|e| {
                    CamelError::ProcessorError(format!("SQL health check failed: {}", e))
                })?;

            Ok(())
        })
    }
}

pub struct SqlHealthCheck {
    probe: Arc<dyn SqlHealthProbe>,
    timeout: Duration,
}

impl SqlHealthCheck {
    pub fn new(pool: Arc<OnceCell<Arc<AnyPool>>>) -> Self {
        Self {
            probe: Arc::new(SqlPoolProbe::new(pool)),
            timeout: Duration::from_secs(2),
        }
    }

    #[cfg(test)]
    fn with_probe_for_tests(probe: Arc<dyn SqlHealthProbe>, timeout: Duration) -> Self {
        Self { probe, timeout }
    }
}

#[async_trait]
impl AsyncHealthCheck for SqlHealthCheck {
    fn name(&self) -> &str {
        "sql"
    }

    async fn check(&self) -> CheckResult {
        match tokio::time::timeout(self.timeout, self.probe.probe()).await {
            Ok(Ok(())) => CheckResult::healthy(self.name()),
            Ok(Err(err)) => CheckResult::unhealthy(self.name(), &err.to_string()),
            Err(_) => CheckResult::unhealthy(self.name(), "SELECT 1 timed out"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_api::HealthStatus;

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

    impl SqlHealthProbe for MockProbe {
        fn probe(&self) -> ProbeFuture {
            (self.responder)()
        }
    }

    #[tokio::test]
    async fn sql_health_check_healthy_when_probe_succeeds() {
        let probe = Arc::new(MockProbe::new(|| Box::pin(async { Ok(()) })));
        let check = SqlHealthCheck::with_probe_for_tests(probe, Duration::from_millis(50));

        let result = check.check().await;

        assert_eq!(result.name, "sql");
        assert_eq!(result.status, HealthStatus::Healthy);
        assert!(result.message.is_none());
    }

    #[tokio::test]
    async fn sql_health_check_unhealthy_when_probe_fails() {
        let probe = Arc::new(MockProbe::new(|| {
            Box::pin(async {
                Err(CamelError::ProcessorError(
                    "simulated sql error".to_string(),
                ))
            })
        }));
        let check = SqlHealthCheck::with_probe_for_tests(probe, Duration::from_millis(50));

        let result = check.check().await;

        assert_eq!(result.name, "sql");
        assert_eq!(result.status, HealthStatus::Unhealthy);
        assert!(
            result
                .message
                .as_deref()
                .is_some_and(|m| m.contains("simulated sql error"))
        );
    }

    #[tokio::test]
    async fn sql_health_check_unhealthy_when_probe_times_out() {
        let probe = Arc::new(MockProbe::new(|| {
            Box::pin(async {
                tokio::time::sleep(Duration::from_millis(50)).await;
                Ok(())
            })
        }));
        let check = SqlHealthCheck::with_probe_for_tests(probe, Duration::from_millis(5));

        let result = check.check().await;

        assert_eq!(result.name, "sql");
        assert_eq!(result.status, HealthStatus::Unhealthy);
        assert_eq!(result.message.as_deref(), Some("SELECT 1 timed out"));
    }
}
