//! Datasource-catalog factory for standalone Redis connections.

use camel_api::{
    CamelError,
    datasource::{CheckFuture, CreatePoolFuture, DatasourceConfig, DatasourceHandle, PoolFactory},
    lifecycle::HealthStatus,
};
use std::{any::Any, sync::Arc};

/// Creates a multiplexed Redis connection using the driver's URL grammar.
/// Connections are released when all catalog and boot owners are dropped.
pub struct RedisPoolFactory;

pub(crate) fn open_client(db_url: &str) -> Result<redis::Client, CamelError> {
    if !(db_url.starts_with("redis://") || db_url.starts_with("rediss://")) {
        return Err(CamelError::Config(
            "redis datasource: unsupported URL scheme [REDACTED]".into(),
        ));
    }
    #[cfg(not(feature = "tls"))]
    if db_url.starts_with("rediss://") {
        return Err(CamelError::Config(
            "redis datasource: rediss requires the tls feature [REDACTED]".into(),
        ));
    }
    // Omit the URL and parser error: malformed credential-bearing inputs
    // cannot be reliably redacted by a URL parser.
    redis::Client::open(db_url)
        .map_err(|_| CamelError::Config("redis datasource: invalid driver URL [REDACTED]".into()))
}

impl PoolFactory for RedisPoolFactory {
    fn name(&self) -> &'static str {
        "redis"
    }
    fn supported_schemes(&self) -> &[&str] {
        &["redis", "rediss"]
    }
    fn create<'a>(&'a self, config: &'a DatasourceConfig) -> CreatePoolFuture<'a> {
        Box::pin(async move {
            let connection = open_client(&config.db_url)?
                .get_multiplexed_async_connection()
                .await
                .map_err(|_| {
                    CamelError::Config("redis datasource: connection failed [REDACTED]".into())
                })?;
            Ok(Arc::new(connection) as Arc<dyn Any + Send + Sync>)
        })
    }
    fn check<'a>(&'a self, handle: &'a DatasourceHandle) -> CheckFuture<'a> {
        Box::pin(async move {
            let Ok(connection) = handle.downcast::<redis::aio::MultiplexedConnection>() else {
                return HealthStatus::Unhealthy;
            };
            match redis::cmd("PING")
                .query_async::<String>(&mut (*connection).clone())
                .await
            {
                Ok(_) => HealthStatus::Healthy,
                Err(_) => HealthStatus::Unhealthy,
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_api::datasource::{DatasourceConfig, DatasourceHandle, PoolFactory};
    use std::collections::HashMap;
    use std::sync::Arc;

    fn config(url: &str) -> DatasourceConfig {
        DatasourceConfig {
            provider: Some("redis".into()),
            db_url: url.into(),
            max_connections: None,
            min_connections: None,
            idle_timeout_secs: None,
            max_lifetime_secs: None,
            ssl_mode: None,
            ssl_root_cert: None,
            ssl_cert: None,
            ssl_key: None,
            extra: HashMap::new(),
        }
    }
    #[test]
    fn factory_name_is_redis() {
        assert_eq!(RedisPoolFactory.name(), "redis");
    }
    #[test]
    fn factory_supports_redis_and_rediss() {
        assert_eq!(RedisPoolFactory.supported_schemes(), &["redis", "rediss"]);
    }
    #[test]
    fn factory_matches_redis_urls_and_rejects_others() {
        assert!(RedisPoolFactory.matches(&config("redis://localhost:6379")));
        assert!(RedisPoolFactory.matches(&config("rediss://localhost:6379")));
        assert!(!RedisPoolFactory.matches(&config("postgresql://localhost:5432/db")));
    }
    async fn rejected(url: &str) -> String {
        let error = RedisPoolFactory
            .create(&config(url))
            .await
            .expect_err("must reject")
            .to_string();
        assert!(!error.contains(url));
        error
    }
    #[tokio::test]
    async fn unsupported_scheme_is_rejected_redacted() {
        rejected("redis+sentinel://host:26379").await;
        rejected("redis+cluster://host:6379").await;
    }
    #[tokio::test]
    async fn malformed_url_is_rejected_redacted() {
        rejected("redis://[::bad").await;
    }
    #[tokio::test]
    async fn malformed_url_credential_is_redacted() {
        assert!(
            !rejected("redis://user:s3cr3t-sentinel@[::bad")
                .await
                .contains("s3cr3t-sentinel")
        );
    }
    #[cfg(not(feature = "tls"))]
    #[tokio::test]
    async fn rediss_without_tls_is_rejected() {
        rejected("rediss://host:6379").await;
    }
    #[cfg(feature = "tls")]
    #[test]
    fn rediss_with_tls_opens_client() {
        assert!(open_client("rediss://host:6379").is_ok());
    }
    #[test]
    fn open_client_honors_acl_username_and_database() {
        let client = open_client("redis://alice:secret@host:6379/1").expect("driver URL");
        let settings = client.get_connection_info().redis_settings();
        assert_eq!(settings.username(), Some("alice"));
        assert_eq!(settings.password(), Some("secret"));
        assert_eq!(settings.db(), 1);
        assert!(open_client("redis://host:6379/1").is_ok());
    }
    #[tokio::test]
    async fn close_is_default_noop() {
        let handle = DatasourceHandle::new("statedb".into(), "redis".into(), Arc::new(()));
        assert!(RedisPoolFactory.close(&handle).await.is_ok());
    }
}
