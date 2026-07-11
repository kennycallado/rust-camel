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

use camel_api::{Body, CamelError, Exchange, StreamBody};
use camel_core::Registry;

fn poisoned<T>(e: std::sync::PoisonError<T>) -> CamelError {
    CamelError::ProcessorError(format!("lock poisoned: {}", e))
}

/// Returns `true` when the body is a streaming body that requires the
/// asynchronous streaming host bridge (`process_streaming_exchange`).
///
/// Non-stream variants (`Empty`, `Text`, `Bytes`, `Json`, `Xml`) are handled
/// by `process_streaming_exchange`'s internal stream extraction (the `else`
/// branch in its `make_drive` closure).
#[cfg(test)]
pub(crate) fn is_streaming(body: &Body) -> bool {
    matches!(body, Body::Stream(_))
}

/// Default watchdog window: if the guest makes no progress shipping/receiving
/// stream chunks for this duration, the call is cancelled.
pub(crate) const DEFAULT_NO_PROGRESS_TIMEOUT: Duration = Duration::from_secs(60);

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
use crate::serde_bridge::wasm_to_exchange;

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
        let max_stream_bytes = config.max_stream_bytes;
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
            max_bytes: max_stream_bytes,
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

            let pending_permit = permit.expect("poll_ready gates Some"); // allow-unwrap

            // All plugin calls go through `process_streaming_exchange`, which
            // handles both stream and non-stream inputs (its `make_drive`
            // closure extracts the stream if present, otherwise uses the
            // pre-converted non-stream body) AND drains return streams via
            // `take_stream`. This closes the hole where a non-stream-input
            // plugin returning a `WasmBody::Stream` was dropped by
            // `wasm_to_body`'s Stream arm.
            let mut out = exchange.clone();
            let cancel = cancel_root.child_token();
            let mut guard = crate::cancel_guard::InvokeCancelGuard::new(cancel.clone());
            let result = runtime
                .process_streaming_exchange(
                    registry2,
                    exchange.properties.clone(),
                    state_store2,
                    exchange,
                    pending_permit,
                    cancel,
                    max_bytes,
                    no_progress_timeout,
                )
                .await;

            match result {
                Ok(streaming_result) => {
                    wasm_to_exchange(streaming_result.exchange, &mut out);
                    if let Some(drain_rx) = streaming_result.drain_rx {
                        out.output
                            .as_mut()
                            .expect("output present") // allow-unwrap
                            .body = Body::Stream(StreamBody {
                            stream: Arc::new(tokio::sync::Mutex::new(Some(
                                crate::return_stream::receiver_to_body_stream(drain_rx),
                            ))),
                            metadata: streaming_result.metadata.clone(),
                        });
                    }
                    guard.complete();
                    debug!(
                        module = %module_path.display(),
                        "WASM producer completed successfully"
                    );
                    Ok(out)
                }
                Err(e) => {
                    // log-policy: handler-owned
                    warn!(
                        component = "wasm",
                        error = %e,
                        "wasm call failed"
                    );
                    // guard drops here without complete() — fires cancel on the
                    // child token. Benign: the work already failed, and the
                    // drain task observes the cancel via its select! branch.
                    Err(e.into())
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
        let config = WasmConfig {
            max_stream_bytes: 52_428_800,
            ..WasmConfig::default()
        };
        let producer = WasmProducer::new(
            PathBuf::from("test.wasm"),
            Arc::new(std::sync::Mutex::new(Registry::new())),
            config.clone(),
            test_rt(),
        );
        assert_eq!(producer.max_bytes, config.max_stream_bytes);
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
