use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;
use wasmtime::AsContextMut;

use camel_api::{Body, CamelError, Exchange};
use camel_bean::BeanProcessor;
use camel_core::Registry;

use crate::bean_bindings::Bean as BeanGuest;
use crate::error::WasmError;
use crate::serde_bridge;
use crate::wasm_plugin_context::WasmPluginContext;

pub struct WasmBean {
    ctx: WasmPluginContext,
    methods: Vec<String>,
    /// Per-stream byte cap forwarded to the streaming host bridge.
    max_bytes: u64,
    /// No-progress watchdog window forwarded to the streaming host bridge.
    no_progress_timeout: Duration,
    /// Drain-completion lifecycle hook: fires when the streaming drain task
    /// ends (Store freed). Useful for metrics, readiness probes, and test
    /// assertions about cancel-on-drop / drain timing.
    drain_completion_notify: Option<Arc<tokio::sync::Notify>>,
    /// Root cancellation token for caller-drop cancellation. A child token
    /// is derived per `call` and wrapped in an `InvokeCancelGuard` so that
    /// dropping the caller's future cancels the in-flight guest invocation.
    cancel: CancellationToken,
}

impl WasmBean {
    pub async fn new(
        module_path: impl AsRef<Path>,
        wasm_config: crate::config::WasmConfig,
        registry: Arc<std::sync::Mutex<Registry>>,
        bean_config: HashMap<String, String>,
    ) -> Result<Self, WasmError> {
        let max_stream_bytes = wasm_config.max_stream_bytes;
        let (ctx, methods) =
            WasmPluginContext::new_bean(module_path, wasm_config, registry, bean_config).await?;
        Ok(Self {
            ctx,
            methods,
            max_bytes: max_stream_bytes,
            no_progress_timeout: crate::producer::DEFAULT_NO_PROGRESS_TIMEOUT,
            drain_completion_notify: None,
            cancel: CancellationToken::new(),
        })
    }

    /// Override the streaming-body knobs (per-stream byte cap + no-progress
    /// watchdog window). Mirrors `WasmProducer`'s tuneable fields; defaults
    /// come from [`crate::producer`] and apply when this is never called.
    pub fn with_streaming_knobs(mut self, max_bytes: u64, no_progress_timeout: Duration) -> Self {
        self.max_bytes = max_bytes;
        self.no_progress_timeout = no_progress_timeout;
        self
    }

    /// Install a drain-completion lifecycle hook: the supplied `Notify` fires
    /// once when the streaming drain task ends (Store freed). Production uses
    /// include metrics (drain latency histograms), readiness probes (await
    /// drain before shutdown), and integration tests (assert cancel-on-drop
    /// timing deterministically, without sleep).
    pub fn with_drain_completion_notify(mut self, notify: Arc<tokio::sync::Notify>) -> Self {
        self.drain_completion_notify = Some(notify);
        self
    }
}

#[async_trait]
impl BeanProcessor for WasmBean {
    async fn call(&self, method: &str, exchange: &mut Exchange) -> Result<(), CamelError> {
        if !self.methods.iter().any(|m| m == method) {
            return Err(CamelError::ProcessorError(format!(
                "unknown bean method: {method}"
            )));
        }

        let mut store = self.ctx.create_store(exchange.properties.clone());

        let plugin =
            BeanGuest::instantiate_async(&mut store, &self.ctx.component, &self.ctx.linker)
                .await
                .map_err(|e| WasmError::InstantiationFailed(e.to_string()))?;

        let method_owned = method.to_string();

        // Route on body shape, exactly like WasmProducer: a `Body::Stream`
        // input must cross the WASM boundary via the streaming host bridge
        // (run_concurrent + BoxStreamProducer) because the guest reads bytes
        // asynchronously; anything else stays on the materialised
        // exchange_to_wasm path. Before this branch existed, a streaming
        // body reached exchange_to_wasm and was rejected (rc-2lge).
        //
        // NEW-E (Task 7): the cross-binding `From` macro treats `Stream` as
        // unreachable and SILENTLY DROPS it. So we must extract the output
        // stream from the bean `WasmExchange` BEFORE the `.into()` cross-binding
        // conversion. The drain runs via `spawn_return_drain` (shared helper).
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

        // Convert to bean WasmExchange (owned, can move into spawn).
        // For stream inputs, the body is assembled inside run_concurrent
        // (needs the &Accessor); for non-stream inputs, convert now.
        let bean_exchange_in: crate::bean_bindings::camel::plugin::types::WasmExchange =
            crate::serde_bridge::exchange_to_wasm(exchange)
                .map_err(|e| WasmError::TypeConversion(e.to_string()))?
                .into();

        let classify_config = self.ctx.config.clone();
        let classify_module_path = self.ctx.module_path.clone();
        let method_for_drain = method_owned.clone();
        let max_bytes = self.max_bytes;
        let no_progress_timeout = self.no_progress_timeout;
        let drain_completion_notify = self.drain_completion_notify.clone();

        // Use the shared spawn_return_drain helper. Bean has no concurrency
        // limiter, so pass None for the permit (unlike the plugin path).
        // The closure builds the drive future that runs the guest invocation
        // + output stream drain.
        let cancel = self.cancel.child_token();
        let mut guard = crate::cancel_guard::InvokeCancelGuard::new(cancel.clone());
        let (bean_exchange_out, drain_rx, metadata_from_drain) = crate::return_stream::spawn_return_drain(
            None,
            cancel,
            no_progress_timeout,
            drain_completion_notify,
            // Bean make_drive closure — mirrors runtime.rs process_streaming_exchange
            // (keep in sync; the shared spawn_return_drain scaffold is identical,
            // only the binding + input/output field differ).
            move |handoff_shared, dtx, drx, drain_started, coord| async move {
                let handoff_drive = handoff_shared.clone();

                // ONE run_concurrent block: branch on stream_parts inside to
                // assemble the input body (the only real difference between
                // streaming and non-streaming input paths).
                let result: Result<(), WasmError> = async {
                    let method_for_peel = method_for_drain.clone();
                    let nested = store
                        .as_context_mut()
                        .run_concurrent(async |accessor| {
                            // Assemble the input body: streaming input gets the
                            // stream body assembled here; non-streaming uses the
                            // materialised exchange.
                            let bean_exchange = if let Some((stream_opt, metadata)) =
                                stream_parts.take()
                            {
                                let mut wx = bean_exchange_in;
                                wx.input.body = match stream_opt {
                                    Some(stream) => {
                                        crate::stream_bridge::assemble_stream_body_bean(
                                            accessor,
                                            stream,
                                            &metadata,
                                            coord.cancel.clone(),
                                            max_bytes,
                                            coord.progress.clone(),
                                        )?
                                    }
                                    None => {
                                        return Err(wasmtime::Error::msg(
                                            "wasm: stream body already consumed before bean invocation",
                                        ));
                                    }
                                };
                                wx
                            } else {
                                bean_exchange_in
                            };

                            let bean_result = plugin
                                .call_invoke(accessor, method_for_drain, bean_exchange)
                                .await?;
                            let mut bean_exchange = match bean_result {
                                Ok(exchange) => exchange,
                                Err(e) => {
                                    return Err(wasmtime::Error::msg(format!("{e}")));
                                }
                            };

                            // NEW-E: extract the output stream BEFORE cross-binding
                            use crate::return_stream::StreamReturnable;
                            match bean_exchange.take_stream() {
                                Some((reader, terminal_future, guest_metadata)) => {
                                    drain_started.notify_one();
                                    if let Some(tx) =
                                        crate::return_stream::take_stream_handoff_sender(
                                            &handoff_drive,
                                        )
                                    {
                                        let _ = tx.send(Ok((bean_exchange, Some(crate::return_stream::DrainReceiver { rx: drx, terminal: coord.terminal_slot.clone() }), guest_metadata)));
                                    }
                                    // Cancel-on-drop select! (F2, spec §5)
                                    tokio::select! {
                                        _ = crate::return_stream::drain_guest_stream(
                                            accessor, reader, terminal_future, dtx,
                                            coord.clone(),
                                        ) => {}
                                        _ = coord.receiver_gone.notified() => { coord.cancel.cancel(); }
                                    }
                                }
                                None => {
                                    if let Some(tx) =
                                        crate::return_stream::take_stream_handoff_sender(
                                            &handoff_drive,
                                        )
                                    {
                                        let _ = tx.send(Ok((bean_exchange, None, camel_api::StreamMetadata::default())));
                                    }
                                    drop(drx);
                                    drop(dtx);
                                }
                            }
                            Ok(())
                        })
                        .await;

                    crate::error::peel_concurrent(
                        nested,
                        |e| {
                            crate::config::classify_error(
                                &classify_config,
                                &classify_module_path,
                                e,
                            )
                        },
                        |e| WasmError::GuestPanic(format!("bean method '{method_for_peel}' trapped: {e}")),
                    )
                }
                .await;

                // If the inner async returned an error AND we haven't sent through handoff yet,
                // send the error now.
                match &result {
                    Ok(()) => {}
                    Err(e) => {
                        if let Some(tx) =
                            crate::return_stream::take_stream_handoff_sender(&handoff_shared)
                        {
                            let _ = tx.send(Err(e.clone()));
                        }
                    }
                }
                result
            },
        )
        .await?;

        // Cross-bind the (now Empty-body) exchange
        let wasm_result =
            crate::bindings::camel::plugin::types::WasmExchange::from(bean_exchange_out);
        serde_bridge::wasm_to_exchange(wasm_result, exchange);

        // Reattach Body::Stream if present. Bean convention: the guest
        // transforms input.body in place, so the drained stream goes back
        // to input.body (not output.body like the plugin path).
        if let Some(drain_rx) = drain_rx {
            exchange.input.body = Body::Stream(camel_api::StreamBody {
                stream: Arc::new(tokio::sync::Mutex::new(Some(
                    crate::return_stream::receiver_to_body_stream(drain_rx),
                ))),
                metadata: metadata_from_drain,
            });
        }

        guard.complete();
        Ok(())
    }

    fn methods(&self) -> Vec<String> {
        self.methods.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Note: WasmBean::new requires a real WASM file. The WasmConfig propagation
    // chain (limits → from_limits → WasmConfig → create_host_state) is tested at
    // the runtime layer (memory_growth_*, timeout_kills_infinite_loop_guest).
    // All plugin types share that runtime layer, so coverage is implicit.

    #[test]
    fn test_wasm_bean_host_state_creation() {
        let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
        let host_state = crate::runtime::WasmRuntime::create_host_state(
            registry,
            HashMap::new(),
            crate::state_store::StateStore::new(),
            0,
            crate::capabilities::WasmCapabilities::default(),
        );
        assert!(host_state.properties.is_empty());
    }

    #[test]
    fn test_config_vec_conversion() {
        let config = HashMap::from([
            ("key1".to_string(), "val1".to_string()),
            ("key2".to_string(), "val2".to_string()),
        ]);
        let pairs: Vec<(String, String)> = config.into_iter().collect();
        assert_eq!(pairs.len(), 2);
    }

    #[test]
    fn test_empty_config_vec() {
        let config: HashMap<String, String> = HashMap::new();
        let pairs: Vec<(String, String)> = config.into_iter().collect();
        assert!(pairs.is_empty());
    }
}
