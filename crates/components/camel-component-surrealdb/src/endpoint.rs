//! SurrealDbEndpoint — creates producers, consumers, and polling consumers.

use std::sync::Arc;

use camel_api::datasource::DatasourceCatalog;
use camel_component_api::{
    BodyType, BoxProcessor, CamelError, Consumer, Endpoint, PollingConsumer, ProducerContext,
    RuntimeObservability,
};

use crate::config::{SurrealDbEndpointConfig, SurrealDbOperation};
use crate::consumer::SurrealDbConsumer;
use crate::polling::SurrealDbPollingConsumer;
use crate::producer::SurrealDbProducer;

/// Endpoint for SurrealDB operations.
///
/// Each endpoint corresponds to a single URI (e.g., `surrealdb:query?datasource=main`).
/// The endpoint creates:
/// - A [`SurrealDbProducer`] for all operations (writes/sends).
/// - A [`SurrealDbConsumer`] only for `live` operation (event-driven push).
/// - A [`SurrealDbPollingConsumer`] for `select` and `query` operations (pollEnrich support).
pub struct SurrealDbEndpoint {
    uri: String,
    pub(crate) config: SurrealDbEndpointConfig,
    pub(crate) catalog: Option<Arc<dyn DatasourceCatalog>>,
}

impl SurrealDbEndpoint {
    pub fn new(
        uri: String,
        config: SurrealDbEndpointConfig,
        catalog: Option<Arc<dyn DatasourceCatalog>>,
    ) -> Self {
        Self {
            uri,
            config,
            catalog,
        }
    }

    pub fn with_catalog(
        uri: String,
        config: SurrealDbEndpointConfig,
        catalog: Arc<dyn DatasourceCatalog>,
    ) -> Self {
        Self {
            uri,
            config,
            catalog: Some(catalog),
        }
    }
}

impl Endpoint for SurrealDbEndpoint {
    fn uri(&self) -> &str {
        &self.uri
    }

    fn create_producer(
        &self,
        _rt: Arc<dyn RuntimeObservability>,
        ctx: &ProducerContext,
    ) -> Result<BoxProcessor, CamelError> {
        let route_id = ctx.route_id().unwrap_or("unknown").to_string();
        let producer = SurrealDbProducer::new(self.config.clone(), self.catalog.clone(), route_id);
        Ok(BoxProcessor::new(producer))
    }

    fn create_consumer(
        &self,
        rt: Arc<dyn RuntimeObservability>,
    ) -> Result<Box<dyn Consumer>, CamelError> {
        if self.config.operation != SurrealDbOperation::Live {
            return Err(CamelError::EndpointCreationFailed(format!(
                "surrealdb consumer requires 'live' operation, got '{}'",
                self.config.operation
            )));
        }
        let consumer = SurrealDbConsumer::new(self.config.clone(), self.catalog.clone(), rt);
        Ok(Box::new(consumer))
    }

    fn body_contract(&self) -> Option<BodyType> {
        match self.config.operation {
            // Query reads SQL from the body (or CamelSurrealDbQuery header).
            SurrealDbOperation::Query => Some(BodyType::Text),
            SurrealDbOperation::Create
            | SurrealDbOperation::Update
            | SurrealDbOperation::Upsert
            | SurrealDbOperation::Patch
            | SurrealDbOperation::Relate
            | SurrealDbOperation::Vector
            | SurrealDbOperation::Search
            | SurrealDbOperation::Run => Some(BodyType::Json),
            SurrealDbOperation::Select | SurrealDbOperation::Delete | SurrealDbOperation::Live => {
                None
            }
        }
    }

    fn polling_consumer(&self) -> Option<Box<dyn PollingConsumer>> {
        match self.config.operation {
            SurrealDbOperation::Select | SurrealDbOperation::Query => {
                let polling =
                    SurrealDbPollingConsumer::new(self.config.clone(), self.catalog.clone());
                Some(Box::new(polling))
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use camel_api::datasource::{DatasourceCatalog, DatasourceConfig, GetPoolFuture, PoolFactory};
    use camel_component_api::test_support::PanicRuntimeObservability;
    use camel_component_api::{Component, Endpoint, NoOpComponentContext};

    use crate::SurrealDbComponent;
    use crate::config::SurrealDbEndpointConfig;

    fn test_rt() -> Arc<dyn camel_component_api::RuntimeObservability> {
        Arc::new(PanicRuntimeObservability)
    }

    fn make_endpoint(uri: &str) -> super::SurrealDbEndpoint {
        let config = SurrealDbEndpointConfig::from_uri(uri).unwrap();
        super::SurrealDbEndpoint::new(uri.to_string(), config, None)
    }

    /// Minimal in-memory DatasourceCatalog stub for unit testing create_endpoint.
    pub struct StubCatalog {
        configs: std::sync::Mutex<std::collections::HashMap<String, DatasourceConfig>>,
    }

    impl StubCatalog {
        pub fn new(items: Vec<(&str, DatasourceConfig)>) -> Self {
            let map = items.into_iter().map(|(k, v)| (k.to_string(), v)).collect();
            Self {
                configs: std::sync::Mutex::new(map),
            }
        }
    }

    impl DatasourceCatalog for StubCatalog {
        fn get_config(&self, name: &str) -> Option<DatasourceConfig> {
            self.configs.lock().unwrap().get(name).cloned()
        }

        fn get_pool<'a>(&'a self, name: &'a str) -> GetPoolFuture<'a> {
            Box::pin(async move {
                Err(camel_api::CamelError::Config(format!(
                    "StubCatalog has no pool for '{name}'"
                )))
            })
        }

        fn register_factory(
            &self,
            _kind: &str,
            _factory: std::sync::Arc<dyn PoolFactory>,
        ) -> Result<(), camel_api::CamelError> {
            Ok(())
        }
    }

    // --- Basic endpoint tests ---

    #[test]
    fn endpoint_uri_returns_stored() {
        let ep = make_endpoint("surrealdb:query?datasource=main");
        assert_eq!(ep.uri(), "surrealdb:query?datasource=main");
    }

    #[test]
    fn endpoint_create_producer_succeeds() {
        let ep = make_endpoint("surrealdb:query?datasource=main");
        let ctx = camel_component_api::ProducerContext::new();
        let result = ep.create_producer(test_rt(), &ctx);
        assert!(result.is_ok());
    }

    #[test]
    fn endpoint_create_consumer_succeeds_for_live() {
        let ep = make_endpoint("surrealdb:live?datasource=main&table=events");
        let result = ep.create_consumer(test_rt());
        assert!(result.is_ok());
    }

    #[test]
    fn endpoint_polling_consumer_succeeds_for_select() {
        let ep = make_endpoint("surrealdb:select?datasource=main&table=docs&id=42");
        let polling = ep.polling_consumer();
        assert!(polling.is_some());
    }

    #[test]
    fn endpoint_polling_consumer_none_for_create() {
        let ep = make_endpoint("surrealdb:create?datasource=main&table=docs");
        assert!(ep.polling_consumer().is_none());
    }

    #[test]
    fn endpoint_polling_consumer_none_for_search() {
        let ep = make_endpoint("surrealdb:search?datasource=main&table=docs&top_k=5");
        assert!(ep.polling_consumer().is_none());
    }

    // --- LIVE protocol validation ---

    #[test]
    fn live_operation_rejects_http_scheme() {
        let ds_config = DatasourceConfig {
            db_url: "http://localhost:8000".into(),
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
        };

        let catalog = Arc::new(StubCatalog::new(vec![("http-ds", ds_config)]));
        let component = SurrealDbComponent::with_catalog(catalog);
        let ctx = NoOpComponentContext;

        let result =
            component.create_endpoint("surrealdb:live?datasource=http-ds&table=events", &ctx);

        let err = match result {
            Err(e) => e,
            Ok(_) => panic!("http:// must be rejected for live"),
        };
        let err_msg = format!("{err:?}");
        assert!(
            err_msg.contains("LiveRequiresWebSocket") || err_msg.contains("WebSocket"),
            "expected WebSocket protocol error, got: {err_msg}"
        );
    }

    #[test]
    fn live_operation_accepts_ws_scheme() {
        let ds_config = DatasourceConfig {
            db_url: "ws://localhost:8000".into(),
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
        };

        let catalog = Arc::new(StubCatalog::new(vec![("ws-ds", ds_config)]));
        let component = SurrealDbComponent::with_catalog(catalog);
        let ctx = NoOpComponentContext;

        let result =
            component.create_endpoint("surrealdb:live?datasource=ws-ds&table=events", &ctx);

        if let Err(ref e) = result {
            let msg = format!("{:?}", e);
            assert!(
                !msg.contains("LiveRequiresWebSocket"),
                "ws:// must pass protocol check, got: {msg}"
            );
        }
    }
}
