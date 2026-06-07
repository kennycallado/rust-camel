use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use futures::TryStreamExt;
use serde_json::Value as JsonValue;
use sqlx::AnyPool;
use sqlx::any::AnyPoolOptions;
use sqlx::any::AnyRow;
use tokio::sync::OnceCell;
use tracing::{debug, error, info, warn};

use camel_component_api::retry_async;
use camel_component_api::{
    Body, CamelError, Exchange, Message, RuntimeObservability, StreamBody, StreamMetadata,
};
use camel_component_api::{ConcurrencyModel, Consumer, ConsumerContext};

use crate::config::{
    PollStrategy, ProcessingStrategy, SqlEndpointConfig, SqlOutputType, TransactionMode,
    enrich_db_url_with_ssl, redact_db_url,
};
use crate::headers;
use crate::query::{QueryTemplate, parse_query_template, resolve_params};
use crate::utils::{bind_json_values, is_retryable_sqlx_error, row_to_json};

pub struct SqlConsumer {
    pub(crate) config: SqlEndpointConfig,
    pub(crate) pool: Arc<OnceCell<AnyPool>>,
    stopped: bool,
    /// Phase B will use this for `rt.metrics().increment_errors(...)` and
    /// `rt.health().force_unhealthy_for_route(...)` calls per ADR-0012.
    #[allow(dead_code)]
    runtime: Arc<dyn RuntimeObservability>,
}

impl SqlConsumer {
    pub fn new(
        config: SqlEndpointConfig,
        pool: Arc<OnceCell<AnyPool>>,
        runtime: Arc<dyn RuntimeObservability>,
    ) -> Self {
        Self {
            config,
            pool,
            stopped: false,
            runtime,
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

        debug!(query = %prepared.sql, "executing SQL consumer poll");

        if self.config.output_type == SqlOutputType::StreamList {
            return self.poll_database_stream(pool, context, &prepared).await;
        }

        let query = bind_json_values(sqlx::query(&prepared.sql), &prepared.bindings);
        let rows: Vec<AnyRow> = query.fetch_all(pool).await.map_err(|e| {
            warn!(error = %e, "SQL consumer poll query failed");
            CamelError::ProcessorError(format!("Query execution failed: {}", e))
        })?;

        debug!(rows = rows.len(), "SQL consumer poll completed");

        if rows.is_empty() && !self.config.route_empty_result_set {
            return Ok(());
        }

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
                    // TODO(ADR-0012-e-metrics): wire increment_errors("b-prime:sql:on-consume") via bd rc-mf3
                    error!(error = %e, "Post-processing failed"); // allow-log-levels
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
                    // TODO(ADR-0012-e-metrics): wire increment_errors("b-prime:sql:on-consume-batch") via bd rc-mf3
                    error!(error = %e, "Post-processing failed for batch row"); // allow-log-levels
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
        if let Some(ref batch_query) = self.config.on_consume_batch_complete {
            let _ = self
                .execute_post_query(pool, batch_query, &JsonValue::Null)
                .await;
        }

        Ok(())
    }

    async fn poll_database_stream(
        &self,
        pool: &AnyPool,
        context: &ConsumerContext,
        prepared: &crate::query::PreparedQuery,
    ) -> Result<(), CamelError> {
        let pool_clone = pool.clone();
        let sql_str = prepared.sql.clone();
        let bindings = prepared.bindings.clone();

        let byte_stream = async_stream::try_stream! {
            let mut q = sqlx::query(&sql_str);
            q = bind_json_values(q, &bindings);
            let mut rows = q.fetch(&pool_clone);
            while let Some(row) = rows.try_next().await.map_err(|e| {
                CamelError::ProcessorError(format!("Query execution failed: {}", e))
            })? {
                let json_val = row_to_json(&row).map_err(|e| {
                    CamelError::ProcessorError(format!("JSON serialization failed: {}", e))
                })?;
                let mut bytes = serde_json::to_vec(&json_val)
                    .map_err(|e| CamelError::ProcessorError(format!("JSON serialization failed: {}", e)))?;
                bytes.push(b'\n');
                yield Bytes::from(bytes);
            }
        };

        let msg = Message::new(Body::Stream(StreamBody {
            stream: Arc::new(tokio::sync::Mutex::new(Some(Box::pin(byte_stream)))),
            metadata: StreamMetadata {
                content_type: Some("application/x-ndjson".to_string()),
                size_hint: None,
                origin: None,
            },
        }));

        let exchange = Exchange::new(msg);
        let result = context.send_and_wait(exchange).await;
        if let Err(e) = result {
            // log-policy: outside-contract
            // (category b′: send_and_wait returned Err on a normal-data send,
            // meaning the route handler did NOT absorb the failure —
            // see ADR-0012 "b-bridged discriminator". This emitter is the
            // only ERROR signal for the unhandled failure; must stay loud.
            // Regression-tested by unbridged_send_and_wait_failure_emits_error_loud.)
            // TODO(ADR-0012-e-metrics): wire increment_errors("b-prime:sql:stream-list") via bd rc-mf3
            error!(error = %e, "StreamList consumer downstream processing failed"); // allow-log-levels
            return Err(e);
        }

        debug!("StreamList: consumer poll completed (lazy stream emitted)");
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

    /// Handle the result of a single poll cycle, including bridging if configured.
    /// Extracted from `run()` so tests can exercise the error-handling branch directly.
    async fn handle_poll_result(
        &self,
        pool: &AnyPool,
        context: &ConsumerContext,
        template: &QueryTemplate,
    ) {
        if let Err(e) = self.poll_database(pool, context, template).await {
            if self.config.bridge_error_handler {
                // log-policy: handler-owned
                // (category b-bridged: error will be wrapped as Exchange
                // and flow into the route's error handler)
                warn!(error = %e, "SQL consumer poll failed (bridged)");
                if let Err(route_err) = self.bridge_poll_error(context, e).await {
                    // (the bridge channel itself broke — route will CrashNotification per ADR-0007)
                    // log-policy: system-broken
                    error!(error = %route_err, "Failed to bridge SQL consumer error to route");
                }
            } else {
                // TODO(ADR-0012-e-metrics): wire increment_errors("b-prime:sql:poll-failed")
                //   via bd rc-mf3 (Endpoint trait change to expose ComponentContext::metrics())
                error!(error = %e, "SQL consumer poll failed"); // allow-log-levels
            }
        }
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
                let retry_policy = &self.config.retry;
                retry_async::<_, _, _, _, sqlx::Error>(
                    retry_policy,
                    Some("sql-consumer"),
                    || {
                        AnyPoolOptions::new()
                            .max_connections(max_conn)
                            .min_connections(min_conn)
                            .idle_timeout(Duration::from_secs(idle_timeout))
                            .max_lifetime(Duration::from_secs(max_lifetime))
                            .connect(&db_url)
                    },
                    is_retryable_sqlx_error,
                )
                .await
                .map_err(|e| {
                    // TODO(ADR-0012-g): replace with force_unhealthy_for_route once bd rc-1mo lands
                    error!(error = %e, db_url = %redact_db_url(&self.config.db_url), "SQL connect failed, giving up"); // allow-log-levels
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

        if self.config.output_type == SqlOutputType::StreamList
            && (self.config.on_consume.is_some()
                || self.config.on_consume_failed.is_some()
                || self.config.on_consume_batch_complete.is_some())
        {
            warn!(
                "onConsume/onConsumeFailed/onConsumeBatchComplete are not executed in StreamList mode \
                 (rows are consumed lazily downstream)"
            );
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
        //
        // This is a POLLING LOOP with fixed cadence (delay_ms), NOT a
        // retry loop. It polls the database until cancelled or repeat_count
        // is reached — there is no "transient error → retry with backoff"
        // contract at this level. retry_async / retry_async_cancelable do
        // not apply because they are designed for bounded retry, not
        // repeated polling with uniform delay.
        //
        // The pool-connect retry at startup (Step 1) was migrated to
        // retry_async in rc-d2r. The per-poll error handling (poll_database
        // failures) is an error-bridge pattern, not a retry loop.
        //
        // See camel-redis/src/consumer.rs:325 for a similar polling-loop
        // justification.
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
                    self.handle_poll_result(pool, &context, &template).await;
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
    use camel_component_api::test_support::PanicRuntimeObservability;
    fn test_rt() -> std::sync::Arc<dyn camel_component_api::RuntimeObservability> {
        std::sync::Arc::new(PanicRuntimeObservability)
    }
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
        let c = SqlConsumer::new(config(), Arc::new(OnceCell::new()), test_rt());
        assert_eq!(c.concurrency_model(), ConcurrencyModel::Sequential);
    }

    #[test]
    fn consumer_stores_config() {
        let mut config = SqlEndpointConfig::from_uri(
            "sql:select * from t?db_url=postgres://localhost/test&delay=2000&onConsume=update t set done=true"
        ).unwrap();
        config.resolve_defaults();
        let c = SqlConsumer::new(config.clone(), Arc::new(OnceCell::new()), test_rt());
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

        let consumer = SqlConsumer::new(config.clone(), Arc::new(OnceCell::new()), test_rt());
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

        let consumer = SqlConsumer::new(config.clone(), Arc::new(OnceCell::new()), test_rt());
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

        let consumer = SqlConsumer::new(config.clone(), Arc::new(OnceCell::new()), test_rt());
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

        let mut consumer = SqlConsumer::new(config, Arc::new(OnceCell::new()), test_rt());
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

        let mut consumer = SqlConsumer::new(config, pool_cell, test_rt());
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

        let mut consumer = SqlConsumer::new(config, pool_cell, test_rt());
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

        let mut consumer = SqlConsumer::new(config, pool_cell, test_rt());
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

        let consumer = SqlConsumer::new(config.clone(), Arc::new(OnceCell::new()), test_rt());
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

        let consumer = SqlConsumer::new(config.clone(), Arc::new(OnceCell::new()), test_rt());
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
        let consumer = SqlConsumer::new(config, Arc::new(OnceCell::new()), test_rt());

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

    /// Regression for ADR-0012: when bridge_error_handler=true, the poll
    /// failure must NOT emit error! (the route's error handler owns ERROR
    /// for bridged failures). Was previously duplicated at line 429 + 431.
    #[tracing_test::traced_test]
    #[tokio::test]
    async fn bridged_poll_failure_emits_warn_not_error() {
        let pool = sqlite_pool().await;
        // Do NOT create any table — the query against a non-existent
        // table will fail at fetch_all, returning Err BEFORE any
        // downstream send (so lines 103/205 are never reached).

        let mut config = config();
        config.bridge_error_handler = true;
        // Query a non-existent table to trigger a query-failure poll error.
        config.query = "select * from nonexistent_table".to_string();
        config.resolve_defaults();
        let consumer = SqlConsumer::new(config.clone(), Arc::new(OnceCell::new()), test_rt());
        let template = parse_query_template(&config.query, config.placeholder).unwrap();

        // Healthy downstream — replies Ok so bridge_poll_error succeeds
        // and does NOT emit its own error!.
        let (tx, mut rx) = mpsc::channel::<ExchangeEnvelope>(4);
        tokio::spawn(async move {
            while let Some(env) = rx.recv().await {
                if let Some(reply_tx) = env.reply_tx {
                    let _ = reply_tx.send(Ok(env.exchange));
                }
            }
        });
        let ctx = ConsumerContext::new(tx, CancellationToken::new());

        // Drive poll — fetch_all will fail because the table is missing.
        consumer.handle_poll_result(&pool, &ctx, &template).await;

        // The bridged path must NOT emit ERROR (handler owns it).
        assert!(
            !logs_contain("ERROR"),
            "bridged poll failure must not emit ERROR (handler owns it); check captured logs for stray ERROR lines"
        );
        // Sanity: warn! was emitted so the failure is still visible.
        assert!(
            logs_contain("WARN"),
            "bridged poll failure should emit warn! for operator visibility"
        );
    }

    /// Regression for ADR-0012 "b-bridged discriminator": when
    /// send_and_wait returns Err on a NORMAL-DATA send (i.e., not a
    /// deliberate bridge_poll_error handoff), the route handler did NOT
    /// absorb the failure (consumer.rs:77-91 contract; error_handler.rs
    /// returns Ok in every branch). The consumer's error! is the only
    /// ERROR signal for the unhandled failure and MUST stay at error!.
    ///
    /// Protects consumer.rs:205 (StreamList downstream send) and any
    /// future site that uses send_and_wait on a non-bridge path.
    #[tracing_test::traced_test]
    #[tokio::test]
    async fn unbridged_send_and_wait_failure_emits_error_loud() {
        let pool = sqlite_pool().await;
        sqlx::query("CREATE TABLE items (id INTEGER PRIMARY KEY, name TEXT)")
            .execute(&pool)
            .await
            .expect("create table");
        sqlx::query("INSERT INTO items (id, name) VALUES (1, 'alpha')")
            .execute(&pool)
            .await
            .expect("seed rows");

        let mut config = SqlEndpointConfig::from_uri(
            "sql:select id, name from items order by id?db_url=sqlite::memory:&outputType=StreamList&initialDelay=0&delay=1",
        )
        .unwrap();
        config.resolve_defaults();
        // Explicitly non-bridged: normal-data send path.
        config.bridge_error_handler = false;
        let consumer = SqlConsumer::new(config.clone(), Arc::new(OnceCell::new()), test_rt());
        let template = parse_query_template(&config.query, config.placeholder).unwrap();

        // Downstream that returns Err — simulates unhandled route failure.
        let (tx, mut rx) = mpsc::channel::<ExchangeEnvelope>(8);
        tokio::spawn(async move {
            while let Some(env) = rx.recv().await {
                if let Some(reply_tx) = env.reply_tx {
                    let _ = reply_tx.send(Err(CamelError::ProcessorError("boom".into())));
                }
            }
        });
        let ctx = ConsumerContext::new(tx, CancellationToken::new());

        let _ = consumer.poll_database(&pool, &ctx, &template).await;

        // The unbridged path MUST emit ERROR — consumer owns the signal.
        assert!(
            logs_contain("ERROR"),
            "unbridged send_and_wait failure MUST emit ERROR (consumer owns the signal)"
        );
    }

    /// Regression for ADR-0012: when bridge_error_handler=false, the unbridged
    /// branch of handle_poll_result MUST emit ERROR for unhandled poll failure.
    #[tracing_test::traced_test]
    #[tokio::test]
    async fn unbridged_handle_poll_result_emits_error_loud() {
        let pool = sqlite_pool().await;
        // Do NOT create any table — fetch_all will fail in poll_database.

        let mut config = config();
        config.bridge_error_handler = false;
        config.query = "select * from nonexistent_table".to_string();
        config.resolve_defaults();
        let consumer = SqlConsumer::new(config.clone(), Arc::new(OnceCell::new()), test_rt());
        let template = parse_query_template(&config.query, config.placeholder).unwrap();

        // Healthy downstream task; should not be reached for this poll-failure path.
        let (tx, mut rx) = mpsc::channel::<ExchangeEnvelope>(4);
        tokio::spawn(async move {
            while let Some(env) = rx.recv().await {
                if let Some(reply_tx) = env.reply_tx {
                    let _ = reply_tx.send(Ok(env.exchange));
                }
            }
        });
        let ctx = ConsumerContext::new(tx, CancellationToken::new());

        consumer.handle_poll_result(&pool, &ctx, &template).await;

        assert!(
            logs_contain("ERROR"),
            "unbridged handle_poll_result failure MUST emit ERROR (consumer owns signal)"
        );
    }

    #[tokio::test]
    async fn stream_list_consumer_emits_ndjson_body() {
        let pool = sqlite_pool().await;
        sqlx::query("CREATE TABLE items (id INTEGER PRIMARY KEY, name TEXT)")
            .execute(&pool)
            .await
            .expect("create table");
        sqlx::query("INSERT INTO items (id, name) VALUES (1, 'alpha'), (2, 'beta'), (3, 'gamma')")
            .execute(&pool)
            .await
            .expect("seed rows");

        let mut config = SqlEndpointConfig::from_uri(
            "sql:select id, name from items order by id?db_url=sqlite::memory:&outputType=StreamList&initialDelay=0&delay=1",
        )
        .unwrap();
        config.resolve_defaults();

        let consumer = SqlConsumer::new(config.clone(), Arc::new(OnceCell::new()), test_rt());
        let template = parse_query_template(&config.query, config.placeholder).unwrap();

        let (tx, rx) = mpsc::channel::<ExchangeEnvelope>(8);
        let (result_tx, result_rx) = tokio::sync::oneshot::channel::<Exchange>();
        tokio::spawn(async move {
            let mut rx = rx;
            if let Some(env) = rx.recv().await {
                if let Some(reply_tx) = env.reply_tx {
                    let _ = reply_tx.send(Ok(env.exchange.clone()));
                }
                let _ = result_tx.send(env.exchange);
            }
        });
        let ctx = ConsumerContext::new(tx, CancellationToken::new());

        consumer
            .poll_database(&pool, &ctx, &template)
            .await
            .expect("poll must succeed");

        let exchange = result_rx.await.expect("should have received one exchange");

        match exchange.input.body {
            Body::Stream(ref stream_body) => {
                let stream = stream_body.stream.clone();
                let mut guard = stream.lock().await;
                let stream_opt = guard.take();
                assert!(stream_opt.is_some(), "stream should be present");

                use futures::StreamExt;
                let mut collected = Vec::new();
                let mut stream = stream_opt.unwrap();
                while let Some(chunk) = stream.next().await {
                    let chunk = chunk.expect("stream chunk should not error");
                    collected.extend_from_slice(&chunk);
                }

                let ndjson = String::from_utf8(collected).expect("valid utf8");
                let lines: Vec<&str> = ndjson.trim().lines().collect();
                assert_eq!(lines.len(), 3, "should have 3 NDJSON lines");

                let row0: serde_json::Value =
                    serde_json::from_str(lines[0]).expect("valid json line 0");
                assert_eq!(row0["id"], 1);
                assert_eq!(row0["name"], "alpha");

                let row1: serde_json::Value =
                    serde_json::from_str(lines[1]).expect("valid json line 1");
                assert_eq!(row1["id"], 2);
                assert_eq!(row1["name"], "beta");

                let row2: serde_json::Value =
                    serde_json::from_str(lines[2]).expect("valid json line 2");
                assert_eq!(row2["id"], 3);
                assert_eq!(row2["name"], "gamma");
            }
            ref other => panic!("expected Body::Stream, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn stream_list_consumer_empty_result_set_emits_empty_stream() {
        let pool = sqlite_pool().await;
        sqlx::query("CREATE TABLE empty_items (id INTEGER PRIMARY KEY, name TEXT)")
            .execute(&pool)
            .await
            .expect("create table");

        let mut config = SqlEndpointConfig::from_uri(
            "sql:select id, name from empty_items?db_url=sqlite::memory:&outputType=StreamList&initialDelay=0&delay=1",
        )
        .unwrap();
        config.resolve_defaults();

        let consumer = SqlConsumer::new(config.clone(), Arc::new(OnceCell::new()), test_rt());
        let template = parse_query_template(&config.query, config.placeholder).unwrap();

        let (tx, rx) = tokio::sync::oneshot::channel();
        let (mpsc_tx, mut mpsc_rx) = mpsc::channel::<ExchangeEnvelope>(8);
        tokio::spawn(async move {
            while let Some(env) = mpsc_rx.recv().await {
                if let Some(reply_tx) = env.reply_tx {
                    let _ = reply_tx.send(Ok(env.exchange.clone()));
                }
                let _ = tx.send(env.exchange);
                break;
            }
        });
        let ctx = ConsumerContext::new(mpsc_tx, CancellationToken::new());

        consumer
            .poll_database(&pool, &ctx, &template)
            .await
            .expect("poll must succeed");

        let exchange = rx
            .await
            .expect("StreamList should emit exchange even for empty results");

        match exchange.input.body {
            Body::Stream(ref stream_body) => {
                let stream = stream_body.stream.clone();
                let mut guard = stream.lock().await;
                let stream_opt = guard.take();

                use futures::StreamExt;
                let mut count = 0;
                if let Some(mut stream) = stream_opt {
                    while let Some(chunk) = stream.next().await {
                        let chunk = chunk.expect("stream chunk should not error");
                        count += chunk.len();
                    }
                }
                assert_eq!(count, 0, "empty table should produce zero stream bytes");
            }
            ref other => panic!("expected Body::Stream, got {:?}", other),
        }
    }

    /// Regression: max_attempts=N → exactly N invocations (caught OpenSearch off-by-one 1f5c4c2a).
    /// Replicates the exact retry loop from SqlConsumer::start() (consumer.rs:343-367):
    ///   attempt starts at 0, incremented at top, should_retry(attempt), delay_for(attempt-1)
    #[tokio::test]
    async fn retry_loop_invokes_operation_exactly_max_attempts_times() {
        use camel_component_api::NetworkRetryPolicy;
        use std::sync::Arc;
        use std::sync::atomic::{AtomicU32, Ordering};

        let policy = NetworkRetryPolicy {
            max_attempts: 3,
            initial_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(1),
            multiplier: 1.0,
            ..NetworkRetryPolicy::default()
        };

        let calls = Arc::new(AtomicU32::new(0));
        let calls_clone = Arc::clone(&calls);

        let mut attempt: u32 = 0;
        let _result: Result<(), ()> = loop {
            attempt += 1;
            calls_clone.fetch_add(1, Ordering::SeqCst);
            let op_result: Result<(), ()> = Err(());
            match op_result {
                Ok(v) => break Ok(v),
                Err(_) if policy.should_retry(attempt) => {
                    let delay = policy.delay_for(attempt - 1);
                    tokio::time::sleep(delay).await;
                    continue;
                }
                Err(_) => break Err(()),
            }
        };

        assert_eq!(
            calls.load(Ordering::SeqCst),
            3,
            "max_attempts=3 must yield exactly 3 invocations"
        );
    }
}
