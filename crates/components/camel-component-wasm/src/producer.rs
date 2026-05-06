use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
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
}

impl WasmProducer {
    pub fn new(
        module_path: PathBuf,
        registry: Arc<std::sync::Mutex<Registry>>,
    ) -> Self {
        Self {
            module_path,
            registry,
            runtime: Arc::new(std::sync::Mutex::new(None)),
        }
    }

    async fn ensure_runtime(&self) -> Result<Arc<WasmRuntime>, CamelError> {
        {
            let guard = self.runtime.lock().map_err(poisoned)?;
            if let Some(ref rt) = *guard {
                return Ok(Arc::clone(rt));
            }
        }

        let runtime = Arc::new(WasmRuntime::new(&self.module_path).await?);

        let host_state =
            WasmRuntime::create_host_state(self.registry.clone(), HashMap::new());

        runtime.call_init_once(host_state).await?;

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
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, exchange: Exchange) -> Self::Future {
        let this = self.clone();
        Box::pin(async move {
            let runtime = match this.ensure_runtime().await {
                Ok(rt) => rt,
                Err(e) => {
                    warn!(
                        module = %this.module_path.display(),
                        error = %e,
                        "Failed to initialize WASM runtime"
                    );
                    return Err(e);
                }
            };

            let wasm_exchange = exchange_to_wasm(&exchange);
            let host_state = WasmRuntime::create_host_state(
                this.registry.clone(),
                exchange.properties.clone(),
            );

            let result = runtime.call_process(host_state, wasm_exchange).await;

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
