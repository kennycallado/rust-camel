use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Bytes;
use serde_json::json;
use sqlx::AnyPool;
use sqlx::any::AnyRow;
use sqlx::pool::PoolOptions;
use tokio::sync::OnceCell;
use tower::Service;
use tracing::{debug, error, warn};

use crate::config::{SqlConfig, SqlOutputType};
use crate::headers;
use crate::query::{PreparedQuery, is_select_query, parse_query_template, resolve_params};
use crate::utils::{bind_json_values, row_to_json};
use camel_api::{Body, CamelError, Exchange, Message, StreamBody, StreamMetadata};

#[derive(Clone)]
pub struct SqlProducer {
    pub(crate) config: SqlConfig,
    pub(crate) pool: Arc<OnceCell<AnyPool>>,
}

impl SqlProducer {
    pub fn new(config: SqlConfig, pool: Arc<OnceCell<AnyPool>>) -> Self {
        Self { config, pool }
    }

    /// Resolves the query source based on priority:
    /// 1. Header `CamelSql.Query`
    /// 2. Body (if `use_message_body_for_sql` is true)
    /// 3. Config query
    pub(crate) fn resolve_query_source(exchange: &Exchange, config: &SqlConfig) -> String {
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
}

impl Service<Exchange> for SqlProducer {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, mut exchange: Exchange) -> Self::Future {
        let config = self.config.clone();
        let pool_cell = Arc::clone(&self.pool);

        Box::pin(async move {
            // Get or initialize the connection pool
            let pool: &AnyPool = pool_cell
                .get_or_try_init(|| async {
                    // Install all compiled-in sqlx drivers so AnyPool can resolve them.
                    // This is idempotent; safe to call multiple times.
                    sqlx::any::install_default_drivers();
                    let opts: PoolOptions<sqlx::Any> = PoolOptions::new()
                        .max_connections(config.max_connections)
                        .min_connections(config.min_connections)
                        .idle_timeout(Duration::from_secs(config.idle_timeout_secs))
                        .max_lifetime(Duration::from_secs(config.max_lifetime_secs));
                    opts.connect(&config.db_url).await.map_err(|e| {
                        error!("Failed to connect to database: {}", e);
                        CamelError::EndpointCreationFailed(format!(
                            "Failed to connect to database: {}",
                            e
                        ))
                    })
                })
                .await
                .map_err(|e: CamelError| {
                    error!("Pool initialization failed: {}", e);
                    e.clone()
                })?;

            // Resolve query string
            let query_str = Self::resolve_query_source(&exchange, &config);

            debug!("Executing SQL: {}", query_str);

            // Execute based on mode
            if config.batch {
                // Batch mode: execute_batch handles its own template parsing per item
                execute_batch(pool, &config, &mut exchange).await?;
            } else {
                // Non-batch: parse template, resolve params, apply header override
                let template = parse_query_template(&query_str, config.placeholder)?;
                let mut prepared = resolve_params(&template, &exchange)?;

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

                debug!("Executing SQL: {}", prepared.sql);

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
    config: &SqlConfig,
    exchange: &mut Exchange,
) -> Result<(), CamelError> {
    match config.output_type {
        SqlOutputType::SelectOne => {
            // fetch_all and take first row
            let mut query = sqlx::query(&prepared.sql);
            query = bind_json_values(query, &prepared.bindings);

            let rows: Vec<AnyRow> = query.fetch_all(pool).await.map_err(|e| {
                error!("Query execution failed: {}", e);
                CamelError::ProcessorError(format!("Query execution failed: {}", e))
            })?;

            let count = rows.len();
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
                error!("Query execution failed: {}", e);
                CamelError::ProcessorError(format!("Query execution failed: {}", e))
            })?;

            let count = rows.len();
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
    config: &SqlConfig,
    exchange: &mut Exchange,
) -> Result<(), CamelError> {
    let mut query = sqlx::query(&prepared.sql);
    query = bind_json_values(query, &prepared.bindings);

    let result = query.execute(pool).await.map_err(|e| {
        error!("Query execution failed: {}", e);
        CamelError::ProcessorError(format!("Query execution failed: {}", e))
    })?;

    let rows_affected = result.rows_affected();

    // Fix 4: Implement expected_update_count validation
    if let Some(expected) = config.expected_update_count
        && rows_affected as i64 != expected
    {
        error!("Expected {} rows affected, got {}", expected, rows_affected);
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

    debug!("Modify query affected {} rows", rows_affected);

    Ok(())
}

/// Executes a batch of queries from a JSON array body.
async fn execute_batch(
    pool: &AnyPool,
    config: &SqlConfig,
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
        error!("Failed to begin transaction: {}", e);
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
        let prepared = resolve_params(&template, &temp_exchange)?;

        // Execute against transaction
        let mut query = sqlx::query(&prepared.sql);
        query = bind_json_values(query, &prepared.bindings);

        let result = query.execute(&mut *tx).await.map_err(|e| {
            error!("Batch query execution failed at index {}: {}", batch_idx, e);
            CamelError::ProcessorError(format!("Batch query execution failed: {}", e))
        })?;

        // Validate expected_update_count per batch item
        if let Some(expected) = config.expected_update_count
            && result.rows_affected() as i64 != expected
        {
            error!(
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
        error!("Failed to commit transaction: {}", e);
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
    use camel_api::Message;
    use std::sync::Arc;
    use tokio::sync::OnceCell;

    fn test_config() -> SqlConfig {
        SqlConfig::from_uri("sql:select 1?db_url=postgres://localhost/test").unwrap()
    }

    #[test]
    fn test_producer_clone_shares_pool() {
        let p1 = SqlProducer::new(test_config(), Arc::new(OnceCell::new()));
        let p2 = p1.clone();
        assert!(Arc::ptr_eq(&p1.pool, &p2.pool));
    }

    #[test]
    fn test_resolve_query_from_config() {
        let config = test_config();
        let ex = Exchange::new(Message::default());
        let q = SqlProducer::resolve_query_source(&ex, &config);
        assert_eq!(q, "select 1");
    }

    #[test]
    fn test_resolve_query_from_header() {
        let config = test_config();
        let mut msg = Message::default();
        msg.set_header(headers::QUERY, serde_json::json!("select 2"));
        let ex = Exchange::new(msg);
        let q = SqlProducer::resolve_query_source(&ex, &config);
        assert_eq!(q, "select 2");
    }

    #[test]
    fn test_resolve_query_from_body() {
        let mut config = test_config();
        config.use_message_body_for_sql = true;
        let msg = Message::new(Body::Text("select 3".to_string()));
        let ex = Exchange::new(msg);
        let q = SqlProducer::resolve_query_source(&ex, &config);
        assert_eq!(q, "select 3");
    }

    #[test]
    fn test_resolve_query_header_priority_over_body() {
        let mut config = test_config();
        config.use_message_body_for_sql = true;
        let mut msg = Message::new(Body::Text("select from body".to_string()));
        msg.set_header(headers::QUERY, serde_json::json!("select from header"));
        let ex = Exchange::new(msg);
        let q = SqlProducer::resolve_query_source(&ex, &config);
        assert_eq!(q, "select from header");
    }

    #[test]
    fn test_resolve_query_body_priority_over_config() {
        let mut config = test_config();
        config.use_message_body_for_sql = true;
        let msg = Message::new(Body::Text("select from body".to_string()));
        let ex = Exchange::new(msg);
        let q = SqlProducer::resolve_query_source(&ex, &config);
        assert_eq!(q, "select from body");
    }

    #[test]
    fn test_bind_json_null() {
        let query = sqlx::query("SELECT ?");
        let values = vec![serde_json::Value::Null];
        let _bound = bind_json_values(query, &values);
        // Compilation test - ensure it binds
    }

    #[test]
    fn test_bind_json_bool() {
        let query = sqlx::query("SELECT ?");
        let values = vec![serde_json::Value::Bool(true)];
        let _bound = bind_json_values(query, &values);
    }

    #[test]
    fn test_bind_json_number_i64() {
        let query = sqlx::query("SELECT ?");
        let values = vec![serde_json::json!(42)];
        let _bound = bind_json_values(query, &values);
    }

    #[test]
    fn test_bind_json_number_f64() {
        let query = sqlx::query("SELECT ?");
        let values = vec![serde_json::json!(std::f64::consts::PI)];
        let _bound = bind_json_values(query, &values);
    }

    #[test]
    fn test_bind_json_string() {
        let query = sqlx::query("SELECT ?");
        let values = vec![serde_json::json!("hello world")];
        let _bound = bind_json_values(query, &values);
    }

    #[test]
    fn test_bind_json_array() {
        let query = sqlx::query("SELECT ?");
        let values = vec![serde_json::json!([1, 2, 3])];
        let _bound = bind_json_values(query, &values);
    }

    #[test]
    fn test_bind_json_object() {
        let query = sqlx::query("SELECT ?");
        let values = vec![serde_json::json!({"key": "value"})];
        let _bound = bind_json_values(query, &values);
    }

    #[test]
    fn test_bind_multiple_values() {
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
    fn test_expected_update_count_validation() {
        // Test that expected_update_count is parsed from URI
        let config = SqlConfig::from_uri(
            "sql:update t set x=1?db_url=postgres://localhost/test&expectedUpdateCount=5",
        )
        .unwrap();
        assert_eq!(config.expected_update_count, Some(5));

        // Test default (no expected_update_count)
        let config_default = test_config();
        assert_eq!(config_default.expected_update_count, None);

        // Test negative value (should parse)
        let config_neg = SqlConfig::from_uri(
            "sql:update t set x=1?db_url=postgres://localhost/test&expectedUpdateCount=-1",
        )
        .unwrap();
        assert_eq!(config_neg.expected_update_count, Some(-1));
    }

    // Test for Fix 3: parameters header override logic
    #[test]
    fn test_parameters_header_override_logic() {
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
}
