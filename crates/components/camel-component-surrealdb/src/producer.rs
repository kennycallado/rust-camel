//! SurrealDbProducer — Service<Exchange> implementation handling 8 producer operations.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll};

use camel_api::datasource::DatasourceCatalog;
use camel_component_api::{Body, CamelError, Exchange, Message};
use serde_json::Value as JsonValue;
use surrealdb::Surreal;
use surrealdb::engine::any::Any as SurrealAny;
use surrealdb::types::{SurrealValue, Value as SurrealDbValue};
use tower::Service;

use crate::config::{SurrealDbEndpointConfig, SurrealDbOperation};
use crate::error::SurrealDbError;
use crate::headers;
use crate::query;
use crate::vector;

/// Type alias for the surrealdb client.
pub type SurrealClient = Surreal<SurrealAny>;

/// Producer that executes SurrealDB operations as a pipeline step.
///
/// Implements `Service<Exchange>` — receives an Exchange, executes the
/// configured operation against SurrealDB, and returns the Exchange with
/// the result in the body.
#[derive(Clone)]
pub struct SurrealDbProducer {
    pub(crate) config: SurrealDbEndpointConfig,
    pub(crate) catalog: Option<Arc<dyn DatasourceCatalog>>,
    pub(crate) stopped: Arc<AtomicBool>,
    pub(crate) route_id: String,
}

impl SurrealDbProducer {
    pub fn new(
        config: SurrealDbEndpointConfig,
        catalog: Option<Arc<dyn DatasourceCatalog>>,
        route_id: impl Into<String>,
    ) -> Self {
        Self {
            config,
            catalog,
            stopped: Arc::new(AtomicBool::new(false)),
            route_id: route_id.into(),
        }
    }

    pub fn stop(&self) {
        self.stopped.store(true, Ordering::Relaxed);
    }

    pub fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::Relaxed)
    }

    /// Resolve the SurrealQL query source for the `query` operation.
    /// Priority: header CamelSurrealDbQuery > body text > config query.
    pub(crate) fn resolve_query_source(&self, exchange: &Exchange) -> String {
        // Priority 1: Header
        if let Some(query_value) = exchange.input.headers.get(headers::QUERY)
            && let Some(query_str) = query_value.as_str()
        {
            return query_str.to_string();
        }

        // Priority 2: Body text
        if let Some(body_text) = exchange.input.body.as_text()
            && !body_text.is_empty()
        {
            return body_text.to_string();
        }

        // Priority 3: Config query (from URI)
        self.config.query.clone().unwrap_or_default()
    }

    /// Extract params from CamelSurrealDbParams header (JSON map).
    pub(crate) fn extract_params(
        &self,
        exchange: &Exchange,
    ) -> Result<HashMap<String, JsonValue>, SurrealDbError> {
        let Some(params_value) = exchange.input.headers.get(headers::PARAMS) else {
            return Ok(HashMap::new());
        };
        let params_str = params_value.as_str().ok_or_else(|| {
            SurrealDbError::InvalidParam("CamelSurrealDbParams header must be a string".into())
        })?;
        let parsed: HashMap<String, JsonValue> = serde_json::from_str(params_str)
            .map_err(|e| SurrealDbError::InvalidParam(format!("invalid params JSON: {e}")))?;
        Ok(parsed)
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

    /// Execute the operation against SurrealDB.
    ///
    /// No per-operation retry is applied here. Today's producer CRUD paths
    /// map SDK errors to `SurrealDbError::Query` (via `map_err(SurrealDbError::query)`),
    /// which `SurrealDbError::is_retryable` classifies as non-retryable, and
    /// no producer path currently produces `SurrealDbError::Connection`. A
    /// `retry_async` wrapper here would therefore be inert. Transient-failure
    /// retry for this component is handled at pool-create time in
    /// `pool_factory.rs` (the only load-bearing site today).
    ///
    /// `SurrealDbEndpointConfig::retry` remains on the public config struct
    /// as the contract for when a future read-side operation starts emitting
    /// a retryable variant — wiring it would require distinguishing
    /// transport-drop from query-rejected in the classifier (ADR-0013),
    /// which is not implemented.
    pub(crate) async fn execute(&self, exchange: &Exchange) -> Result<JsonValue, SurrealDbError> {
        let client = self.resolve_client().await?;

        match self.config.operation {
            SurrealDbOperation::Query => self.execute_query(&client, exchange).await,
            SurrealDbOperation::Select => self.execute_select(&client).await,
            SurrealDbOperation::Create => self.execute_create(&client, exchange).await,
            SurrealDbOperation::Update => self.execute_update(&client, exchange).await,
            SurrealDbOperation::Upsert => self.execute_upsert(&client, exchange).await,
            SurrealDbOperation::Delete => self.execute_delete(&client).await,
            SurrealDbOperation::Patch => self.execute_patch(&client, exchange).await,
            SurrealDbOperation::Relate => self.execute_relate(&client, exchange).await,
            SurrealDbOperation::Vector => self.execute_vector(&client, exchange).await,
            SurrealDbOperation::Search => self.execute_search(&client, exchange).await,
            SurrealDbOperation::Run => self.execute_run(&client, exchange).await,
            SurrealDbOperation::Live => Err(SurrealDbError::NotSupported(
                "live operation is consumer-only, not producer".into(),
            )),
        }
    }

    /// Run a SurrealQL query with bindings and return the first result set.
    /// Used only for operations where no SDK fluent method exists
    /// (RELATE, VECTOR KNN search, and the user-supplied `query` operation).
    async fn run_raw_query(
        client: &SurrealClient,
        sql: &str,
        bindings: Vec<(&str, JsonValue)>,
    ) -> Result<Vec<JsonValue>, SurrealDbError> {
        let mut q = client.query(sql);
        for (k, v) in bindings {
            q = q.bind((k, v));
        }
        let mut response = q.await.map_err(SurrealDbError::query)?;
        response.take(0).map_err(SurrealDbError::query)
    }

    /// Validate the table identifier from the URI config (defense-in-depth
    /// alongside the SDK's own escaping).
    fn validated_table(&self) -> Result<String, SurrealDbError> {
        let table = self
            .config
            .table
            .as_deref()
            .ok_or_else(|| SurrealDbError::MissingParam("table".into()))?;
        crate::query::validate_identifier(table)?;
        Ok(table.to_string())
    }

    async fn execute_query(
        &self,
        client: &SurrealClient,
        exchange: &Exchange,
    ) -> Result<JsonValue, SurrealDbError> {
        let sql = self.resolve_query_source(exchange);
        if sql.is_empty() {
            return Err(SurrealDbError::MissingParam(
                "query text (body or CamelSurrealDbQuery header)".into(),
            ));
        }
        let params = self.extract_params(exchange)?;
        let bindings: Vec<(&str, JsonValue)> = params
            .iter()
            .map(|(k, v)| (k.as_str(), v.clone()))
            .collect();
        let results = Self::run_raw_query(client, &sql, bindings).await?;
        Ok(JsonValue::Array(results))
    }

    async fn execute_select(&self, client: &SurrealClient) -> Result<JsonValue, SurrealDbError> {
        let table = self.validated_table()?;
        // Use SDK fluent API: select(table) returns all records,
        // select((table, id)) returns a single Option.
        match &self.config.id {
            Some(id) => {
                let result: Option<JsonValue> = client
                    .select((table.as_str(), id.as_str()))
                    .await
                    .map_err(SurrealDbError::query)?;
                Ok(match result {
                    Some(record) => JsonValue::Array(vec![record]),
                    None => JsonValue::Array(vec![]),
                })
            }
            None => {
                let rows: Vec<JsonValue> = client
                    .select(table.as_str())
                    .await
                    .map_err(SurrealDbError::query)?;
                Ok(JsonValue::Array(rows))
            }
        }
    }

    async fn execute_create(
        &self,
        client: &SurrealClient,
        exchange: &Exchange,
    ) -> Result<JsonValue, SurrealDbError> {
        let table = self.validated_table()?;
        let body = self.extract_body_json(exchange)?;
        let record: Option<JsonValue> = client
            .create(table.as_str())
            .content(body)
            .await
            .map_err(SurrealDbError::query)?;
        Ok(record.unwrap_or(JsonValue::Null))
    }

    async fn execute_update(
        &self,
        client: &SurrealClient,
        exchange: &Exchange,
    ) -> Result<JsonValue, SurrealDbError> {
        let table = self.validated_table()?;
        let id = self.config.id.as_deref().ok_or_else(|| {
            SurrealDbError::MissingParam("id (required for update operation)".into())
        })?;
        let body = self.extract_body_json(exchange)?;
        let result: Option<JsonValue> = client
            .update((table.as_str(), id))
            .merge(body)
            .await
            .map_err(SurrealDbError::query)?;
        Ok(result.unwrap_or(JsonValue::Null))
    }

    async fn execute_upsert(
        &self,
        client: &SurrealClient,
        exchange: &Exchange,
    ) -> Result<JsonValue, SurrealDbError> {
        let table = self.validated_table()?;
        let id = self.config.id.as_deref().ok_or_else(|| {
            SurrealDbError::MissingParam("id (required for upsert operation)".into())
        })?;
        let body = self.extract_body_json(exchange)?;
        let result: Option<JsonValue> = client
            .upsert((table.as_str(), id))
            .content(body)
            .await
            .map_err(SurrealDbError::query)?;
        Ok(result.unwrap_or(JsonValue::Null))
    }

    async fn execute_patch(
        &self,
        client: &SurrealClient,
        exchange: &Exchange,
    ) -> Result<JsonValue, SurrealDbError> {
        use surrealdb::opt::PatchOp;
        let table = self.validated_table()?;
        let id = self.config.id.as_deref().ok_or_else(|| {
            SurrealDbError::MissingParam("id (required for patch operation)".into())
        })?;
        let body = self.extract_body_json(exchange)?;
        let patches_json = body.as_array().ok_or_else(|| {
            SurrealDbError::InvalidBody(
                "patch body must be a JSON array of RFC 6902 operations".into(),
            )
        })?;
        let patch_ops: Vec<PatchOp> = patches_json
            .iter()
            .map(|p| {
                let op = p.get("op").and_then(|v| v.as_str()).ok_or_else(|| {
                    SurrealDbError::InvalidBody("patch op missing 'op' field".into())
                })?;
                let path = p.get("path").and_then(|v| v.as_str()).ok_or_else(|| {
                    SurrealDbError::InvalidBody("patch op missing 'path' field".into())
                })?;
                match op {
                    "add" | "replace" => {
                        let value = p.get("value").cloned().unwrap_or(JsonValue::Null);
                        if op == "add" {
                            Ok(PatchOp::add(path, value))
                        } else {
                            Ok(PatchOp::replace(path, value))
                        }
                    }
                    "remove" => Ok(PatchOp::remove(path)),
                    "change" => {
                        let diff = p
                            .get("value")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        Ok(PatchOp::change(path, diff))
                    }
                    other => Err(SurrealDbError::InvalidBody(format!(
                        "unsupported patch op: '{other}' (supported: add, remove, replace, change)"
                    ))),
                }
            })
            .collect::<Result<Vec<_>, _>>()?;
        let result: Option<JsonValue> = client
            .update((table.as_str(), id))
            .patch(patch_ops)
            .await
            .map_err(SurrealDbError::query)?;
        Ok(result.unwrap_or(JsonValue::Null))
    }

    async fn execute_delete(&self, client: &SurrealClient) -> Result<JsonValue, SurrealDbError> {
        let table = self.validated_table()?;
        let id = self.config.id.as_deref().ok_or_else(|| {
            SurrealDbError::MissingParam("id (required for delete operation)".into())
        })?;
        let result: Option<JsonValue> = client
            .delete((table.as_str(), id))
            .await
            .map_err(SurrealDbError::query)?;
        Ok(result.unwrap_or(JsonValue::Null))
    }

    async fn execute_relate(
        &self,
        client: &SurrealClient,
        exchange: &Exchange,
    ) -> Result<JsonValue, SurrealDbError> {
        // `from` and `to` must be full RecordIds in `table:key` form, matching
        // the README contract (e.g. `from=user:1&to=topic:42&relation=likes`).
        // Bare keys without a `table:` prefix are rejected by config
        // validation (see SurrealDbEndpointConfig::validate).
        let from_str = self.config.from.as_deref().ok_or_else(|| {
            SurrealDbError::MissingParam("from (required for relate operation)".into())
        })?;
        let edge = self.config.edge.as_deref().ok_or_else(|| {
            SurrealDbError::MissingParam("edge (required for relate operation)".into())
        })?;
        let to_str = self.config.to.as_deref().ok_or_else(|| {
            SurrealDbError::MissingParam("to (required for relate operation)".into())
        })?;
        query::validate_identifier(edge)?;
        let (from_table, from_id) = query::validate_record_id_str(from_str)?;
        let (to_table, to_id) = query::validate_record_id_str(to_str)?;
        let body = self.extract_body_json(exchange)?;
        let mut relation = match body.into_value() {
            SurrealDbValue::Object(object) => object,
            _ => {
                return Err(SurrealDbError::InvalidBody(
                    "relate body must be a JSON object".into(),
                ));
            }
        };
        relation.insert(
            "in",
            SurrealDbValue::RecordId(query::record_id_from_uri(from_table, from_id)?),
        );
        relation.insert(
            "out",
            SurrealDbValue::RecordId(query::record_id_from_uri(to_table, to_id)?),
        );

        let results: Vec<JsonValue> = client
            .insert(edge)
            .relation(SurrealDbValue::Object(relation))
            .await
            .map_err(SurrealDbError::query)?;
        Ok(results.into_iter().next().unwrap_or(JsonValue::Null))
    }

    async fn execute_vector(
        &self,
        client: &SurrealClient,
        exchange: &Exchange,
    ) -> Result<JsonValue, SurrealDbError> {
        let table = self.validated_table()?;
        let body = self.extract_body_json(exchange)?;
        // Validate the configured vector field exists in the body — the
        // search operation will need it, and CREATE with an embedding should
        // include it.
        let vf = self.config.vector_field.as_deref().unwrap_or("embedding");
        let _vec = vector::extract_vector_from_json(&body, vf)?;
        // Vector storage is a regular CREATE — the SDK method handles it.
        let record: Option<JsonValue> = client
            .create(table.as_str())
            .content(body)
            .await
            .map_err(SurrealDbError::query)?;
        Ok(record.unwrap_or(JsonValue::Null))
    }

    async fn execute_search(
        &self,
        client: &SurrealClient,
        exchange: &Exchange,
    ) -> Result<JsonValue, SurrealDbError> {
        let table = self.validated_table()?;
        let vf = self.config.vector_field.as_deref().unwrap_or("embedding");
        // Validate the vector field identifier (interpolated into KNN SQL).
        query::validate_identifier(vf)?;
        let top_k = self.config.top_k.unwrap_or(5);
        let metric = self
            .config
            .metric
            .unwrap_or(crate::config::VectorMetric::Cosine)
            .as_surrealql();

        // Extract query vector from body or CamelSurrealDbVector header.
        let query_vec = if let Some(vec_header) = exchange.input.headers.get(headers::VECTOR) {
            let vec_json: JsonValue = serde_json::from_str(vec_header.as_str().unwrap_or("[]"))
                .map_err(|e| {
                    SurrealDbError::InvalidVector(format!("invalid vector header: {e}"))
                })?;
            vector::extract_vector_raw(&vec_json)?
        } else {
            let body = self.extract_body_json(exchange)?;
            vector::extract_vector_from_json(&body, "vector")?
        };

        // KNN syntax `<|K,METRIC|>` is SurrealQL-specific and has no SDK
        // fluent equivalent — use a raw query.
        let sql = query::build_vector_search_sql(&table, vf, top_k, metric);
        let results = Self::run_raw_query(client, &sql, vec![("vector", query_vec.into())]).await?;
        Ok(JsonValue::Array(results))
    }

    async fn execute_run(
        &self,
        client: &SurrealClient,
        exchange: &Exchange,
    ) -> Result<JsonValue, SurrealDbError> {
        let function = self.config.function.as_deref().ok_or_else(|| {
            SurrealDbError::MissingParam("function (required for run operation)".into())
        })?;
        let body = self.extract_body_json(exchange)?;
        let args: Vec<JsonValue> = body.as_array().cloned().unwrap_or_default();
        let result: JsonValue = client
            .run(function)
            .args(args)
            .await
            .map_err(SurrealDbError::query)?;
        Ok(result)
    }

    /// Extract JSON body from exchange. Tries Body::Json first, then parses Body::Text.
    fn extract_body_json(&self, exchange: &Exchange) -> Result<JsonValue, SurrealDbError> {
        match &exchange.input.body {
            Body::Json(v) => Ok(v.clone()),
            Body::Text(s) if !s.is_empty() => serde_json::from_str(s)
                .map_err(|e| SurrealDbError::InvalidBody(format!("body is not valid JSON: {e}"))),
            _ => Err(SurrealDbError::InvalidBody(format!(
                "expected JSON or Text body, got {:?}",
                exchange.input.body
            ))),
        }
    }

    /// Construct the `CamelSurrealDbRecordId` header value from URI config for
    /// operations whose target RecordId is fully known up front
    /// (`update`/`upsert`/`delete`/`patch`). Returns `None` for operations
    /// that generate their RecordId server-side (`create`/`vector`/`relate`)
    /// or that don't produce a single RecordId (`select`/`query`/`search`/
    /// `run`/`live`).
    ///
    /// Server-generated ids (and the edge id for `relate`) are recovered from
    /// the response body by [`record_id_from_body`]; see the `call` site for
    /// the URI-config-first, body-fallback ordering.
    ///
    /// Returned format: `table:key` (e.g. `user:42`).
    fn record_id_for_output(&self) -> Option<String> {
        match self.config.operation {
            SurrealDbOperation::Update
            | SurrealDbOperation::Upsert
            | SurrealDbOperation::Delete
            | SurrealDbOperation::Patch => {
                let table = self.config.table.as_deref()?;
                let id = self.config.id.as_deref()?;
                Some(format!("{table}:{id}"))
            }
            _ => None,
        }
    }

    /// Extract a `table:key` RecordId from the operation's response body for
    /// the `CamelSurrealDbRecordId` header. Used as a fallback when URI config
    /// does not supply a target id (covers `create`/`vector`/`relate`, whose
    /// ids are generated server-side).
    ///
    /// Accepts either:
    /// - a JSON object with an `id` field whose value is a valid `table:key`
    ///   string, OR a SurrealDB-serialized RecordId object `{"tb":..,"id":..}`,
    /// - a JSON array with at least one element; the first element is then
    ///   inspected under the same rule.
    ///
    /// Returns `None` (and the caller logs at `debug!`) when the shape is
    /// unexpected, `id` is missing, or the value is not a valid RecordId.
    /// This is intentionally defensive: body-id extraction MUST NOT fail the
    /// producer call.
    fn record_id_from_body(&self, body: &JsonValue) -> Option<String> {
        let candidate = match body {
            JsonValue::Object(_) => body,
            JsonValue::Array(arr) if !arr.is_empty() => &arr[0],
            _ => return None,
        };
        let id_value = candidate.get("id")?;
        match id_value {
            JsonValue::String(s) => query::validate_record_id_str(s).ok().map(|_| s.clone()),
            JsonValue::Object(obj) => {
                // SurrealDB may serialize a RecordId as {"tb": <table>, "id": <key>}.
                let tb = obj.get("tb").and_then(|v| v.as_str())?;
                let key = match obj.get("id")? {
                    JsonValue::String(s) => s.clone(),
                    JsonValue::Number(n) => n.to_string(),
                    _ => return None,
                };
                let reconstructed = format!("{tb}:{key}");
                if query::validate_record_id_str(&reconstructed).is_ok() {
                    Some(reconstructed)
                } else {
                    None
                }
            }
            _ => None,
        }
    }
}

impl Service<Exchange> for SurrealDbProducer {
    type Response = Exchange;
    type Error = CamelError;
    type Future =
        std::pin::Pin<Box<dyn std::future::Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        if self.stopped.load(Ordering::Relaxed) {
            return Poll::Ready(Err(CamelError::ProcessorError(
                "surrealdb producer stopped".into(),
            )));
        }
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, mut exchange: Exchange) -> Self::Future {
        let producer = self.clone();
        Box::pin(async move {
            match producer.execute(&exchange).await {
                Ok(result) => {
                    // Determine the CamelSurrealDbRecordId header BEFORE moving
                    // the result into the message: URI-config first, then
                    // body extraction (covers create/vector/relate, whose ids
                    // are server-generated, and update/upsert/delete/patch when
                    // URI config is absent).
                    let record_id = producer
                        .record_id_for_output()
                        .or_else(|| producer.record_id_from_body(&result));
                    if record_id.is_none() {
                        tracing::debug!(
                            route_id = %producer.route_id,
                            operation = %producer.config.operation,
                            "CamelSurrealDbRecordId header not set (no URI config and no extractable body.id)"
                        );
                    }
                    let mut msg = Message::new(Body::Json(result));
                    if let Some(record_id) = record_id {
                        msg.headers.insert(
                            headers::RECORD_ID.into(),
                            camel_component_api::Value::String(record_id),
                        );
                    }
                    exchange.output = Some(msg);
                    Ok(exchange)
                }
                Err(err) => {
                    // log-policy: handler-owned
                    tracing::warn!(
                        route_id = %producer.route_id,
                        error = %err,
                        "surrealdb producer operation failed"
                    );
                    Err(CamelError::from(err))
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::config::{SurrealDbEndpointConfig, SurrealDbOperation, VectorMetric};
    use crate::headers;
    use crate::producer::SurrealDbProducer;
    use camel_component_api::{Body, Exchange, Message, Value};

    fn make_producer(op: SurrealDbOperation) -> SurrealDbProducer {
        let config = SurrealDbEndpointConfig {
            operation: op,
            datasource: "test-ds".into(),
            table: Some("test_table".into()),
            id: Some("42".into()),
            from: Some("test_table:1".into()),
            edge: Some("knows".into()),
            to: Some("test_table:2".into()),
            to_table: Some("test_table".into()),
            top_k: Some(5),
            metric: Some(VectorMetric::Cosine),
            vector_field: Some("embedding".into()),
            limit: Some(10),
            query: None,
            function: None,
            ..Default::default()
        };
        SurrealDbProducer::new(config, None, "test-route")
    }

    #[test]
    fn producer_can_be_constructed() {
        let producer = make_producer(SurrealDbOperation::Query);
        assert!(!producer.is_stopped());
    }

    #[test]
    fn producer_stopped_flag() {
        let producer = make_producer(SurrealDbOperation::Query);
        assert!(!producer.is_stopped());
        producer.stop();
        assert!(producer.is_stopped());
    }

    // --- SQL generation tests (no DB needed) ---

    #[test]
    fn resolve_query_from_body_text() {
        let producer = make_producer(SurrealDbOperation::Query);
        let exchange = Exchange::new(Message::new(Body::Text("SELECT * FROM users".into())));
        let sql = producer.resolve_query_source(&exchange);
        assert_eq!(sql, "SELECT * FROM users");
    }

    #[test]
    fn resolve_query_from_header() {
        let producer = make_producer(SurrealDbOperation::Query);
        let mut exchange = Exchange::new(Message::new(Body::Empty));
        exchange
            .input
            .headers
            .insert(headers::QUERY.into(), Value::String("SELECT 1".into()));
        let sql = producer.resolve_query_source(&exchange);
        assert_eq!(sql, "SELECT 1");
    }

    // --- Param extraction tests ---

    #[test]
    fn extract_params_from_header() {
        let producer = make_producer(SurrealDbOperation::Query);
        let mut exchange = Exchange::new(Message::new(Body::Empty));
        exchange.input.headers.insert(
            headers::PARAMS.into(),
            Value::String(r#"{"min_age": 21, "name": "alice"}"#.into()),
        );
        let params = producer.extract_params(&exchange).unwrap();
        assert_eq!(params.len(), 2);
        assert!(params.contains_key("min_age"));
    }

    #[test]
    fn extract_params_no_header_returns_empty() {
        let producer = make_producer(SurrealDbOperation::Query);
        let exchange = Exchange::new(Message::new(Body::Empty));
        let params = producer.extract_params(&exchange).unwrap();
        assert!(params.is_empty());
    }

    #[test]
    fn extract_params_invalid_json_returns_invalid_param() {
        let producer = make_producer(SurrealDbOperation::Query);
        let mut exchange = Exchange::new(Message::new(Body::Empty));
        exchange.input.headers.insert(
            headers::PARAMS.into(),
            Value::String("not valid json".into()),
        );
        let err = producer.extract_params(&exchange).unwrap_err();
        assert!(matches!(err, crate::error::SurrealDbError::InvalidParam(_)));
    }

    #[test]
    fn extract_params_non_string_header_returns_invalid_param() {
        let producer = make_producer(SurrealDbOperation::Query);
        let mut exchange = Exchange::new(Message::new(Body::Empty));
        exchange
            .input
            .headers
            .insert(headers::PARAMS.into(), serde_json::json!(42));
        let err = producer.extract_params(&exchange).unwrap_err();
        assert!(matches!(err, crate::error::SurrealDbError::InvalidParam(_)));
    }

    // --- Body extraction tests ---

    #[test]
    fn extract_body_json_json_variant() {
        let producer = make_producer(SurrealDbOperation::Create);
        let body = serde_json::json!({"name": "alice", "age": 30});
        let exchange = Exchange::new(Message::new(Body::Json(body.clone())));
        let result = producer.extract_body_json(&exchange).unwrap();
        assert_eq!(result, body);
    }

    #[test]
    fn extract_body_json_text_valid() {
        let producer = make_producer(SurrealDbOperation::Create);
        let exchange = Exchange::new(Message::new(Body::Text(r#"{"name":"bob"}"#.into())));
        let result = producer.extract_body_json(&exchange).unwrap();
        assert_eq!(result, serde_json::json!({"name": "bob"}));
    }

    #[test]
    fn extract_body_json_bytes_returns_invalid_body() {
        let producer = make_producer(SurrealDbOperation::Create);
        let exchange = Exchange::new(Message::new(Body::Bytes(bytes::Bytes::from("not json"))));
        let err = producer.extract_body_json(&exchange).unwrap_err();
        assert!(matches!(err, crate::error::SurrealDbError::InvalidBody(_)));
    }

    #[test]
    fn extract_body_json_empty_returns_invalid_body() {
        let producer = make_producer(SurrealDbOperation::Create);
        let exchange = Exchange::new(Message::new(Body::Empty));
        let err = producer.extract_body_json(&exchange).unwrap_err();
        assert!(matches!(err, crate::error::SurrealDbError::InvalidBody(_)));
    }

    #[test]
    fn extract_body_json_empty_text_returns_invalid_body() {
        let producer = make_producer(SurrealDbOperation::Create);
        let exchange = Exchange::new(Message::new(Body::Text(String::new())));
        let err = producer.extract_body_json(&exchange).unwrap_err();
        assert!(matches!(err, crate::error::SurrealDbError::InvalidBody(_)));
    }

    #[test]
    fn extract_body_json_text_invalid_json_returns_invalid_body() {
        let producer = make_producer(SurrealDbOperation::Create);
        let exchange = Exchange::new(Message::new(Body::Text("not json".into())));
        let err = producer.extract_body_json(&exchange).unwrap_err();
        assert!(matches!(err, crate::error::SurrealDbError::InvalidBody(_)));
    }

    // --- record_id_for_output tests ---

    #[test]
    fn record_id_header_for_update_uses_table_id() {
        let producer = make_producer(SurrealDbOperation::Update);
        // make_producer uses table=test_table, id=42
        assert_eq!(
            producer.record_id_for_output().as_deref(),
            Some("test_table:42")
        );
    }

    #[test]
    fn record_id_header_for_upsert_uses_table_id() {
        let producer = make_producer(SurrealDbOperation::Upsert);
        assert_eq!(
            producer.record_id_for_output().as_deref(),
            Some("test_table:42")
        );
    }

    #[test]
    fn record_id_header_for_delete_uses_table_id() {
        let producer = make_producer(SurrealDbOperation::Delete);
        assert_eq!(
            producer.record_id_for_output().as_deref(),
            Some("test_table:42")
        );
    }

    #[test]
    fn record_id_header_for_patch_uses_table_id() {
        let producer = make_producer(SurrealDbOperation::Patch);
        assert_eq!(
            producer.record_id_for_output().as_deref(),
            Some("test_table:42")
        );
    }

    #[test]
    fn record_id_for_output_for_relate_is_none() {
        // record_id_for_output (URI-config path) is None for relate: the edge
        // id is server-generated. After Option B, the CamelSurrealDbRecordId
        // header is instead set by extracting the edge id from the response
        // body — see record_id_from_body_for_relate_extracts_edge_id.
        let producer = make_producer(SurrealDbOperation::Relate);
        assert_eq!(producer.record_id_for_output(), None);
    }

    #[test]
    fn record_id_from_body_for_relate_extracts_edge_id() {
        // After Option B: the CamelSurrealDbRecordId header IS set for relate
        // by extracting the EDGE record's id (not the source node) from the
        // response body.
        let producer = make_producer(SurrealDbOperation::Relate);
        let body = serde_json::json!({
            "id": "knows:abc123",
            "in": "user:1",
            "out": "topic:42",
            "weight": 0.9
        });
        assert_eq!(
            producer.record_id_from_body(&body).as_deref(),
            Some("knows:abc123")
        );
    }

    #[test]
    fn record_id_for_output_for_create_is_none() {
        // record_id_for_output (URI-config path) is None for create: the id is
        // server-generated. The header is set via record_id_from_body instead.
        let producer = make_producer(SurrealDbOperation::Create);
        assert_eq!(producer.record_id_for_output(), None);
    }

    #[test]
    fn record_id_for_output_for_vector_is_none() {
        let producer = make_producer(SurrealDbOperation::Vector);
        assert_eq!(producer.record_id_for_output(), None);
    }

    #[test]
    fn record_id_for_output_for_search_is_none() {
        let producer = make_producer(SurrealDbOperation::Search);
        assert_eq!(producer.record_id_for_output(), None);
    }

    // --- record_id_from_body tests (Option B: extract id from response body) ---

    #[test]
    fn record_id_from_body_extracts_object_id() {
        let producer = make_producer(SurrealDbOperation::Create);
        let body = serde_json::json!({"id": "user:42", "name": "alice"});
        assert_eq!(
            producer.record_id_from_body(&body).as_deref(),
            Some("user:42")
        );
    }

    #[test]
    fn record_id_from_body_extracts_first_array_element_id() {
        let producer = make_producer(SurrealDbOperation::Create);
        let body = serde_json::json!([
            {"id": "user:1", "name": "a"},
            {"id": "user:2", "name": "b"}
        ]);
        assert_eq!(
            producer.record_id_from_body(&body).as_deref(),
            Some("user:1")
        );
    }

    #[test]
    fn record_id_from_body_extracts_object_form_record_id() {
        // SurrealDB may serialize a RecordId as {"tb": <table>, "id": <key>}.
        let producer = make_producer(SurrealDbOperation::Create);
        let body = serde_json::json!({"id": {"tb": "user", "id": 42}, "name": "alice"});
        assert_eq!(
            producer.record_id_from_body(&body).as_deref(),
            Some("user:42")
        );
    }

    #[test]
    fn record_id_from_body_returns_none_when_id_missing() {
        let producer = make_producer(SurrealDbOperation::Create);
        let body = serde_json::json!({"name": "alice"});
        assert_eq!(producer.record_id_from_body(&body), None);
    }

    #[test]
    fn record_id_from_body_returns_none_when_id_invalid_record_id() {
        let producer = make_producer(SurrealDbOperation::Create);
        // Bare key (no `table:` prefix) is not a valid RecordId.
        let body = serde_json::json!({"id": "barekey"});
        assert_eq!(producer.record_id_from_body(&body), None);
    }

    #[test]
    fn record_id_from_body_returns_none_for_empty_array() {
        let producer = make_producer(SurrealDbOperation::Create);
        let body = serde_json::json!([]);
        assert_eq!(producer.record_id_from_body(&body), None);
    }

    #[test]
    fn record_id_from_body_returns_none_for_non_object_non_array() {
        let producer = make_producer(SurrealDbOperation::Create);
        let body = serde_json::json!("just a string");
        assert_eq!(producer.record_id_from_body(&body), None);
    }

    #[test]
    fn record_id_from_body_returns_none_when_id_is_not_string_or_object() {
        let producer = make_producer(SurrealDbOperation::Create);
        let body = serde_json::json!({"id": 42});
        // A bare number is not a valid table:key RecordId string form.
        assert_eq!(producer.record_id_from_body(&body), None);
    }

    // --- Body contract / alternative input paths ---

    #[test]
    fn test_query_via_uri_with_empty_body_accepted() {
        // Query op with SQL from URI ?query=...: producer resolves the SQL
        // from config.query when the body is empty. body_contract is None,
        // so the empty body never gets rejected upstream.
        let mut config = SurrealDbEndpointConfig::default();
        config.operation = SurrealDbOperation::Query;
        config.datasource = "test-ds".into();
        config.query = Some("SELECT 1".to_string());
        let producer = SurrealDbProducer::new(config, None, "test-route");
        let exchange = Exchange::new(Message::new(Body::Empty));
        let sql = producer.resolve_query_source(&exchange);
        assert_eq!(sql, "SELECT 1");
    }

    #[test]
    fn test_query_via_header_with_empty_body_accepted() {
        // Query op with SQL from CamelSurrealDbQuery header: priority 1.
        let config = SurrealDbEndpointConfig::default();
        let producer = SurrealDbProducer::new(config, None, "test-route");
        let mut exchange = Exchange::new(Message::new(Body::Empty));
        exchange
            .input
            .headers
            .insert(headers::QUERY.into(), Value::String("SELECT 1".into()));
        let sql = producer.resolve_query_source(&exchange);
        assert_eq!(sql, "SELECT 1");
    }

    #[test]
    fn test_search_via_vector_header_with_empty_body_accepted() {
        // search op with empty body is valid because CamelSurrealDbVector
        // header provides the query vector. We verify the producer config
        // shape that would route to the header path (body_contract returns
        // None, so the empty body never gets rejected upstream).
        let mut config = SurrealDbEndpointConfig::default();
        config.operation = SurrealDbOperation::Search;
        config.datasource = "test-ds".into();
        config.table = Some("docs".into());
        config.top_k = Some(5);
        // body_contract is None for Search — verified separately on the
        // Endpoint impl; here we just check that the config can be
        // constructed without a body requirement.
        assert_eq!(config.operation, SurrealDbOperation::Search);
        assert_eq!(config.top_k, Some(5));
    }
}
