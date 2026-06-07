use camel_component_api::{Body, CamelError, Exchange, RuntimeObservability, retry_async};
use opensearch::auth::Credentials;
use opensearch::http::response::Response;
use opensearch::http::transport::{SingleNodeConnectionPool, TransportBuilder};
use opensearch::indices::IndicesDeleteParts;
use opensearch::{
    BulkParts, DeleteParts, ExistsParts, GetParts, IndexParts, MgetParts, OpenSearch, SearchParts,
    UpdateParts,
};
use std::future::Future;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::sync::{AcquireError, Mutex, OwnedSemaphorePermit, Semaphore};
use tower::Service;
use tracing::{debug, error, warn};

use crate::config::{OpenSearchEndpointConfig, OpenSearchOperation};

mod retry;
use retry::{ProducerError, is_retryable_producer_error, is_transient};

impl OpenSearchProducer {
    /// Like [`run_with_timeout`] but operating on [`ProducerError`].
    async fn run_with_timeout_pe<F, T>(
        config: &OpenSearchEndpointConfig,
        fut: F,
    ) -> Result<T, ProducerError>
    where
        F: Future<Output = Result<T, ProducerError>>,
    {
        let timeout = Duration::from_millis(config.timeout_ms.unwrap_or(30_000));
        tokio::time::timeout(timeout, fut)
            .await
            .map_err(|_| ProducerError::Transient("opensearch request timed out".to_string()))?
    }
}

/// OpenSearch producer that implements Tower `Service<Exchange>` for integration
/// with rust-camel pipelines.
///
/// Manages a shared `OpenSearch` client that is created lazily on first use
/// and reused across multiple calls.
pub struct OpenSearchProducer {
    config: OpenSearchEndpointConfig,
    /// Shared OpenSearch client — created lazily on first use
    client: Arc<Mutex<Option<OpenSearch>>>,
    /// Semaphore bounding concurrent in-flight requests.
    semaphore: Arc<Semaphore>,
    /// Permit acquired in poll_ready and held until call completes.
    pending_permit: Option<OwnedSemaphorePermit>,
    /// Pinned permit acquisition future.
    acquire_fut: Option<AcquirePermitFut>,
    runtime: Arc<dyn RuntimeObservability>,
}

const DEFAULT_CONCURRENCY_LIMIT: usize = 128;
type AcquirePermitFut =
    Pin<Box<dyn Future<Output = Result<OwnedSemaphorePermit, AcquireError>> + Send>>;

impl Clone for OpenSearchProducer {
    fn clone(&self) -> Self {
        Self {
            config: self.config.clone(),
            client: Arc::clone(&self.client),
            semaphore: Arc::clone(&self.semaphore),
            pending_permit: None,
            acquire_fut: None,
            runtime: Arc::clone(&self.runtime),
        }
    }
}

impl OpenSearchProducer {
    /// Creates a new OpenSearchProducer with the given configuration.
    ///
    /// The client is not established until the first call to `call()`.
    pub fn new(config: OpenSearchEndpointConfig, runtime: Arc<dyn RuntimeObservability>) -> Self {
        Self {
            config,
            client: Arc::new(Mutex::new(None)),
            semaphore: Arc::new(Semaphore::new(DEFAULT_CONCURRENCY_LIMIT)),
            pending_permit: None,
            acquire_fut: None,
            runtime,
        }
    }

    /// Builds an OpenSearch client from the endpoint configuration.
    fn build_client(config: &OpenSearchEndpointConfig) -> Result<OpenSearch, CamelError> {
        let url = config.base_url();
        let parsed_url = url::Url::parse(&url).map_err(|e| {
            let err = CamelError::EndpointCreationFailed(format!("Invalid OpenSearch URL: {}", e));
            error!(endpoint = %url, error = %e, "opensearch client init failed");
            err
        })?;
        let pool = SingleNodeConnectionPool::new(parsed_url);
        let mut builder = TransportBuilder::new(pool);
        if let (Some(username), Some(password)) = (&config.username, &config.password) {
            builder = builder.auth(Credentials::Basic(username.clone(), password.clone()));
        }
        // TODO(OS-018): AWS SigV4 signing deferred; requires AWS SDK/signing dependency.
        let transport = builder.build().map_err(|e| {
            error!(endpoint = %url, error = %e, "opensearch client init failed");
            CamelError::EndpointCreationFailed(format!("Failed to build transport: {}", e))
        })?;
        debug!(endpoint = %url, "opensearch client initialized");
        Ok(OpenSearch::new(transport))
    }

    /// Resolves the operation to execute.
    ///
    /// Priority:
    /// 1. Header `CamelOpenSearch.Operation` if present (must be a string)
    /// 2. Configuration default operation
    fn resolve_operation(
        exchange: &Exchange,
        config: &OpenSearchEndpointConfig,
    ) -> OpenSearchOperation {
        exchange
            .input
            .header("CamelOpenSearch.Operation")
            .and_then(|v| v.as_str())
            .and_then(|s| {
                let op = OpenSearchOperation::from_str(s).ok()?;
                match op {
                    OpenSearchOperation::UNKNOWN(_) => None,
                    valid => Some(valid),
                }
            })
            .unwrap_or_else(|| config.operation.clone())
    }

    /// Extracts the body from the exchange as a `serde_json::Value`.
    fn extract_body(exchange: &Exchange) -> Result<serde_json::Value, CamelError> {
        match &exchange.input.body {
            Body::Json(v) => Ok(v.clone()),
            Body::Text(s) => serde_json::from_str(s).map_err(|e| {
                CamelError::TypeConversionFailed(format!("Failed to parse body as JSON: {}", e))
            }),
            Body::Bytes(b) => serde_json::from_slice(b).map_err(|e| {
                CamelError::TypeConversionFailed(format!("Failed to parse body as JSON: {}", e))
            }),
            Body::Empty => Ok(serde_json::Value::Object(serde_json::Map::new())),
            Body::Xml(s) => serde_json::from_str(s).map_err(|e| {
                CamelError::TypeConversionFailed(format!("Failed to parse XML body as JSON: {}", e))
            }),
            Body::Stream(_) => Err(CamelError::TypeConversionFailed(
                "Stream body not supported for OpenSearch operations. Materialize first."
                    .to_string(),
            )),
        }
    }

    /// Like [`extract_body`] but returns [`ProducerError`] for the retry loop.
    fn extract_body_pe(exchange: &Exchange) -> Result<serde_json::Value, ProducerError> {
        Self::extract_body(exchange).map_err(|e| ProducerError::Permanent(format!("{}", e)))
    }

    /// Reads and parses an OpenSearch HTTP response.
    async fn read_response(response: Response) -> Result<serde_json::Value, ProducerError> {
        let status = response.status_code().as_u16();
        let body = response
            .json::<serde_json::Value>()
            .await
            .map_err(|e| ProducerError::Permanent(format!("Failed to parse response: {}", e)))?;
        if status >= 400 {
            let reason = body
                .get("error")
                .and_then(|e| e.get("reason"))
                .and_then(|r| r.as_str())
                .unwrap_or("unknown error");
            let msg = format!("OpenSearch error ({}): {}", status, reason);
            if is_transient(status) {
                return Err(ProducerError::Transient(msg));
            } else {
                return Err(ProducerError::Permanent(msg));
            }
        }
        Ok(body)
    }

    /// Build the response exchange with the result as the body.
    fn build_response(mut exchange: Exchange, result: serde_json::Value) -> Exchange {
        exchange.input.body = Body::from(result);
        exchange
    }

    fn build_bulk_lines(
        config: &OpenSearchEndpointConfig,
        body: serde_json::Value,
    ) -> Result<Vec<String>, CamelError> {
        let lines: Vec<String> = match body {
            serde_json::Value::Array(arr) => arr
                .iter()
                .map(serde_json::to_string)
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| {
                    CamelError::ProcessorError(format!("Failed to serialize bulk body: {}", e))
                })?,
            other => vec![serde_json::to_string(&other).map_err(|e| {
                CamelError::ProcessorError(format!("Failed to serialize bulk body: {}", e))
            })?],
        };

        if let Some(max_bulk_bytes) = config.max_bulk_bytes {
            let total_bytes: usize = lines.iter().map(|line| line.len() + 1).sum();
            if total_bytes > max_bulk_bytes {
                return Err(CamelError::ProcessorError(format!(
                    "bulk payload size {} exceeds max_bulk_bytes {}",
                    total_bytes, max_bulk_bytes
                )));
            }
        }

        Ok(lines)
    }

    /// Like [`build_bulk_lines`] but returns [`ProducerError`] for the retry loop.
    fn build_bulk_lines_pe(
        config: &OpenSearchEndpointConfig,
        body: serde_json::Value,
    ) -> Result<Vec<String>, ProducerError> {
        Self::build_bulk_lines(config, body).map_err(|e| ProducerError::Permanent(format!("{}", e)))
    }

    fn apply_search_pagination(
        config: &OpenSearchEndpointConfig,
        mut body: serde_json::Value,
    ) -> serde_json::Value {
        if let serde_json::Value::Object(ref mut map) = body {
            if let Some(size) = config.size {
                map.entry("size").or_insert_with(|| serde_json::json!(size));
            }
            if let Some(from) = config.from {
                map.entry("from").or_insert_with(|| serde_json::json!(from));
            }
        }
        body
    }

    // --- Operation implementations ---

    async fn execute_index(
        client: &OpenSearch,
        config: &OpenSearchEndpointConfig,
        exchange: &Exchange,
    ) -> Result<serde_json::Value, ProducerError> {
        debug!(index = %config.index_name, "indexing document");
        let body = Self::extract_body_pe(exchange)?;
        let doc_id = exchange
            .input
            .header("CamelOpenSearch.Id")
            .and_then(|v| v.as_str());

        let response = match doc_id {
            Some(id) => {
                client
                    .index(IndexParts::IndexId(&config.index_name, id))
                    .body(body)
                    .send()
                    .await
            }
            None => {
                client
                    .index(IndexParts::Index(&config.index_name))
                    .body(body)
                    .send()
                    .await
            }
        }
        .map_err(|e| {
            warn!(index = %config.index_name, error = %e, "index operation failed");
            ProducerError::Transient(format!(
                "[opensearch] index '{}' operation failed: {}",
                config.index_name, e
            ))
        })?;

        Self::read_response(response).await
    }

    async fn execute_search(
        client: &OpenSearch,
        config: &OpenSearchEndpointConfig,
        exchange: &Exchange,
    ) -> Result<serde_json::Value, ProducerError> {
        debug!(index = %config.index_name, "searching");
        let body = Self::apply_search_pagination(config, Self::extract_body_pe(exchange)?);

        let response = client
            .search(SearchParts::Index(&[&config.index_name]))
            .body(body)
            .send()
            .await
            .map_err(|e| {
                warn!(index = %config.index_name, error = %e, "search failed");
                ProducerError::Transient(format!(
                    "[opensearch] search '{}' operation failed: {}",
                    config.index_name, e
                ))
            })?;

        Self::read_response(response).await
    }

    async fn execute_get(
        client: &OpenSearch,
        config: &OpenSearchEndpointConfig,
        exchange: &Exchange,
    ) -> Result<serde_json::Value, ProducerError> {
        debug!(index = %config.index_name, "getting document");
        let doc_id = exchange
            .input
            .header("CamelOpenSearch.Id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ProducerError::Permanent(
                    "Missing CamelOpenSearch.Id header for GET operation".to_string(),
                )
            })?;

        let response = client
            .get(GetParts::IndexId(&config.index_name, doc_id))
            .send()
            .await
            .map_err(|e| {
                warn!(index = %config.index_name, error = %e, "get failed");
                ProducerError::Transient(format!(
                    "[opensearch] get '{}' operation failed for id '{}': {}",
                    config.index_name, doc_id, e
                ))
            })?;

        Self::read_response(response).await
    }

    async fn execute_delete(
        client: &OpenSearch,
        config: &OpenSearchEndpointConfig,
        exchange: &Exchange,
    ) -> Result<serde_json::Value, ProducerError> {
        debug!(index = %config.index_name, "deleting document");
        let doc_id = exchange
            .input
            .header("CamelOpenSearch.Id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ProducerError::Permanent(
                    "Missing CamelOpenSearch.Id header for DELETE operation".to_string(),
                )
            })?;

        let response = client
            .delete(DeleteParts::IndexId(&config.index_name, doc_id))
            .send()
            .await
            .map_err(|e| {
                warn!(index = %config.index_name, error = %e, "delete failed");
                ProducerError::Transient(format!(
                    "[opensearch] delete '{}' operation failed for id '{}': {}",
                    config.index_name, doc_id, e
                ))
            })?;

        Self::read_response(response).await
    }

    async fn execute_update(
        client: &OpenSearch,
        config: &OpenSearchEndpointConfig,
        exchange: &Exchange,
    ) -> Result<serde_json::Value, ProducerError> {
        debug!(index = %config.index_name, "updating document");
        let doc_id = exchange
            .input
            .header("CamelOpenSearch.Id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ProducerError::Permanent(
                    "Missing CamelOpenSearch.Id header for UPDATE operation".to_string(),
                )
            })?;

        let body = Self::extract_body_pe(exchange)?;

        let response = client
            .update(UpdateParts::IndexId(&config.index_name, doc_id))
            .body(body)
            .send()
            .await
            .map_err(|e| {
                warn!(index = %config.index_name, error = %e, "update failed");
                ProducerError::Transient(format!(
                    "[opensearch] update '{}' operation failed for id '{}': {}",
                    config.index_name, doc_id, e
                ))
            })?;

        Self::read_response(response).await
    }

    async fn execute_bulk(
        client: &OpenSearch,
        config: &OpenSearchEndpointConfig,
        exchange: &Exchange,
    ) -> Result<serde_json::Value, ProducerError> {
        debug!(index = %config.index_name, "bulk operation");
        let body = Self::extract_body_pe(exchange)?;
        let lines = Self::build_bulk_lines_pe(config, body)?;

        let response = client
            .bulk(BulkParts::Index(&config.index_name))
            .body(lines)
            .send()
            .await
            .map_err(|e| {
                warn!(index = %config.index_name, error = %e, "bulk operation failed");
                ProducerError::Transient(format!(
                    "[opensearch] bulk '{}' operation failed: {}",
                    config.index_name, e
                ))
            })?;

        Self::read_response(response).await
    }

    async fn execute_multiget(
        client: &OpenSearch,
        config: &OpenSearchEndpointConfig,
        exchange: &Exchange,
    ) -> Result<serde_json::Value, ProducerError> {
        debug!(index = %config.index_name, "multiget operation");
        let body = Self::extract_body_pe(exchange)?;

        let response = client
            .mget(MgetParts::Index(&config.index_name))
            .body(body)
            .send()
            .await
            .map_err(|e| {
                warn!(index = %config.index_name, error = %e, "multiget failed");
                ProducerError::Transient(format!(
                    "[opensearch] multiget '{}' operation failed: {}",
                    config.index_name, e
                ))
            })?;

        Self::read_response(response).await
    }
    async fn execute_exists(
        client: &OpenSearch,
        config: &OpenSearchEndpointConfig,
        exchange: &Exchange,
    ) -> Result<serde_json::Value, ProducerError> {
        debug!(index = %config.index_name, "checking document existence");
        let doc_id = exchange
            .input
            .header("CamelOpenSearch.Id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ProducerError::Permanent(
                    "Missing CamelOpenSearch.Id header for EXISTS operation".to_string(),
                )
            })?;

        let response = client
            .exists(ExistsParts::IndexId(&config.index_name, doc_id))
            .send()
            .await
            .map_err(|e| {
                warn!(index = %config.index_name, error = %e, "exists failed");
                ProducerError::Transient(format!(
                    "[opensearch] exists '{}' operation failed for id '{}': {}",
                    config.index_name, doc_id, e
                ))
            })?;

        Ok(serde_json::json!({
            "exists": response.status_code().as_u16() == 200,
            "status": response.status_code().as_u16()
        }))
    }

    async fn execute_delete_index(
        client: &OpenSearch,
        config: &OpenSearchEndpointConfig,
    ) -> Result<serde_json::Value, ProducerError> {
        debug!(index = %config.index_name, "deleting index");
        let response = client
            .indices()
            .delete(IndicesDeleteParts::Index(&[&config.index_name]))
            .send()
            .await
            .map_err(|e| {
                warn!(index = %config.index_name, error = %e, "delete index failed");
                ProducerError::Transient(format!(
                    "[opensearch] delete_index '{}' operation failed: {}",
                    config.index_name, e
                ))
            })?;
        Self::read_response(response).await
    }

    async fn execute_ping(client: &OpenSearch) -> Result<serde_json::Value, ProducerError> {
        debug!("pinging opensearch");
        let response = client.ping().send().await.map_err(|e| {
            warn!(error = %e, "ping failed");
            ProducerError::Transient(format!("[opensearch] ping operation failed: {}", e))
        })?;
        Ok(serde_json::json!({"ok": response.status_code().is_success()}))
    }
}

impl Service<Exchange> for OpenSearchProducer {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        if self.pending_permit.is_some() {
            return Poll::Ready(Ok(()));
        }

        let fut = self
            .acquire_fut
            .get_or_insert_with(|| Box::pin(Arc::clone(&self.semaphore).acquire_owned()));

        match fut.as_mut().poll(cx) {
            Poll::Ready(Ok(permit)) => {
                self.acquire_fut = None;
                self.pending_permit = Some(permit);
                Poll::Ready(Ok(()))
            }
            Poll::Ready(Err(_)) => Poll::Ready(Err(CamelError::Stopped)),
            Poll::Pending => Poll::Pending,
        }
    }

    /// Delivery semantics: at-least-once with internal retry for transient errors.
    ///
    /// Transient errors (5xx, network failures, timeouts) are retried up to
    /// `config.retry.max_attempts` times with exponential backoff. Permanent
    /// errors (4xx, missing headers, parse failures) are surfaced immediately.
    /// All operations are retried on transient errors — the caller is responsible
    /// for idempotency (e.g., providing a doc ID for INDEX operations).
    fn call(&mut self, req: Exchange) -> Self::Future {
        let client = self.client.clone();
        let config = self.config.clone();
        let _permit = self
            .pending_permit
            .take()
            .expect("call() after poll_ready()"); // allow-unwrap

        Box::pin(async move {
            let os_client = {
                let mut guard = client.lock().await;
                if guard.is_none() {
                    *guard = Some(Self::build_client(&config)?);
                }
                guard.as_ref().unwrap().clone() // allow-unwrap
            };

            // Operation resolution: header > URI param (already in config.operation)
            let operation = Self::resolve_operation(&req, &config);
            debug!(operation = %operation, retry_enabled = config.retry.enabled, "opensearch call dispatched");

            let result = retry_async::<_, _, _, _, ProducerError>(
                &config.retry,
                Some("opensearch-producer"),
                || {
                    let op = operation.clone();
                    async {
                        Self::run_with_timeout_pe(&config, async {
                            match op {
                                OpenSearchOperation::INDEX => {
                                    Self::execute_index(&os_client, &config, &req).await
                                }
                                OpenSearchOperation::SEARCH => {
                                    Self::execute_search(&os_client, &config, &req).await
                                }
                                OpenSearchOperation::GET => {
                                    Self::execute_get(&os_client, &config, &req).await
                                }
                                OpenSearchOperation::DELETE => {
                                    Self::execute_delete(&os_client, &config, &req).await
                                }
                                OpenSearchOperation::EXISTS => {
                                    Self::execute_exists(&os_client, &config, &req).await
                                }
                                OpenSearchOperation::UPDATE => {
                                    Self::execute_update(&os_client, &config, &req).await
                                }
                                OpenSearchOperation::BULK => {
                                    Self::execute_bulk(&os_client, &config, &req).await
                                }
                                OpenSearchOperation::MULTIGET => {
                                    Self::execute_multiget(&os_client, &config, &req).await
                                }
                                OpenSearchOperation::DELETEINDEX => {
                                    Self::execute_delete_index(&os_client, &config).await
                                }
                                OpenSearchOperation::MULTISEARCH => Err(ProducerError::Permanent(
                                    "MULTI_SEARCH operation not implemented yet".to_string(),
                                )),
                                OpenSearchOperation::PING => Self::execute_ping(&os_client).await,
                                OpenSearchOperation::UNKNOWN(op) => Err(ProducerError::Permanent(
                                    format!("Unsupported operation: {}", op),
                                )),
                            }
                        })
                        .await
                    }
                },
                is_retryable_producer_error,
            )
            .await;
            let result = result.map_err(CamelError::from)?;

            Ok(Self::build_response(req, result))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camel_component_api::test_support::PanicRuntimeObservability;
    fn test_rt() -> std::sync::Arc<dyn camel_component_api::RuntimeObservability> {
        std::sync::Arc::new(PanicRuntimeObservability)
    }
    use camel_component_api::Message;
    use futures_util::task::noop_waker_ref;
    use std::task::Context;

    #[test]
    fn test_producer_new() {
        let config =
            OpenSearchEndpointConfig::from_uri("opensearch://localhost:9200/myindex").unwrap();
        let producer = OpenSearchProducer::new(config, test_rt());
        assert!(Arc::strong_count(&producer.client) == 1);
    }

    #[test]
    fn test_producer_clone_shares_client() {
        let config =
            OpenSearchEndpointConfig::from_uri("opensearch://localhost:9200/myindex").unwrap();
        let producer = OpenSearchProducer::new(config, test_rt());
        let producer2 = producer.clone();

        // Both producers share the same client Arc
        assert!(Arc::ptr_eq(&producer.client, &producer2.client));
    }

    #[test]
    fn test_producer_clone_increments_arc_count() {
        let config =
            OpenSearchEndpointConfig::from_uri("opensearch://localhost:9200/myindex").unwrap();
        let producer = OpenSearchProducer::new(config, test_rt());
        assert_eq!(Arc::strong_count(&producer.client), 1);

        let _producer2 = producer.clone();
        assert_eq!(Arc::strong_count(&producer.client), 2);
    }

    #[tokio::test]
    async fn test_producer_client_is_none_initially() {
        let config =
            OpenSearchEndpointConfig::from_uri("opensearch://localhost:9200/myindex").unwrap();
        let producer = OpenSearchProducer::new(config, test_rt());

        let guard = producer.client.lock().await;
        assert!(guard.is_none());
    }

    #[test]
    fn test_build_client_with_valid_config() {
        let config = OpenSearchEndpointConfig::from_uri(
            "opensearch://localhost:9200/myindex?operation=INDEX&username=admin&password=admin",
        )
        .unwrap();
        // build_client should succeed with a valid config
        let result = OpenSearchProducer::build_client(&config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_build_client_with_defaults() {
        let config =
            OpenSearchEndpointConfig::from_uri("opensearch://es-host:9200/myindex").unwrap();
        let result = OpenSearchProducer::build_client(&config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_build_client_with_tls() {
        let config =
            OpenSearchEndpointConfig::from_uri("opensearchs://es-host:443/myindex").unwrap();
        let result = OpenSearchProducer::build_client(&config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_resolve_operation_from_config() {
        let config = OpenSearchEndpointConfig::from_uri(
            "opensearch://localhost:9200/myindex?operation=INDEX",
        )
        .unwrap();
        let exchange = Exchange::new(Message::default());

        let op = OpenSearchProducer::resolve_operation(&exchange, &config);
        assert_eq!(op, OpenSearchOperation::INDEX);
    }

    #[test]
    fn test_resolve_operation_from_header() {
        let config = OpenSearchEndpointConfig::from_uri(
            "opensearch://localhost:9200/myindex?operation=SEARCH",
        )
        .unwrap();
        let mut msg = Message::default();
        msg.set_header("CamelOpenSearch.Operation", serde_json::json!("INDEX"));
        let exchange = Exchange::new(msg);

        let op = OpenSearchProducer::resolve_operation(&exchange, &config);
        assert_eq!(op, OpenSearchOperation::INDEX);
    }

    #[test]
    fn test_resolve_operation_header_overrides_config() {
        let config =
            OpenSearchEndpointConfig::from_uri("opensearch://localhost:9200/myindex?operation=GET")
                .unwrap();
        let mut msg = Message::default();
        msg.set_header("CamelOpenSearch.Operation", serde_json::json!("DELETE"));
        let exchange = Exchange::new(msg);

        let op = OpenSearchProducer::resolve_operation(&exchange, &config);
        assert_eq!(op, OpenSearchOperation::DELETE);
    }

    #[test]
    fn test_resolve_operation_invalid_header_falls_back_to_config() {
        let config = OpenSearchEndpointConfig::from_uri(
            "opensearch://localhost:9200/myindex?operation=SEARCH",
        )
        .unwrap();
        let mut msg = Message::default();
        msg.set_header("CamelOpenSearch.Operation", serde_json::json!("INVALID_OP"));
        let exchange = Exchange::new(msg);

        let op = OpenSearchProducer::resolve_operation(&exchange, &config);
        assert_eq!(op, OpenSearchOperation::SEARCH);
    }

    #[test]
    fn test_resolve_operation_non_string_header_falls_back_to_config() {
        let config =
            OpenSearchEndpointConfig::from_uri("opensearch://localhost:9200/myindex?operation=GET")
                .unwrap();
        let mut msg = Message::default();
        msg.set_header("CamelOpenSearch.Operation", serde_json::json!(123));
        let exchange = Exchange::new(msg);

        let op = OpenSearchProducer::resolve_operation(&exchange, &config);
        // Non-string header → as_str() returns None → falls back to config
        assert_eq!(op, OpenSearchOperation::GET);
    }

    #[test]
    fn test_extract_body_from_json() {
        let msg = Message::new(Body::Json(serde_json::json!({"key": "value"})));
        let exchange = Exchange::new(msg);

        let result = OpenSearchProducer::extract_body(&exchange).unwrap();
        assert_eq!(result, serde_json::json!({"key": "value"}));
    }

    #[test]
    fn test_extract_body_from_text() {
        let msg = Message::new(Body::Text(r#"{"key":"value"}"#.to_string()));
        let exchange = Exchange::new(msg);

        let result = OpenSearchProducer::extract_body(&exchange).unwrap();
        assert_eq!(result, serde_json::json!({"key": "value"}));
    }

    #[test]
    fn test_extract_body_from_empty() {
        let exchange = Exchange::new(Message::default());

        let result = OpenSearchProducer::extract_body(&exchange).unwrap();
        assert_eq!(result, serde_json::json!({}));
    }

    #[test]
    fn test_extract_body_from_invalid_text() {
        let msg = Message::new(Body::Text("not json".to_string()));
        let exchange = Exchange::new(msg);

        let result = OpenSearchProducer::extract_body(&exchange);
        assert!(result.is_err());
    }

    #[test]
    fn test_extract_body_from_stream_returns_error() {
        use camel_component_api::StreamBody;
        use futures_util::stream;
        use std::sync::Arc;
        use tokio::sync::Mutex as TokioMutex;

        let stream = stream::iter(vec![]);
        let body = Body::Stream(StreamBody {
            stream: Arc::new(TokioMutex::new(Some(Box::pin(stream)))),
            metadata: Default::default(),
        });
        let exchange = Exchange::new(Message::new(body));

        let result = OpenSearchProducer::extract_body(&exchange);
        assert!(result.is_err());
    }

    #[test]
    fn test_opensearch_bulk_size_limit() {
        let mut config =
            OpenSearchEndpointConfig::from_uri("opensearch://localhost:9200/myindex").unwrap();
        config.max_bulk_bytes = Some(100);

        let payload = serde_json::json!([
            {"index": {"_id": "1"}},
            {"message": "this document is intentionally larger than one hundred bytes to trigger bulk limit"}
        ]);

        let err = OpenSearchProducer::build_bulk_lines(&config, payload)
            .expect_err("bulk payload should exceed max_bulk_bytes");

        assert!(
            err.to_string().contains("max_bulk_bytes"),
            "expected bulk size limit error, got: {err}"
        );
    }

    #[tokio::test]
    async fn test_poll_ready_always_returns_ready() {
        let config =
            OpenSearchEndpointConfig::from_uri("opensearch://localhost:9200/myindex").unwrap();
        let mut producer = OpenSearchProducer::new(config, test_rt());
        let mut cx = Context::from_waker(noop_waker_ref());
        let result = producer.poll_ready(&mut cx);
        assert!(matches!(result, std::task::Poll::Ready(Ok(()))));
    }

    #[test]
    fn test_build_response_sets_body() {
        let exchange = Exchange::new(Message::default());
        let result = serde_json::json!({"_id": "123", "result": "created"});

        let response = OpenSearchProducer::build_response(exchange, result.clone());
        assert_eq!(response.input.body, Body::Json(result));
    }
}
