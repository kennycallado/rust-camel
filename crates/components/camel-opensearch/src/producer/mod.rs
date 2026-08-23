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
use tokio::sync::{Mutex, Semaphore};
use tower::Service;
use tracing::{debug, error, warn};

use crate::config::{DEFAULT_TIMEOUT_MS, OpenSearchEndpointConfig, OpenSearchOperation};

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
        let timeout = Duration::from_millis(config.timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS));
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
    runtime: Arc<dyn RuntimeObservability>,
}

const DEFAULT_CONCURRENCY_LIMIT: usize = 128;

impl Clone for OpenSearchProducer {
    fn clone(&self) -> Self {
        Self {
            config: self.config.clone(),
            client: Arc::clone(&self.client),
            semaphore: Arc::clone(&self.semaphore),
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
            runtime,
        }
    }

    /// Builds an OpenSearch client from the endpoint configuration.
    fn build_client(config: &OpenSearchEndpointConfig) -> Result<OpenSearch, CamelError> {
        let url = config.base_url();
        let parsed_url = url::Url::parse(&url).map_err(|e| {
            let err = CamelError::EndpointCreationFailed(format!("Invalid OpenSearch URL: {}", e));
            // log-policy: system-broken
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
            // log-policy: system-broken
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

    /// Validates a `CamelOpenSearch.Id` header value before it is interpolated
    /// into a `_doc/{id}` request path. Rejects characters that enable path
    /// injection (per ADR-0032 trust boundary).
    pub(crate) fn validate_doc_id(id: &str) -> Result<(), ProducerError> {
        if id.is_empty() {
            return Err(ProducerError::Permanent(
                "invalid doc_id: empty".to_string(),
            ));
        }
        if id.contains('\0') {
            return Err(ProducerError::Permanent(
                "invalid doc_id: null byte".to_string(),
            ));
        }
        if id.contains('/') {
            return Err(ProducerError::Permanent(
                "invalid doc_id: path separator '/'".to_string(),
            ));
        }
        if id.contains('?') {
            return Err(ProducerError::Permanent(
                "invalid doc_id: query separator '?'".to_string(),
            ));
        }
        if id.contains('#') {
            return Err(ProducerError::Permanent(
                "invalid doc_id: fragment separator '#'".to_string(),
            ));
        }
        if id.contains('%') {
            return Err(ProducerError::Permanent(
                "invalid doc_id: percent sign '%'".to_string(),
            ));
        }
        if id.contains('\\') {
            return Err(ProducerError::Permanent(
                "invalid doc_id: backslash".to_string(),
            ));
        }
        if id == "." || id == ".." {
            return Err(ProducerError::Permanent(
                "invalid doc_id: dot segment".to_string(),
            ));
        }
        if id.chars().any(|c| (c as u32) <= 0x1F || c == '\u{7F}') {
            return Err(ProducerError::Permanent(
                "invalid doc_id: control character".to_string(),
            ));
        }
        if id.len() > 512 {
            return Err(ProducerError::Permanent(
                "invalid doc_id: exceeds 512 bytes".to_string(),
            ));
        }
        Ok(())
    }

    /// Resolves the optional `CamelOpenSearch.Id` header from the exchange,
    /// validating it before returning. Returns `Ok(None)` when absent.
    pub(crate) fn resolve_doc_id(exchange: &Exchange) -> Result<Option<&str>, ProducerError> {
        if let Some(id) = exchange
            .input
            .header("CamelOpenSearch.Id")
            .and_then(|v| v.as_str())
        {
            Self::validate_doc_id(id)?;
            Ok(Some(id))
        } else {
            Ok(None)
        }
    }

    /// Resolves a required `CamelOpenSearch.Id` header for operations where
    /// the doc id is mandatory (GET, DELETE, UPDATE, EXISTS). Returns the
    /// `ProducerError::Permanent` if the header is missing or invalid.
    #[allow(clippy::needless_lifetimes)]
    pub(crate) fn resolve_required_doc_id<'a>(
        exchange: &'a Exchange,
        op_name: &str,
    ) -> Result<&'a str, ProducerError> {
        Self::resolve_doc_id(exchange)?.ok_or_else(|| {
            ProducerError::Permanent(format!(
                "Missing CamelOpenSearch.Id header for {} operation",
                op_name
            ))
        })
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
            _ => Err(CamelError::TypeConversionFailed(
                "unsupported body type for OpenSearch operations".to_string(),
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
        let doc_id: Option<&str> = Self::resolve_doc_id(exchange)?;

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
        let doc_id = Self::resolve_required_doc_id(exchange, "GET")?;

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
        let doc_id = Self::resolve_required_doc_id(exchange, "DELETE")?;

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
        let doc_id = Self::resolve_required_doc_id(exchange, "UPDATE")?;

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
        let doc_id = Self::resolve_required_doc_id(exchange, "EXISTS")?;

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

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        // Permits are NOT acquired here: a permit reserved in poll_ready is
        // held across the poll_ready/call boundary, wedging the semaphore
        // when a wrapping Service re-readies a clone (tower contract:
        // call() may only reserve resources for its own future).
        Poll::Ready(Ok(()))
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
        let semaphore = Arc::clone(&self.semaphore);

        Box::pin(async move {
            let _permit = semaphore
                .acquire_owned()
                .await
                .map_err(|_| CamelError::ConsumerStopping)?;

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
    fn poll_ready_returns_ok_unconditionally() {
        let config =
            OpenSearchEndpointConfig::from_uri("opensearch://localhost:9200/myindex").unwrap();
        let mut producer = OpenSearchProducer::new(config, test_rt());
        let mut cx = Context::from_waker(noop_waker_ref());
        assert!(
            matches!(producer.poll_ready(&mut cx), Poll::Ready(Ok(()))),
            "fresh producer with an open semaphore must be ready"
        );
        assert_eq!(
            producer.semaphore.available_permits(),
            DEFAULT_CONCURRENCY_LIMIT,
            "poll_ready must not consume a call permit"
        );
    }

    #[tokio::test]
    async fn call_closed_semaphore_returns_error() {
        let config =
            OpenSearchEndpointConfig::from_uri("opensearch://localhost:9200/myindex").unwrap();
        let mut producer = OpenSearchProducer::new(config, test_rt());
        producer.semaphore.close();

        let result = producer.call(Exchange::new(Message::default())).await;
        assert!(
            matches!(result, Err(CamelError::ConsumerStopping)),
            "closed semaphore must surface ConsumerStopping from call"
        );
    }

    #[tokio::test]
    async fn call_blocks_on_semaphore_until_release() {
        let config =
            OpenSearchEndpointConfig::from_uri("opensearch://localhost:9200/myindex").unwrap();
        let mut producer = OpenSearchProducer::new(config, test_rt());

        // Drain all 128 permits externally: the call future must pend on
        // acquisition until they are released.
        let mut held: Vec<tokio::sync::OwnedSemaphorePermit> = Vec::new();
        while let Ok(permit) = Arc::clone(&producer.semaphore).try_acquire_owned() {
            held.push(permit);
        }
        assert_eq!(
            held.len(),
            DEFAULT_CONCURRENCY_LIMIT,
            "all permits must be drained"
        );

        let mut fut = producer.call(Exchange::new(Message::default()));
        let mut cx = Context::from_waker(noop_waker_ref());
        assert!(
            fut.as_mut().poll(&mut cx).is_pending(),
            "call must pend while all permits are held elsewhere"
        );

        drop(held);
        // The next poll completes acquisition and proceeds into client setup
        // and the request send (which pends on the absent server). The
        // permit count one below the limit proves the future got past
        // acquisition; the send result itself is not under test.
        assert!(fut.as_mut().poll(&mut cx).is_pending());
        assert_eq!(
            producer.semaphore.available_permits(),
            DEFAULT_CONCURRENCY_LIMIT - 1,
            "call future must now hold a permit (past acquisition)"
        );
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

    // --- Trust boundary: CamelOpenSearch.Id validation ---

    #[test]
    fn validate_doc_id_rejects_path_separator() {
        let result = OpenSearchProducer::validate_doc_id("foo/bar");
        assert!(matches!(result, Err(ProducerError::Permanent(_))));
    }

    #[test]
    fn validate_doc_id_rejects_query_separator() {
        let result = OpenSearchProducer::validate_doc_id("foo?bar");
        assert!(matches!(result, Err(ProducerError::Permanent(_))));
    }

    #[test]
    fn validate_doc_id_rejects_fragment_separator() {
        let result = OpenSearchProducer::validate_doc_id("foo#bar");
        assert!(matches!(result, Err(ProducerError::Permanent(_))));
    }

    #[test]
    fn validate_doc_id_rejects_percent() {
        let result = OpenSearchProducer::validate_doc_id("foo%2F");
        assert!(matches!(result, Err(ProducerError::Permanent(_))));
    }

    #[test]
    fn validate_doc_id_rejects_null_byte() {
        let result = OpenSearchProducer::validate_doc_id("foo\0bar");
        assert!(matches!(result, Err(ProducerError::Permanent(_))));
    }

    #[test]
    fn validate_doc_id_rejects_backslash() {
        let result = OpenSearchProducer::validate_doc_id("foo\\bar");
        assert!(matches!(result, Err(ProducerError::Permanent(_))));
    }

    #[test]
    fn validate_doc_id_rejects_dot_segment() {
        let dot = OpenSearchProducer::validate_doc_id(".");
        assert!(matches!(dot, Err(ProducerError::Permanent(_))));
        let dotdot = OpenSearchProducer::validate_doc_id("..");
        assert!(matches!(dotdot, Err(ProducerError::Permanent(_))));
    }

    #[test]
    fn validate_doc_id_rejects_control_char() {
        let nul = OpenSearchProducer::validate_doc_id("foo\u{0000}bar");
        assert!(matches!(nul, Err(ProducerError::Permanent(_))));
        let del = OpenSearchProducer::validate_doc_id("foo\u{007F}");
        assert!(matches!(del, Err(ProducerError::Permanent(_))));
    }

    #[test]
    fn validate_doc_id_rejects_empty() {
        let result = OpenSearchProducer::validate_doc_id("");
        assert!(matches!(result, Err(ProducerError::Permanent(_))));
    }

    #[test]
    fn validate_doc_id_rejects_oversized() {
        let oversized = "a".repeat(513);
        let result = OpenSearchProducer::validate_doc_id(&oversized);
        assert!(matches!(result, Err(ProducerError::Permanent(_))));
    }

    #[test]
    fn validate_doc_id_accepts_valid_ids() {
        for id in ["abc123", "user-42", "doc_001", "a.b.c", "type:id"] {
            let result = OpenSearchProducer::validate_doc_id(id);
            assert!(
                result.is_ok(),
                "expected Ok for valid id {:?}, got {:?}",
                id,
                result
            );
        }
    }

    #[test]
    fn resolve_doc_id_wiring_rejects_poisoned_header() {
        let mut msg = Message::default();
        msg.set_header("CamelOpenSearch.Id", serde_json::json!("foo/bar"));
        let exchange = Exchange::new(msg);

        let result = OpenSearchProducer::resolve_doc_id(&exchange);
        assert!(matches!(result, Err(ProducerError::Permanent(_))));
    }

    #[test]
    fn resolve_doc_id_wiring_accepts_valid_header() {
        let mut msg = Message::default();
        msg.set_header("CamelOpenSearch.Id", serde_json::json!("abc123"));
        let exchange = Exchange::new(msg);

        let result = OpenSearchProducer::resolve_doc_id(&exchange);
        assert!(matches!(result, Ok(Some("abc123"))));
    }

    #[test]
    fn resolve_doc_id_wiring_none_when_header_absent() {
        let exchange = Exchange::new(Message::default());

        let result = OpenSearchProducer::resolve_doc_id(&exchange);
        assert!(matches!(result, Ok(None)));
    }

    #[test]
    fn resolve_required_doc_id_wiring_missing_header_error() {
        let exchange = Exchange::new(Message::default());

        let result = OpenSearchProducer::resolve_required_doc_id(&exchange, "GET");
        assert!(matches!(result, Err(ProducerError::Permanent(_))));
    }
}
