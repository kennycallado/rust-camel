use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use serde_json::json;
use sqlx::any::AnyRow;
use sqlx::pool::PoolOptions;
use sqlx::AnyPool;
use tokio::sync::OnceCell;
use tower::Service;

use camel_api::{Body, CamelError, Exchange, Message};
use crate::config::{SqlConfig, SqlOutputType};
use crate::headers;
use crate::query::{is_select_query, parse_query_template, resolve_params, PreparedQuery};
use crate::utils::{bind_json_values, row_to_json};

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
                    let opts: PoolOptions<sqlx::Any> = PoolOptions::new()
                        .max_connections(config.max_connections)
                        .min_connections(config.min_connections)
                        .idle_timeout(Duration::from_secs(config.idle_timeout_secs))
                        .max_lifetime(Duration::from_secs(config.max_lifetime_secs));
                    opts.connect(&config.db_url)
                        .await
                        .map_err(|e| {
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

            // Parse template
            let template = parse_query_template(&query_str, config.placeholder)?;

            // Resolve parameters
            let prepared = resolve_params(&template, &exchange)?;

            // Execute based on mode
            if config.batch {
                execute_batch(pool, &config, &mut exchange).await?;
            } else if is_select_query(&prepared.sql) {
                execute_select(pool, &prepared, &config, &mut exchange).await?;
            } else {
                execute_modify(pool, &prepared, &config, &mut exchange).await?;
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
    let mut query = sqlx::query(&prepared.sql);
    query = bind_json_values(query, &prepared.bindings);

    let rows: Vec<AnyRow> = query
        .fetch_all(pool)
        .await
        .map_err(|e| CamelError::ProcessorError(format!("Query execution failed: {}", e)))?;

    let count = rows.len();

    // Convert rows to JSON
    let json_rows: Vec<serde_json::Value> = rows
        .iter()
        .map(row_to_json)
        .collect::<Result<Vec<_>, _>>()?;

    match config.output_type {
        SqlOutputType::SelectOne => {
            if let Some(first_row) = json_rows.into_iter().next() {
                exchange.input.body = Body::Json(first_row);
            } else {
                exchange.input.body = Body::Empty;
            }
        }
        SqlOutputType::SelectList | SqlOutputType::StreamList => {
            exchange.input.body = Body::Json(serde_json::Value::Array(json_rows));
        }
    }

    exchange
        .input
        .set_header(headers::ROW_COUNT, serde_json::json!(count));

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

    let result = query
        .execute(pool)
        .await
        .map_err(|e| CamelError::ProcessorError(format!("Query execution failed: {}", e)))?;

    let rows_affected = result.rows_affected();

    exchange
        .input
        .set_header(headers::UPDATE_COUNT, serde_json::json!(rows_affected));

    if config.noop {
        // Preserve original body
    } else {
        exchange.input.body = Body::Json(json!({ "rowsAffected": rows_affected }));
    }

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

    let mut total_rows_affected: u64 = 0;

    for params_array in batch_data {
        // Each item must be an array of parameters
        params_array.as_array().ok_or_else(|| {
            CamelError::ProcessorError(
                "Each batch item must be a JSON array of parameters".to_string(),
            )
        })?;

        // Create a temporary exchange with the params as body for resolution
        let temp_msg = Message::new(Body::Json(params_array.clone()));
        let temp_exchange = Exchange::new(temp_msg);

        // Resolve parameters for this batch item
        let prepared = resolve_params(&template, &temp_exchange)?;

        // Execute
        let mut query = sqlx::query(&prepared.sql);
        query = bind_json_values(query, &prepared.bindings);

        let result = query.execute(pool).await.map_err(|e| {
            CamelError::ProcessorError(format!("Batch query execution failed: {}", e))
        })?;

        total_rows_affected += result.rows_affected();
    }

    exchange
        .input
        .set_header(headers::UPDATE_COUNT, serde_json::json!(total_rows_affected));

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
        let values = vec![serde_json::json!(3.14159)];
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
}
