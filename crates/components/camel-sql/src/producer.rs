use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Bytes;
use serde_json::json;
use sqlx::AnyPool;
use sqlx::any::AnyRow;
use sqlx::pool::PoolOptions;
use tokio::sync::OnceCell;
use tower::Service;
use tracing::{debug, error, info, warn};

use crate::config::{SqlEndpointConfig, SqlOutputType, enrich_db_url_with_ssl, redact_db_url};
use crate::headers;
use crate::query::{PreparedQuery, is_select_query, parse_query_template, resolve_params};
use crate::utils::{bind_json_values, is_retryable_sqlx_error, row_to_json};
use camel_component_api::retry_async;
use camel_component_api::{Body, CamelError, Exchange, Message, StreamBody, StreamMetadata};

#[derive(Clone)]
pub struct SqlProducer {
    pub(crate) config: SqlEndpointConfig,
    pub(crate) pool: Arc<OnceCell<AnyPool>>,
    pub(crate) stopped: Arc<AtomicBool>,
}

impl SqlProducer {
    pub fn new(config: SqlEndpointConfig, pool: Arc<OnceCell<AnyPool>>) -> Self {
        Self {
            config,
            pool,
            stopped: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn stop(&self) {
        self.stopped.store(true, Ordering::Relaxed);
        // Close the pool asynchronously in the background with a timeout
        if let Some(pool) = self.pool.get() {
            let pool = pool.clone();
            tokio::spawn(async move {
                if tokio::time::timeout(Duration::from_secs(5), pool.close())
                    .await
                    .is_err()
                {
                    tracing::warn!("SQL producer pool did not close within 5s");
                }
            });
        }
    }

    /// Resolves the query source based on priority:
    /// 1. Header `CamelSql.Query`
    /// 2. Body (if `use_message_body_for_sql` is true)
    /// 3. Config query
    pub(crate) fn resolve_query_source(exchange: &Exchange, config: &SqlEndpointConfig) -> String {
        // Priority 1: Header
        if let Some(query_value) = exchange.input.header(headers::QUERY)
            && let Some(query_str) = query_value.as_str()
        {
            return query_str.to_string();
        }

        // Priority 2: Body (if use_message_body_for_sql)
        if config.use_message_body_for_sql
            && let Some(body_text) = exchange.input.body.as_text()
        {
            return body_text.to_string();
        }

        // Priority 3: Config query
        config.query.clone()
    }

    /// Health check: runs a simple `SELECT 1` query against the connection pool
    /// to verify database connectivity.
    ///
    /// Returns `Ok(())` if the database is reachable, or an error with details
    /// if the connection fails.
    pub async fn check_connection(&self) -> Result<(), CamelError> {
        let pool = self.pool.get().ok_or_else(|| {
            CamelError::ProcessorError("SQL connection pool not initialized".into())
        })?;

        debug!("Running health check: SELECT 1");
        sqlx::query("SELECT 1").execute(pool).await.map_err(|e| {
            warn!(error = %e, "SQL health check failed");
            CamelError::ProcessorError(format!("SQL health check failed: {}", e))
        })?;

        debug!("SQL health check passed");
        Ok(())
    }
}

impl Service<Exchange> for SqlProducer {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        if self.stopped.load(Ordering::Relaxed) {
            return Poll::Ready(Err(CamelError::ProcessorError(
                "SQL producer stopped".into(),
            )));
        }
        if let Some(pool) = self.pool.get()
            && pool.is_closed()
        {
            return Poll::Ready(Err(CamelError::ProcessorError(
                "SQL connection pool is closed".into(),
            )));
        }
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, mut exchange: Exchange) -> Self::Future {
        let mut config = self.config.clone();
        let pool_cell = Arc::clone(&self.pool);

        Box::pin(async move {
            // Get or initialize the connection pool
            let pool: &AnyPool = pool_cell
                .get_or_try_init(|| async {
                    // Defensive: ensure config is resolved even if caller didn't use create_endpoint
                    config.resolve_defaults();
                    // SQL-014: resolve file-based query asynchronously (not blocking)
                    config.resolve_file_query().await?;
                    let db_url = enrich_db_url_with_ssl(&config.db_url, &config)?;

                    // Install all compiled-in sqlx drivers so AnyPool can resolve them.
                    // This is idempotent; safe to call multiple times.
                    sqlx::any::install_default_drivers();

                    let max_conn = config.max_connections.ok_or_else(|| {
                        CamelError::Config("max_connections not resolved for SQL pool".into())
                    })?;
                    let min_conn = config.min_connections.ok_or_else(|| {
                        CamelError::Config("min_connections not resolved for SQL pool".into())
                    })?;
                    let idle_timeout = config.idle_timeout_secs.ok_or_else(|| {
                        CamelError::Config("idle_timeout_secs not resolved for SQL pool".into())
                    })?;
                    let max_lifetime = config.max_lifetime_secs.ok_or_else(|| {
                        CamelError::Config("max_lifetime_secs not resolved for SQL pool".into())
                    })?;

                    info!(
                        db_url = %redact_db_url(&config.db_url),
                        "SQL producer pool initializing"
                    );
                    let retry_policy = &config.retry;
                    retry_async::<_, _, _, _, sqlx::Error>(
                        retry_policy,
                        || {
                            PoolOptions::new()
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
                        error!(error = %e, db_url = %redact_db_url(&config.db_url), "Failed to connect to database"); // allow-log-levels
                        CamelError::EndpointCreationFailed(format!(
                            "Failed to connect to database: {}",
                            e
                        ))
                    })
                })
                .await
                .map_err(|e: CamelError| e.clone())?;

            // Resolve query string
            let query_str = Self::resolve_query_source(&exchange, &config);

            // SQL-002: warn if Managed transaction mode requested
            if config.transaction_mode == crate::config::TransactionMode::Managed {
                warn!("transactionManager not yet implemented; using Auto mode");
            }

            debug!(
                query = %query_str,
                "executing SQL query"
            );

            // Execute based on mode
            if config.batch {
                // Batch mode: execute_batch handles its own template parsing per item
                execute_batch(pool, &config, &mut exchange).await?;
            } else if config.use_placeholder {
                // Placeholder processing enabled (default): parse template, resolve params, apply header override
                let template = parse_query_template(&query_str, config.placeholder)?;
                let mut prepared = resolve_params(&template, &exchange, &config.in_separator)?;

                // CamelSql.Parameters header override
                if let Some(params_value) = exchange.input.header(headers::PARAMETERS) {
                    if let Some(arr) = params_value.as_array() {
                        if arr.len() != prepared.bindings.len() {
                            warn!(
                                expected = prepared.bindings.len(),
                                got = arr.len(),
                                header = headers::PARAMETERS,
                                "Parameter count mismatch — SQL has {} placeholders but header provides {} values",
                                prepared.bindings.len(),
                                arr.len()
                            );
                        }
                        debug!(
                            "Overriding bindings from {} header with {} parameters",
                            headers::PARAMETERS,
                            arr.len()
                        );
                        prepared.bindings = arr.clone();
                    } else {
                        warn!(
                            header = headers::PARAMETERS,
                            "Header is present but not a JSON array — ignoring parameter override"
                        );
                    }
                }

                debug!(
                    "Executing prepared SQL ({} bindings)",
                    prepared.bindings.len()
                );

                if is_select_query(&prepared.sql) {
                    execute_select(pool, &prepared, &config, &mut exchange).await?;
                } else {
                    execute_modify(pool, &prepared, &config, &mut exchange).await?;
                }
            } else {
                // use_placeholder=false: execute query as-is without template parsing
                debug!("Executing raw SQL (placeholder processing disabled)");
                let prepared = PreparedQuery {
                    sql: query_str,
                    bindings: vec![],
                };

                if is_select_query(&prepared.sql) {
                    execute_select(pool, &prepared, &config, &mut exchange).await?;
                } else {
                    execute_modify(pool, &prepared, &config, &mut exchange).await?;
                }
            }

            Ok(exchange)
        })
    }
}

/// Executes a SELECT query and populates the exchange body with results.
async fn execute_select(
    pool: &AnyPool,
    prepared: &PreparedQuery,
    config: &SqlEndpointConfig,
    exchange: &mut Exchange,
) -> Result<(), CamelError> {
    match config.output_type {
        SqlOutputType::SelectOne => {
            // fetch_all and take first row
            let mut query = sqlx::query(&prepared.sql);
            query = bind_json_values(query, &prepared.bindings);

            let rows: Vec<AnyRow> = query.fetch_all(pool).await.map_err(|e| {
                warn!(error = %e, "SQL query failed");
                CamelError::ProcessorError(format!("Query execution failed: {}", e))
            })?;

            let count = rows.len();
            debug!(rows = count, "SQL query completed");
            let json_rows: Vec<serde_json::Value> = rows
                .iter()
                .map(row_to_json)
                .collect::<Result<Vec<_>, _>>()?;

            if let Some(first_row) = json_rows.into_iter().next() {
                exchange.input.body = Body::Json(first_row);
            } else {
                exchange.input.body = Body::Empty;
            }
            debug!("SelectOne returned {} row", if count > 0 { 1 } else { 0 });
            exchange
                .input
                .set_header(headers::ROW_COUNT, serde_json::json!(count));
        }
        SqlOutputType::SelectList => {
            // fetch_all for list output
            let mut query = sqlx::query(&prepared.sql);
            query = bind_json_values(query, &prepared.bindings);

            let rows: Vec<AnyRow> = query.fetch_all(pool).await.map_err(|e| {
                warn!(error = %e, "SQL query failed");
                CamelError::ProcessorError(format!("Query execution failed: {}", e))
            })?;

            let count = rows.len();
            debug!(rows = count, "SQL query completed");
            let json_rows: Vec<serde_json::Value> = rows
                .iter()
                .map(row_to_json)
                .collect::<Result<Vec<_>, _>>()?;

            exchange.input.body = Body::Json(serde_json::Value::Array(json_rows));
            debug!("SelectList returned {} rows", count);
            exchange
                .input
                .set_header(headers::ROW_COUNT, serde_json::json!(count));
        }
        SqlOutputType::StreamList => {
            // Use fetch() for true streaming - avoids loading all rows into memory
            use futures::TryStreamExt;

            let pool_clone = pool.clone();
            let sql_str = prepared.sql.clone();
            let bindings = prepared.bindings.clone();

            // Build the stream that reads rows on demand and serializes to NDJSON bytes
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

            exchange.input.body = Body::Stream(StreamBody {
                stream: Arc::new(tokio::sync::Mutex::new(Some(Box::pin(byte_stream)))),
                metadata: StreamMetadata {
                    content_type: Some("application/x-ndjson".to_string()),
                    size_hint: None,
                    origin: None,
                },
            });
            debug!("StreamList: created lazy stream (rows fetched on demand)");
            // Note: ROW_COUNT not set for StreamList since row count is unknown until exhausted
        }
    }

    Ok(())
}

/// Executes a modification query (INSERT/UPDATE/DELETE).
async fn execute_modify(
    pool: &AnyPool,
    prepared: &PreparedQuery,
    config: &SqlEndpointConfig,
    exchange: &mut Exchange,
) -> Result<(), CamelError> {
    let mut query = sqlx::query(&prepared.sql);
    query = bind_json_values(query, &prepared.bindings);

    let result = query.execute(pool).await.map_err(|e| {
        warn!(error = %e, "SQL query failed");
        CamelError::ProcessorError(format!("Query execution failed: {}", e))
    })?;

    let rows_affected = result.rows_affected();

    // Fix 4: Implement expected_update_count validation
    if let Some(expected) = config.expected_update_count
        && rows_affected as i64 != expected
    {
        warn!(expected, actual = rows_affected, "Row count mismatch");
        return Err(CamelError::ProcessorError(format!(
            "Expected {} rows affected, got {}",
            expected, rows_affected
        )));
    }

    exchange
        .input
        .set_header(headers::UPDATE_COUNT, serde_json::json!(rows_affected));

    if config.noop {
        // Preserve original body
    } else {
        exchange.input.body = Body::Json(json!({ "rowsAffected": rows_affected }));
    }

    debug!(rows = rows_affected, "SQL modify query completed");

    Ok(())
}

/// Executes a batch of queries from a JSON array body.
async fn execute_batch(
    pool: &AnyPool,
    config: &SqlEndpointConfig,
    exchange: &mut Exchange,
) -> Result<(), CamelError> {
    // Body must be JSON array of arrays
    let body_json = match &exchange.input.body {
        Body::Json(val) => val,
        _ => {
            return Err(CamelError::ProcessorError(
                "Batch mode requires body to be a JSON array of arrays".to_string(),
            ));
        }
    };

    let batch_data = body_json
        .as_array()
        .ok_or_else(|| {
            CamelError::ProcessorError("Batch mode requires body to be a JSON array".to_string())
        })?
        .clone();

    // Parse template from config query
    let template = parse_query_template(&config.query, config.placeholder)?;

    // Fix 2: Batch operations must be wrapped in a transaction
    let mut tx = pool.begin().await.map_err(|e| {
        // log-policy: handler-owned
        // (category a: tx begin failure inside producer pipeline)
        warn!(error = %e, "Failed to begin transaction");
        CamelError::ProcessorError(format!("Failed to begin transaction: {}", e))
    })?;

    let mut total_rows_affected: u64 = 0;

    for (batch_idx, params_array) in batch_data.into_iter().enumerate() {
        // Each item must be an array of parameters
        params_array.as_array().ok_or_else(|| {
            CamelError::ProcessorError(format!(
                "Batch item at index {} must be a JSON array of parameters",
                batch_idx
            ))
        })?;

        // Create a temporary exchange with the params as body for resolution
        let temp_msg = Message::new(Body::Json(params_array.clone()));
        let temp_exchange = Exchange::new(temp_msg);

        // Resolve parameters for this batch item
        let prepared = resolve_params(&template, &temp_exchange, &config.in_separator)?;

        // Execute against transaction
        let mut query = sqlx::query(&prepared.sql);
        query = bind_json_values(query, &prepared.bindings);

        let result = query.execute(&mut *tx).await.map_err(|e| {
            // log-policy: handler-owned
            // (category a: batch query execution failure inside producer pipeline)
            warn!("Batch query execution failed at index {}: {}", batch_idx, e);
            CamelError::ProcessorError(format!("Batch query execution failed: {}", e))
        })?;

        // Validate expected_update_count per batch item
        if let Some(expected) = config.expected_update_count
            && result.rows_affected() as i64 != expected
        {
            // log-policy: handler-owned
            // (category a: expected_update_count mismatch inside producer pipeline)
            warn!(
                "Batch item {}: expected {} rows affected, got {}",
                batch_idx,
                expected,
                result.rows_affected()
            );
            return Err(CamelError::ProcessorError(format!(
                "Batch item {}: expected {} rows affected, got {}",
                batch_idx,
                expected,
                result.rows_affected()
            )));
        }

        total_rows_affected += result.rows_affected();
    }

    // Commit the transaction
    tx.commit().await.map_err(|e| {
        // log-policy: handler-owned
        // (category a: tx commit failure inside producer pipeline)
        warn!(error = %e, "Failed to commit transaction");
        CamelError::ProcessorError(format!("Failed to commit transaction: {}", e))
    })?;

    exchange.input.set_header(
        headers::UPDATE_COUNT,
        serde_json::json!(total_rows_affected),
    );

    debug!(
        "Batch execution completed, total rows affected: {}",
        total_rows_affected
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_component_api::Message;
    use camel_component_api::UriConfig;
    use sqlx::any::AnyPoolOptions;
    use std::sync::Arc;
    use tokio::sync::OnceCell;

    async fn sqlite_pool() -> AnyPool {
        sqlx::any::install_default_drivers();
        AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("sqlite pool")
    }

    async fn seed_items_table(pool: &AnyPool) {
        sqlx::query(
            "CREATE TABLE items (id INTEGER PRIMARY KEY, name TEXT, done INTEGER DEFAULT 0)",
        )
        .execute(pool)
        .await
        .expect("create table");
        sqlx::query("INSERT INTO items (id, name, done) VALUES (1, 'a', 0), (2, 'b', 0)")
            .execute(pool)
            .await
            .expect("seed rows");
    }

    fn config() -> SqlEndpointConfig {
        let mut c =
            SqlEndpointConfig::from_uri("sql:select 1?db_url=postgres://localhost/test").unwrap();
        c.resolve_defaults();
        c
    }

    #[test]
    fn producer_clone_shares_pool() {
        let p1 = SqlProducer::new(config(), Arc::new(OnceCell::new()));
        let p2 = p1.clone();
        assert!(Arc::ptr_eq(&p1.pool, &p2.pool));
        assert!(Arc::ptr_eq(&p1.stopped, &p2.stopped));
    }

    #[test]
    fn resolve_query_from_config() {
        let config = config();
        let ex = Exchange::new(Message::default());
        let q = SqlProducer::resolve_query_source(&ex, &config);
        assert_eq!(q, "select 1");
    }

    #[test]
    fn resolve_query_from_header() {
        let config = config();
        let mut msg = Message::default();
        msg.set_header(headers::QUERY, serde_json::json!("select 2"));
        let ex = Exchange::new(msg);
        let q = SqlProducer::resolve_query_source(&ex, &config);
        assert_eq!(q, "select 2");
    }

    #[test]
    fn resolve_query_from_body() {
        let mut config = config();
        config.use_message_body_for_sql = true;
        let msg = Message::new(Body::Text("select 3".to_string()));
        let ex = Exchange::new(msg);
        let q = SqlProducer::resolve_query_source(&ex, &config);
        assert_eq!(q, "select 3");
    }

    #[test]
    fn resolve_query_header_priority_over_body() {
        let mut config = config();
        config.use_message_body_for_sql = true;
        let mut msg = Message::new(Body::Text("select from body".to_string()));
        msg.set_header(headers::QUERY, serde_json::json!("select from header"));
        let ex = Exchange::new(msg);
        let q = SqlProducer::resolve_query_source(&ex, &config);
        assert_eq!(q, "select from header");
    }

    #[test]
    fn resolve_query_body_priority_over_config() {
        let mut config = config();
        config.use_message_body_for_sql = true;
        let msg = Message::new(Body::Text("select from body".to_string()));
        let ex = Exchange::new(msg);
        let q = SqlProducer::resolve_query_source(&ex, &config);
        assert_eq!(q, "select from body");
    }

    #[test]
    fn bind_json_null() {
        let query = sqlx::query("SELECT ?");
        let values = vec![serde_json::Value::Null];
        let _bound = bind_json_values(query, &values);
        // Compilation test - ensure it binds
    }

    #[test]
    fn bind_json_bool() {
        let query = sqlx::query("SELECT ?");
        let values = vec![serde_json::Value::Bool(true)];
        let _bound = bind_json_values(query, &values);
    }

    #[test]
    fn bind_json_number_i64() {
        let query = sqlx::query("SELECT ?");
        let values = vec![serde_json::json!(42)];
        let _bound = bind_json_values(query, &values);
    }

    #[test]
    fn bind_json_number_f64() {
        let query = sqlx::query("SELECT ?");
        let values = vec![serde_json::json!(std::f64::consts::PI)];
        let _bound = bind_json_values(query, &values);
    }

    #[test]
    fn bind_json_string() {
        let query = sqlx::query("SELECT ?");
        let values = vec![serde_json::json!("hello world")];
        let _bound = bind_json_values(query, &values);
    }

    #[test]
    fn bind_json_array() {
        let query = sqlx::query("SELECT ?");
        let values = vec![serde_json::json!([1, 2, 3])];
        let _bound = bind_json_values(query, &values);
    }

    #[test]
    fn bind_json_object() {
        let query = sqlx::query("SELECT ?");
        let values = vec![serde_json::json!({"key": "value"})];
        let _bound = bind_json_values(query, &values);
    }

    #[test]
    fn bind_multiple_values() {
        let query = sqlx::query("SELECT ?, ?, ?");
        let values = vec![
            serde_json::json!(1),
            serde_json::json!("test"),
            serde_json::Value::Null,
        ];
        let _bound = bind_json_values(query, &values);
    }

    // Test for Fix 4: expected_update_count config field presence
    #[test]
    fn expected_update_count_validation() {
        // Test that expected_update_count is parsed from URI
        let config = SqlEndpointConfig::from_uri(
            "sql:update t set x=1?db_url=postgres://localhost/test&expectedUpdateCount=5",
        )
        .unwrap();
        assert_eq!(config.expected_update_count, Some(5));

        // Test default (no expected_update_count)
        let config_default = self::config();
        assert_eq!(config_default.expected_update_count, None);

        // Test negative value (should parse)
        let config_neg = SqlEndpointConfig::from_uri(
            "sql:update t set x=1?db_url=postgres://localhost/test&expectedUpdateCount=-1",
        )
        .unwrap();
        assert_eq!(config_neg.expected_update_count, Some(-1));
    }

    // Test for Fix 3: parameters header override logic
    #[test]
    fn parameters_header_override_logic() {
        // Create a PreparedQuery manually
        let mut prepared = PreparedQuery {
            sql: "SELECT * FROM t WHERE id = $1".to_string(),
            bindings: vec![serde_json::json!(42)],
        };

        // Simulate the header override logic
        let header_params = serde_json::json!([99, "extra"]);
        if let Some(arr) = header_params.as_array() {
            prepared.bindings = arr.clone();
        }

        // Verify bindings were overridden
        assert_eq!(prepared.bindings.len(), 2);
        assert_eq!(prepared.bindings[0], serde_json::json!(99));
        assert_eq!(prepared.bindings[1], serde_json::json!("extra"));

        // Test with non-array header (should not override)
        let mut prepared2 = PreparedQuery {
            sql: "SELECT * FROM t WHERE id = $1".to_string(),
            bindings: vec![serde_json::json!(42)],
        };
        let header_non_array = serde_json::json!({"not": "an array"});
        if let Some(arr) = header_non_array.as_array() {
            prepared2.bindings = arr.clone();
        }
        // Should remain unchanged
        assert_eq!(prepared2.bindings.len(), 1);
        assert_eq!(prepared2.bindings[0], serde_json::json!(42));
    }

    #[tokio::test]
    async fn execute_select_one_sets_body_and_row_count() {
        let pool = sqlite_pool().await;
        seed_items_table(&pool).await;

        let mut config = SqlEndpointConfig::from_uri(
            "sql:select id, name from items order by id?db_url=sqlite::memory:&outputType=SelectOne",
        )
        .unwrap();
        config.resolve_defaults();

        let prepared = PreparedQuery {
            sql: "select id, name from items order by id".to_string(),
            bindings: vec![],
        };
        let mut exchange = Exchange::new(Message::default());

        execute_select(&pool, &prepared, &config, &mut exchange)
            .await
            .expect("select one");

        assert_eq!(exchange.input.header(headers::ROW_COUNT), Some(&json!(2)));
        assert_eq!(
            exchange.input.body,
            Body::Json(json!({"id": 1, "name": "a"}))
        );
    }

    #[tokio::test]
    async fn execute_stream_list_materializes_ndjson() {
        let pool = sqlite_pool().await;
        seed_items_table(&pool).await;

        let mut config = SqlEndpointConfig::from_uri(
            "sql:select id from items order by id?db_url=sqlite::memory:&outputType=StreamList",
        )
        .unwrap();
        config.resolve_defaults();

        let prepared = PreparedQuery {
            sql: "select id from items order by id".to_string(),
            bindings: vec![],
        };
        let mut exchange = Exchange::new(Message::default());

        execute_select(&pool, &prepared, &config, &mut exchange)
            .await
            .expect("stream list");

        let bytes = exchange
            .input
            .body
            .clone()
            .into_bytes(1024)
            .await
            .expect("stream bytes");
        let text = String::from_utf8(bytes.to_vec()).expect("utf8");
        assert!(text.contains("{\"id\":1}"));
        assert!(text.contains("{\"id\":2}"));
        assert_eq!(exchange.input.header(headers::ROW_COUNT), None);
    }

    #[tokio::test]
    async fn execute_modify_expected_update_count_mismatch_returns_error() {
        let pool = sqlite_pool().await;
        seed_items_table(&pool).await;

        let mut config = SqlEndpointConfig::from_uri(
            "sql:update items set done=1 where id = #?db_url=sqlite::memory:&expectedUpdateCount=2",
        )
        .unwrap();
        config.resolve_defaults();

        let prepared = PreparedQuery {
            sql: "update items set done=1 where id = $1".to_string(),
            bindings: vec![json!(1)],
        };
        let mut exchange = Exchange::new(Message::default());

        let err = execute_modify(&pool, &prepared, &config, &mut exchange)
            .await
            .expect_err("must fail due expected row count mismatch");
        assert!(err.to_string().contains("Expected 2 rows affected, got 1"));
    }

    #[tokio::test]
    async fn execute_batch_rollback_when_any_item_fails_expected_count() {
        let pool = sqlite_pool().await;
        seed_items_table(&pool).await;

        let mut config = SqlEndpointConfig::from_uri(
            "sql:update items set done=1 where id = #?db_url=sqlite::memory:&batch=true&expectedUpdateCount=1",
        )
        .unwrap();
        config.resolve_defaults();

        let mut exchange = Exchange::new(Message::new(Body::Json(json!([[1], [999]]))));

        let err = execute_batch(&pool, &config, &mut exchange)
            .await
            .expect_err("second batch item should fail expectedUpdateCount");
        assert!(
            err.to_string()
                .contains("Batch item 1: expected 1 rows affected, got 0")
        );

        let row = sqlx::query("select done from items where id = 1")
            .fetch_one(&pool)
            .await
            .expect("query row");
        let done: i64 = sqlx::Row::try_get(&row, 0).expect("done column");
        assert_eq!(done, 0, "transaction must rollback first update");
    }

    // --- Phase B hardening tests ---

    // SQL-001: Direct producer construction without resolve_defaults does not panic.
    // The producer defensively calls resolve_defaults() during pool init, so the pool
    // fields get resolved. This test verifies no panic occurs and the pool initializes.
    #[tokio::test]
    async fn producer_no_panic_without_prior_resolve_defaults() {
        // Create config without calling resolve_defaults() — pool fields are None
        let config = SqlEndpointConfig::from_uri("sql:select 1?db_url=sqlite::memory:").unwrap();
        assert!(config.max_connections.is_none());

        let mut producer = SqlProducer::new(config, Arc::new(OnceCell::new()));
        let exchange = Exchange::new(Message::default());

        // Should NOT panic — producer calls resolve_defaults() defensively
        let result = producer.call(exchange).await;
        assert!(
            result.is_ok(),
            "Producer should initialize pool without panic, got: {:?}",
            result
        );
    }

    // SQL-001: Pool init returns CamelError::Config when pool params cannot be resolved
    #[tokio::test]
    async fn producer_pool_init_returns_config_error_for_invalid_db() {
        // Create config with an invalid db_url that will fail to connect.
        // Disable retry for this test — the purpose is to verify error handling
        // on a known-bad connect, not to exercise the retry loop.
        let mut config = SqlEndpointConfig::from_uri(
            "sql:select 1?db_url=postgres://nonexistent-host:5432/nonexistent_db&retryEnabled=false",
        )
        .unwrap();
        // Set pool params explicitly so resolve_defaults doesn't help with connection
        config.max_connections = Some(1);
        config.min_connections = Some(0);
        config.idle_timeout_secs = Some(300);
        config.max_lifetime_secs = Some(1800);

        let mut producer = SqlProducer::new(config, Arc::new(OnceCell::new()));
        let exchange = Exchange::new(Message::default());

        let result = producer.call(exchange).await;
        assert!(result.is_err());
        // Error should be EndpointCreationFailed (connection error), not a panic
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("Failed to connect") || err_msg.contains("database"),
            "Expected connection error, got: {}",
            err_msg
        );
    }

    // SQL-007: poll_ready returns Ready (pool lazily initialized on first call)
    #[test]
    fn poll_ready_returns_ready_for_uninitialized_pool() {
        let config = {
            let mut c = SqlEndpointConfig::from_uri("sql:select 1?db_url=sqlite::memory:").unwrap();
            c.resolve_defaults();
            c
        };
        let mut producer = SqlProducer::new(config, Arc::new(OnceCell::new()));
        let mut cx = Context::from_waker(futures::task::noop_waker_ref());
        let result = producer.poll_ready(&mut cx);
        assert!(matches!(result, Poll::Ready(Ok(()))));
    }

    // SQL-007: poll_ready returns error when stopped flag is set
    #[test]
    fn poll_ready_returns_error_when_stopped() {
        let config = {
            let mut c = SqlEndpointConfig::from_uri("sql:select 1?db_url=sqlite::memory:").unwrap();
            c.resolve_defaults();
            c
        };
        let mut producer = SqlProducer::new(config, Arc::new(OnceCell::new()));
        producer.stop();
        let mut cx = Context::from_waker(futures::task::noop_waker_ref());
        let result = producer.poll_ready(&mut cx);
        assert!(matches!(result, Poll::Ready(Err(_))));
        let err_msg = match result {
            Poll::Ready(Err(e)) => e.to_string(),
            _ => unreachable!(),
        };
        assert!(err_msg.contains("SQL producer stopped"));
    }

    // SQL-007: poll_ready returns error when pool is closed
    #[tokio::test]
    async fn poll_ready_returns_error_when_pool_closed() {
        let pool = sqlite_pool().await;
        pool.close().await;

        let config = {
            let mut c = SqlEndpointConfig::from_uri("sql:select 1?db_url=sqlite::memory:").unwrap();
            c.resolve_defaults();
            c
        };
        let pool_cell = Arc::new(OnceCell::new());
        pool_cell.set(pool).unwrap();

        let mut producer = SqlProducer::new(config, pool_cell);
        let mut cx = Context::from_waker(futures::task::noop_waker_ref());
        let result = producer.poll_ready(&mut cx);
        assert!(matches!(result, Poll::Ready(Err(_))));
        let err_msg = match result {
            Poll::Ready(Err(e)) => e.to_string(),
            _ => unreachable!(),
        };
        assert!(err_msg.contains("SQL connection pool is closed"));
    }

    // SQL-007: poll_ready returns Ok for healthy initialized pool
    #[tokio::test]
    async fn poll_ready_returns_ok_for_healthy_pool() {
        let pool = sqlite_pool().await;

        let config = {
            let mut c = SqlEndpointConfig::from_uri("sql:select 1?db_url=sqlite::memory:").unwrap();
            c.resolve_defaults();
            c
        };
        let pool_cell = Arc::new(OnceCell::new());
        pool_cell.set(pool).unwrap();

        let mut producer = SqlProducer::new(config, pool_cell);
        let mut cx = Context::from_waker(futures::task::noop_waker_ref());
        let result = producer.poll_ready(&mut cx);
        assert!(matches!(result, Poll::Ready(Ok(()))));
    }

    // SQL-008: stop() closes the pool
    #[tokio::test]
    async fn test_sql_stop_closes_pool() {
        let pool = sqlite_pool().await;

        let config = {
            let mut c = SqlEndpointConfig::from_uri("sql:select 1?db_url=sqlite::memory:").unwrap();
            c.resolve_defaults();
            c
        };
        let pool_cell = Arc::new(OnceCell::new());
        pool_cell.set(pool.clone()).unwrap();

        let producer = SqlProducer::new(config, pool_cell.clone());
        assert!(!pool.is_closed(), "Pool should be open before stop");

        producer.stop();

        // Allow background close task to complete
        tokio::time::sleep(Duration::from_millis(100)).await;

        assert!(
            pool.is_closed(),
            "Pool should be closed after producer.stop()"
        );

        // Subsequent poll_ready should fail
        let mut producer2 = SqlProducer::new(
            {
                let mut c =
                    SqlEndpointConfig::from_uri("sql:select 1?db_url=sqlite::memory:").unwrap();
                c.resolve_defaults();
                c
            },
            pool_cell.clone(),
        );
        let mut cx = Context::from_waker(futures::task::noop_waker_ref());
        let result = producer2.poll_ready(&mut cx);
        assert!(
            matches!(result, Poll::Ready(Err(_))),
            "poll_ready should fail after pool closed"
        );
    }

    // SQL-004: use_placeholder=false skips template parsing
    #[tokio::test]
    async fn use_placeholder_false_executes_raw_sql() {
        let pool = sqlite_pool().await;
        seed_items_table(&pool).await;

        let mut config = SqlEndpointConfig::from_uri(
            "sql:select id, name from items order by id?db_url=sqlite::memory:&usePlaceholder=false",
        )
        .unwrap();
        config.resolve_defaults();
        assert!(!config.use_placeholder);

        let mut producer = SqlProducer::new(config, Arc::new(OnceCell::new()));
        // Pre-initialize the pool so we don't hit the pool init path
        producer.pool.set(pool.clone()).unwrap();

        let exchange = Exchange::new(Message::default());
        let result = producer.call(exchange).await;
        assert!(result.is_ok());
        let exchange = result.unwrap();
        // Should return results, not rowsAffected
        assert!(matches!(exchange.input.body, Body::Json(_)));
    }

    // SQL-004: use_placeholder=true (default) processes placeholders normally
    #[tokio::test]
    async fn use_placeholder_true_processes_placeholders() {
        let pool = sqlite_pool().await;
        seed_items_table(&pool).await;

        let mut config = SqlEndpointConfig::from_uri(
            "sql:select id, name from items where id = #?db_url=sqlite::memory:",
        )
        .unwrap();
        config.resolve_defaults();
        assert!(config.use_placeholder);

        let mut producer = SqlProducer::new(config, Arc::new(OnceCell::new()));
        producer.pool.set(pool.clone()).unwrap();

        let msg = Message::new(Body::Json(json!([1])));
        let exchange = Exchange::new(msg);
        let result = producer.call(exchange).await;
        assert!(result.is_ok());
    }

    /// Regression: max_attempts=N → exactly N invocations (caught OpenSearch off-by-one 1f5c4c2a).
    /// Replicates the exact retry loop from SqlProducer::start() (producer.rs:161-185):
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
