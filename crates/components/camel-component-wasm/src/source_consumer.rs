//! WasmSourceConsumer — Consumer trait implementation for the `source` world.
//!
//! The guest IS the source: it owns an async `run()` loop that awaits
//! host-provided `accept-http` / `submit-exchange`. The host bridges the
//! async guest to the pipeline via bounded tokio channels, and uses the
//! cancel token (raced in each import) + epoch deadline for cancellation.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use camel_api::CamelError;
use camel_component_api::{ConcurrencyModel, Consumer, ConsumerContext};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};
use wasmtime::component::{Component, Linker};
use wasmtime::{AsContextMut, Config, Engine, Store};

use crate::config::WasmConfig;
use crate::source_bindings::Source;
use crate::source_bindings::camel::plugin::source_host::CapabilityRequest;
use crate::source_host::{
    DEFAULT_MAX_REQUEST_BODY_BYTES, HttpListenerHandle, SourceChannels, SourceHostState,
    add_to_linker, run_http_listener, run_pipeline_bridge,
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
        // 1. Engine config: component model + async + epoch interruption.
        //    concurrency_support is REQUIRED for Store::run_concurrent /
        //    call_run_async (async source world). Mirrors the plugin engine
        //    (runtime.rs).
        let mut wasm_config = Config::new();
        wasm_config.wasm_component_model(true);
        wasm_config.epoch_interruption(true);
        wasm_config.concurrency_support(true);
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
                max_request_body_bytes: DEFAULT_MAX_REQUEST_BODY_BYTES,
            },
        );

        // 6. Create linker + register host functions
        let mut linker: Linker<SourceHostState> = Linker::new(&engine);
        add_to_linker(&mut linker)
            .map_err(|e| CamelError::ProcessorError(format!("linker: {e}")))?;

        // 7. Set a large epoch deadline before any guest code runs. With
        // epoch_interruption(true) the store's default deadline is 0 (already
        // expired); instantiate_async and configure both run guest WASM, so
        // without this they trap immediately with "wasm trap: interrupt". No
        // epoch ticker runs during setup, so a u64::MAX budget disables
        // interruption for both calls. The run task later resets this to 1
        // (a pure stop tripwire). Mirrors the plugin path (runtime.rs:175).
        store.set_epoch_deadline(CONFIGURE_EPOCH_DEADLINE);

        // 8. Instantiate component (async bindings — see source_bindings.rs).
        let source = Source::instantiate_async(&mut store, &component, &linker)
            .await
            .map_err(|e| CamelError::ProcessorError(format!("instantiate: {e}")))?;

        // 9. Call guest configure() → SourcePlan
        //
        // configure() is a SYNC export (only run/accept-http/submit-exchange
        // are async), but concurrency_support(true) forbids the sync
        // `TypedFunc::call` path — so dispatch it via `call_async` on the
        // underlying typed func. No async imports are needed during configure,
        // so this stays out of run_concurrent. The large deadline set above
        // covers it.
        let (plan_result,) = source
            .func_configure()
            .call_async(&mut store, (self.guest_config.as_slice(),))
            .await
            .map_err(|e| CamelError::ProcessorError(format!("configure: {e}")))?;
        let plan = plan_result
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

        // 15. Spawn the async guest run() loop on a tokio task — THE CRITICAL STEP.
        //
        // The guest's `run` export is async; its host imports (accept-http,
        // submit-exchange) are async too. The whole thing is driven under
        // `Store::run_concurrent`, which is what makes the accessor-based
        // async imports dispatchable (concurrency_support(true) is required).
        //
        // Epoch: deadline(1) is a pure stop tripwire. No epoch ticker runs
        // during normal operation (each source consumer owns a private engine),
        // so the deadline never advances until stop()'s single
        // engine.increment_epoch(). That one increment traps a guest stuck in
        // a CPU-bound loop; a guest parked in an import is unblocked first by
        // the import's cancel_token select!.
        //
        // No watchdog: the source `run()` is an unbounded loop that
        // legitimately parks in `accept-http` for arbitrary durations between
        // requests. A fixed no-progress timeout would kill idle webhook
        // sources. The existing safeguards cover all failure modes:
        //   - Epoch interruption (deadline(1) + increment_epoch from stop())
        //     catches CPU-bound guest loops.
        //   - Cancel-token select! inside each import unblocks park-on-stop.
        //   - Route-level supervision catches "guest hangs forever" at the
        //     route manager level.
        let run_handle = tokio::spawn(async move {
            store.set_epoch_deadline(1);

            // 3-layer result: run_concurrent Result< trappable Result<
            // WIT Result > >. peel_concurrent flattens the outer two
            // wasmtime::Error layers.
            let result = crate::error::peel_concurrent(
                store
                    .as_context_mut()
                    .run_concurrent(async |accessor| source.call_run(accessor, listener).await)
                    .await,
                |e| CamelError::ProcessorError(format!("guest trap: {e}")),
                |e| CamelError::ProcessorError(format!("guest trap: {e}")),
            );

            match result {
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
                    Err(e)
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
        // If the Runtime has already taken the handle, run_task is None here
        // and the Runtime is responsible for joining it. If we still own it
        // (stop() called without background_task_handle, or a test path),
        // join with a grace timeout and abort on timeout — dropping a
        // JoinHandle only detaches the task (keeps it running forever).
        let grace = std::time::Duration::from_secs(5);
        if let Some(task) = self.run_task.take() {
            join_or_abort(task, "source run", grace).await;
        }

        // Join listener + bridge tasks. Graceful shutdown via the cancel token
        // is raced against a timeout; on timeout we abort() the task.
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
        // Mirrors the real run task shape (tokio::spawn of an async future).
        consumer.run_task = Some(tokio::spawn(async {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            Ok(())
        }));

        // Simulate the Runtime taking ownership of the run task handle.
        // Once taken, stop() no longer sees run_task and must not wait for it.
        let _runtime_owned = consumer.background_task_handle();

        tokio::time::timeout(std::time::Duration::from_millis(100), consumer.stop())
            .await
            .expect("stop() must not wait for a runtime-owned run_task")
            .expect("stop() should succeed");
    }

    /// When stop() still owns the run task (Runtime hasn't taken it), it must
    /// abort it after the grace timeout rather than detaching (leaking) it.
    #[tokio::test]
    async fn stop_aborts_owned_run_task_after_grace() {
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
        // A run task that never exits on its own — only abort will stop it.
        consumer.run_task = Some(tokio::spawn(async {
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            Ok(())
        }));

        // stop() should abort the stuck task within the grace window (5s) +
        // margin. Asserting 15s ceiling; the abort should fire at ~5s.
        let result =
            tokio::time::timeout(std::time::Duration::from_secs(15), consumer.stop()).await;
        assert!(
            result.is_ok(),
            "stop() must abort a stuck run task within the grace window, not hang"
        );
        result.unwrap().expect("stop() should succeed");
    }
}
