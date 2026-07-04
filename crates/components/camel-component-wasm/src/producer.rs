use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;

use tokio::sync::{AcquireError, OwnedSemaphorePermit, Semaphore};
use tokio_util::sync::CancellationToken;
use tower::Service;
use tracing::{debug, warn};

use camel_api::body::DEFAULT_MATERIALIZE_LIMIT;
use camel_api::{Body, CamelError, Exchange};
use camel_core::Registry;

fn poisoned<T>(e: std::sync::PoisonError<T>) -> CamelError {
    CamelError::ProcessorError(format!("lock poisoned: {}", e))
}

/// Returns `true` when the body is a streaming body that requires the
/// asynchronous streaming host bridge (`process_streaming_exchange`).
///
/// Non-stream variants (`Empty`, `Text`, `Bytes`, `Json`, `Xml`) route to
/// the synchronous `call_process` path instead.
pub(crate) fn is_streaming(body: &Body) -> bool {
    matches!(body, Body::Stream(_))
}

/// Default per-stream byte cap passed to `process_streaming_exchange`.
/// Mirrors `camel_api::DEFAULT_MATERIALIZE_LIMIT`; re-stated as `u64` because
/// the streaming host bridge takes bytes as `u64`.
const DEFAULT_STREAM_MAX_BYTES: u64 = DEFAULT_MATERIALIZE_LIMIT as u64;

/// Default watchdog window: if the guest makes no progress shipping/receiving
/// stream chunks for this duration, the call is cancelled.
const DEFAULT_NO_PROGRESS_TIMEOUT: Duration = Duration::from_secs(60);

/// Standalone runtime init — takes owned Arcs so the resulting future is `Send`
/// without requiring `WasmProducer: Sync`.
async fn ensure_runtime_fn(
    module_path: PathBuf,
    config: crate::config::WasmConfig,
    registry: Arc<std::sync::Mutex<Registry>>,
    runtime_store: Arc<std::sync::Mutex<Option<Arc<WasmRuntime>>>>,
    state_store: crate::state_store::StateStore,
) -> Result<Arc<WasmRuntime>, CamelError> {
    {
        let guard = runtime_store.lock().map_err(poisoned)?;
        if let Some(ref rt) = *guard {
            return Ok(Arc::clone(rt));
        }
    }

    let runtime = Arc::new(WasmRuntime::new(&module_path, config).await?);

    runtime
        .call_init_once(registry, HashMap::new(), state_store)
        .await?;

    {
        let mut guard = runtime_store.lock().map_err(poisoned)?;
        if guard.is_none() {
            *guard = Some(Arc::clone(&runtime));
        }
    }

    Ok(runtime)
}

use crate::runtime::WasmRuntime;
use crate::serde_bridge::{exchange_to_wasm, wasm_to_exchange};

type AcquireFut =
    Option<Pin<Box<dyn Future<Output = Result<OwnedSemaphorePermit, AcquireError>> + Send>>>;

pub struct WasmProducer {
    module_path: PathBuf,
    registry: Arc<std::sync::Mutex<Registry>>,
    runtime: Arc<std::sync::Mutex<Option<Arc<WasmRuntime>>>>,
    config: crate::config::WasmConfig,
    state_store: crate::state_store::StateStore,
    init_failed: Arc<AtomicBool>,
    sem: Arc<Semaphore>,
    pending_permit: Option<OwnedSemaphorePermit>,
    acquire_fut: AcquireFut,
    /// `Arc<dyn RuntimeObservability>` for Phase B metric/health calls.
    /// Named `observability` (not `runtime`) to avoid collision with the
    /// existing `runtime: Arc<Mutex<Option<Arc<WasmRuntime>>>>` field above
    /// which holds the WASM runtime instance, not the observability surface.
    observability: Arc<dyn camel_component_api::RuntimeObservability>,
    /// Producer-local cancellation root. A child token is derived per `call`
    /// and handed to `process_streaming_exchange` so a stuck guest can be
    /// cancelled without disturbing sibling in-flight calls.
    ///
    /// Root cancellation (graceful shutdown of all in-flight streams) is
    /// deferred — currently only the per-call watchdog triggers cancellation.
    cancel: CancellationToken,
    /// Per-stream byte cap forwarded to `process_streaming_exchange`.
    max_bytes: u64,
    /// Watchdog window forwarded to `process_streaming_exchange`.
    no_progress_timeout: Duration,
}

impl Clone for WasmProducer {
    fn clone(&self) -> Self {
        Self {
            module_path: self.module_path.clone(),
            registry: Arc::clone(&self.registry),
            runtime: Arc::clone(&self.runtime),
            config: self.config.clone(),
            state_store: self.state_store.clone(),
            init_failed: Arc::clone(&self.init_failed),
            sem: Arc::clone(&self.sem),
            pending_permit: None,
            acquire_fut: None,
            observability: Arc::clone(&self.observability),
            cancel: self.cancel.clone(),
            max_bytes: self.max_bytes,
            no_progress_timeout: self.no_progress_timeout,
        }
    }
}

impl WasmProducer {
    pub fn new(
        module_path: PathBuf,
        registry: Arc<std::sync::Mutex<Registry>>,
        config: crate::config::WasmConfig,
        observability: Arc<dyn camel_component_api::RuntimeObservability>,
    ) -> Self {
        let max_concurrent_calls = config.max_concurrent_calls;
        Self {
            module_path,
            registry,
            runtime: Arc::new(std::sync::Mutex::new(None)),
            config,
            state_store: crate::state_store::StateStore::new(),
            init_failed: Arc::new(AtomicBool::new(false)),
            sem: Arc::new(Semaphore::new(max_concurrent_calls)),
            pending_permit: None,
            acquire_fut: None,
            observability,
            cancel: CancellationToken::new(),
            max_bytes: DEFAULT_STREAM_MAX_BYTES,
            no_progress_timeout: DEFAULT_NO_PROGRESS_TIMEOUT,
        }
    }

    pub fn config(&self) -> &crate::config::WasmConfig {
        &self.config
    }
}

impl Service<Exchange> for WasmProducer {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        if self.init_failed.load(Ordering::Relaxed) {
            return Poll::Ready(Err(CamelError::ProcessorError(
                "wasm runtime initialization failed".to_string(),
            )));
        }

        if self.pending_permit.is_some() {
            return Poll::Ready(Ok(()));
        }

        let fut = self
            .acquire_fut
            .get_or_insert_with(|| Box::pin(Arc::clone(&self.sem).acquire_owned()));

        match fut.as_mut().poll(cx) {
            Poll::Ready(Ok(permit)) => {
                self.acquire_fut = None;
                self.pending_permit = Some(permit);
                Poll::Ready(Ok(()))
            }
            Poll::Ready(Err(_)) => Poll::Ready(Err(CamelError::ProcessorError(
                "wasm producer semaphore closed".to_string(),
            ))),
            Poll::Pending => Poll::Pending,
        }
    }

    fn call(&mut self, exchange: Exchange) -> Self::Future {
        let permit = self.pending_permit.take();
        // Extract owned fields before entering the async block to avoid
        // borrowing &self / &this across an await (which would require
        // WasmProducer: Sync, violated by the dyn Future in acquire_fut).
        let module_path = self.module_path.clone();
        let config = self.config.clone();
        let registry = Arc::clone(&self.registry);
        let registry2 = Arc::clone(&self.registry);
        let runtime_store = Arc::clone(&self.runtime);
        let state_store = self.state_store.clone();
        let state_store2 = self.state_store.clone();
        let init_failed = Arc::clone(&self.init_failed);
        // Streaming knobs: clone the root token (cheap) and derive a child
        // per call inside the future; the byte cap and watchdog window are
        // Copy types.
        let cancel_root = self.cancel.clone();
        let max_bytes = self.max_bytes;
        let no_progress_timeout = self.no_progress_timeout;
        Box::pin(async move {
            tracing::debug!(component = "wasm", "wasm call started");
            let _permit = permit;
            let runtime = match ensure_runtime_fn(
                module_path.clone(),
                config,
                registry,
                runtime_store,
                state_store,
            )
            .await
            {
                Ok(rt) => {
                    init_failed.store(false, Ordering::Relaxed);
                    rt
                }
                Err(e) => {
                    init_failed.store(true, Ordering::Relaxed);
                    warn!(
                        module = %module_path.display(),
                        error = %e,
                        "Failed to initialize WASM runtime"
                    );
                    return Err(e);
                }
            };

            // Route on body shape: a Stream body must traverse the streaming
            // host bridge (run_concurrent + BoxStreamProducer) because the
            // guest reads bytes asynchronously; anything else stays on the
            // synchronous call_process path.
            let is_stream = is_streaming(&exchange.input.body);

            if is_stream {
                // `process_streaming_exchange` takes the Exchange by value and
                // extracts the stream internally, so clone first to preserve
                // the non-body fields (extensions, otel_context, correlation_id)
                // that `wasm_to_exchange` does not overwrite. The clone shares
                // the stream's Arc handle, but its body is discarded by the
                // merge anyway.
                let mut out = exchange.clone();
                let cancel = cancel_root.child_token();
                let result = runtime
                    .process_streaming_exchange(
                        registry2,
                        exchange.properties.clone(),
                        state_store2,
                        exchange,
                        cancel,
                        max_bytes,
                        no_progress_timeout,
                    )
                    .await;

                match result {
                    Ok(wasm_result) => {
                        wasm_to_exchange(wasm_result, &mut out);
                        debug!(
                            module = %module_path.display(),
                            "WASM streaming producer completed successfully"
                        );
                        Ok(out)
                    }
                    Err(e) => {
                        // log-policy: handler-owned
                        warn!(
                            component = "wasm",
                            error = %e,
                            "wasm streaming call failed"
                        );
                        Err(e.into())
                    }
                }
            } else {
                let wasm_exchange = exchange_to_wasm(&exchange)?;

                let result = runtime
                    .call_process(
                        registry2,
                        exchange.properties.clone(),
                        state_store2,
                        wasm_exchange,
                    )
                    .await;

                match result {
                    Ok(wasm_result) => {
                        let mut out = exchange;
                        wasm_to_exchange(wasm_result, &mut out);
                        debug!(
                            module = %module_path.display(),
                            "WASM producer completed successfully"
                        );
                        Ok(out)
                    }
                    Err(e) => {
                        // log-policy: handler-owned
                        warn!(component = "wasm", error = %e, "wasm call failed");
                        warn!(
                            module = %module_path.display(),
                            error = %e,
                            "WASM guest error"
                        );
                        Err(e.into())
                    }
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::WasmConfig;
    use camel_component_api::test_support::PanicRuntimeObservability;
    fn test_rt() -> std::sync::Arc<dyn camel_component_api::RuntimeObservability> {
        std::sync::Arc::new(PanicRuntimeObservability)
    }

    #[test]
    fn test_producer_stores_config() {
        let config = WasmConfig {
            timeout_secs: 5,
            max_memory_bytes: 1024,
            max_concurrent_calls: 4,
            ..WasmConfig::default()
        };
        let producer = WasmProducer::new(
            PathBuf::from("test.wasm"),
            Arc::new(std::sync::Mutex::new(Registry::new())),
            config,
            test_rt(),
        );
        assert_eq!(producer.config().timeout_secs, 5);
        assert_eq!(producer.config().max_memory_bytes, 1024);
    }

    #[test]
    fn test_producer_is_clone() {
        let config = WasmConfig::default();
        let producer = WasmProducer::new(
            PathBuf::from("test.wasm"),
            Arc::new(std::sync::Mutex::new(Registry::new())),
            config,
            test_rt(),
        );
        let _cloned = producer.clone();
    }

    #[test]
    fn test_poll_ready_before_init_failure() {
        let config = WasmConfig::default();
        let mut producer = WasmProducer::new(
            PathBuf::from("test.wasm"),
            Arc::new(std::sync::Mutex::new(Registry::new())),
            config,
            test_rt(),
        );
        let mut cx = Context::from_waker(futures::task::noop_waker_ref());
        let result = producer.poll_ready(&mut cx);
        assert!(matches!(result, Poll::Ready(Ok(()))));
    }

    #[test]
    fn test_poll_ready_after_init_failure() {
        let config = WasmConfig::default();
        let mut producer = WasmProducer::new(
            PathBuf::from("test.wasm"),
            Arc::new(std::sync::Mutex::new(Registry::new())),
            config,
            test_rt(),
        );
        producer.init_failed.store(true, Ordering::Relaxed);
        let mut cx = Context::from_waker(futures::task::noop_waker_ref());
        let result = producer.poll_ready(&mut cx);
        assert!(
            matches!(result, Poll::Ready(Err(CamelError::ProcessorError(msg))) if msg.contains("wasm runtime initialization failed"))
        );
    }

    #[test]
    fn test_poll_ready_pending_when_no_permits_available() {
        let config = WasmConfig {
            timeout_secs: 5,
            max_memory_bytes: 1024,
            max_concurrent_calls: 1,
            ..WasmConfig::default()
        };
        let mut producer = WasmProducer::new(
            PathBuf::from("test.wasm"),
            Arc::new(std::sync::Mutex::new(Registry::new())),
            config,
            test_rt(),
        );

        let permit = Arc::clone(&producer.sem)
            .try_acquire_owned()
            .expect("test should consume sole permit");

        let mut cx = Context::from_waker(futures::task::noop_waker_ref());
        let first = producer.poll_ready(&mut cx);
        assert!(matches!(first, Poll::Pending));

        drop(permit);
        let second = producer.poll_ready(&mut cx);
        assert!(matches!(second, Poll::Ready(Ok(()))));
    }

    #[test]
    fn test_streaming_defaults_set_on_construction() {
        let producer = WasmProducer::new(
            PathBuf::from("test.wasm"),
            Arc::new(std::sync::Mutex::new(Registry::new())),
            WasmConfig::default(),
            test_rt(),
        );
        assert_eq!(producer.max_bytes, DEFAULT_STREAM_MAX_BYTES);
        assert_eq!(producer.no_progress_timeout, DEFAULT_NO_PROGRESS_TIMEOUT);
        // Root token is not yet cancelled; a child must also be live.
        assert!(!producer.cancel.is_cancelled());
        let child = producer.cancel.child_token();
        assert!(!child.is_cancelled());
        producer.cancel.cancel();
        assert!(child.is_cancelled(), "child must observe parent cancel");
    }

    #[test]
    fn test_is_streaming_all_variants() {
        // Property: Stream → true, all other variants → false.
        // Locks the discriminator so a future refactor that widens the
        // stream set must also update this test.
        use bytes::Bytes;
        use camel_api::{StreamBody, StreamMetadata};

        let stream_body = Body::Stream(StreamBody {
            stream: Arc::new(tokio::sync::Mutex::new(Some(Box::pin(
                futures::stream::empty(),
            )))),
            metadata: StreamMetadata::default(),
        });
        assert!(is_streaming(&stream_body));

        assert!(!is_streaming(&Body::Empty));
        assert!(!is_streaming(&Body::Text("x".into())));
        assert!(!is_streaming(&Body::Bytes(Bytes::from_static(b"x"))));
        assert!(!is_streaming(&Body::Json(serde_json::Value::Null)));
        assert!(!is_streaming(&Body::Xml("<x/>".into())));
    }

    #[test]
    fn test_clone_preserves_streaming_knobs() {
        let mut producer = WasmProducer::new(
            PathBuf::from("test.wasm"),
            Arc::new(std::sync::Mutex::new(Registry::new())),
            WasmConfig::default(),
            test_rt(),
        );
        producer.max_bytes = 2048;
        producer.no_progress_timeout = Duration::from_secs(7);
        let cloned = producer.clone();
        assert_eq!(cloned.max_bytes, 2048);
        assert_eq!(cloned.no_progress_timeout, Duration::from_secs(7));
        // Cloned token shares the same cancellation lineage.
        let child = cloned.cancel.child_token();
        producer.cancel.cancel();
        assert!(child.is_cancelled());
    }
}
