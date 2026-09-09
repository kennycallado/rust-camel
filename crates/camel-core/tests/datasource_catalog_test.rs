use std::any::Any;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use camel_api::datasource::{
    CheckFuture, CloseFuture, CreatePoolFuture, DatasourceCatalog, DatasourceConfig,
    DatasourceHandle, PoolFactory,
};
use camel_api::lifecycle::HealthStatus;
use camel_core::datasource::RuntimeDatasourceCatalog;

struct IntegrationMockFactory;

impl PoolFactory for IntegrationMockFactory {
    fn create<'a>(&'a self, config: &'a DatasourceConfig) -> CreatePoolFuture<'a> {
        Box::pin(async move { Ok(Arc::new(config.db_url.clone()) as Arc<dyn Any + Send + Sync>) })
    }

    fn check<'a>(&'a self, _handle: &'a DatasourceHandle) -> CheckFuture<'a> {
        Box::pin(async { HealthStatus::Healthy })
    }

    fn supported_schemes(&self) -> &[&str] {
        &["postgres"]
    }

    fn name(&self) -> &'static str {
        "integration-mock"
    }
}

/// A factory whose `close` records invocation — the seam the boot
/// teardown drains (bd rc-25lup.4).
struct CloseRecordingFactory {
    close_count: Arc<AtomicUsize>,
}

impl PoolFactory for CloseRecordingFactory {
    fn create<'a>(&'a self, _config: &'a DatasourceConfig) -> CreatePoolFuture<'a> {
        Box::pin(async { Ok(Arc::new("pool") as Arc<dyn Any + Send + Sync>) })
    }

    fn check<'a>(&'a self, _handle: &'a DatasourceHandle) -> CheckFuture<'a> {
        Box::pin(async { HealthStatus::Healthy })
    }

    fn close<'a>(&'a self, handle: &'a DatasourceHandle) -> CloseFuture<'a> {
        let count = self.close_count.clone();
        let name = handle.name.clone();
        Box::pin(async move {
            assert_eq!(name, "orders", "close must receive the initialized handle");
            count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
    }

    fn supported_schemes(&self) -> &[&str] {
        &["postgres"]
    }

    fn name(&self) -> &'static str {
        "close-recording"
    }
}

fn make_config(url: &str) -> DatasourceConfig {
    DatasourceConfig {
        db_url: url.into(),
        provider: Some("integration-mock".into()),
        max_connections: Some(5),
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
async fn full_datasource_catalog_flow() {
    let mut configs = HashMap::new();
    configs.insert("orders".into(), make_config("postgres://localhost/orders"));
    configs.insert(
        "analytics".into(),
        make_config("postgres://localhost/analytics"),
    );

    let catalog = RuntimeDatasourceCatalog::new(configs);
    catalog
        .register_factory("integration-mock", Arc::new(IntegrationMockFactory))
        .unwrap();

    let orders = catalog.get_pool("orders").await.unwrap();
    assert_eq!(orders.name, "orders");
    assert_eq!(orders.provider, "integration-mock");

    let url: Arc<String> = orders.downcast::<String>().unwrap();
    assert_eq!(*url, "postgres://localhost/orders");

    let analytics = catalog.get_pool("analytics").await.unwrap();
    assert_eq!(analytics.name, "analytics");
}

#[tokio::test]
async fn shared_pool_same_datasource() {
    let mut configs = HashMap::new();
    configs.insert("orders".into(), make_config("postgres://localhost/orders"));

    let catalog = RuntimeDatasourceCatalog::new(configs);
    catalog
        .register_factory("integration-mock", Arc::new(IntegrationMockFactory))
        .unwrap();

    let h1 = catalog.get_pool("orders").await.unwrap();
    let h2 = catalog.get_pool("orders").await.unwrap();

    let url1: Arc<String> = h1.downcast::<String>().unwrap();
    let url2: Arc<String> = h2.downcast::<String>().unwrap();
    assert_eq!(*url1, *url2);
}

#[tokio::test]
async fn unknown_datasource_returns_error() {
    let catalog = RuntimeDatasourceCatalog::new(HashMap::new());
    catalog
        .register_factory("integration-mock", Arc::new(IntegrationMockFactory))
        .unwrap();

    let result = catalog.get_pool("nonexistent").await;
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("not found"),
        "expected clear not-found error, got: {}",
        msg
    );
}

#[tokio::test]
async fn close_all_closes_initialized_pools_and_is_idempotent() {
    let close_count = Arc::new(AtomicUsize::new(0));
    let mut config = make_config("postgres://localhost/orders");
    config.provider = Some("close-recording".into());
    let mut configs = HashMap::new();
    configs.insert("orders".into(), config);

    let catalog = RuntimeDatasourceCatalog::new(configs);
    catalog
        .register_factory(
            "close-recording",
            Arc::new(CloseRecordingFactory {
                close_count: close_count.clone(),
            }),
        )
        .unwrap();

    // Before any get_pool the catalog has nothing to close: Ok, no calls.
    catalog.close_all().await.unwrap();
    assert_eq!(close_count.load(Ordering::SeqCst), 0);

    catalog.get_pool("orders").await.unwrap();

    catalog.close_all().await.unwrap();
    assert_eq!(
        close_count.load(Ordering::SeqCst),
        1,
        "close_all must close exactly the initialized pool once"
    );

    // Second close_all is Ok; the handle cell is still present, so the
    // factory's close runs again (idempotent by contract).
    catalog.close_all().await.unwrap();
    assert_eq!(close_count.load(Ordering::SeqCst), 2);
}

/// The registry key is an arbitrary kind string; the handle's provider
/// is the factory NAME. `config.provider` pins the registry KEY (that
/// is how `get_pool` resolves), while the cached handle stores
/// `factory.name()`. close_all must resolve by NAME, so a factory
/// registered under a key different from its name still closes its
/// pools (review finding, bd rc-25lup.4).
#[tokio::test]
async fn close_all_resolves_factory_by_name_not_registry_key() {
    let close_count = Arc::new(AtomicUsize::new(0));
    let mut config = make_config("postgres://localhost/orders");
    // The KEY, not the factory name.
    config.provider = Some("pg-pool".into());
    let mut configs = HashMap::new();
    configs.insert("orders".into(), config);

    let catalog = RuntimeDatasourceCatalog::new(configs);
    // Registry key "pg-pool" != factory name "close-recording".
    catalog
        .register_factory(
            "pg-pool",
            Arc::new(CloseRecordingFactory {
                close_count: close_count.clone(),
            }),
        )
        .unwrap();

    catalog.get_pool("orders").await.unwrap();
    catalog.close_all().await.unwrap();
    assert_eq!(
        close_count.load(Ordering::SeqCst),
        1,
        "close_all must close a pool whose factory was registered under a key != its name"
    );
}
