use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll};

use tower::Service;
use tracing::{debug, warn};

use camel_api::{CamelError, Exchange};
use camel_core::Registry;

fn poisoned<T>(e: std::sync::PoisonError<T>) -> CamelError {
    CamelError::ProcessorError(format!("lock poisoned: {}", e))
}

use crate::runtime::WasmRuntime;
use crate::serde_bridge::{exchange_to_wasm, wasm_to_exchange};

#[derive(Clone)]
pub struct WasmProducer {
    module_path: PathBuf,
    registry: Arc<std::sync::Mutex<Registry>>,
    runtime: Arc<std::sync::Mutex<Option<Arc<WasmRuntime>>>>,
    config: crate::config::WasmConfig,
    state_store: crate::state_store::StateStore,
    init_failed: Arc<AtomicBool>,
}

impl WasmProducer {
    pub fn new(
        module_path: PathBuf,
        registry: Arc<std::sync::Mutex<Registry>>,
        config: crate::config::WasmConfig,
    ) -> Self {
        Self {
            module_path,
            registry,
            runtime: Arc::new(std::sync::Mutex::new(None)),
            config,
            state_store: crate::state_store::StateStore::new(),
            init_failed: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn config(&self) -> &crate::config::WasmConfig {
        &self.config
    }

    async fn ensure_runtime(&self) -> Result<Arc<WasmRuntime>, CamelError> {
        {
            let guard = self.runtime.lock().map_err(poisoned)?;
            if let Some(ref rt) = *guard {
                return Ok(Arc::clone(rt));
            }
        }

        let runtime = Arc::new(WasmRuntime::new(&self.module_path, self.config.clone()).await?);

        runtime
            .call_init_once(
                self.registry.clone(),
                HashMap::new(),
                self.state_store.clone(),
            )
            .await?;

        {
            let mut guard = self.runtime.lock().map_err(poisoned)?;
            if guard.is_none() {
                *guard = Some(Arc::clone(&runtime));
            }
        }

        Ok(runtime)
    }
}

impl Service<Exchange> for WasmProducer {
    type Response = Exchange;
    type Error = CamelError;
    type Future = Pin<Box<dyn Future<Output = Result<Exchange, CamelError>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        if self.init_failed.load(Ordering::Relaxed) {
            return Poll::Ready(Err(CamelError::ProcessorError(
                "wasm runtime initialization failed".to_string(),
            )));
        }
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, exchange: Exchange) -> Self::Future {
        let this = self.clone();
        Box::pin(async move {
            let runtime = match this.ensure_runtime().await {
                Ok(rt) => {
                    this.init_failed.store(false, Ordering::Relaxed);
                    rt
                }
                Err(e) => {
                    this.init_failed.store(true, Ordering::Relaxed);
                    warn!(
                        module = %this.module_path.display(),
                        error = %e,
                        "Failed to initialize WASM runtime"
                    );
                    return Err(e);
                }
            };

            let wasm_exchange = exchange_to_wasm(&exchange);

            let result = runtime
                .call_process(
                    this.registry.clone(),
                    exchange.properties.clone(),
                    this.state_store.clone(),
                    wasm_exchange,
                )
                .await;

            match result {
                Ok(wasm_result) => {
                    let mut out = exchange;
                    wasm_to_exchange(wasm_result, &mut out);
                    debug!(
                        module = %this.module_path.display(),
                        "WASM producer completed successfully"
                    );
                    Ok(out)
                }
                Err(e) => {
                    warn!(
                        module = %this.module_path.display(),
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

    #[test]
    fn test_producer_stores_config() {
        let config = WasmConfig {
            timeout_secs: 5,
            max_memory_bytes: 1024,
        };
        let producer = WasmProducer::new(
            PathBuf::from("test.wasm"),
            Arc::new(std::sync::Mutex::new(Registry::new())),
            config,
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
        );
        producer.init_failed.store(true, Ordering::Relaxed);
        let mut cx = Context::from_waker(futures::task::noop_waker_ref());
        let result = producer.poll_ready(&mut cx);
        assert!(
            matches!(result, Poll::Ready(Err(CamelError::ProcessorError(msg))) if msg.contains("wasm runtime initialization failed"))
        );
    }
}
