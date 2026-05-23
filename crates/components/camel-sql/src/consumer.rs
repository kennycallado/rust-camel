use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value as JsonValue;
use sqlx::AnyPool;
use sqlx::any::AnyPoolOptions;
use sqlx::any::AnyRow;
use tokio::sync::OnceCell;
use tracing::{debug, error, info, warn};

use camel_component_api::{Body, CamelError, Exchange, Message};
use camel_component_api::{ConcurrencyModel, Consumer, ConsumerContext};

use crate::config::{
    PollStrategy, ProcessingStrategy, SqlEndpointConfig, TransactionMode, enrich_db_url_with_ssl,
    redact_db_url,
};
use crate::headers;
use crate::query::{QueryTemplate, parse_query_template, resolve_params};
use crate::utils::{bind_json_values, row_to_json};

pub struct SqlConsumer {
    pub(crate) config: SqlEndpointConfig,
    pub(crate) pool: Arc<OnceCell<AnyPool>>,
    stopped: bool,
}

impl SqlConsumer {
    pub fn new(config: SqlEndpointConfig, pool: Arc<OnceCell<AnyPool>>) -> Self {
        Self {
            config,
            pool,
            stopped: false,
        }
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
        let prepared = resolve_params(template, &empty_exchange, &self.config.in_separator)?;

        // Build and execute the query
        debug!(query = %prepared.sql, "executing SQL consumer poll");
        let query = bind_json_values(sqlx::query(&prepared.sql), &prepared.bindings);
        let rows: Vec<AnyRow> = query.fetch_all(pool).await.map_err(|e| {
            warn!(error = %e, "SQL consumer poll query failed");
            CamelError::ProcessorError(format!("Query execution failed: {}", e))
        })?;

        debug!(rows = rows.len(), "SQL consumer poll completed");

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
                    if self.config.break_batch_on_consume_fail {
                        return Err(e);
                    }
                }

                // If downstream processing itself failed, honour break_batch_on_consume_fail
                if let Err(ref consume_err) = result
                    && self.config.break_batch_on_consume_fail
                {
                    return Err(consume_err.clone());
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
            let mut msg = Message::new(Body::Json(JsonValue::Array(rows_json.clone())));
            msg.set_header(headers::ROW_COUNT, JsonValue::Number(row_count.into()));

            let exchange = Exchange::new(msg);

            // Send and wait for result
            let result = context.send_and_wait(exchange).await;

            // SQL-021: Run per-row post-processing even in batch mode so that
            // onConsume/onConsumeFailed queries can reference row-specific parameters
            // (e.g. `:#id`). Each row gets its own post-processing query execution.
            for row_json in rows_json.iter() {
                if let Err(e) = self.handle_post_processing(pool, &result, row_json).await {
                    error!(error = %e, "Post-processing failed for batch row");
                    if self.config.break_batch_on_consume_fail {
                        return Err(e);
                    }
                }
            }

            // If downstream processing itself failed, honour break_batch_on_consume_fail
            if let Err(ref consume_err) = result
                && self.config.break_batch_on_consume_fail
            {
                return Err(consume_err.clone());
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
        let prepared = resolve_params(&template, &temp_exchange, &self.config.in_separator)?;

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

    async fn bridge_poll_error(
        &self,
        context: &ConsumerContext,
        error: CamelError,
    ) -> Result<(), CamelError> {
        if !self.config.bridge_error_handler {
            return Ok(());
        }
        let mut exchange = Exchange::new(Message::default());
        exchange.set_error(error);
        context.send_and_wait(exchange).await.map(|_| ())
    }
}

#[async_trait]
impl Consumer for SqlConsumer {
    async fn start(&mut self, context: ConsumerContext) -> Result<(), CamelError> {
        // Reject double-start
        if self.stopped {
            return Err(CamelError::Config(
                "SQL consumer cannot be restarted after stop".into(),
            ));
        }

        // Step 1: Initialize the connection pool
        let pool = self
            .pool
            .get_or_try_init(|| async {
                // Defensive: ensure config is resolved even if caller didn't use create_endpoint
                self.config.resolve_defaults();
                // SQL-014: resolve file-based query asynchronously (not blocking)
                self.config.resolve_file_query().await?;

                // Install all compiled-in sqlx drivers so AnyPool can resolve them.
                // This is idempotent; safe to call multiple times.
                sqlx::any::install_default_drivers();
                let db_url = enrich_db_url_with_ssl(&self.config.db_url, &self.config)?;

                let max_conn = self.config.max_connections.ok_or_else(|| {
                    CamelError::Config("max_connections not resolved for SQL consumer pool".into())
                })?;
                let min_conn = self.config.min_connections.ok_or_else(|| {
                    CamelError::Config("min_connections not resolved for SQL consumer pool".into())
                })?;
                let idle_timeout = self.config.idle_timeout_secs.ok_or_else(|| {
                    CamelError::Config(
                        "idle_timeout_secs not resolved for SQL consumer pool".into(),
                    )
                })?;
                let max_lifetime = self.config.max_lifetime_secs.ok_or_else(|| {
                    CamelError::Config(
                        "max_lifetime_secs not resolved for SQL consumer pool".into(),
                    )
                })?;

                info!(
                    db_url = %redact_db_url(&self.config.db_url),
                    "SQL consumer pool initializing"
                );
                AnyPoolOptions::new()
                    .max_connections(max_conn)
                    .min_connections(min_conn)
                    .idle_timeout(Duration::from_secs(idle_timeout))
                    .max_lifetime(Duration::from_secs(max_lifetime))
                    .connect(&db_url)
                    .await
                    .map_err(|e| {
                        CamelError::EndpointCreationFailed(format!(
                            "Failed to connect to database: {}",
                            e
                        ))
                    })
            })
            .await?;

        // SQL-002: warn if Managed transaction mode requested
        if self.config.transaction_mode == TransactionMode::Managed {
            warn!("transactionManager not yet implemented; using Auto mode");
        }

        // SQL-017/SQL-018: log processing and poll strategies
        if self.config.processing_strategy == ProcessingStrategy::Scheduled {
            debug!(
                "Processing strategy: Scheduled (rows dispatched individually via send_and_wait)"
            );
        }
        if self.config.poll_strategy == PollStrategy::Burst {
            debug!("Poll strategy: Burst (rapid successive polls)");
        }

        // Warn if no onConsume configured
        if self.config.on_consume.is_none() {
            warn!(
                "SQL consumer started without onConsume configured — consumed rows will not be marked/deleted"
            );
        }

        info!(
            db_url = %redact_db_url(&self.config.db_url),
            query_len = self.config.query.len(),
            "SQL consumer started"
        );

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
        let mut poll_count: u32 = 0;
        loop {
            // SQL-015: check repeat_count limit
            if let Some(max_repeats) = self.config.repeat_count
                && poll_count >= max_repeats
            {
                info!(
                    repeat_count = max_repeats,
                    "SQL consumer reached repeat_count limit, stopping"
                );
                break;
            }

            tokio::select! {
                _ = context.cancelled() => {
                    info!("SQL consumer stopped");
                    break;
                }
                _ = tokio::time::sleep(Duration::from_millis(self.config.delay_ms)) => {
                    poll_count += 1;
                    if let Err(e) = self.poll_database(pool, &context, &template).await {
                        error!(error = %e, "SQL consumer poll failed");
                        if let Err(route_err) = self.bridge_poll_error(&context, e).await {
                            error!(error = %route_err, "Failed to bridge SQL consumer error to route");
                        }
                    }
                }
            }
        }

        Ok(())
    }

    async fn stop(&mut self) -> Result<(), CamelError> {
        // Double-stop is safe — no-op after first stop
        if self.stopped {
            debug!("SQL consumer stop called on already-stopped consumer");
            return Ok(());
        }

        // Close the connection pool if it was initialized
        if let Some(pool) = self.pool.get() {
            debug!("SQL consumer closing connection pool");
            pool.close().await;
            debug!("SQL consumer pool closed");
        }

        self.stopped = true;
        info!("SQL consumer stopped");
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
    use crate::config::SqlEndpointConfig;
    use camel_component_api::ExchangeEnvelope;
    use camel_component_api::UriConfig;
    use sqlx::any::AnyPoolOptions;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::sync::mpsc;
    use tokio_util::sync::CancellationToken;

    async fn sqlite_pool() -> AnyPool {
        sqlx::any::install_default_drivers();
        AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("sqlite pool")
    }

    async fn seed_consumer_table(pool: &AnyPool) {
        sqlx::query("CREATE TABLE jobs (id INTEGER PRIMARY KEY, processed INTEGER DEFAULT 0, failed INTEGER DEFAULT 0)")
            .execute(pool)
            .await
            .expect("create table");
        sqlx::query("INSERT INTO jobs (id, processed, failed) VALUES (1, 0, 0), (2, 0, 0)")
            .execute(pool)
            .await
            .expect("seed rows");
    }

    fn config() -> SqlEndpointConfig {
        let mut c =
            SqlEndpointConfig::from_uri("sql:select * from t?db_url=postgres://localhost/test")
                .unwrap();
        c.resolve_defaults();
        c
    }

    #[test]
    fn consumer_concurrency_model() {
        let c = SqlConsumer::new(config(), Arc::new(OnceCell::new()));
        assert_eq!(c.concurrency_model(), ConcurrencyModel::Sequential);
    }

    #[test]
    fn consumer_stores_config() {
        let mut config = SqlEndpointConfig::from_uri(
            "sql:select * from t?db_url=postgres://localhost/test&delay=2000&onConsume=update t set done=true"
        ).unwrap();
        config.resolve_defaults();
        let c = SqlConsumer::new(config.clone(), Arc::new(OnceCell::new()));
        assert_eq!(c.config.delay_ms, 2000);
        assert!(c.config.on_consume.is_some());
    }

    #[tokio::test]
    async fn poll_database_runs_on_consume_for_successful_rows() {
        let pool = sqlite_pool().await;
        seed_consumer_table(&pool).await;

        let mut config = SqlEndpointConfig::from_uri(
            "sql:select id, processed, failed from jobs where processed = 0 order by id?db_url=sqlite::memory:&onConsume=update jobs set processed=1 where id=:#id&initialDelay=0&delay=1",
        )
        .unwrap();
        config.resolve_defaults();

        let consumer = SqlConsumer::new(config.clone(), Arc::new(OnceCell::new()));
        let template = parse_query_template(&config.query, config.placeholder).unwrap();

        let (tx, mut rx) = mpsc::channel::<ExchangeEnvelope>(8);
        tokio::spawn(async move {
            while let Some(env) = rx.recv().await {
                if let Some(reply_tx) = env.reply_tx {
                    let _ = reply_tx.send(Ok(env.exchange));
                }
            }
        });
        let ctx = ConsumerContext::new(tx, CancellationToken::new());

        consumer
            .poll_database(&pool, &ctx, &template)
            .await
            .expect("poll must succeed");

        let row = sqlx::query("select processed from jobs where id = 1")
            .fetch_one(&pool)
            .await
            .expect("row 1");
        let processed_1: i64 = sqlx::Row::try_get(&row, 0).expect("processed");

        let row = sqlx::query("select processed from jobs where id = 2")
            .fetch_one(&pool)
            .await
            .expect("row 2");
        let processed_2: i64 = sqlx::Row::try_get(&row, 0).expect("processed");

        assert_eq!(processed_1, 1);
        assert_eq!(processed_2, 1);
    }

    #[tokio::test]
    async fn poll_database_runs_on_consume_failed_when_downstream_fails() {
        let pool = sqlite_pool().await;
        seed_consumer_table(&pool).await;

        let mut config = SqlEndpointConfig::from_uri(
            "sql:select id, processed, failed from jobs where processed = 0 order by id?db_url=sqlite::memory:&onConsumeFailed=update jobs set failed=1 where id=:#id&initialDelay=0&delay=1",
        )
        .unwrap();
        config.resolve_defaults();

        let consumer = SqlConsumer::new(config.clone(), Arc::new(OnceCell::new()));
        let template = parse_query_template(&config.query, config.placeholder).unwrap();

        let (tx, mut rx) = mpsc::channel::<ExchangeEnvelope>(8);
        tokio::spawn(async move {
            while let Some(env) = rx.recv().await {
                if let Some(reply_tx) = env.reply_tx {
                    let _ =
                        reply_tx.send(Err(CamelError::ProcessorError("downstream boom".into())));
                }
            }
        });
        let ctx = ConsumerContext::new(tx, CancellationToken::new());

        consumer
            .poll_database(&pool, &ctx, &template)
            .await
            .expect("consumer should swallow downstream errors when breakBatchOnConsumeFail=false");

        let row = sqlx::query("select failed from jobs where id = 1")
            .fetch_one(&pool)
            .await
            .expect("row 1");
        let failed_1: i64 = sqlx::Row::try_get(&row, 0).expect("failed");

        let row = sqlx::query("select failed from jobs where id = 2")
            .fetch_one(&pool)
            .await
            .expect("row 2");
        let failed_2: i64 = sqlx::Row::try_get(&row, 0).expect("failed");

        assert_eq!(failed_1, 1);
        assert_eq!(failed_2, 1);
    }

    #[tokio::test]
    async fn poll_database_breaks_batch_on_consume_fail() {
        let pool = sqlite_pool().await;
        seed_consumer_table(&pool).await;

        let mut config = SqlEndpointConfig::from_uri(
            "sql:select id, processed, failed from jobs where processed = 0 order by id?db_url=sqlite::memory:&onConsumeFailed=update jobs set failed=1 where id=:#id&breakBatchOnConsumeFail=true&initialDelay=0&delay=1",
        )
        .unwrap();
        config.resolve_defaults();

        let consumer = SqlConsumer::new(config.clone(), Arc::new(OnceCell::new()));
        let template = parse_query_template(&config.query, config.placeholder).unwrap();

        let (tx, mut rx) = mpsc::channel::<ExchangeEnvelope>(8);
        tokio::spawn(async move {
            while let Some(env) = rx.recv().await {
                if let Some(reply_tx) = env.reply_tx {
                    let _ =
                        reply_tx.send(Err(CamelError::ProcessorError("downstream boom".into())));
                }
            }
        });
        let ctx = ConsumerContext::new(tx, CancellationToken::new());

        let err = consumer
            .poll_database(&pool, &ctx, &template)
            .await
            .expect_err("must stop on first downstream failure");
        assert!(err.to_string().contains("downstream boom"));

        let row = sqlx::query("select failed from jobs where id = 1")
            .fetch_one(&pool)
            .await
            .expect("row 1");
        let failed_1: i64 = sqlx::Row::try_get(&row, 0).expect("failed");

        let row = sqlx::query("select failed from jobs where id = 2")
            .fetch_one(&pool)
            .await
            .expect("row 2");
        let failed_2: i64 = sqlx::Row::try_get(&row, 0).expect("failed");

        assert_eq!(failed_1, 1);
        assert_eq!(failed_2, 0, "second row must not be processed");
    }

    // --- Phase B hardening tests ---

    // SQL-001: Direct consumer construction without resolve_defaults does not panic.
    // The consumer defensively calls resolve_defaults() during pool init, so the pool
    // fields get resolved. This test verifies no panic occurs.
    #[tokio::test]
    async fn consumer_no_panic_without_prior_resolve_defaults() {
        let config = SqlEndpointConfig::from_uri(
            "sql:select 1?db_url=sqlite::memory:&initialDelay=0&delay=1",
        )
        .unwrap();
        // Deliberately NOT calling resolve_defaults() — pool fields remain None
        assert!(config.max_connections.is_none());

        let mut consumer = SqlConsumer::new(config, Arc::new(OnceCell::new()));
        let (tx, mut rx) = mpsc::channel::<ExchangeEnvelope>(8);
        tokio::spawn(async move {
            while let Some(env) = rx.recv().await {
                if let Some(reply_tx) = env.reply_tx {
                    let _ = reply_tx.send(Ok(env.exchange));
                }
            }
        });
        let token = CancellationToken::new();
        let ctx = ConsumerContext::new(tx, token.clone());

        // Spawn the consumer and cancel it quickly — it should not panic
        let consumer_handle = tokio::spawn(async move { consumer.start(ctx).await });

        // Cancel after a short delay
        tokio::time::sleep(Duration::from_millis(50)).await;
        token.cancel();

        let result = consumer_handle.await.expect("task should not panic");
        // Should complete without panic (may be Ok or Err depending on timing)
        let _ = result;
    }

    // SQL-008: stop() closes the pool
    #[tokio::test]
    async fn stop_closes_pool() {
        let pool = sqlite_pool().await;
        seed_consumer_table(&pool).await;

        let mut config = SqlEndpointConfig::from_uri(
            "sql:select id from jobs?db_url=sqlite::memory:&onConsume=update jobs set processed=1 where id=:#id&initialDelay=0&delay=1",
        )
        .unwrap();
        config.resolve_defaults();

        let pool_cell = Arc::new(OnceCell::new());
        pool_cell.set(pool.clone()).unwrap();

        let mut consumer = SqlConsumer::new(config, pool_cell);
        consumer.stop().await.expect("stop should succeed");

        // After stop, the pool should be closed
        assert!(
            pool.is_closed(),
            "Pool should be closed after consumer.stop()"
        );
    }

    // SQL-008: double-stop is safe
    #[tokio::test]
    async fn double_stop_is_safe() {
        let pool = sqlite_pool().await;
        let mut config = SqlEndpointConfig::from_uri(
            "sql:select 1?db_url=sqlite::memory:&initialDelay=0&delay=1",
        )
        .unwrap();
        config.resolve_defaults();

        let pool_cell = Arc::new(OnceCell::new());
        pool_cell.set(pool.clone()).unwrap();

        let mut consumer = SqlConsumer::new(config, pool_cell);
        consumer.stop().await.expect("first stop should succeed");
        consumer
            .stop()
            .await
            .expect("second stop should also succeed");
    }

    // SQL-008: start after stop is rejected
    #[tokio::test]
    async fn start_after_stop_rejected() {
        let pool = sqlite_pool().await;
        let mut config = SqlEndpointConfig::from_uri(
            "sql:select 1?db_url=sqlite::memory:&initialDelay=0&delay=1",
        )
        .unwrap();
        config.resolve_defaults();

        let pool_cell = Arc::new(OnceCell::new());
        pool_cell.set(pool.clone()).unwrap();

        let mut consumer = SqlConsumer::new(config, pool_cell);
        consumer.stop().await.expect("stop should succeed");

        let (tx, mut rx) = mpsc::channel::<ExchangeEnvelope>(8);
        tokio::spawn(async move {
            while let Some(env) = rx.recv().await {
                if let Some(reply_tx) = env.reply_tx {
                    let _ = reply_tx.send(Ok(env.exchange));
                }
            }
        });
        let ctx = ConsumerContext::new(tx, CancellationToken::new());

        let result = consumer.start(ctx).await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("cannot be restarted") || err_msg.contains("after stop"),
            "Expected restart error, got: {}",
            err_msg
        );
    }

    // SQL-021: batch mode per-row post-processing
    #[tokio::test]
    async fn batch_mode_per_row_post_processing() {
        let pool = sqlite_pool().await;
        seed_consumer_table(&pool).await;

        let mut config = SqlEndpointConfig::from_uri(
            "sql:select id, processed, failed from jobs where processed = 0 order by id?db_url=sqlite::memory:&onConsume=update jobs set processed=1 where id=:#id&useIterator=false&initialDelay=0&delay=1",
        )
        .unwrap();
        config.resolve_defaults();

        let consumer = SqlConsumer::new(config.clone(), Arc::new(OnceCell::new()));
        let template = parse_query_template(&config.query, config.placeholder).unwrap();

        let (tx, mut rx) = mpsc::channel::<ExchangeEnvelope>(8);
        tokio::spawn(async move {
            while let Some(env) = rx.recv().await {
                if let Some(reply_tx) = env.reply_tx {
                    let _ = reply_tx.send(Ok(env.exchange));
                }
            }
        });
        let ctx = ConsumerContext::new(tx, CancellationToken::new());

        consumer
            .poll_database(&pool, &ctx, &template)
            .await
            .expect("poll must succeed");

        // SQL-021: Each row should have been processed individually via onConsume
        let row = sqlx::query("select processed from jobs where id = 1")
            .fetch_one(&pool)
            .await
            .expect("row 1");
        let processed_1: i64 = sqlx::Row::try_get(&row, 0).expect("processed");

        let row = sqlx::query("select processed from jobs where id = 2")
            .fetch_one(&pool)
            .await
            .expect("row 2");
        let processed_2: i64 = sqlx::Row::try_get(&row, 0).expect("processed");

        assert_eq!(
            processed_1, 1,
            "row 1 should be marked processed via per-row onConsume"
        );
        assert_eq!(
            processed_2, 1,
            "row 2 should be marked processed via per-row onConsume"
        );
    }

    // SQL-021: batch mode per-row onConsumeFailed when downstream fails
    #[tokio::test]
    async fn batch_mode_per_row_post_processing_on_failure() {
        let pool = sqlite_pool().await;
        seed_consumer_table(&pool).await;

        let mut config = SqlEndpointConfig::from_uri(
            "sql:select id, processed, failed from jobs where processed = 0 order by id?db_url=sqlite::memory:&onConsumeFailed=update jobs set failed=1 where id=:#id&useIterator=false&initialDelay=0&delay=1",
        )
        .unwrap();
        config.resolve_defaults();

        let consumer = SqlConsumer::new(config.clone(), Arc::new(OnceCell::new()));
        let template = parse_query_template(&config.query, config.placeholder).unwrap();

        let (tx, mut rx) = mpsc::channel::<ExchangeEnvelope>(8);
        tokio::spawn(async move {
            while let Some(env) = rx.recv().await {
                if let Some(reply_tx) = env.reply_tx {
                    let _ =
                        reply_tx.send(Err(CamelError::ProcessorError("downstream boom".into())));
                }
            }
        });
        let ctx = ConsumerContext::new(tx, CancellationToken::new());

        consumer
            .poll_database(&pool, &ctx, &template)
            .await
            .expect("consumer should swallow downstream errors when breakBatchOnConsumeFail=false");

        // SQL-021: Each row should have onConsumeFailed executed individually
        let row = sqlx::query("select failed from jobs where id = 1")
            .fetch_one(&pool)
            .await
            .expect("row 1");
        let failed_1: i64 = sqlx::Row::try_get(&row, 0).expect("failed");

        let row = sqlx::query("select failed from jobs where id = 2")
            .fetch_one(&pool)
            .await
            .expect("row 2");
        let failed_2: i64 = sqlx::Row::try_get(&row, 0).expect("failed");

        assert_eq!(
            failed_1, 1,
            "row 1 should be marked failed via per-row onConsumeFailed"
        );
        assert_eq!(
            failed_2, 1,
            "row 2 should be marked failed via per-row onConsumeFailed"
        );
    }

    #[tokio::test]
    async fn bridge_error_handler_routes_poll_errors_to_exchange_error() {
        let mut config = config();
        config.bridge_error_handler = true;
        let consumer = SqlConsumer::new(config, Arc::new(OnceCell::new()));

        let (tx, mut rx) = mpsc::channel::<ExchangeEnvelope>(4);
        tokio::spawn(async move {
            while let Some(env) = rx.recv().await {
                assert!(env.exchange.error.is_some(), "exchange must carry error");
                if let Some(reply_tx) = env.reply_tx {
                    let _ = reply_tx.send(Ok(env.exchange));
                }
                break;
            }
        });

        let ctx = ConsumerContext::new(tx, CancellationToken::new());
        consumer
            .bridge_poll_error(&ctx, CamelError::ProcessorError("poll failed".into()))
            .await
            .expect("bridging should succeed");
    }
}
