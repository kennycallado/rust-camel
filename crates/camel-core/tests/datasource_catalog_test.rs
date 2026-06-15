use std::any::Any;
use std::collections::HashMap;
use std::sync::Arc;

use camel_api::datasource::{
    CheckFuture, CreatePoolFuture, DatasourceCatalog, DatasourceConfig, DatasourceHandle,
    PoolFactory,
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
