//! SurrealDbConsumer — LIVE SELECT push-based consumer.
//!
//! Uses the surrealdb v3 Stream API (`db.select().live().await`) to receive
//! real-time notifications when records in a table are created, updated, or
//! deleted. Each notification is converted to an Exchange and sent into the
//! route pipeline via `ConsumerContext`.
//!
//! # Stream lifecycle
//!
//! - `start()` spawns a background task that reads from the LIVE SELECT stream.
//! - On shutdown, the runtime cancels `ConsumerContext::cancelled()`, the task
//!   exits, and the Stream's `Drop` automatically sends KILL to the server.
//! - No manual UUID capture or KILL is needed — the SDK handles cleanup.
//!
//! # Error handling (ADR-0012)
//!
//! - Pipeline errors (b' taxonomy: pipeline rejects the exchange): logged at
//!   `warn!`, handler-owned.
//! - Live query errors (Action::Error): logged at `warn!`, consumer continues.
//! - Stream errors: returned as `CamelError::ProcessorError` → supervision
//!   treats this as a consumer crash; propagated via background_task_handle()
//!   for runtime supervision (ADR-0007).

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use serde_json::Value as JsonValue;
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

use camel_api::datasource::DatasourceCatalog;
use camel_component_api::{
    Body, CamelError, ConcurrencyModel, Consumer, ConsumerContext, ConsumerStartupMode, Exchange,
    Message, RuntimeObservability, Value,
};
use surrealdb::types::Action;
use surrealdb::types::Value as SurrealValue;

use crate::config::SurrealDbEndpointConfig;
use crate::error::SurrealDbError;
use crate::headers;
use crate::producer::SurrealClient;

/// Establishes a LIVE SELECT subscription on `table`, bounded by
/// `operation_timeout` (design site 8).
///
/// Extracted from `SurrealDbConsumer::start` so the production wrapper is
/// directly testable. The SDK's `Select::live()` builder is awaited inside
/// [`crate::error::bound_op`], which drops it at the deadline and returns the
/// named [`SurrealDbError::Timeout`]; non-timeout SDK failures surface as
/// [`SurrealDbError::Query`] with the source preserved.
pub(crate) async fn subscribe_live(
    client: &SurrealClient,
    table: &str,
    operation_timeout: Duration,
) -> Result<surrealdb::Stream<Vec<SurrealValue>>, SurrealDbError> {
    let builder = client.select::<Vec<SurrealValue>>(table).live();
    crate::error::bound_op("live subscription", operation_timeout, async {
        builder.await.map_err(SurrealDbError::query)
    })
    .await
}

/// Push-based consumer for SurrealDB LIVE SELECT notifications.
///
/// Struct is deliberately simple — no `live_query_id` field. The
/// `surrealdb::Stream` drives its own lifecycle and cleanup via Drop.
pub struct SurrealDbConsumer {
    config: SurrealDbEndpointConfig,
    catalog: Option<Arc<dyn DatasourceCatalog>>,
    runtime: Arc<dyn RuntimeObservability>,
    task_handle: Option<JoinHandle<Result<(), CamelError>>>,
}

impl SurrealDbConsumer {
    pub fn new(
        config: SurrealDbEndpointConfig,
        catalog: Option<Arc<dyn DatasourceCatalog>>,
        runtime: Arc<dyn RuntimeObservability>,
    ) -> Self {
        Self {
            config,
            catalog,
            runtime,
            task_handle: None,
        }
    }

    /// Resolve the SurrealDB client from the datasource catalog.
    async fn resolve_client(&self) -> Result<SurrealClient, SurrealDbError> {
        let catalog = self.catalog.as_ref().ok_or_else(|| {
            SurrealDbError::DatasourceNotFound("no datasource catalog available".into())
        })?;
        let handle = catalog
            .get_pool(&self.config.datasource)
            .await
            .map_err(|e| {
                SurrealDbError::DatasourceNotFound(format!(
                    "datasource '{}': {e}",
                    self.config.datasource
                ))
            })?;
        let client: Arc<SurrealClient> = handle.downcast().map_err(|e| {
            SurrealDbError::DowncastFailed(format!("datasource '{}': {e}", self.config.datasource))
        })?;
        Ok((*client).clone())
    }

    /// Convert a live notification into an Exchange for the route pipeline.
    ///
    /// Extracted as a `pub(crate)` helper for testability. Returns an Exchange
    /// with:
    /// - Body: the notification data as JSON
    /// - `CamelSurrealDbAction` header: action string (CREATE / UPDATE / DELETE)
    /// - `CamelSurrealDbTable` header: table name
    pub(crate) fn notification_to_exchange(
        data: SurrealValue,
        action: &str,
        table: &str,
    ) -> Exchange {
        let json_value: JsonValue = data.into_json_value();
        let mut msg = Message::new(Body::Json(json_value));
        msg.headers
            .insert(headers::ACTION.into(), Value::String(action.to_string()));
        msg.headers
            .insert(headers::TABLE.into(), Value::String(table.to_string()));
        Exchange::new(msg)
    }
}

#[async_trait]
impl Consumer for SurrealDbConsumer {
    async fn start(&mut self, context: ConsumerContext) -> Result<(), CamelError> {
        // The query operation is the live-query establishment (datasource
        // resolution + LIVE SELECT bind) — the consumer's remaining
        // principal op now that pipeline rejections carry the retained
        // `b-prime:surrealdb:notification` label. Facade failures land on
        // `e:surrealdb:query` (never equal to the b-prime label, so a
        // pipeline rejection does NOT double-count here); the component
        // series flows only with the lever on (dashboard-observability 4.2).
        let component_metrics = self.runtime.component_metrics("surrealdb");
        let established = async {
            let client = self.resolve_client().await.map_err(CamelError::from)?;
            let table = self.config.table.clone().ok_or_else(|| {
                CamelError::Config("surrealdb live consumer requires table parameter".into())
            })?;

            crate::query::validate_identifier(&table).map_err(CamelError::from)?;

            // Start the LIVE SELECT stream using the SDK's fluent API, under
            // the configured operation bound (design site 8).
            let stream = subscribe_live(&client, &table, self.config.operation_timeout)
                .await
                .map_err(|e| match e {
                    SurrealDbError::Timeout { .. } => CamelError::from(e),
                    other => {
                        CamelError::ProcessorError(format!("surrealdb LIVE SELECT failed: {other}"))
                    }
                })?;

            Ok((client, table, stream))
        }
        .await;

        let (client, table, mut stream) = match established {
            Ok(established) => {
                component_metrics.observe("query", false);
                established
            }
            Err(e) => {
                component_metrics.observe("query", true);
                return Err(e);
            }
        };

        // LIVE SELECT bind succeeded — signal readiness before entering the
        // background task so the runtime knows the route is fully started.
        context.mark_ready();

        let route_id = context.route_id().to_string();
        let route_id_for_task = route_id.clone();
        let table_for_task = table.clone();
        let runtime_for_task = Arc::clone(&self.runtime);
        let live_session = client;

        let handle = tokio::spawn(async move {
            let _live_session = live_session;
            loop {
                tokio::select! {
                    _ = context.cancelled() => {
                        info!(
                            route_id = %route_id_for_task,
                            table = %table_for_task,
                            "surrealdb live consumer cancelled by shutdown"
                        );
                        // Stream dropped here → automatic KILL
                        drop(stream);
                        return Ok(());
                    }
                    notification = stream.next() => {
                        match notification {
                            Some(Ok(notif)) => {
                                // Action::Killed means server killed the live query
                                if matches!(notif.action, Action::Killed) {
                                    info!(
                                        route_id = %route_id_for_task,
                                        table = %table_for_task,
                                        "surrealdb live stream killed by server"
                                    );
                                    return Ok(());
                                }

                                // Action::Error means a WHERE/projection evaluation
                                // error — log and continue
                                if matches!(notif.action, Action::Error) {
                                    warn!(
                                        route_id = %route_id_for_task,
                                        table = %table_for_task,
                                        "surrealdb live query evaluation error in notification"
                                    );
                                    continue;
                                }

                                let action_str = notif.action.to_string();
                                let exchange = Self::notification_to_exchange(
                                    notif.data,
                                    &action_str,
                                    &table_for_task,
                                );

                                // send_and_wait: pipeline processes exchange, we
                                // wait for the result. Pipeline errors are
                                // handler-owned (b' taxonomy), logged at warn!.
                                if let Err(e) = context.send_and_wait(exchange).await {
                                    // b-prime: locally terminal notification-send failure (wired at the increment_errors site below)
                                    runtime_for_task
                                        .metrics()
                                        .increment_errors(&route_id_for_task, "b-prime:surrealdb:notification");
                                    warn!(
                                        route_id = %route_id_for_task,
                                        error = %e,
                                        "pipeline error processing live notification (b-prime)"
                                    );
                                }
                            }
                            Some(Err(e)) => {
                                // Stream-level error (e.g. deserialization
                                // failure) → crash the consumer for supervision.
                                return Err(CamelError::ProcessorError(format!(
                                    "live stream error: {e}"
                                )));
                            }
                            None => {
                                // Stream ended naturally (e.g. server closed
                                // connection, or all consumers disconnected).
                                info!(
                                    route_id = %route_id_for_task,
                                    table = %table_for_task,
                                    "surrealdb live stream ended"
                                );
                                return Ok(());
                            }
                        }
                    }
                }
            }
        });

        self.task_handle = Some(handle);
        info!(
            route_id = %route_id,
            table = %table,
            "surrealdb live consumer started"
        );
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), CamelError> {
        // The runtime has already cancelled ConsumerContext's cancel token
        // before calling stop(), so the background task has exited (or is
        // about to exit). We await the handle with a timeout for safety.
        if let Some(handle) = self.task_handle.take() {
            match tokio::time::timeout(Duration::from_secs(5), handle).await {
                Ok(Ok(Ok(()))) => {
                    debug!("surrealdb consumer task exited cleanly");
                }
                Ok(Ok(Err(e))) => {
                    // Task exited with an application error (crash). Log at
                    // warn! per ADR-0012 (route supervision owns the error!
                    // signal in the g taxonomy).
                    warn!(
                        error = %e,
                        "surrealdb consumer task exited with error"
                    );
                }
                Ok(Err(join_err)) => {
                    warn!(
                        error = %join_err,
                        "surrealdb consumer task panicked"
                    );
                }
                Err(_) => {
                    warn!("surrealdb consumer task did not stop within 5s");
                }
            }
        }
        // No manual KILL needed — the Stream was dropped when the background
        // task exited, which sends KILL via the Stream's Drop impl.
        info!("surrealdb consumer stopped");
        Ok(())
    }

    fn concurrency_model(&self) -> ConcurrencyModel {
        // LIVE SELECT is sequential: one notification at a time, in order.
        ConcurrencyModel::Sequential
    }

    fn startup_mode(&self) -> ConsumerStartupMode {
        ConsumerStartupMode::Explicit
    }

    fn background_task_handle(&mut self) -> Option<JoinHandle<Result<(), CamelError>>> {
        self.task_handle.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: build a minimal SurrealValue for testing notification conversion.
    fn make_surreal_value(name: &str, age: i64) -> SurrealValue {
        let mut map = surrealdb::types::Object::new();
        map.insert("name".to_string(), SurrealValue::String(name.to_string()));
        map.insert("age".to_string(), SurrealValue::Number(age.into()));
        SurrealValue::Object(map)
    }

    // =========================================================================
    // notification_to_exchange tests
    // =========================================================================

    #[test]
    fn notification_to_exchange_sets_body_and_headers() {
        let data = make_surreal_value("alice", 30);
        let exchange = SurrealDbConsumer::notification_to_exchange(data, "CREATE", "users");
        // Body should be a JSON object
        assert!(
            matches!(&exchange.input.body, Body::Json(_)),
            "expected Body::Json"
        );
        // ACTION header
        assert_eq!(
            exchange.input.headers.get("CamelSurrealDbAction"),
            Some(&Value::String("CREATE".into()))
        );
        // TABLE header
        assert_eq!(
            exchange.input.headers.get("CamelSurrealDbTable"),
            Some(&Value::String("users".into()))
        );
    }

    #[test]
    fn notification_to_exchange_with_update_action() {
        let data = make_surreal_value("bob", 25);
        let exchange = SurrealDbConsumer::notification_to_exchange(data, "UPDATE", "profiles");
        assert_eq!(
            exchange.input.headers.get("CamelSurrealDbAction"),
            Some(&Value::String("UPDATE".into()))
        );
        assert_eq!(
            exchange.input.headers.get("CamelSurrealDbTable"),
            Some(&Value::String("profiles".into()))
        );
    }

    #[test]
    fn notification_to_exchange_with_delete_action() {
        let data = SurrealValue::Null;
        let exchange = SurrealDbConsumer::notification_to_exchange(data, "DELETE", "sessions");
        assert_eq!(
            exchange.input.headers.get("CamelSurrealDbAction"),
            Some(&Value::String("DELETE".into()))
        );
        assert_eq!(
            exchange.input.headers.get("CamelSurrealDbTable"),
            Some(&Value::String("sessions".into()))
        );
        // Null value should become Body::Json(JsonValue::Null)
        if let Body::Json(json) = &exchange.input.body {
            assert_eq!(*json, JsonValue::Null);
        } else {
            panic!("expected Body::Json(Null)");
        }
    }

    #[test]
    fn notification_to_exchange_converts_complex_value() {
        let mut inner = surrealdb::types::Object::new();
        inner.insert("x".to_string(), SurrealValue::Number(10.into()));
        inner.insert("y".to_string(), SurrealValue::Number(20.into()));

        let mut outer = surrealdb::types::Object::new();
        outer.insert("id".to_string(), SurrealValue::String("rec_1".to_string()));
        outer.insert("coord".to_string(), SurrealValue::Object(inner));

        let exchange = SurrealDbConsumer::notification_to_exchange(
            SurrealValue::Object(outer),
            "CREATE",
            "points",
        );
        // Verify body is valid JSON and contains nested structure
        if let Body::Json(json) = &exchange.input.body {
            assert_eq!(json.get("id"), Some(&JsonValue::String("rec_1".into())));
            assert!(json.get("coord").is_some());
        } else {
            panic!("expected Body::Json");
        }
    }

    // --- design site 8: bounded live subscription ---

    #[tokio::test]
    async fn live_subscription_elapsed_arm_trips_named_timeout() {
        // The SAME production wrapper `subscribe_live` routes through: a
        // pending builder future must trip the bound and name the phase.
        tokio::time::pause();
        let err = crate::error::bound_op::<surrealdb::Stream<Vec<SurrealValue>>, _>(
            "live subscription",
            Duration::from_millis(5000),
            std::future::pending(),
        )
        .await
        .expect_err("a pending subscription must trip the deadline");
        let msg = err.to_string();
        assert!(
            msg.contains("live subscription"),
            "error must name the phase: {msg}"
        );
        assert!(msg.contains("5000ms"), "error must name the bound: {msg}");
    }

    #[tokio::test]
    async fn subscribe_live_on_mem_succeeds() {
        use camel_api::datasource::{DatasourceConfig, PoolFactory};

        use crate::pool_factory::SurrealDbPoolFactory;

        let config = DatasourceConfig {
            db_url: "mem://".to_string(),
            provider: Some("surrealdb".into()),
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
        let handle = SurrealDbPoolFactory
            .create(&config)
            .await
            .expect("mem:// create must succeed");
        let client = handle
            .downcast::<SurrealClient>()
            .expect("handle must be a Surreal<Any> client");
        // LIVE SELECT resolves the table up front; define it so the
        // subscription targets a real (schemaless) table.
        client
            .query("DEFINE TABLE events SCHEMALESS")
            .await
            .expect("define-table transport must succeed")
            .check()
            .expect("define-table statement must succeed");

        // The subscription wrapper must be transparent: the embedded tier
        // supports LIVE queries, so a healthy subscribe returns the stream.
        let stream = subscribe_live(&client, "events", Duration::from_secs(30))
            .await
            .expect("mem:// supports LIVE queries");
        // Dropping the stream sends KILL (see the module docs).
        drop(stream);
    }
}
