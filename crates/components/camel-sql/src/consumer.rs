use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value as JsonValue;
use sqlx::AnyPool;
use sqlx::any::AnyPoolOptions;
use sqlx::any::AnyRow;
use tokio::sync::OnceCell;
use tracing::{error, info, warn};

use camel_api::{Body, CamelError, Exchange, Message};
use camel_component::{ConcurrencyModel, Consumer, ConsumerContext};

use crate::config::SqlConfig;
use crate::headers;
use crate::query::{QueryTemplate, parse_query_template, resolve_params};
use crate::utils::{bind_json_values, row_to_json};

pub struct SqlConsumer {
    pub(crate) config: SqlConfig,
    pub(crate) pool: Arc<OnceCell<AnyPool>>,
}

impl SqlConsumer {
    pub fn new(config: SqlConfig, pool: Arc<OnceCell<AnyPool>>) -> Self {
        Self { config, pool }
    }

    /// Poll the database for new rows and process them.
    async fn poll_database(
        &self,
        pool: &AnyPool,
        context: &ConsumerContext,
        template: &QueryTemplate,
    ) -> Result<(), CamelError> {
        // Create an empty exchange for parameter resolution (consumer has no input)
        let empty_exchange = Exchange::new(Message::default());

        // Resolve parameters
        let prepared = resolve_params(template, &empty_exchange)?;

        // Build and execute the query
        let query = bind_json_values(sqlx::query(&prepared.sql), &prepared.bindings);
        let rows: Vec<AnyRow> = query
            .fetch_all(pool)
            .await
            .map_err(|e| CamelError::ProcessorError(format!("Query execution failed: {}", e)))?;

        // Check for empty result set
        if rows.is_empty() && !self.config.route_empty_result_set {
            return Ok(());
        }

        // Apply max_messages_per_poll limit
        let rows_to_process: Vec<AnyRow> = if let Some(max) = self.config.max_messages_per_poll {
            if max > 0 {
                rows.into_iter().take(max as usize).collect()
            } else {
                rows
            }
        } else {
            rows
        };

        if self.config.use_iterator {
            // Process each row individually
            for row in rows_to_process {
                let row_json = row_to_json(&row)?;

                // Create exchange with the row as JSON body
                let mut msg = Message::new(Body::Json(row_json.clone()));

                // Set individual column headers with CamelSql. prefix per Apache Camel convention
                if let Some(obj) = row_json.as_object() {
                    for (key, value) in obj {
                        msg.set_header(format!("CamelSql.{}", key), value.clone());
                    }
                }

                let exchange = Exchange::new(msg);

                // Send and wait for processing
                let result = context.send_and_wait(exchange).await;

                // Handle post-processing (onConsume/onConsumeFailed)
                if let Err(e) = self.handle_post_processing(pool, &result, &row_json).await {
                    error!(error = %e, "Post-processing failed");
                    // Continue processing other rows even if post-processing fails
                    // unless break_batch_on_consume_fail is set
                    if self.config.break_batch_on_consume_fail {
                        return Err(e);
                    }
                }
            }
        } else {
            // Process all rows as a single batch
            let rows_json: Vec<JsonValue> = rows_to_process
                .iter()
                .map(row_to_json)
                .collect::<Result<Vec<_>, CamelError>>()?;

            let row_count = rows_json.len();

            // Create exchange with array of rows
            let mut msg = Message::new(Body::Json(JsonValue::Array(rows_json)));
            msg.set_header(headers::ROW_COUNT, JsonValue::Number(row_count.into()));

            let exchange = Exchange::new(msg);

            // Send and wait for result, then run post-processing with Null row
            let result = context.send_and_wait(exchange).await;
            if let Err(e) = self
                .handle_post_processing(pool, &result, &JsonValue::Null)
                .await
            {
                error!(error = %e, "Post-processing failed for batch");
                if self.config.break_batch_on_consume_fail {
                    return Err(e);
                }
            }
        }

        // Execute on_consume_batch_complete if configured
        if let Some(ref batch_query) = self.config.on_consume_batch_complete
            && let Err(e) = self
                .execute_post_query(pool, batch_query, &JsonValue::Null)
                .await
        {
            error!(error = %e, "onConsumeBatchComplete query failed");
        }

        Ok(())
    }

    /// Handle post-processing after a row is processed (onConsume/onConsumeFailed).
    async fn handle_post_processing(
        &self,
        pool: &AnyPool,
        result: &Result<Exchange, CamelError>,
        row_json: &JsonValue,
    ) -> Result<(), CamelError> {
        match result {
            Ok(_) => {
                // Success - execute onConsume if configured
                if let Some(ref on_consume) = self.config.on_consume {
                    self.execute_post_query(pool, on_consume, row_json).await?;
                }
            }
            Err(_) => {
                // Failure - execute onConsumeFailed if configured
                if let Some(ref on_consume_failed) = self.config.on_consume_failed {
                    self.execute_post_query(pool, on_consume_failed, row_json)
                        .await?;
                }
            }
        }
        Ok(())
    }

    /// Execute a post-processing query with the row data as parameters.
    async fn execute_post_query(
        &self,
        pool: &AnyPool,
        query_str: &str,
        row_json: &JsonValue,
    ) -> Result<(), CamelError> {
        // Parse the query template
        let template = parse_query_template(query_str, self.config.placeholder)?;

        // Create a temporary exchange with the row as body for parameter resolution
        // Populate CamelSql.* headers so named params can reference them
        let mut temp_msg = Message::new(Body::Json(row_json.clone()));
        if let Some(obj) = row_json.as_object() {
            for (key, value) in obj {
                temp_msg.set_header(format!("CamelSql.{}", key), value.clone());
            }
        }
        let temp_exchange = Exchange::new(temp_msg);

        // Resolve parameters
        let prepared = resolve_params(&template, &temp_exchange)?;

        // Build and execute the query
        let query = bind_json_values(sqlx::query(&prepared.sql), &prepared.bindings);
        let result = query.execute(pool).await.map_err(|e| {
            CamelError::ProcessorError(format!("Post-query execution failed: {}", e))
        })?;

        // Warn if 0 rows affected (the row may not have been marked correctly)
        if result.rows_affected() == 0 {
            warn!(
                query = query_str,
                "Post-processing query affected 0 rows — the row may not have been marked correctly"
            );
        }

        Ok(())
    }
}

#[async_trait]
impl Consumer for SqlConsumer {
    async fn start(&mut self, context: ConsumerContext) -> Result<(), CamelError> {
        // Step 1: Initialize the connection pool
        let pool = self
            .pool
            .get_or_try_init(|| async {
                AnyPoolOptions::new()
                    .max_connections(self.config.max_connections)
                    .min_connections(self.config.min_connections)
                    .idle_timeout(Duration::from_secs(self.config.idle_timeout_secs))
                    .max_lifetime(Duration::from_secs(self.config.max_lifetime_secs))
                    .connect(&self.config.db_url)
                    .await
                    .map_err(|e| {
                        CamelError::EndpointCreationFailed(format!(
                            "Failed to connect to database: {}",
                            e
                        ))
                    })
            })
            .await?;

        // Warn if no onConsume configured
        if self.config.on_consume.is_none() {
            warn!(
                "SQL consumer started without onConsume configured — consumed rows will not be marked/deleted"
            );
        }

        // Step 2: Parse query template once (avoid re-parsing every poll)
        let template = parse_query_template(&self.config.query, self.config.placeholder)
            .map_err(|e| CamelError::Config(format!("Invalid query template: {}", e)))?;

        // Step 3: Initial delay before starting polling
        if self.config.initial_delay_ms > 0 {
            tokio::select! {
                _ = context.cancelled() => {
                    info!("SQL consumer stopped during initial delay");
                    return Ok(());
                }
                _ = tokio::time::sleep(Duration::from_millis(self.config.initial_delay_ms)) => {}
            }
        }

        // Step 4: Polling loop
        loop {
            tokio::select! {
                _ = context.cancelled() => {
                    info!("SQL consumer stopped");
                    break;
                }
                _ = tokio::time::sleep(Duration::from_millis(self.config.delay_ms)) => {
                    if let Err(e) = self.poll_database(pool, &context, &template).await {
                        error!(error = %e, "SQL consumer poll failed");
                    }
                }
            }
        }

        Ok(())
    }

    async fn stop(&mut self) -> Result<(), CamelError> {
        Ok(())
    }

    fn concurrency_model(&self) -> ConcurrencyModel {
        // Sequential is correct for SQL consumers: concurrent polls would fetch
        // duplicate rows. The design doc mentioned SharedState (which doesn't exist
        // in this runtime) — Sequential is the correct equivalent.
        ConcurrencyModel::Sequential
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SqlConfig;

    fn test_config() -> SqlConfig {
        SqlConfig::from_uri("sql:select * from t?db_url=postgres://localhost/test").unwrap()
    }

    #[test]
    fn test_consumer_concurrency_model() {
        let c = SqlConsumer::new(test_config(), Arc::new(OnceCell::new()));
        assert_eq!(c.concurrency_model(), ConcurrencyModel::Sequential);
    }

    #[test]
    fn test_consumer_stores_config() {
        let config = SqlConfig::from_uri(
            "sql:select * from t?db_url=postgres://localhost/test&delay=2000&onConsume=update t set done=true"
        ).unwrap();
        let c = SqlConsumer::new(config.clone(), Arc::new(OnceCell::new()));
        assert_eq!(c.config.delay_ms, 2000);
        assert!(c.config.on_consume.is_some());
    }
}
