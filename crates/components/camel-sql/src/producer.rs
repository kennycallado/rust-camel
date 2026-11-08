use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Bytes;
use camel_api::datasource::DatasourceCatalog;
use serde_json::json;
use sqlx::AnyConnection;
use sqlx::AnyPool;
use sqlx::Either;
use sqlx::Executor;
use sqlx::SqlSafeStr;
use sqlx::any::AnyRow;
use sqlx::any::AnyTypeInfoKind;
use sqlx::pool::PoolOptions;
use tokio::sync::OnceCell;
use tower::Service;
use tracing::{debug, error, info, warn};

use crate::bind_kinds::{
    BindTarget, bind_batch_item, bind_single_item, bind_target_from_any_kind,
    described_mode_eligible, hybrid_fallback_targets, scan_bind_targets,
};
use crate::config::{SqlEndpointConfig, SqlOutputType, enrich_db_url_with_ssl, redact_db_url};
use crate::headers;
use crate::query::{PreparedQuery, is_select_query, parse_query_template, resolve_params};
use crate::utils::{fetch_error, is_retryable_sqlx_error, row_to_json};
use camel_component_api::retry_async;
use camel_component_api::{
    Body, CamelError, Exchange, Message, RuntimeObservability, StreamBody, StreamMetadata,
};

#[derive(Clone)]
pub struct SqlProducer {
    pub(crate) config: SqlEndpointConfig,
    pub(crate) pool: Arc<OnceCell<Arc<AnyPool>>>,
    pub(crate) catalog: Option<Arc<dyn DatasourceCatalog>>,
    pub(crate) stopped: Arc<AtomicBool>,
    pub(crate) runtime: Arc<dyn RuntimeObservability>,
    /// Route identifier for ADR-0012 (g) health/observability calls.
    pub(crate) route_id: String,
}

impl SqlProducer {
    pub fn new(
        config: SqlEndpointConfig,
        pool: Arc<OnceCell<Arc<AnyPool>>>,
        catalog: Option<Arc<dyn DatasourceCatalog>>,
        runtime: Arc<dyn RuntimeObservability>,
        route_id: impl Into<String>,
    ) -> Self {
        Self {
            config,
            pool,
            catalog,
            stopped: Arc::new(AtomicBool::new(false)),
            runtime,
            route_id: route_id.into(),
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
    /// 1. Header `CamelSql.Query` (only if `allow_dynamic_query=true`)
    /// 2. Body (only if `allow_dynamic_query=true` AND `use_message_body_for_sql=true`)
    /// 3. Config query
    ///
    /// H7: header/body are untrusted exchange data (ADR-0032). Sourcing the
    /// query from them is full SQLi (incl. DDL). Both routes require the
    /// explicit `allow_dynamic_query=true` opt-in.
    pub(crate) fn resolve_query_source(exchange: &Exchange, config: &SqlEndpointConfig) -> String {
        if config.allow_dynamic_query {
            // Priority 1: Header
            if let Some(query_value) = exchange.input.header(headers::QUERY)
                && let Some(query_str) = query_value.as_str()
            {
                return query_str.to_string();
            }

            // Priority 2: Body (requires BOTH flags)
            if config.use_message_body_for_sql
                && let Some(body_text) = exchange.input.body.as_text()
            {
                return body_text.to_string();
            }
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
        let pool =
            if let (Some(catalog), Some(ds_name)) = (&self.catalog, &self.config.datasource_name) {
                let handle = catalog.get_pool(ds_name).await?;
                let arc_pool: Arc<AnyPool> = handle.downcast()?;
                // We don't cache this here — check_connection is a one-shot call
                arc_pool
            } else {
                self.pool
                    .get()
                    .ok_or_else(|| {
                        CamelError::ProcessorError("SQL connection pool not initialized".into())
                    })?
                    .clone()
            };

        debug!("Running health check: SELECT 1");
        sqlx::query("SELECT 1").execute(&*pool).await.map_err(|e| {
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
        let catalog = self.catalog.clone();
        let runtime = Arc::clone(&self.runtime);
        let route_id = self.route_id.clone();

        Box::pin(async move {
            // SQL-014: resolve file-based query before pool init, regardless of pool source
            config.resolve_defaults();
            config.resolve_file_query().await?;

            // Get or initialize the connection pool
            let pool: &Arc<AnyPool> = pool_cell
                .get_or_try_init(|| async {
                    // Catalog path: resolve shared pool from the datasource catalog
                    let ds_name = config.datasource_name.clone();
                    if let (Some(ref cat), Some(ref name)) = (catalog, ds_name) {
                        let handle = cat.get_pool(name).await?;
                        return handle.downcast::<AnyPool>();
                    }

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
                    let pool = retry_async::<_, _, _, _, sqlx::Error>(
                        retry_policy,
                        "sql",
                        "producer-pool-init",
                        || {
                            async {
                                PoolOptions::new()
                                    .max_connections(max_conn)
                                    .min_connections(min_conn)
                                    .idle_timeout(Duration::from_secs(idle_timeout))
                                    .max_lifetime(Duration::from_secs(max_lifetime))
                                    .connect(&db_url)
                                    .await
                            }
                        },
                        is_retryable_sqlx_error,
                        Some(runtime.metrics().as_ref()),
                    )
                    .await
                    .map_err(|e| {
                        runtime.health().force_unhealthy_for_route(
                            &route_id,
                            "g:sql:producer-pool-init",
                            &e.to_string(),
                        );
                        // log-policy: outside-contract
                        error!(error = %e, db_url = %redact_db_url(&config.db_url), "Failed to connect to database");
                        CamelError::EndpointCreationFailed(format!(
                            "Failed to connect to database: {}",
                            e
                        ))
                    })?;
                    Ok(Arc::new(pool))
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
                execute_batch(pool.as_ref(), &config, &mut exchange).await?;
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
                    execute_select(pool.as_ref(), &prepared, &config, &mut exchange).await?;
                } else {
                    execute_modify(pool.as_ref(), &prepared, &config, &mut exchange).await?;
                }
            } else {
                // use_placeholder=false: execute query as-is without template parsing
                debug!("Executing raw SQL (placeholder processing disabled)");
                let prepared = PreparedQuery {
                    sql: query_str,
                    bindings: vec![],
                };

                if is_select_query(&prepared.sql) {
                    execute_select(pool.as_ref(), &prepared, &config, &mut exchange).await?;
                } else {
                    execute_modify(pool.as_ref(), &prepared, &config, &mut exchange).await?;
                }
            }

            Ok(exchange)
        })
    }
}

/// The bind plan for one single-mode statement (bd rc-mn139): which SQL
/// text to execute, the per-position wire targets, and whether the
/// statement may use the persistent cache.
struct SingleBindPlan {
    sql: String,
    targets: Vec<Option<BindTarget>>,
    persistent: bool,
}

/// Plans the binds for one single-mode statement on `conn` (the SAME
/// pooled connection that will execute it — normalization must target
/// the OIDs of the statement that actually runs, bd rc-mn139).
async fn plan_single_binds(
    conn: &mut AnyConnection,
    sql: &str,
    bindings: &[serde_json::Value],
) -> Result<SingleBindPlan, CamelError> {
    // Nothing to reinterpret: keep the default persistent path, with no
    // probe round trip.
    if bindings.is_empty() {
        return Ok(SingleBindPlan {
            sql: sql.to_string(),
            targets: vec![],
            persistent: true,
        });
    }

    // Probe on a distinct SQL-text namespace: preparing with empty
    // parameter types seeds a statement cache entry under the probed text,
    // and for unmapped parameter types that entry is one no payload can
    // satisfy. Keeping the probe on its own text keeps the user's SQL text
    // namespace clean (mirrors the batch probe).
    let probe_sql = format!("{sql} /* single-bind-kinds-probe */");
    let described_kinds = match Executor::prepare_with(
        &mut *conn,
        sqlx::AssertSqlSafe(probe_sql).into_sql_str(),
        &[],
    )
    .await
    {
        Ok(statement) => match statement.parameters {
            // An empty type list cannot drive normalization.
            Some(Either::Left(types)) if !types.is_empty() => Some(
                types
                    .iter()
                    .map(|type_info| type_info.kind())
                    .collect::<Vec<AnyTypeInfoKind>>(),
            ),
            _ => None,
        },
        // Metadata unavailable (e.g. NUMERIC columns have no Any mapping).
        // Server and connection failures propagate unchanged.
        Err(sqlx::Error::AnyDriverError(_)) => None,
        Err(e) => {
            // log-policy: handler-owned
            // (category a: single-mode probe failure inside producer pipeline)
            warn!("Single describe probe failed: {}", e);
            return Err(CamelError::ProcessorError(format!(
                "SQL query prepare failed: {e}"
            )));
        }
    };

    // The fallback alias is distinct from the probe alias on purpose: sqlx
    // consults the SQL-text cache before the persistence flag, so a
    // probe-seeded entry would otherwise be reused. Nothing is cached
    // under the fallback text.
    let fallback_sql = format!("{sql} /* single-bind-kinds-fallback */");

    let Some(kinds) = described_kinds else {
        return Ok(SingleBindPlan {
            sql: fallback_sql,
            targets: scan_bind_targets(&[bindings]),
            persistent: false,
        });
    };

    let described_targets = kinds
        .iter()
        .map(|kind| bind_target_from_any_kind(*kind))
        .collect::<Vec<_>>();

    if !described_mode_eligible(&kinds, bindings) {
        // Hybrid fallback: keep every described numeric target so the
        // checked guards still fire where the server described a numeric
        // position; scan only the unresolved ones.
        return Ok(SingleBindPlan {
            sql: fallback_sql,
            targets: hybrid_fallback_targets(&described_targets, bindings),
            persistent: false,
        });
    }

    // Describe with empty Parse parameter types so the server infers the
    // parameter OIDs from the SQL text. On a cache hit the read-back
    // parameters are the cached statement's actual OIDs — normalization
    // targets those, so payloads always match the executed statement.
    let statement = Executor::prepare_with(
        &mut *conn,
        sqlx::AssertSqlSafe(sql.to_string()).into_sql_str(),
        &[],
    )
    .await
    .map_err(|e| {
        // log-policy: handler-owned
        // (category a: single-mode described prepare failure inside producer pipeline)
        warn!("Single describe failed: {}", e);
        CamelError::ProcessorError(format!("SQL query prepare failed: {e}"))
    })?;

    let targets = match statement.parameters {
        Some(Either::Left(types)) => types
            .iter()
            .map(|type_info| bind_target_from_any_kind(type_info.kind()))
            .collect::<Vec<_>>(),
        _ => scan_bind_targets(&[bindings]),
    };

    Ok(SingleBindPlan {
        sql: sql.to_string(),
        targets,
        persistent: true,
    })
}

/// Executes one single-mode SELECT on a pooled connection, planning the
/// binds first (bd rc-mn139). Statements without bindings plan to the
/// default persistent path with no probe and no binds.
async fn fetch_single_rows(
    pool: &AnyPool,
    prepared: &PreparedQuery,
) -> Result<Vec<AnyRow>, CamelError> {
    let mut conn = pool
        .acquire()
        .await
        .map_err(|e| fetch_error("Failed to acquire database connection", e))?;

    let plan = plan_single_binds(&mut conn, &prepared.sql, &prepared.bindings).await?;
    let query = sqlx::query(sqlx::AssertSqlSafe(plan.sql.as_str())).persistent(plan.persistent);
    let query = bind_single_item(query, &prepared.bindings, &plan.targets)?;
    query
        .fetch_all(&mut *conn)
        .await
        .map_err(|e| fetch_error("SQL query failed", e))
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
            let rows = fetch_single_rows(pool, prepared).await?;

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
            let rows = fetch_single_rows(pool, prepared).await?;

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
                let mut conn = pool_clone
                    .acquire()
                    .await
                    .map_err(|e| fetch_error("Failed to acquire database connection", e))?;

                // Plan and execute against the SAME pooled connection so
                // normalization targets the OIDs of the statement that
                // actually runs (bd rc-mn139).
                let plan = plan_single_binds(&mut conn, &sql_str, &bindings).await?;
                let q = sqlx::query(sqlx::AssertSqlSafe(plan.sql.as_str()))
                    .persistent(plan.persistent);
                let q = bind_single_item(q, &bindings, &plan.targets)?;
                let mut rows = q.fetch(&mut *conn);
                while let Some(row) = rows
                    .try_next()
                    .await
                    .map_err(|e| fetch_error("Query execution failed", e))?
                {
                    let json_val = row_to_json(&row)?;
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
    let mut conn = pool
        .acquire()
        .await
        .map_err(|e| fetch_error("Failed to acquire database connection", e))?;

    let plan = plan_single_binds(&mut conn, &prepared.sql, &prepared.bindings).await?;
    let query = sqlx::query(sqlx::AssertSqlSafe(plan.sql.as_str())).persistent(plan.persistent);
    let query = bind_single_item(query, &prepared.bindings, &plan.targets)?;
    let result = query.execute(&mut *conn).await.map_err(|e| {
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
///
/// Bind-kind safety (bd rc-fef4z): sqlx keys its statement cache by SQL text
/// alone and fixes parameter OIDs at the first `Parse`, so a position that
/// carries different Rust bind kinds across items silently reinterprets
/// bits. Every position therefore binds exactly one wire encoding per
/// statement: the server-described encoding when available, otherwise a
/// per-batch scan (float wins) executed without statement caching.
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

    // Resolve every item up front so the fallback bind-kind scan sees the
    // whole batch.
    let mut prepared_items = Vec::with_capacity(batch_data.len());
    for (batch_idx, params_array) in batch_data.iter().enumerate() {
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

        prepared_items.push(resolve_params(
            &template,
            &temp_exchange,
            &config.in_separator,
        )?);
    }

    // Fallback targets: one bind kind per position across the batch, float wins.
    let binding_rows: Vec<&[serde_json::Value]> = prepared_items
        .iter()
        .map(|prepared| prepared.bindings.as_slice())
        .collect();
    let scanned_targets = scan_bind_targets(&binding_rows);

    // Fix 2: Batch operations must be wrapped in a transaction
    let mut tx = pool.begin().await.map_err(|e| {
        // log-policy: handler-owned
        // (category a: tx begin failure inside producer pipeline)
        warn!(error = %e, "Failed to begin transaction");
        CamelError::ProcessorError(format!("Failed to begin transaction: {}", e))
    })?;

    // Probe the described parameter types once with the first item's SQL
    // text. Described batches keep using the transaction connection (later
    // items with the same text hit the statement cache for free).
    let mut described_mode = true;
    if let Some(first) = prepared_items.first() {
        // Probe on a distinct SQL-text namespace: preparing with empty
        // parameter types seeds a statement cache entry under the probed
        // text, and for unmapped types (e.g. NUMERIC) that entry is one no
        // payload can satisfy. Keeping the probe on its own text keeps the
        // user's SQL text namespace clean.
        let probe_sql = format!("{} /* batch-bind-kinds-probe */", first.sql);
        described_mode = match Executor::prepare_with(
            &mut *tx,
            sqlx::AssertSqlSafe(probe_sql).into_sql_str(),
            &[],
        )
        .await
        {
            Ok(statement) => match statement.parameters {
                // Any-mapped parameter types are described-eligible: numeric
                // kinds get bind targets, non-numeric kinds (text, bool,
                // blob) keep the default JSON binding under their described
                // OIDs. Unmapped kinds (e.g. NUMERIC) fail the probe itself.
                // An empty type list cannot drive normalization.
                Some(Either::Left(types)) => !types.is_empty(),
                _ => false,
            },
            // Metadata unavailable (e.g. NUMERIC columns have no Any
            // mapping). Server and connection failures propagate unchanged.
            Err(sqlx::Error::AnyDriverError(_)) => false,
            Err(e) => {
                // log-policy: handler-owned
                // (category a: batch probe failure inside producer pipeline)
                warn!("Batch describe probe failed: {}", e);
                return Err(CamelError::ProcessorError(format!(
                    "Batch query prepare failed: {e}"
                )));
            }
        };
    }

    let mut total_rows_affected: u64 = 0;

    for (batch_idx, prepared) in prepared_items.iter().enumerate() {
        let (mut query, targets) = if described_mode {
            // Describe with empty Parse parameter types so the server infers
            // the parameter OIDs from the SQL text. On a cache hit the
            // read-back parameters are the cached statement's actual OIDs —
            // normalization targets those, so payloads always match the
            // executed statement.
            let statement = Executor::prepare_with(
                &mut *tx,
                sqlx::AssertSqlSafe(prepared.sql.clone()).into_sql_str(),
                &[],
            )
            .await
            .map_err(|e| {
                // log-policy: handler-owned
                // (category a: batch describe failure inside producer pipeline)
                warn!("Batch describe failed at index {}: {}", batch_idx, e);
                CamelError::ProcessorError(format!("Batch query prepare failed: {e}"))
            })?;
            let query = sqlx::query(sqlx::AssertSqlSafe(prepared.sql.as_str()));
            let targets = match statement.parameters {
                Some(Either::Left(types)) => types
                    .iter()
                    .map(|type_info| bind_target_from_any_kind(type_info.kind()))
                    .collect::<Vec<_>>(),
                _ => scanned_targets.clone(),
            };
            (query, targets)
        } else {
            // Uncached fallback: execute a semantically identical alias text
            // in its own cache namespace with persistent(false), so every
            // item prepares an unnamed statement whose parameter types
            // derive from the scan-normalized binds. Nothing is cached and
            // nothing else uses this namespace, so no incompatible cached
            // entry can be reused.
            let fallback_sql = format!("{} /* batch-bind-kinds-fallback */", prepared.sql);
            let query = sqlx::query(sqlx::AssertSqlSafe(fallback_sql.as_str())).persistent(false);
            (query, scanned_targets.clone())
        };

        query = bind_batch_item(query, &prepared.bindings, &targets, batch_idx)?;

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
    use crate::bind_kinds::bind_json_values;
    use camel_api::MetricsCollector;
    use camel_component_api::HealthCheckRegistry;
    use camel_component_api::test_support::PanicRuntimeObservability;
    fn test_rt() -> std::sync::Arc<dyn camel_component_api::RuntimeObservability> {
        std::sync::Arc::new(PanicRuntimeObservability)
    }
    use camel_component_api::Message;
    use camel_component_api::UriConfig;
    use sqlx::any::AnyPoolOptions;
    use std::sync::Arc;
    use std::sync::Mutex;
    use tokio::sync::OnceCell;

    // ── Recording health fixture for ADR-0012 (g) tests ───────────────────

    #[derive(Debug, Default)]
    struct RecordingHealth {
        forced: Mutex<Vec<(String, String, String)>>,
    }

    impl HealthCheckRegistry for RecordingHealth {
        fn force_unhealthy_for_route(&self, route_id: &str, name: &str, reason: &str) {
            self.forced.lock().unwrap().push((
                route_id.to_string(),
                name.to_string(),
                reason.to_string(),
            ));
        }
    }

    struct NoopMetrics;

    impl MetricsCollector for NoopMetrics {
        fn record_exchange_duration(&self, _: &str, _: Duration) {}
        fn increment_errors(&self, _: &str, _: &str) {}
        fn increment_exchanges(&self, _: &str) {}
        fn set_queue_depth(&self, _: &str, _: usize) {}
        fn record_circuit_breaker_change(&self, _: &str, _: &str, _: &str) {}
    }

    struct RecordingRuntime {
        health: Arc<RecordingHealth>,
    }

    impl RuntimeObservability for RecordingRuntime {
        fn metrics(&self) -> Arc<dyn MetricsCollector> {
            Arc::new(NoopMetrics)
        }
        fn health(&self) -> Arc<dyn HealthCheckRegistry> {
            self.health.clone()
        }
    }

    async fn sqlite_pool() -> AnyPool {
        sqlx::any::install_default_drivers();
        // Per-attempt deadline (lintwiden D4.3): a wedged pool connect
        // must fail the helper instead of parking it.
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            AnyPoolOptions::new()
                .max_connections(1)
                .connect("sqlite::memory:"),
        )
        .await
        .expect("sqlite pool connect timed out after 10s")
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
        let p1 = SqlProducer::new(
            config(),
            Arc::new(OnceCell::new()),
            None,
            test_rt(),
            "sql-producer-test-route",
        );
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
        let mut config = config();
        config.allow_dynamic_query = true;
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
        config.allow_dynamic_query = true;
        let msg = Message::new(Body::Text("select 3".to_string()));
        let ex = Exchange::new(msg);
        let q = SqlProducer::resolve_query_source(&ex, &config);
        assert_eq!(q, "select 3");
    }

    #[test]
    fn resolve_query_header_priority_over_body() {
        let mut config = config();
        config.use_message_body_for_sql = true;
        config.allow_dynamic_query = true;
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
        config.allow_dynamic_query = true;
        let msg = Message::new(Body::Text("select from body".to_string()));
        let ex = Exchange::new(msg);
        let q = SqlProducer::resolve_query_source(&ex, &config);
        assert_eq!(q, "select from body");
    }

    // H7: dynamic queries from exchange data must be denied by default.
    // The CamelSql.Query header and body are untrusted (ADR-0032). Reading
    // them as a SQL source is a SQLi vector including DDL. Default is closed.
    #[test]
    fn dynamic_query_denied_by_default() {
        let mut msg = Message::default();
        msg.set_header(headers::QUERY, "DROP TABLE users; --");
        let ex = Exchange::new(msg);

        let mut config = config();
        config.query = "SELECT 1".to_string();
        // allow_dynamic_query defaults to false → header ignored

        let q = SqlProducer::resolve_query_source(&ex, &config);
        // Must return the config query, NOT the header
        assert_eq!(q, "SELECT 1");
    }

    // H7: with explicit opt-in (allow_dynamic_query=true), header takes priority.
    #[test]
    fn dynamic_query_allowed_with_opt_in() {
        let mut msg = Message::default();
        msg.set_header(headers::QUERY, "SELECT 1 FROM users WHERE id = :#id");
        let ex = Exchange::new(msg);

        let mut config = config();
        config.query = "SELECT 1".to_string();
        config.allow_dynamic_query = true;

        let q = SqlProducer::resolve_query_source(&ex, &config);
        // With opt-in, header takes priority
        assert_eq!(q, "SELECT 1 FROM users WHERE id = :#id");
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

        let mut producer = SqlProducer::new(
            config,
            Arc::new(OnceCell::new()),
            None,
            // Noop runtime: pool-init retries now record per-attempt
            // telemetry by design, which the panic runtime would reject.
            std::sync::Arc::new(camel_component_api::test_support::NoopRuntimeObservability),
            "sql-producer-test-route",
        );
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

        let health = Arc::new(RecordingHealth::default());
        let rt: Arc<dyn RuntimeObservability> = Arc::new(RecordingRuntime {
            health: health.clone(),
        });
        let mut producer = SqlProducer::new(
            config,
            Arc::new(OnceCell::new()),
            None,
            rt,
            "sql-producer-test-route",
        );
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
        // Verify force_unhealthy_for_route was also called
        let forced = health.forced.lock().unwrap();
        assert_eq!(
            forced.len(),
            1,
            "expected one force_unhealthy_for_route call"
        );
        assert_eq!(forced[0].0, "sql-producer-test-route");
        assert_eq!(forced[0].1, "g:sql:producer-pool-init");
    }

    // SQL-007: poll_ready returns Ready (pool lazily initialized on first call)
    #[test]
    fn poll_ready_returns_ready_for_uninitialized_pool() {
        let config = {
            let mut c = SqlEndpointConfig::from_uri("sql:select 1?db_url=sqlite::memory:").unwrap();
            c.resolve_defaults();
            c
        };
        let mut producer = SqlProducer::new(
            config,
            Arc::new(OnceCell::new()),
            None,
            test_rt(),
            "sql-producer-test-route",
        );
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
        let mut producer = SqlProducer::new(
            config,
            Arc::new(OnceCell::new()),
            None,
            test_rt(),
            "sql-producer-test-route",
        );
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
        pool_cell.set(Arc::new(pool)).unwrap();

        let mut producer = SqlProducer::new(
            config,
            pool_cell,
            None,
            test_rt(),
            "sql-producer-test-route",
        );
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
        pool_cell.set(Arc::new(pool)).unwrap();

        let mut producer = SqlProducer::new(
            config,
            pool_cell,
            None,
            test_rt(),
            "sql-producer-test-route",
        );
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
        pool_cell.set(Arc::new(pool.clone())).unwrap();

        let producer = SqlProducer::new(
            config,
            pool_cell.clone(),
            None,
            test_rt(),
            "sql-producer-test-route",
        );
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
            None,
            test_rt(),
            "sql-producer-test-route",
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

        let mut producer = SqlProducer::new(
            config,
            Arc::new(OnceCell::new()),
            None,
            test_rt(),
            "sql-producer-test-route",
        );
        // Pre-initialize the pool so we don't hit the pool init path
        producer.pool.set(Arc::new(pool.clone())).unwrap();

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

        let mut producer = SqlProducer::new(
            config,
            Arc::new(OnceCell::new()),
            None,
            test_rt(),
            "sql-producer-test-route",
        );
        producer.pool.set(Arc::new(pool.clone())).unwrap();

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
        let _result: Result<(), ()> = tokio::time::timeout(Duration::from_secs(30), async {
            loop {
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
            }
        })
        .await
        .expect("retry loop must finish within 30s");

        assert_eq!(
            calls.load(Ordering::SeqCst),
            3,
            "max_attempts=3 must yield exactly 3 invocations"
        );
    }

    // ── ADR-0012 (g) regression tests ──────────────────────────────────

    /// Regression: producer pool init failure calls force_unhealthy_for_route
    /// with correct route_id + name "g:sql:producer-pool-init" + non-empty reason.
    #[tokio::test]
    async fn producer_pool_init_failure_calls_force_unhealthy_for_route() {
        let health = Arc::new(RecordingHealth::default());
        let rt: Arc<dyn RuntimeObservability> = Arc::new(RecordingRuntime {
            health: health.clone(),
        });

        let mut config = SqlEndpointConfig::from_uri(
            "sql:select 1?db_url=postgres://nonexistent-host:5432/nonexistent_db&retryEnabled=false",
        )
        .unwrap();
        config.max_connections = Some(1);
        config.min_connections = Some(0);
        config.idle_timeout_secs = Some(300);
        config.max_lifetime_secs = Some(1800);

        let mut producer = SqlProducer::new(
            config,
            Arc::new(OnceCell::new()),
            None,
            rt,
            "sql-producer-test-route",
        );
        let exchange = Exchange::new(Message::default());

        let result = producer.call(exchange).await;
        assert!(result.is_err());

        let forced = health.forced.lock().unwrap();
        assert_eq!(
            forced.len(),
            1,
            "expected one force_unhealthy_for_route call"
        );
        assert_eq!(forced[0].0, "sql-producer-test-route");
        assert_eq!(forced[0].1, "g:sql:producer-pool-init");
        assert!(!forced[0].2.is_empty(), "reason should be non-empty");
    }

    // H7: from_uri must default allow_dynamic_query to false when the
    // URI does not include the allowDynamicQuery parameter.
    #[test]
    fn sql_allow_dynamic_query_defaults_false_from_uri() {
        let config =
            SqlEndpointConfig::from_uri("sql:query?datasource=mydb&query=SELECT+1").unwrap();
        assert!(!config.allow_dynamic_query);
    }
}
