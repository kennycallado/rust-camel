//! SurrealDbPollingConsumer — PollingConsumer for pollEnrich and camel_poll support.
//!
//! Constraint (ADR-0015): `receive(timeout)` gets ONLY a timeout, no incoming Exchange.
//! Query must be fully defined in URI/config.
//! Supported operations: `select` (table+id from URI), `query` (static SurrealQL from URI).

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value as JsonValue;
use tracing::{debug, warn};

use camel_api::datasource::DatasourceCatalog;
use camel_component_api::{Body, CamelError, Exchange, Message, PollingConsumer};

use crate::config::{SurrealDbEndpointConfig, SurrealDbOperation};
use crate::error::SurrealDbError;
use crate::producer::SurrealClient;

pub struct SurrealDbPollingConsumer {
    config: SurrealDbEndpointConfig,
    catalog: Option<Arc<dyn DatasourceCatalog>>,
}

impl SurrealDbPollingConsumer {
    pub fn new(
        config: SurrealDbEndpointConfig,
        catalog: Option<Arc<dyn DatasourceCatalog>>,
    ) -> Self {
        Self { config, catalog }
    }

    async fn resolve_client(&self) -> Result<SurrealClient, SurrealDbError> {
        let catalog = self
            .catalog
            .as_ref()
            .ok_or_else(|| SurrealDbError::DatasourceNotFound("no datasource catalog".into()))?;
        let handle = catalog
            .get_pool(&self.config.datasource)
            .await
            .map_err(|e| SurrealDbError::DatasourceNotFound(e.to_string()))?;
        let client: Arc<SurrealClient> = handle
            .downcast()
            .map_err(|e| SurrealDbError::DowncastFailed(e.to_string()))?;
        Ok((*client).clone())
    }

    /// Build the polling query based on operation.
    fn build_poll_query(&self) -> Result<String, SurrealDbError> {
        match self.config.operation {
            SurrealDbOperation::Select => {
                let _table = self
                    .config
                    .table
                    .as_deref()
                    .ok_or_else(|| SurrealDbError::MissingParam("table".into()))?;
                let _id = self
                    .config
                    .id
                    .as_deref()
                    .ok_or_else(|| SurrealDbError::MissingParam("id".into()))?;
                // Use type::record() for safe RecordId construction (v3 rename from type::thing)
                Ok("SELECT * FROM type::record($table, $id)".into())
            }
            SurrealDbOperation::Query => self
                .config
                .query
                .as_deref()
                .ok_or_else(|| {
                    SurrealDbError::MissingParam(
                        "query (in URI) — PollingConsumer requires static query".into(),
                    )
                })
                .map(|s| s.to_string()),
            other => Err(SurrealDbError::NotSupported(format!(
                "operation '{other}' is not supported by PollingConsumer"
            ))),
        }
    }
}

#[async_trait]
impl PollingConsumer for SurrealDbPollingConsumer {
    async fn receive(&mut self, timeout: Duration) -> Result<Option<Exchange>, CamelError> {
        let sql = self.build_poll_query().map_err(CamelError::from)?;
        let client = self.resolve_client().await.map_err(CamelError::from)?;

        debug!(sql = %sql, "surrealdb polling consumer receive");

        let mut query_builder = client.query(sql.as_str());

        // Bind table and id for select operation
        if self.config.operation == SurrealDbOperation::Select {
            let table = self.config.table.clone().unwrap_or_default();
            let id = self.config.id.clone().unwrap_or_default();
            query_builder = query_builder.bind(("table", table)).bind(("id", id));
        }

        // Execute with timeout
        let result = tokio::time::timeout(timeout, query_builder).await;

        match result {
            Ok(Ok(mut response)) => {
                let results: Vec<JsonValue> = response.take(0).map_err(|e| {
                    CamelError::ProcessorError(format!("surrealdb polling take failed: {e}"))
                })?;
                if results.is_empty() {
                    Ok(None)
                } else {
                    let record = results.into_iter().next().unwrap();
                    let exchange = Exchange::new(Message::new(Body::Json(record)));
                    Ok(Some(exchange))
                }
            }
            Ok(Err(e)) => {
                warn!(error = %e, "surrealdb polling query failed");
                Err(CamelError::ProcessorError(format!(
                    "surrealdb polling query failed: {e}"
                )))
            }
            Err(_) => {
                warn!("surrealdb polling query timed out");
                Err(CamelError::ProcessorError(
                    "surrealdb polling query timed out".into(),
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SurrealDbEndpointConfig;

    fn make_config(uri: &str) -> SurrealDbEndpointConfig {
        SurrealDbEndpointConfig::from_uri(uri).unwrap()
    }

    #[test]
    fn build_poll_query_select() {
        let config = make_config("surrealdb:select?datasource=main&table=docs&id=doc:1");
        let poller = SurrealDbPollingConsumer::new(config, None);
        let sql = poller.build_poll_query().unwrap();
        assert!(sql.contains("SELECT"), "select op must build SELECT: {sql}");
        // Uses type::record($table, $id) — table/id are bound params, not inlined
        assert!(
            sql.contains("type::record"),
            "select op must use type::record: {sql}"
        );
        assert!(
            sql.contains("$table"),
            "select op must bind $table param: {sql}"
        );
        assert!(sql.contains("$id"), "select op must bind $id param: {sql}");
    }

    #[test]
    fn build_poll_query_query() {
        let config = make_config("surrealdb:query?datasource=main&query=SELECT%20*%20FROM%20docs");
        let poller = SurrealDbPollingConsumer::new(config, None);
        let sql = poller.build_poll_query().unwrap();
        assert!(
            sql.contains("SELECT"),
            "query op must pass through SQL: {sql}"
        );
    }

    #[test]
    fn build_poll_query_rejects_live() {
        let config = make_config("surrealdb:live?datasource=main&table=events");
        let poller = SurrealDbPollingConsumer::new(config, None);
        assert!(
            poller.build_poll_query().is_err(),
            "LIVE must not be pollable"
        );
    }

    #[test]
    fn build_poll_query_rejects_create() {
        let config = make_config("surrealdb:create?datasource=main&table=docs");
        let poller = SurrealDbPollingConsumer::new(config, None);
        assert!(
            poller.build_poll_query().is_err(),
            "CREATE must not be pollable"
        );
    }
}
