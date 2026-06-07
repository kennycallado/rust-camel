use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll};

use tokio::sync::{AcquireError, OwnedSemaphorePermit, Semaphore};
use tower::Service;
use tracing::{debug, error, warn};

use camel_api::{CamelError, Exchange};
use camel_core::Registry;

#[cfg(test)]
use camel_component_api::test_support::PanicRuntimeObservability;
#[cfg(test)]
fn rt() -> std::sync::Arc<dyn camel_component_api::RuntimeObservability> {
    std::sync::Arc::new(PanicRuntimeObservability)
}

fn poisoned<T>(e: std::sync::PoisonError<T>) -> CamelError {
    CamelError::ProcessorError(format!("lock poisoned: {}", e))
}

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
        .call_init_once(
            registry,
            HashMap::new(),
            state_store,
            tokio::runtime::Handle::current(),
        )
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

            let wasm_exchange = exchange_to_wasm(&exchange)?;

            let result = runtime
                .call_process(
                    registry2,
                    exchange.properties.clone(),
                    state_store2,
                    tokio::runtime::Handle::current(),
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
                    error!(component = "wasm", error = %e, "wasm call failed");
                    warn!(
                        module = %module_path.display(),
                        error = %e,
                        "WASM guest error"
                    );
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
}
