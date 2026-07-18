//! # datasource — single-ring adapter module
//!
//! Runtime catalog of datasource pools keyed by name, with a `PoolFactory`
//! registry and lazy `OnceCell` pool initialization.
//!
//! ## Ring classification (ADR-0045 §5)
//!
//! Single-ring **Interface Adapters** module. `RuntimeDatasourceCatalog` holds
//! `dashmap::DashMap`, `tokio::sync::OnceCell`, `parking_lot::RwLock` fields
//! and orchestrates framework I/O via `get_pool`/`resolve_factory`. Earlier
//! 3-ring split mislabeled the stateful catalog as "domain"; this single-file
//! adapter form corrects the label so the dependency rule holds.

use std::collections::HashMap;
use std::sync::Arc;

use camel_api::datasource::{DatasourceConfig, DatasourceHandle, PoolFactory};
use camel_api::error::CamelError;
use camel_api::health::{AsyncHealthCheck, CheckResult};
use dashmap::DashMap;
use parking_lot::RwLock;
use tokio::sync::OnceCell;

use crate::health_registry::HealthCheckRegistry;

// === Value types (formerly domain.rs) ===

type CacheKey = (String, String);

// === Aggregate root ===

pub struct RuntimeDatasourceCatalog {
    configs: HashMap<String, DatasourceConfig>,
    factories: RwLock<HashMap<String, Arc<dyn PoolFactory>>>,
    pools: DashMap<CacheKey, Arc<OnceCell<DatasourceHandle>>>,
    health_registry: Option<Arc<HealthCheckRegistry>>,
}

impl RuntimeDatasourceCatalog {
    pub fn new(configs: HashMap<String, DatasourceConfig>) -> Self {
        Self {
            configs,
            factories: RwLock::new(HashMap::new()),
            pools: DashMap::new(),
            health_registry: None,
        }
    }

    pub fn with_health_registry(mut self, registry: Arc<HealthCheckRegistry>) -> Self {
        self.health_registry = Some(registry);
        self
    }

    fn configs(&self) -> &HashMap<String, DatasourceConfig> {
        &self.configs
    }

    fn pools(&self) -> &DashMap<CacheKey, Arc<OnceCell<DatasourceHandle>>> {
        &self.pools
    }

    fn health_registry(&self) -> Option<&Arc<HealthCheckRegistry>> {
        self.health_registry.as_ref()
    }

    fn factories(&self) -> &RwLock<HashMap<String, Arc<dyn PoolFactory>>> {
        &self.factories
    }
}

// === Use-case orchestration (formerly application.rs) ===

impl RuntimeDatasourceCatalog {
    pub(crate) fn get_config(&self, name: &str) -> Option<DatasourceConfig> {
        self.configs().get(name).cloned()
    }

    pub(crate) fn register_factory(
        &self,
        kind: &str,
        factory: Arc<dyn PoolFactory>,
    ) -> Result<(), CamelError> {
        let mut factories = self.factories().write(); // allow-unwrap (parking_lot panics on poison)
        if factories.contains_key(kind) {
            return Err(CamelError::Config(format!(
                "factory '{}' already registered",
                kind
            )));
        }
        factories.insert(kind.to_string(), factory);
        Ok(())
    }

    pub(crate) fn get_pool<'a>(
        &'a self,
        name: &'a str,
    ) -> camel_api::datasource::GetPoolFuture<'a> {
        Box::pin(async move {
            let config = self.configs().get(name).cloned().ok_or_else(|| {
                CamelError::Config(format!("datasource '{}' not found in catalog", name))
            })?;

            let factory = self.resolve_factory(&config)?;
            let cache_key: CacheKey = (name.to_string(), factory.name().to_string());

            let cell = self
                .pools()
                .entry(cache_key)
                .or_insert_with(|| Arc::new(OnceCell::new()))
                .clone();

            let handle = cell
                .get_or_try_init(|| async {
                    let inner = factory.create(&config).await?;
                    let handle =
                        DatasourceHandle::new(name.to_string(), factory.name().to_string(), inner);

                    if let Some(registry) = self.health_registry() {
                        let factory_ref = factory.clone();
                        let handle_for_check = handle.clone();
                        let ds_name = name.to_string();
                        registry.register_for_route(
                            &format!("datasource:{}", ds_name),
                            std::sync::Arc::new(DatasourceHealthCheck {
                                check_name: format!("datasource:{}", ds_name),
                                factory: factory_ref,
                                handle: handle_for_check,
                            }),
                        );
                        registry.mark_route_started(&format!("datasource:{}", ds_name));
                    }

                    Ok::<DatasourceHandle, CamelError>(handle)
                })
                .await?;
            Ok(handle.clone())
        })
    }

    fn resolve_factory(
        &self,
        config: &DatasourceConfig,
    ) -> Result<Arc<dyn PoolFactory>, CamelError> {
        let factories = self.factories().read(); // allow-unwrap (parking_lot panics on poison)
        if let Some(ref provider) = config.provider {
            let factory = factories.get(provider).ok_or_else(|| {
                CamelError::Config(format!("unknown datasource provider '{}'", provider))
            })?;
            return Ok(factory.clone());
        }

        let matches: Vec<_> = factories
            .values()
            .filter(|entry| entry.matches(config))
            .collect();

        match matches.len() {
            0 => Err(CamelError::Config(format!(
                "no matching factory for datasource url '{}'",
                scheme_hint(&config.db_url)
            ))),
            1 => Ok(matches[0].clone()),
            _ => {
                let names: Vec<_> = matches.iter().map(|m| m.name()).collect();
                Err(CamelError::Config(format!(
                    "ambiguous datasource: {} factories match '{}'. Set explicit 'provider' field.",
                    names.len(),
                    scheme_hint(&config.db_url)
                )))
            }
        }
    }
}

/// Extract the scheme portion of a database URL for safe display
/// without leaking credentials.
fn scheme_hint(db_url: &str) -> String {
    if let Some(scheme_end) = db_url.find("://") {
        format!("{}://...", &db_url[..scheme_end])
    } else {
        "[REDACTED]".to_string()
    }
}

struct DatasourceHealthCheck {
    check_name: String,
    factory: Arc<dyn PoolFactory>,
    handle: DatasourceHandle,
}

#[async_trait::async_trait]
impl AsyncHealthCheck for DatasourceHealthCheck {
    fn name(&self) -> &str {
        &self.check_name
    }

    async fn check(&self) -> CheckResult {
        let status = self.factory.check(&self.handle).await;
        CheckResult {
            name: self.check_name.clone(),
            status,
            message: None,
        }
    }
}

// === External trait delegation (formerly adapters.rs) ===

use camel_api::datasource::DatasourceCatalog;

impl DatasourceCatalog for RuntimeDatasourceCatalog {
    fn get_config(&self, name: &str) -> Option<DatasourceConfig> {
        self.get_config(name)
    }

    fn get_pool<'a>(&'a self, name: &'a str) -> camel_api::datasource::GetPoolFuture<'a> {
        self.get_pool(name)
    }

    fn register_factory(
        &self,
        kind: &str,
        factory: Arc<dyn PoolFactory>,
    ) -> Result<(), CamelError> {
        self.register_factory(kind, factory)
    }
}

// === Tests ===

#[cfg(test)]
mod tests {
    use super::*;
    use std::any::Any;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use camel_api::datasource::{CheckFuture, CreatePoolFuture};
    use camel_api::lifecycle::HealthStatus;

    struct MockFactory {
        name: &'static str,
        schemes: &'static [&'static str],
        create_count: Arc<AtomicUsize>,
    }

    impl PoolFactory for MockFactory {
        fn create<'a>(&'a self, _config: &'a DatasourceConfig) -> CreatePoolFuture<'a> {
            let count = self.create_count.clone();
            Box::pin(async move {
                count.fetch_add(1, Ordering::SeqCst);
                Ok(Arc::new("mock_pool") as Arc<dyn Any + Send + Sync>)
            })
        }

        fn check<'a>(&'a self, _handle: &'a DatasourceHandle) -> CheckFuture<'a> {
            Box::pin(async { HealthStatus::Healthy })
        }

        fn supported_schemes(&self) -> &[&str] {
            self.schemes
        }

        fn name(&self) -> &'static str {
            self.name
        }
    }

    fn make_config(db_url: &str) -> DatasourceConfig {
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
            extra: std::collections::HashMap::new(),
        }
    }

    #[tokio::test]
    async fn register_factory_and_get_pool() {
        let mut configs = HashMap::new();
        configs.insert(
            "mydb".to_string(),
            make_config("postgresql://localhost/mydb"),
        );

        let catalog = RuntimeDatasourceCatalog::new(configs);
        let factory = Arc::new(MockFactory {
            name: "pg",
            schemes: &["postgresql", "postgres"],
            create_count: Arc::new(AtomicUsize::new(0)),
        });
        catalog.register_factory("postgresql", factory).unwrap();

        let handle = catalog.get_pool("mydb").await.unwrap();
        assert_eq!(handle.name, "mydb");
        assert_eq!(handle.provider, "pg");
    }

    #[tokio::test]
    async fn shared_pool_for_same_datasource() {
        let mut configs = HashMap::new();
        configs.insert(
            "mydb".to_string(),
            make_config("postgresql://localhost/mydb"),
        );

        let count = Arc::new(AtomicUsize::new(0));
        let catalog = RuntimeDatasourceCatalog::new(configs);
        let factory = Arc::new(MockFactory {
            name: "pg",
            schemes: &["postgresql", "postgres"],
            create_count: count.clone(),
        });
        catalog.register_factory("postgresql", factory).unwrap();

        let h1 = catalog.get_pool("mydb").await.unwrap();
        let h2 = catalog.get_pool("mydb").await.unwrap();

        assert_eq!(h1.name, h2.name);
        assert_eq!(h1.provider, h2.provider);
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn unknown_datasource_returns_error() {
        let configs = HashMap::new();
        let catalog = RuntimeDatasourceCatalog::new(configs);

        let result = catalog.get_pool("nonexistent").await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    #[tokio::test]
    async fn duplicate_factory_returns_error() {
        let configs = HashMap::new();
        let catalog = RuntimeDatasourceCatalog::new(configs);
        let factory = Arc::new(MockFactory {
            name: "pg",
            schemes: &["postgresql"],
            create_count: Arc::new(AtomicUsize::new(0)),
        });

        catalog.register_factory("pg", factory.clone()).unwrap();
        let result = catalog.register_factory("pg", factory);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("already registered"));
    }

    #[tokio::test]
    async fn no_matching_factory_returns_error() {
        let mut configs = HashMap::new();
        configs.insert("mydb".to_string(), make_config("mongodb://localhost/mydb"));

        let catalog = RuntimeDatasourceCatalog::new(configs);
        let factory = Arc::new(MockFactory {
            name: "pg",
            schemes: &["postgresql"],
            create_count: Arc::new(AtomicUsize::new(0)),
        });
        catalog.register_factory("postgresql", factory).unwrap();

        let result = catalog.get_pool("mydb").await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("no matching factory"));
    }

    #[tokio::test]
    async fn explicit_provider_overrides_scheme() {
        let mut configs = HashMap::new();
        configs.insert(
            "mydb".to_string(),
            DatasourceConfig {
                db_url: "postgresql://localhost/mydb".to_string(),
                provider: Some("mysql_factory".to_string()),
                max_connections: None,
                min_connections: None,
                idle_timeout_secs: None,
                max_lifetime_secs: None,
                ssl_mode: None,
                ssl_root_cert: None,
                ssl_cert: None,
                ssl_key: None,
                extra: std::collections::HashMap::new(),
            },
        );

        let pg_count = Arc::new(AtomicUsize::new(0));
        let mysql_count = Arc::new(AtomicUsize::new(0));

        let catalog = RuntimeDatasourceCatalog::new(configs);
        let pg_factory = Arc::new(MockFactory {
            name: "pg",
            schemes: &["postgresql"],
            create_count: pg_count.clone(),
        });
        let mysql_factory = Arc::new(MockFactory {
            name: "mysql_factory",
            schemes: &["mysql"],
            create_count: mysql_count.clone(),
        });

        catalog.register_factory("postgresql", pg_factory).unwrap();
        catalog
            .register_factory("mysql_factory", mysql_factory)
            .unwrap();

        let handle = catalog.get_pool("mydb").await.unwrap();
        assert_eq!(handle.provider, "mysql_factory");
        assert_eq!(pg_count.load(Ordering::SeqCst), 0);
        assert_eq!(mysql_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn get_config_returns_clone() {
        let mut configs = HashMap::new();
        let original = make_config("postgresql://localhost/mydb");
        configs.insert("mydb".to_string(), original.clone());

        let catalog = RuntimeDatasourceCatalog::new(configs);
        let retrieved = catalog.get_config("mydb");
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().db_url, original.db_url);
    }

    #[tokio::test]
    async fn get_pool_before_factory_registered_returns_clear_error() {
        let mut configs = HashMap::new();
        configs.insert(
            "mydb".to_string(),
            make_config("postgresql://localhost/mydb"),
        );

        let catalog = RuntimeDatasourceCatalog::new(configs);

        let result = catalog.get_pool("mydb").await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("no matching factory"));
    }

    #[tokio::test]
    async fn ambiguous_factory_returns_error() {
        let mut configs = HashMap::new();
        configs.insert("orders".into(), make_config("postgres://localhost/test"));
        let catalog = RuntimeDatasourceCatalog::new(configs);
        catalog
            .register_factory(
                "mock1",
                Arc::new(MockFactory {
                    name: "mock1",
                    schemes: &["postgres"],
                    create_count: Arc::new(AtomicUsize::new(0)),
                }),
            )
            .unwrap();

        struct MockFactory2;
        impl PoolFactory for MockFactory2 {
            fn create<'a>(&'a self, config: &'a DatasourceConfig) -> CreatePoolFuture<'a> {
                Box::pin(async move {
                    Ok(Arc::new(config.db_url.clone()) as Arc<dyn Any + Send + Sync>)
                })
            }
            fn check<'a>(&'a self, _handle: &'a DatasourceHandle) -> CheckFuture<'a> {
                Box::pin(async { HealthStatus::Healthy })
            }
            fn supported_schemes(&self) -> &[&str] {
                &["postgres"]
            }
            fn name(&self) -> &'static str {
                "mock2"
            }
        }
        catalog
            .register_factory("mock2", Arc::new(MockFactory2))
            .unwrap();

        let result = catalog.get_pool("orders").await;
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("ambiguous"),
            "expected ambiguous error, got: {}",
            msg
        );
    }

    #[tokio::test]
    async fn bad_downcast_returns_clear_error() {
        let mut configs = HashMap::new();
        configs.insert(
            "mydb".to_string(),
            make_config("postgresql://localhost/mydb"),
        );

        let catalog = RuntimeDatasourceCatalog::new(configs);
        let factory = Arc::new(MockFactory {
            name: "pg",
            schemes: &["postgresql"],
            create_count: Arc::new(AtomicUsize::new(0)),
        });
        catalog.register_factory("postgresql", factory).unwrap();

        let handle = catalog.get_pool("mydb").await.unwrap();

        let result: Result<Arc<String>, CamelError> = handle.downcast();
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("failed to downcast"));
        assert!(err.to_string().contains("mydb"));
        assert!(err.to_string().contains("pg"));
    }

    #[tokio::test]
    async fn health_check_registered_after_pool_creation() {
        let mut configs = HashMap::new();
        configs.insert(
            "orders".to_string(),
            make_config("postgresql://localhost/orders"),
        );

        let registry = Arc::new(crate::health_registry::HealthCheckRegistry::new(
            std::time::Duration::from_secs(5),
        ));
        let catalog = RuntimeDatasourceCatalog::new(configs).with_health_registry(registry.clone());
        catalog
            .register_factory(
                "postgresql",
                Arc::new(MockFactory {
                    name: "pg",
                    schemes: &["postgresql", "postgres"],
                    create_count: Arc::new(AtomicUsize::new(0)),
                }),
            )
            .unwrap();

        let _ = catalog.get_pool("orders").await.unwrap();
        registry.mark_route_started("datasource:orders");

        let report = registry.check_all().await;
        assert!(
            report
                .services
                .iter()
                .any(|s| s.name.starts_with("datasource:")),
            "expected datasource health check in report, got: {:?}",
            report.services
        );
    }
}
