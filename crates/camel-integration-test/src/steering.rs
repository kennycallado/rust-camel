//! The datasource steering axis of the integration tier
//! (waist-extraction task 1.1, ADR-0069 §14).
//!
//! Every state-family operation steers one datasource through the same
//! axis: name -> env-steered URL -> pool handle. A scenario names a
//! datasource; the boot's catalog owns the env-steered URL; the
//! resolver returns the pooled handle typed for the calling family.
//!
//! Identifier law: resolution errors name the datasource, never its
//! URL. Every error string carries the family label (`sql action`,
//! `sql validation`, `surreal action`, `surreal validation`,
//! `redis validation`) and the datasource name; driver detail is
//! retained otherwise.
//!
//! Redaction law (ADR-0051, Credential Redaction at Diagnostic
//! Boundaries): database URLs carry credential bytes and must not
//! reach test failure output — [`sanitize_db_error`] replaces every
//! exact, nonempty occurrence of the `db_url` with `[REDACTED]`
//! before a failure is reported.

/// Replaces every occurrence of `db_url` in `err_text` with
/// `[REDACTED]` (ADR-0051). An empty `db_url` is a no-op — an empty
/// pattern would corrupt the text.
pub fn sanitize_db_error(err_text: &str, db_url: &str) -> String {
    if db_url.is_empty() {
        return err_text.to_string();
    }
    err_text.replace(db_url, "[REDACTED]")
}

#[cfg(any(feature = "sql", feature = "surreal", feature = "redis"))]
use std::sync::Arc;

/// Resolves the named datasource through `catalog` into a typed pool
/// handle plus its `db_url` (callers sanitize statement and query
/// errors against it). Errors follow the identifier law: the label
/// and the datasource name, driver detail retained, URL redacted
/// (ADR-0051). Labels stay at the call sites — no family enum, no
/// family-specific text here.
#[cfg(any(feature = "sql", feature = "surreal", feature = "redis"))]
pub(crate) async fn resolve_datasource<T: 'static + Send + Sync>(
    catalog: &Arc<dyn camel_api::datasource::DatasourceCatalog>,
    name: &str,
    label: &str,
) -> Result<(Arc<T>, String), String> {
    let Some(config) = catalog.get_config(name) else {
        return Err(format!("{label}: unknown datasource '{name}'"));
    };
    let db_url = config.db_url.clone();
    let handle = catalog.get_pool(name).await.map_err(|e| {
        format!(
            "{label}: datasource '{name}': {}",
            sanitize_db_error(&e.to_string(), &db_url)
        )
    })?;
    let typed = handle.downcast::<T>().map_err(|e| {
        format!(
            "{label}: datasource '{name}': {}",
            sanitize_db_error(&e.to_string(), &db_url)
        )
    })?;
    Ok((typed, db_url))
}

#[cfg(test)]
mod tests {
    use super::sanitize_db_error;

    #[test]
    fn sanitize_replaces_exact_db_url() {
        let sanitized = sanitize_db_error(
            "connect failed sqlite::memory:?cache=shared&x=1; retry \
             sqlite::memory:?cache=shared&x=1",
            "sqlite::memory:?cache=shared&x=1",
        );
        assert_eq!(sanitized, "connect failed [REDACTED]; retry [REDACTED]");
    }

    #[test]
    fn sanitize_empty_db_url_is_noop() {
        assert_eq!(sanitize_db_error("boom", ""), "boom");
    }
}

/// Resolver exact-string regression tests: every family label pinned
/// against the unknown-name and pool-failure messages (equality, not
/// substring). The resolver is gated `any(sql, surreal)` and its stubs
/// (`sqlite_catalog`, `sqlx::AnyPool`) are sql-gated, so these carry
/// the sql gate to keep featureless and surreal-only test builds
/// compiling.
#[cfg(all(test, feature = "sql"))]
mod resolver_tests {
    use std::collections::HashMap;
    use std::sync::Arc;

    use camel_api::datasource::{DatasourceCatalog, DatasourceConfig, GetPoolFuture, PoolFactory};
    use camel_api::error::CamelError;
    use camel_core::datasource::RuntimeDatasourceCatalog;

    use super::resolve_datasource;
    use crate::sql_stub::sqlite_catalog;

    /// The four family labels, one assertion per label.
    const LABELS: [&str; 4] = [
        "sql action",
        "sql validation",
        "surreal action",
        "surreal validation",
    ];

    /// Sentinel URL with a secret path component, the way a real
    /// config would carry one.
    const SECRET_URL: &str = "sqlite:///tmp/rc-6waist-secret/x.db?mode=rw";

    fn config_with_url(db_url: &str) -> DatasourceConfig {
        DatasourceConfig {
            db_url: db_url.to_string(),
            provider: None,
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

    /// Hand-rolled stub whose `get_pool` always fails with the fixed
    /// processor error carrying the secret URL.
    struct FailingPoolCatalog {
        config: DatasourceConfig,
    }

    impl DatasourceCatalog for FailingPoolCatalog {
        fn get_config(&self, _name: &str) -> Option<DatasourceConfig> {
            Some(self.config.clone())
        }

        fn get_pool<'a>(&'a self, _name: &'a str) -> GetPoolFuture<'a> {
            Box::pin(async {
                Err(CamelError::ProcessorError(format!(
                    "cannot open {SECRET_URL}"
                )))
            })
        }

        fn register_factory(
            &self,
            _kind: &str,
            _factory: Arc<dyn PoolFactory>,
        ) -> Result<(), CamelError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn resolver_unknown_datasource_names_label_and_name() {
        let mut configs = HashMap::new();
        configs.insert(
            "appdb".to_string(),
            config_with_url("sqlite::memory:?cache=shared"),
        );
        let catalog: Arc<dyn DatasourceCatalog> = Arc::new(RuntimeDatasourceCatalog::new(configs));
        let expected = [
            "sql action: unknown datasource 'missing'",
            "sql validation: unknown datasource 'missing'",
            "surreal action: unknown datasource 'missing'",
            "surreal validation: unknown datasource 'missing'",
        ];
        for (label, expected) in LABELS.into_iter().zip(expected) {
            let err = resolve_datasource::<sqlx::AnyPool>(&catalog, "missing", label)
                .await
                .unwrap_err();
            assert_eq!(err, expected);
        }
    }

    #[tokio::test]
    async fn resolver_pool_failure_redacts_url_and_keeps_prefix() {
        let catalog: Arc<dyn DatasourceCatalog> = Arc::new(FailingPoolCatalog {
            config: config_with_url(SECRET_URL),
        });
        let expected = [
            "sql action: datasource 'appdb': Processor error: cannot open [REDACTED]",
            "sql validation: datasource 'appdb': Processor error: cannot open [REDACTED]",
            "surreal action: datasource 'appdb': Processor error: cannot open [REDACTED]",
            "surreal validation: datasource 'appdb': Processor error: cannot open [REDACTED]",
        ];
        for (label, expected) in LABELS.into_iter().zip(expected) {
            let err = resolve_datasource::<sqlx::AnyPool>(&catalog, "appdb", label)
                .await
                .unwrap_err();
            assert_eq!(err, expected);
        }
    }

    #[tokio::test]
    async fn resolver_returns_handle_and_url() {
        let catalog = sqlite_catalog("appdb");
        let (pool, db_url) = resolve_datasource::<sqlx::AnyPool>(&catalog, "appdb", "sql action")
            .await
            .expect("resolution succeeds through the stub catalog");
        assert_eq!(db_url, "sqlite::memory:?cache=shared");
        sqlx::query("SELECT 1")
            .execute(&*pool)
            .await
            .expect("resolved pool is usable");
    }

    #[tokio::test]
    async fn resolver_downcast_failure_keeps_driver_detail() {
        let catalog = sqlite_catalog("appdb");
        let err = resolve_datasource::<String>(&catalog, "appdb", "sql action")
            .await
            .unwrap_err();
        assert!(
            err.starts_with("sql action: datasource 'appdb': "),
            "got: {err}"
        );
        assert!(err.contains("failed to downcast handle"), "got: {err}");
        assert!(!err.contains("sqlite::memory:"), "got: {err}");
    }
}

/// The `redis validation` label pinned against the unknown-name
/// message (equality, not substring), the redis arm of the resolver
/// exact-string regression (redis-state-tier task 1). The stub handles
/// `redis::aio::MultiplexedConnection`, so the module carries the
/// `redis` gate.
#[cfg(all(test, feature = "redis"))]
mod resolver_redis_tests {
    use std::collections::HashMap;
    use std::sync::Arc;

    use camel_api::datasource::{DatasourceCatalog, DatasourceConfig};
    use camel_core::datasource::RuntimeDatasourceCatalog;

    use super::resolve_datasource;

    fn config_with_url(db_url: &str) -> DatasourceConfig {
        DatasourceConfig {
            db_url: db_url.to_string(),
            provider: None,
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

    #[tokio::test]
    async fn resolver_redis_validation_label_pinned() {
        let mut configs = HashMap::new();
        configs.insert(
            "other".to_string(),
            config_with_url("redis://localhost:6379/0"),
        );
        let catalog: Arc<dyn DatasourceCatalog> = Arc::new(RuntimeDatasourceCatalog::new(configs));
        let err = resolve_datasource::<redis::aio::MultiplexedConnection>(
            &catalog,
            "missing",
            "redis validation",
        )
        .await
        .unwrap_err();
        assert_eq!(err, "redis validation: unknown datasource 'missing'");
    }
}
