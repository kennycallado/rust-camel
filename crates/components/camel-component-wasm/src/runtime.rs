use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use wasmtime::component::{Component, Linker, ResourceTable};
use wasmtime::{AsContextMut, Config, Engine, Store};
use wasmtime_wasi::WasiCtxBuilder;

use camel_api::{Body, Exchange};
use camel_core::Registry;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

use crate::bindings::Plugin;
use crate::bindings::camel::plugin::types::WasmExchange;
use crate::error::WasmError;

pub struct WasmHostState {
    pub table: ResourceTable,
    pub wasi: wasmtime_wasi::WasiCtx,
    pub properties: HashMap<String, Value>,
    pub registry: Arc<std::sync::Mutex<Registry>>,
    pub call_depth: Arc<std::sync::atomic::AtomicUsize>,
    pub limits: wasmtime::StoreLimits,
    pub state_store: crate::state_store::StateStore,
    pub capabilities: crate::capabilities::WasmCapabilities,
}

impl wasmtime_wasi::WasiView for WasmHostState {
    fn ctx(&mut self) -> wasmtime_wasi::WasiCtxView<'_> {
        wasmtime_wasi::WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

pub struct WasmRuntime {
    engine: Engine,
    linker: Linker<WasmHostState>,
    component: Component,
    module_path: PathBuf,
    config: crate::config::WasmConfig,
    #[allow(dead_code)]
    epoch_ticker: crate::epoch::EpochTicker,
}

impl WasmRuntime {
    pub async fn new(
        module_path: impl AsRef<Path>,
        wasm_config: crate::config::WasmConfig,
    ) -> Result<Self, WasmError> {
        let module_path = module_path.as_ref().to_path_buf();

        let mut config = Config::new();
        config.wasm_component_model(true);
        config.epoch_interruption(true);
        config.concurrency_support(true);

        let engine =
            Engine::new(&config).map_err(|e| WasmError::CompilationFailed(e.to_string()))?;

        // Existence check first (preserve ModuleNotFound error variant)
        if !module_path.exists() {
            return Err(WasmError::ModuleNotFound(format!(
                "Failed to load WASM module {}: not found",
                module_path.display()
            )));
        }

        // Size cap: reject oversized modules before compilation (R4-H3)
        crate::config::validate_wasm_size(&module_path, wasm_config.max_wasm_size_bytes)
            .map_err(WasmError::CompilationFailed)?;

        let component = Component::from_file(&engine, &module_path).map_err(|e| {
            // File exists and size is OK — compilation error is genuine
            WasmError::CompilationFailed(format!(
                "Failed to load WASM module {}: {}",
                module_path.display(),
                e
            ))
        })?;

        let mut linker: Linker<WasmHostState> = Linker::new(&engine);

        wasmtime_wasi::p2::add_to_linker_async(&mut linker)
            .map_err(|e| WasmError::CompilationFailed(e.to_string()))?;

        crate::host_functions::add_to_linker(&mut linker)
            .map_err(|e| WasmError::CompilationFailed(e.to_string()))?;

        let epoch_ticker =
            crate::epoch::EpochTicker::start(engine.clone(), wasm_config.epoch_interval());

        Ok(Self {
            engine,
            linker,
            component,
            module_path,
            config: wasm_config,
            epoch_ticker,
        })
    }

    /// Construct a fresh `WasmHostState` for one guest invocation.
    ///
    /// `max_memory_bytes` is enforced via `wasmtime::StoreLimitsBuilder::memory_size`.
    /// Pass `0` to fall back to `StoreLimits::default()` (4 GiB wasmtime ceiling);
    /// any positive value is applied as the cap.
    pub fn create_host_state(
        registry: Arc<std::sync::Mutex<Registry>>,
        properties: HashMap<String, Value>,
        state_store: crate::state_store::StateStore,
        max_memory_bytes: u64,
        capabilities: crate::capabilities::WasmCapabilities,
    ) -> WasmHostState {
        let limits = if max_memory_bytes == 0 {
            wasmtime::StoreLimits::default()
        } else {
            wasmtime::StoreLimitsBuilder::new()
                .memory_size(max_memory_bytes as usize)
                .build()
        };
        WasmHostState {
            table: ResourceTable::new(),
            wasi: WasiCtxBuilder::new().inherit_stderr().build(),
            properties,
            registry,
            call_depth: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            limits,
            state_store,
            capabilities,
        }
    }

    /// Classify a wasmtime error into a structured WasmError.
    ///
    /// Downcasts to `wasmtime::Trap` first — if successful, routes to
    /// Timeout/OutOfMemory/Trap variants. Otherwise falls back to GuestPanic.
    fn classify_error(&self, e: wasmtime::Error) -> WasmError {
        self.config.classify_error(&self.module_path, e)
    }

    pub async fn call_init_once(
        &self,
        registry: Arc<std::sync::Mutex<Registry>>,
        properties: HashMap<String, Value>,
        state_store: crate::state_store::StateStore,
    ) -> Result<(), WasmError> {
        let host_state = Self::create_host_state(
            registry,
            properties,
            state_store,
            self.config.max_memory_bytes,
            crate::capabilities::WasmCapabilities::from_scheme_list(
                &self.config.allow_call_schemes,
            ),
        );
        let mut store = Store::new(&self.engine, host_state);
        store.limiter(|state| &mut state.limits);
        store.set_epoch_deadline(self.config.epoch_deadline());

        let plugin = Plugin::instantiate_async(&mut store, &self.component, &self.linker)
            .await
            .map_err(|e| WasmError::InstantiationFailed(e.to_string()))?;

        // The async-with-trappable WIT shape produces a 2-layer Result:
        //   run_concurrent Result<inner, wasmtime::Error>     (outer)
        //   trappable     Result<Result<(), String>, wasmtime::Error>  (inner)
        // Both layers carry wasmtime::Error — use peel_concurrent to map
        // each to WasmError uniformly.
        let result: Result<(), String> = crate::error::peel_concurrent(
            store
                .as_context_mut()
                .run_concurrent(async |accessor| plugin.call_init(accessor).await)
                .await,
            |e| self.classify_error(e),
            |e| self.classify_error(e),
        )?;

        if let Err(e) = result {
            tracing::debug!(
                "WASM init() returned error (optional hook): {} — {}",
                self.module_path.display(),
                e
            );
        }
        Ok(())
    }

    pub async fn call_process(
        &self,
        registry: Arc<std::sync::Mutex<Registry>>,
        properties: HashMap<String, Value>,
        state_store: crate::state_store::StateStore,
        exchange: WasmExchange,
    ) -> Result<WasmExchange, WasmError> {
        let host_state = Self::create_host_state(
            registry,
            properties,
            state_store,
            self.config.max_memory_bytes,
            crate::capabilities::WasmCapabilities::from_scheme_list(
                &self.config.allow_call_schemes,
            ),
        );
        let mut store = Store::new(&self.engine, host_state);
        store.limiter(|state| &mut state.limits);
        store.set_epoch_deadline(self.config.epoch_deadline());

        let plugin = Plugin::instantiate_async(&mut store, &self.component, &self.linker)
            .await
            .map_err(|e| WasmError::InstantiationFailed(e.to_string()))?;

        // 2-layer peel — outer (run_concurrent) and middle (trappable) both
        // carry wasmtime::Error, mapped via the same classify_error closure.
        // The innermost plugin::WasmError is left as-is and remapped below
        // to the canonical WasmError variants.
        let result: Result<WasmExchange, crate::bindings::camel::plugin::types::WasmError> =
            crate::error::peel_concurrent(
                store
                    .as_context_mut()
                    .run_concurrent(async |accessor| plugin.call_process(accessor, exchange).await)
                    .await,
                |e| self.classify_error(e),
                |e| self.classify_error(e),
            )?;

        result.map_err(crate::error::map_plugin_error)
    }

    /// Process an [`Exchange`] through the WASM guest with streaming-body
    /// support and a no-progress watchdog.
    ///
    /// Unlike [`call_process`](Self::call_process), which takes a fully
    /// materialised [`WasmExchange`], this accepts a host [`Exchange`] and
    /// handles a `Body::Stream` input specially:
    ///
    /// 1. The byte stream is drained out of the `Arc<Mutex<Option<BoxStream>>>`
    ///    **before** `run_concurrent` — that mutex is `tokio::sync::Mutex`,
    ///    which cannot be locked from the concurrent runtime thread.
    /// 2. Inside `run_concurrent`, the stream is re-attached as a
    ///    guest-readable `stream<u8>` via
    ///    [`crate::stream_bridge::assemble_stream_body`].
    ///
    /// A **no-progress watchdog** wraps the invocation: if no stream chunk is
    /// shipped within `no_progress_timeout`, the call fails with a timeout.
    /// Progress is signalled by a [`Notify`] shared with
    /// [`crate::stream_bridge::BoxStreamProducer`], which pings it per shipped
    /// chunk. `cancel` is forwarded to the producer (host-side cancellation
    /// ends the stream promptly); `max_bytes` caps total bytes before an
    /// overflow error.
    ///
    /// On success returns the guest's [`WasmExchange`] (same shape as
    /// `call_process`) so callers can apply [`crate::serde_bridge::wasm_to_exchange`].
    #[allow(clippy::too_many_arguments)] // mirrors call_process + 3 streaming knobs
    pub async fn process_streaming_exchange(
        &self,
        registry: Arc<std::sync::Mutex<Registry>>,
        properties: HashMap<String, Value>,
        state_store: crate::state_store::StateStore,
        exchange: Exchange,
        cancel: CancellationToken,
        max_bytes: u64,
        no_progress_timeout: Duration,
    ) -> Result<WasmExchange, WasmError> {
        let host_state = Self::create_host_state(
            registry,
            properties,
            state_store,
            self.config.max_memory_bytes,
            crate::capabilities::WasmCapabilities::from_scheme_list(
                &self.config.allow_call_schemes,
            ),
        );
        let mut store = Store::new(&self.engine, host_state);
        store.limiter(|state| &mut state.limits);
        store.set_epoch_deadline(self.config.epoch_deadline());

        let plugin = Plugin::instantiate_async(&mut store, &self.component, &self.linker)
            .await
            .map_err(|e| WasmError::InstantiationFailed(e.to_string()))?;

        // Take the body out of the exchange so any stream can be extracted
        // before run_concurrent. Non-stream bodies are restored for the
        // closure to route through exchange_to_wasm (→ body_to_wasm), exactly
        // like call_process.
        let mut exchange = exchange;
        let taken_body = std::mem::replace(&mut exchange.input.body, Body::Empty);
        let mut stream_parts = match taken_body {
            Body::Stream(stream_body) => {
                let (stream, metadata) =
                    crate::stream_bridge::extract_stream_body(stream_body).await;
                Some((stream, metadata))
            }
            other => {
                exchange.input.body = other;
                None
            }
        };

        // Shared progress heartbeat: the producer pings it per shipped chunk;
        // the watchdog below consumes it to reset its no-progress window.
        let progress_notify = Arc::new(Notify::new());

        // Drive the guest under run_concurrent. The stream body is assembled
        // INSIDE the closure because assemble_stream_body needs the &Accessor
        // that only exists there.
        let processing = async {
            let run_result = store
                .as_context_mut()
                .run_concurrent(async |accessor| {
                    let wasm_exchange = if let Some((stream_opt, metadata)) = stream_parts.take() {
                        let body = match stream_opt {
                            Some(stream) => crate::stream_bridge::assemble_stream_body(
                                accessor,
                                stream,
                                &metadata,
                                cancel.clone(),
                                max_bytes,
                                progress_notify.clone(),
                            )?,
                            None => {
                                return Err(wasmtime::Error::msg(
                                    "wasm: stream body already consumed before guest invocation",
                                ));
                            }
                        };
                        crate::serde_bridge::exchange_to_wasm_with_body(&exchange, body)
                            .map_err(|e| wasmtime::Error::msg(e.to_string()))?
                    } else {
                        crate::serde_bridge::exchange_to_wasm(&exchange)
                            .map_err(|e| wasmtime::Error::msg(e.to_string()))?
                    };
                    plugin.call_process(accessor, wasm_exchange).await
                })
                .await;

            let peeled: Result<WasmExchange, crate::bindings::camel::plugin::types::WasmError> =
                crate::error::peel_concurrent(
                    run_result,
                    |e| self.classify_error(e),
                    |e| self.classify_error(e),
                )?;

            peeled.map_err(crate::error::map_plugin_error)
        };

        let result =
            WasmRuntime::drive_with_watchdog(processing, &progress_notify, no_progress_timeout)
                .await;
        if result.is_err() {
            cancel.cancel();
        }
        result
    }

    /// No-progress watchdog: drive `run_fut` to completion, but fail with a
    /// [`WasmError::GuestPanic`] timeout if `max_duration` elapses without
    /// `progress_notify` firing.
    ///
    /// Each notification resets the no-progress window. This guards against
    /// guests (or upstream producers) that stall indefinitely without ever
    /// producing another chunk — the epoch deadline alone does not cover that
    /// case, because the guest may be cooperatively awaiting the stream
    /// rather than burning cycles.
    ///
    /// On the stream path, [`crate::stream_bridge::BoxStreamProducer`] calls
    /// `notify_one()` per shipped chunk; the resulting wakeup re-arms the
    /// timer via the `continue` branch.
    async fn drive_with_watchdog<F, T>(
        run_fut: F,
        progress_notify: &Notify,
        max_duration: Duration,
    ) -> Result<T, WasmError>
    where
        F: Future<Output = Result<T, WasmError>>,
    {
        let mut run_fut = std::pin::pin!(run_fut);
        loop {
            tokio::select! {
                r = &mut run_fut => return r,
                _ = progress_notify.notified() => continue,
                _ = tokio::time::sleep(max_duration) => {
                    return Err(WasmError::GuestPanic(
                        "wasm: no-progress timeout (stream stalled)".into(),
                    ));
                }
            }
        }
    }

    pub fn module_path(&self) -> &Path {
        &self.module_path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wasm_host_state_creation() {
        let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
        let props = HashMap::new();
        let state = WasmHostState {
            table: ResourceTable::new(),
            wasi: WasiCtxBuilder::new().inherit_stderr().build(),
            properties: props,
            registry,
            call_depth: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            limits: wasmtime::StoreLimits::default(),
            state_store: crate::state_store::StateStore::new(),
            capabilities: crate::capabilities::WasmCapabilities::default(),
        };
        assert!(state.properties.is_empty());
        assert_eq!(
            state.call_depth.load(std::sync::atomic::Ordering::Relaxed),
            0
        );
    }

    #[test]
    fn create_host_state_with_zero_memory_falls_back_to_default() {
        // Defensive: passing 0 must not produce a StoreLimits that blocks all
        // memory growth — it should fall back to wasmtime's default.
        let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
        let host_state = WasmRuntime::create_host_state(
            registry,
            HashMap::new(),
            crate::state_store::StateStore::new(),
            0,
            crate::capabilities::WasmCapabilities::default(),
        );
        let _ = host_state; // smoke test: constructor tolerates 0
    }

    // Per-plugin-type coverage: this test runs at the shared `create_host_state`
    // layer, so its enforcement guarantee applies to every plugin type (Processor,
    // Bean, AuthorizationPolicy, SecurityPolicy) — they all call this function.
    #[tokio::test]
    async fn memory_growth_rejected_past_configured_cap() {
        // Behavioral test for acceptance criterion #4: max_memory_bytes is
        // *actually enforced*. Builds a tiny core-wasm module that exports a
        // `grow` function calling `memory.grow(64)` (requesting ~4 MiB) against
        // a host state with a 64 KiB cap. The grow call must return -1.
        //
        // We use a core (non-component) module here because the limiter is
        // applied at the wasmtime::Store level — the same store that components
        // use — so core wasm exercises the same enforcement path without the
        // boilerplate of synthesizing a full component.
        let wat = r#"
        (module
          (memory $mem (export "memory") 1)
          (func (export "grow") (param i32) (result i32)
            local.get 0
            memory.grow $mem)
        )
    "#;

        let config = wasmtime::Config::new();
        let engine = Engine::new(&config).unwrap();
        let module = wasmtime::Module::new(&engine, wat).expect("compile wat");

        let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
        let host_state = WasmRuntime::create_host_state(
            registry,
            HashMap::new(),
            crate::state_store::StateStore::new(),
            64 * 1024, // 64 KiB cap
            crate::capabilities::WasmCapabilities::default(),
        );
        let mut store = Store::new(&engine, host_state);
        store.limiter(|state| &mut state.limits);

        let instance = wasmtime::Instance::new_async(&mut store, &module, &[])
            .await
            .expect("instantiate");
        let grow = instance
            .get_typed_func::<i32, i32>(&mut store, "grow")
            .expect("get grow export");

        // memory.grow(64) requests 64 pages = 4 MiB, far above the 64 KiB cap.
        // The limiter must refuse it; memory.grow returns -1 on rejection.
        let result = grow.call_async(&mut store, 64).await.expect("grow call");
        assert_eq!(
            result, -1,
            "memory.grow must return -1 when the cap (64 KiB) would be exceeded"
        );
    }

    #[tokio::test]
    async fn memory_growth_allowed_under_cap() {
        // Companion to the above: a growth request that stays under the cap
        // must succeed. Guards against the limiter being accidentally
        // over-restrictive.
        let wat = r#"
        (module
          (memory $mem (export "memory") 1)
          (func (export "grow") (param i32) (result i32)
            local.get 0
            memory.grow $mem)
        )
    "#;

        let config = wasmtime::Config::new();
        let engine = Engine::new(&config).unwrap();
        let module = wasmtime::Module::new(&engine, wat).expect("compile wat");

        let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
        // Cap = 1 page initial + 1 page growable = 2 pages = 128 KiB.
        let host_state = WasmRuntime::create_host_state(
            registry,
            HashMap::new(),
            crate::state_store::StateStore::new(),
            128 * 1024,
            crate::capabilities::WasmCapabilities::default(),
        );
        let mut store = Store::new(&engine, host_state);
        store.limiter(|state| &mut state.limits);

        let instance = wasmtime::Instance::new_async(&mut store, &module, &[])
            .await
            .expect("instantiate");
        let grow = instance
            .get_typed_func::<i32, i32>(&mut store, "grow")
            .expect("get grow export");

        // memory.grow(1) requests 1 page = 64 KiB. With a 128 KiB cap and
        // 1 page initial, the growable budget is 64 KiB (one page). Must succeed
        // and return the previous page count (1, since initial is 1 page).
        let result = grow.call_async(&mut store, 1).await.expect("grow call");
        assert_eq!(result, 1, "memory.grow of 1 page under cap must succeed");
    }

    #[test]
    fn test_host_state_has_limits_field() {
        let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
        let state = WasmRuntime::create_host_state(
            registry,
            HashMap::new(),
            crate::state_store::StateStore::new(),
            0,
            crate::capabilities::WasmCapabilities::default(),
        );
        let _limits: &wasmtime::StoreLimits = &state.limits;
    }

    #[test]
    fn test_epoch_deadline_set_on_store() {
        let mut config = wasmtime::Config::new();
        config.epoch_interruption(true);
        config.wasm_component_model(true);
        let engine = Engine::new(&config).unwrap();
        let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
        let host_state = WasmRuntime::create_host_state(
            registry,
            HashMap::new(),
            crate::state_store::StateStore::new(),
            0,
            crate::capabilities::WasmCapabilities::default(),
        );
        let mut store = Store::new(&engine, host_state);
        store.set_epoch_deadline(500);
        // NOTE: wasmtime v31 does not expose `get_epoch_deadline()` on Store,
        // so we cannot assert the value was set. This test verifies the API
        // compiles and does not panic at runtime. The actual deadline enforcement
        // is validated indirectly by the epoch_interruption integration tests.
    }

    #[test]
    fn test_store_limiter_uses_host_state_limits() {
        let mut config = wasmtime::Config::new();
        config.epoch_interruption(true);
        config.wasm_component_model(true);
        let engine = Engine::new(&config).unwrap();
        let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
        let host_state = WasmRuntime::create_host_state(
            registry,
            HashMap::new(),
            crate::state_store::StateStore::new(),
            1024, // 1 KiB cap; threaded through create_host_state
            crate::capabilities::WasmCapabilities::default(),
        );
        let mut store = Store::new(&engine, host_state);
        store.limiter(|state| &mut state.limits);
        // Verifies store.limiter accepts WasmHostState::limits after the new
        // create_host_state wires the memory cap through StoreLimitsBuilder.
    }

    // ── Non-stream passthrough (M2) ────────────────────────────────────
    //
    // Drives exchange_to_wasm_with_body on the path that process_streaming_exchange
    // uses for non-stream bodies — proves the Body::Text path is equivalent
    // to what call_process would produce.

    #[test]
    fn test_exchange_to_wasm_with_body_text_passthrough() {
        let msg = camel_api::Message::new("hello-world");
        let exchange = camel_api::Exchange::new(msg);

        let wasm = crate::serde_bridge::exchange_to_wasm_with_body(
            &exchange,
            crate::bindings::camel::plugin::types::WasmBody::Text("hello-world".into()),
        )
        .expect("exchange_to_wasm_with_body must succeed");

        assert!(
            matches!(
                wasm.input.body,
                crate::bindings::camel::plugin::types::WasmBody::Text(ref s)
                if s == "hello-world"
            ),
            "non-stream passthrough must preserve Text body"
        );
    }

    // ── drive_with_watchdog (no-progress watchdog) ───────────────────────

    #[tokio::test]
    async fn watchdog_times_out_on_stalled_future() {
        // A future that never resolves and never reports progress must trip
        // the no-progress timeout.
        use std::future::pending;

        let notify = Arc::new(Notify::new());
        let result: Result<(), WasmError> =
            WasmRuntime::drive_with_watchdog(pending(), &notify, Duration::from_millis(50)).await;
        let err = result.expect_err("stalled future must time out");
        assert!(
            err.to_string().contains("no-progress"),
            "expected no-progress timeout, got: {err}"
        );
    }

    #[tokio::test]
    async fn watchdog_passes_through_immediate_completion() {
        // A future that resolves right away must return its value untouched,
        // even with a long watchdog window still pending.
        let notify = Arc::new(Notify::new());
        let result: Result<i32, WasmError> =
            WasmRuntime::drive_with_watchdog(async { Ok(42) }, &notify, Duration::from_secs(60))
                .await;
        assert_eq!(result.unwrap(), 42);
    }

    #[tokio::test]
    async fn watchdog_passes_through_guest_error() {
        // A future that resolves with an error must surface that error, not a
        // timeout — the watchdog only fires on NO progress.
        let notify = Arc::new(Notify::new());
        let result: Result<(), WasmError> = WasmRuntime::drive_with_watchdog(
            async { Err(WasmError::GuestPanic("boom".into())) },
            &notify,
            Duration::from_secs(60),
        )
        .await;
        let err = result.expect_err("guest error must propagate");
        assert!(err.to_string().contains("boom"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn watchdog_resets_on_progress_avoids_timeout() {
        // A slow future (longer than the watchdog window) that receives
        // periodic progress pings must NOT time out — each ping resets the
        // no-progress timer.
        let notify = Arc::new(Notify::new());
        let pinger_notify = notify.clone();

        // Pinger: fire 4 progress notifications 25ms apart, then stop. The
        // future completes at 120ms; without resets the 40ms watchdog would
        // fire at ~40ms.
        let pinger = tokio::spawn(async move {
            for _ in 0..4 {
                tokio::time::sleep(Duration::from_millis(25)).await;
                pinger_notify.notify_one();
            }
        });

        let result: Result<(), WasmError> = WasmRuntime::drive_with_watchdog(
            async {
                tokio::time::sleep(Duration::from_millis(120)).await;
                Ok(())
            },
            &notify,
            Duration::from_millis(40),
        )
        .await;
        pinger.await.expect("pinger joins");
        assert!(
            result.is_ok(),
            "progress pings should have reset the watchdog, got: {result:?}"
        );
    }

    #[tokio::test]
    async fn timeout_kills_infinite_loop_guest() {
        // Behavioral test for the timeout half of the safety net:
        // a guest that loops forever must be killed by epoch interruption
        // within a bounded time of the configured deadline.
        //
        // This test is the runtime-level proof of acceptance criterion #5
        // ("timeout_secs honoured end-to-end"). All plugin types (Processor,
        // Bean, AuthorizationPolicy, SecurityPolicy) share the same
        // `create_host_state` + `set_epoch_deadline` mechanism, so proving
        // it once at the runtime level covers every plugin type.
        //
        // Per-plugin-type coverage: this test runs at the shared
        // `create_host_state` + `set_epoch_deadline` layer, so its enforcement
        // guarantee applies to every plugin type (Processor, Bean,
        // AuthorizationPolicy, SecurityPolicy) — they all call `create_host_state`
        // and `store.set_epoch_deadline(...)`.
        let wat = r#"
        (module
          (func (export "loop_forever")
            loop
              br 0
            end
          )
        )
        "#;

        let mut config = wasmtime::Config::new();
        config.epoch_interruption(true);
        let engine = Engine::new(&config).unwrap();
        let module = wasmtime::Module::new(&engine, wat).expect("compile wat");

        let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
        let host_state = WasmRuntime::create_host_state(
            registry,
            HashMap::new(),
            crate::state_store::StateStore::new(),
            0, // no memory cap — this test is about timeout, not memory
            crate::capabilities::WasmCapabilities::default(),
        );
        let mut store = Store::new(&engine, host_state);
        // Very short deadline: 1 epoch tick. With a 10ms tick interval, this
        // means the call must be interrupted within ~20ms (one tick + deadline).
        store.set_epoch_deadline(1);

        // Spawn the epoch ticker on a dedicated OS thread. This mirrors the
        // production EpochTicker::start wiring (also a dedicated OS thread,
        // see epoch.rs) — a tokio::spawn ticker would be queued behind
        // call_async on a single-worker runtime and never get polled, so the
        // epoch deadline would never fire. A std::thread is scheduled by the
        // kernel and increments the epoch regardless of tokio's cooperation.
        // The shutdown flag lets us stop the thread as soon as the assertion
        // succeeds, instead of letting it run for the full ~2s budget.
        use std::sync::atomic::{AtomicBool, Ordering};
        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_clone = shutdown.clone();
        let engine_clone = engine.clone();
        let ticker = std::thread::spawn(move || {
            while !shutdown_clone.load(Ordering::SeqCst) {
                std::thread::sleep(std::time::Duration::from_millis(10));
                engine_clone.increment_epoch();
            }
        });

        let instance = wasmtime::Instance::new_async(&mut store, &module, &[])
            .await
            .expect("instantiate");
        let func = instance
            .get_typed_func::<(), ()>(&mut store, "loop_forever")
            .expect("get loop_forever export");

        let start = std::time::Instant::now();
        let result = func.call_async(&mut store, ()).await;
        let elapsed = start.elapsed();

        // Stop the ticker thread before asserting — keeps the test tidy and
        // prevents the thread from outliving the test by ~2 seconds.
        shutdown.store(true, Ordering::SeqCst);
        ticker.join().expect("ticker thread to exit cleanly");

        assert!(
            result.is_err(),
            "infinite loop must be killed by epoch interruption"
        );
        // Loose upper bound: must interrupt within 2 seconds even on slow CI.
        // A typical run is ~20ms.
        assert!(
            elapsed < std::time::Duration::from_secs(2),
            "timeout must trigger quickly, took {:?}",
            elapsed
        );
    }
}
