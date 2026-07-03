//! WasmSourceConsumer — Consumer trait implementation for the `source` world.
//!
//! The guest IS the source: it owns a `run()` loop that calls host-provided
//! `accept-http` / `submit-exchange`. The host bridges sync WASM calls to the
//! async pipeline via bounded tokio channels, and uses channel close +
//! epoch deadline for cancellation.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use camel_api::CamelError;
use camel_component_api::{ConcurrencyModel, Consumer, ConsumerContext};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};
use wasmtime::component::{Component, Linker};
use wasmtime::{Config, Engine, Store};

use crate::config::WasmConfig;
use crate::source_bindings::Source;
use crate::source_bindings::camel::plugin::source_host::CapabilityRequest;
use crate::source_host::{
    HttpListenerHandle, SourceChannels, SourceHostState, add_to_linker, run_http_listener,
    run_pipeline_bridge,
};

/// Epoch deadline (in ticks) granted to the guest's `configure()` call.
///
/// No epoch ticker runs during `configure()`, so this budget is never
/// decremented; it exists solely to move the store off its default
/// already-expired deadline (0) that would otherwise trap the guest at
/// the first epoch check when `epoch_interruption(true)` is enabled.
const CONFIGURE_EPOCH_DEADLINE: u64 = u64::MAX;

/// Consumer backed by a WASM `source` world component.
pub struct WasmSourceConsumer {
    module_path: PathBuf,
    guest_config: Vec<(String, String)>,
    config: WasmConfig,
    #[allow(dead_code)]
    registry: Arc<std::sync::Mutex<camel_core::Registry>>,
    cancel_token: CancellationToken,
    engine: Option<Arc<Engine>>,
    run_task: Option<JoinHandle<Result<(), CamelError>>>,
    listener_task: Option<JoinHandle<()>>,
    bridge_task: Option<JoinHandle<()>>,
}

impl WasmSourceConsumer {
    pub fn new(
        module_path: PathBuf,
        config: WasmConfig,
        guest_config: Vec<(String, String)>,
        registry: Arc<std::sync::Mutex<camel_core::Registry>>,
    ) -> Self {
        Self {
            module_path,
            guest_config,
            config,
            registry,
            cancel_token: CancellationToken::new(),
            engine: None,
            run_task: None,
            listener_task: None,
            bridge_task: None,
        }
    }
}

#[async_trait::async_trait]
impl Consumer for WasmSourceConsumer {
    async fn start(&mut self, ctx: ConsumerContext) -> Result<(), CamelError> {
        // 1. Engine config: component model + async + epoch interruption
        let mut wasm_config = Config::new();
        wasm_config.wasm_component_model(true);
        wasm_config.epoch_interruption(true);
        let engine = Arc::new(
            Engine::new(&wasm_config)
                .map_err(|e| CamelError::ProcessorError(format!("wasmtime engine: {e}")))?,
        );

        // 2. Size cap: reject oversized modules before compilation (R4-H3)
        crate::config::validate_wasm_size(&self.module_path, self.config.max_wasm_size_bytes)
            .map_err(CamelError::ProcessorError)?;

        // 3. Load component
        let component = Component::from_file(&engine, &self.module_path)
            .map_err(|e| CamelError::ProcessorError(format!("component load: {e}")))?;

        // 3. Create bounded channels
        let channels = SourceChannels::new();

        // 4. Create WASI context (guest targets wasm32-wasip2)
        let wasi = wasmtime_wasi::WasiCtxBuilder::new().build();

        // 5. Create store with host state
        let cancel = self.cancel_token.clone();
        let mut store = Store::new(
            &engine,
            SourceHostState {
                table: wasmtime::component::ResourceTable::new(),
                wasi,
                request_rx: channels.request_rx,
                exchange_tx: channels.exchange_tx,
                cancel_token: cancel.clone(),
            },
        );

        // 6. Create linker + register host functions
        let mut linker: Linker<SourceHostState> = Linker::new(&engine);
        add_to_linker(&mut linker)
            .map_err(|e| CamelError::ProcessorError(format!("linker: {e}")))?;

        // 8. Instantiate component (synchronous bindings — see source_bindings.rs)
        let source = Source::instantiate(&mut store, &component, &linker)
            .map_err(|e| CamelError::ProcessorError(format!("instantiate: {e}")))?;

        // 9. Call guest configure() → SourcePlan
        //
        // The engine has epoch_interruption(true), so a store's default epoch
        // deadline is 0 (expired). Any guest call — starting with the host
        // lowering `configure`'s arguments via the guest's `cabi_realloc` —
        // traps immediately unless a deadline is set first. configure() is a
        // short, bounded setup call with no epoch ticker running yet, so a
        // large fixed budget effectively disables interruption for this call.
        store.set_epoch_deadline(CONFIGURE_EPOCH_DEADLINE);
        let plan = source
            .call_configure(&mut store, &self.guest_config)
            .map_err(|e| CamelError::ProcessorError(format!("configure: {e}")))?
            .map_err(|e| CamelError::ProcessorError(format!("configure guest error: {e:?}")))?;

        // 10. Validate source-plan
        if plan.capabilities.len() != 1 {
            return Err(CamelError::EndpointCreationFailed(
                "source-plan must have exactly one capability".into(),
            ));
        }
        let CapabilityRequest::HttpListener(listener_spec) = &plan.capabilities[0];

        // 10b. Reject unsupported concurrency. The host drains the guest
        // strictly sequentially through capacity-1 channels, so silently
        // degrading `concurrent(N)` to sequential would violate the guest's
        // declared contract. Reject explicitly until concurrent mode exists.
        use crate::source_bindings::camel::plugin::source_host::ConcurrencyModel as PlanConcurrency;
        match plan.concurrency {
            PlanConcurrency::Sequential => {}
            PlanConcurrency::Concurrent(max) => {
                return Err(CamelError::EndpointCreationFailed(format!(
                    "WASM source does not support concurrent({max}); only sequential is implemented"
                )));
            }
        }

        // 11. Create http-listener resource handle in the table
        let listener = store
            .data_mut()
            .table
            .push(HttpListenerHandle)
            .map_err(|e| CamelError::ProcessorError(format!("resource table: {e}")))?;

        // 12. Parse bind address from listener spec
        let bind_addr: std::net::SocketAddr = listener_spec.bind.parse().map_err(|e| {
            CamelError::EndpointCreationFailed(format!(
                "invalid bind address '{}': {}",
                listener_spec.bind, e
            ))
        })?;
        let path_filter = listener_spec.path.clone();

        // 13. Bind the TCP listener synchronously BEFORE spawning, so a bind
        // failure (port in use, permission denied) surfaces as a start()
        // error rather than a background warning. Without this the guest
        // exits cleanly and the route looks healthy with nothing accepting.
        let tcp_listener = tokio::net::TcpListener::bind(bind_addr)
            .await
            .map_err(|e| {
                CamelError::Io(format!(
                    "failed to bind source HTTP listener {bind_addr}: {e}"
                ))
            })?;
        tracing::info!(%bind_addr, "source HTTP listener bound");

        // 14. Spawn HTTP listener task (serve on the already-bound listener)
        let listener_cancel = cancel.clone();
        let request_tx = channels.request_tx;
        let lt = tokio::spawn(async move {
            if let Err(e) =
                run_http_listener(tcp_listener, path_filter, request_tx, listener_cancel).await
            {
                warn!("WASM source HTTP listener exited: {e:?}");
            }
        });

        // 14. Spawn pipeline bridge task
        let bridge_ctx = ctx;
        let exchange_rx = channels.exchange_rx;
        let bt = tokio::spawn(async move {
            if let Err(e) = run_pipeline_bridge(exchange_rx, bridge_ctx).await {
                warn!("WASM source pipeline bridge exited: {e:?}");
            }
        });

        // 15. spawn_blocking for guest run() — THE CRITICAL STEP
        //
        // The guest's synchronous run() loop calls host imports (accept-http,
        // submit-exchange) that block on tokio channels via blocking_recv /
        // blocking_send. Those calls are only legal off a tokio runtime worker.
        //
        // `spawn_blocking` provides exactly such a thread (the blocking pool is
        // not part of the async scheduler). We call the SYNC `call_run`
        // directly — no `block_on`, which would re-enter runtime context on
        // this thread and make the blocking channel ops panic.
        let run_handle = tokio::task::spawn_blocking(move || {
            // Reset epoch deadline — one increment_epoch() from stop() traps the guest
            store.set_epoch_deadline(1);

            match source.call_run(&mut store, listener) {
                Ok(Ok(())) => {
                    debug!("WASM source guest run() exited normally");
                    Ok(())
                }
                Ok(Err(e)) => {
                    warn!("WASM source guest run() returned error: {e:?}");
                    Err(CamelError::ProcessorError(format!("guest run: {e:?}")))
                }
                Err(e) => {
                    warn!("WASM source guest trapped: {e}");
                    Err(CamelError::ProcessorError(format!("guest trap: {e}")))
                }
            }
        });

        // 16. Store engine + JoinHandles
        self.engine = Some(engine);
        self.run_task = Some(run_handle);
        self.listener_task = Some(lt);
        self.bridge_task = Some(bt);

        Ok(())
    }

    async fn stop(&mut self) -> Result<(), CamelError> {
        // Idempotent — safe on any exit path
        self.cancel_token.cancel();

        // Epoch tick — interrupts CPU-bound guest loops
        if let Some(ref engine) = self.engine {
            engine.increment_epoch();
        }

        // The Runtime owns and monitors run_task via background_task_handle().
        // stop() only drives cancellation; joining here races with that owner.
        // (run_task is spawn_blocking and cannot be .abort()-ed anyway; the
        // guest is cancelled via cancel-token channel close + epoch tick.)
        let _ = self.run_task.take();

        // Join listener + bridge tasks. Graceful shutdown via the cancel token
        // is raced against a timeout; on timeout we abort() the task — merely
        // dropping a JoinHandle *detaches* it (keeps it running forever), which
        // is not a deterministic stop. See join_or_abort.
        let grace = std::time::Duration::from_secs(5);
        if let Some(task) = self.listener_task.take() {
            join_or_abort(task, "listener", grace).await;
        }
        if let Some(task) = self.bridge_task.take() {
            join_or_abort(task, "bridge", grace).await;
        }

        Ok(())
    }

    fn concurrency_model(&self) -> ConcurrencyModel {
        ConcurrencyModel::Sequential
    }

    fn background_task_handle(&mut self) -> Option<JoinHandle<Result<(), CamelError>>> {
        self.run_task.take()
    }
}

/// Race joining `task` against a `grace` timeout; on timeout, `abort()` it
/// and await the abort so the task is gone before returning.
///
/// Dropping a [`JoinHandle`] only *detaches* the task — it keeps running in
/// the background. Only [`JoinHandle::abort`] actually cancels it, so this is
/// what makes `stop()` deterministic: once it returns, the task is not still
/// running.
async fn join_or_abort<T: Send + 'static>(task: JoinHandle<T>, label: &str, grace: Duration) {
    tokio::pin!(task);
    tokio::select! {
        result = &mut task => {
            if let Err(e) = result {
                warn!("{label} task panicked on shutdown: {e}");
            }
        }
        _ = tokio::time::sleep(grace) => {
            warn!("{label} task did not exit within {grace:?}, aborting");
            task.abort();
            // Await the cancellation so the task is truly gone.
            if let Err(e) = task.await {
                debug!("{label} task aborted: {e}");
            }
        }
    }
}

/// Extract guest config (bind, path, etc.) from URI query params.
/// WasmConfig::from_uri drops unknown params, so we parse them separately.
pub fn parse_guest_config(uri: &str) -> Vec<(String, String)> {
    let query = match uri.find('?') {
        Some(i) => &uri[i + 1..],
        None => return Vec::new(),
    };

    query
        .split('&')
        .filter_map(|pair| {
            let (k, v) = pair.split_once('=')?;
            // Capture all params the guest might need
            if matches!(k, "bind" | "path" | "method") {
                Some((k.to_string(), v.to_string()))
            } else {
                None
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[tokio::test]
    async fn stop_does_not_wait_for_runtime_owned_run_task() {
        let config = WasmConfig {
            timeout_secs: 1,
            ..WasmConfig::default()
        };
        let mut consumer = WasmSourceConsumer::new(
            PathBuf::from("unused.wasm"),
            config,
            Vec::new(),
            Arc::new(Mutex::new(camel_core::Registry::new())),
        );
        consumer.run_task = Some(tokio::task::spawn_blocking(|| {
            std::thread::sleep(std::time::Duration::from_secs(2));
            Ok(())
        }));

        tokio::time::timeout(std::time::Duration::from_millis(100), consumer.stop())
            .await
            .expect("stop() must not wait for run_task; runtime owns that handle")
            .expect("stop() should succeed");
    }
}
