use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
use wasmtime::AsContextMut;
use wasmtime::Store;

use camel_api::{Body, CamelError, Exchange};
use camel_bean::BeanProcessor;
use camel_core::Registry;

use crate::bean_bindings::Bean as BeanGuest;
use crate::error::WasmError;
use crate::serde_bridge;
use crate::wasm_plugin_context::WasmPluginContext;

/// Result of driving a bean `invoke` through the streaming host bridge:
/// the outer layer is a host-side [`WasmError`] (instantiation, peel,
/// watchdog); the inner is the guest's own WIT `result<exchange, wasm-error>`,
/// matched by the caller to produce a [`CamelError`].
type BeanStreamingResult = Result<
    Result<
        crate::bean_bindings::camel::plugin::types::WasmExchange,
        crate::bean_bindings::camel::plugin::types::WasmError,
    >,
    WasmError,
>;

pub struct WasmBean {
    ctx: WasmPluginContext,
    methods: Vec<String>,
    /// Per-stream byte cap forwarded to the streaming host bridge.
    max_bytes: u64,
    /// No-progress watchdog window forwarded to the streaming host bridge.
    no_progress_timeout: Duration,
}

impl WasmBean {
    pub async fn new(
        module_path: impl AsRef<Path>,
        wasm_config: crate::config::WasmConfig,
        registry: Arc<std::sync::Mutex<Registry>>,
        bean_config: HashMap<String, String>,
    ) -> Result<Self, WasmError> {
        let (ctx, methods) =
            WasmPluginContext::new_bean(module_path, wasm_config, registry, bean_config).await?;
        Ok(Self {
            ctx,
            methods,
            max_bytes: crate::producer::DEFAULT_STREAM_MAX_BYTES,
            no_progress_timeout: crate::producer::DEFAULT_NO_PROGRESS_TIMEOUT,
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
        let result: Result<
            crate::bean_bindings::camel::plugin::types::WasmExchange,
            crate::bean_bindings::camel::plugin::types::WasmError,
        > = if crate::producer::is_streaming(&exchange.input.body) {
            self.invoke_streaming(&mut store, &plugin, method_owned, exchange)
                .await?
        } else {
            let wasm_exchange = serde_bridge::exchange_to_wasm(exchange)?;
            let bean_exchange = wasm_exchange.into();
            // 2-layer peel (see error::peel_concurrent). The outer and middle
            // wasmtime::Error layers are mapped to WasmError uniformly; the
            // innermost bean::WasmError is matched below to produce a
            // CamelError.
            crate::error::peel_concurrent(
                store
                    .as_context_mut()
                    .run_concurrent(async |accessor| {
                        plugin
                            .call_invoke(accessor, method_owned, bean_exchange)
                            .await
                    })
                    .await,
                |e| self.ctx.classify_error(e),
                |e| WasmError::GuestPanic(format!("bean method '{method}' trapped: {e}")),
            )?
        };

        match result {
            Ok(bean_result) => {
                let wasm_result =
                    crate::bindings::camel::plugin::types::WasmExchange::from(bean_result);
                serde_bridge::wasm_to_exchange(wasm_result, exchange);
                Ok(())
            }
            Err(wasm_err) => Err(WasmError::GuestPanic(format!(
                "bean method '{method}' failed: {wasm_err:?}"
            ))
            .into()),
        }
    }

    fn methods(&self) -> Vec<String> {
        self.methods.clone()
    }
}

impl WasmBean {
    /// Drive a bean `invoke` whose input body is a `Body::Stream` through the
    /// streaming host bridge.
    ///
    /// Structure follows [`crate::runtime::WasmRuntime::process_streaming_exchange`]:
    /// the byte stream is drained out of its `Arc<tokio::sync::Mutex<..>>`
    /// **before** `run_concurrent` (that mutex cannot lock from the
    /// concurrent-runtime thread), then re-attached as a guest-readable
    /// `stream<u8>` via [`crate::stream_bridge::assemble_stream_body_bean`]
    /// **inside** the closure (it needs the `&Accessor`). A no-progress
    /// watchdog cancels a stalled guest. The error peel mirrors the bean's
    /// own non-stream branch (`classify_error` + `GuestPanic`), not the
    /// producer's `map_plugin_error`.
    #[allow(clippy::too_many_arguments)]
    async fn invoke_streaming(
        &self,
        store: &mut Store<crate::runtime::WasmHostState>,
        plugin: &BeanGuest,
        method: String,
        exchange: &mut Exchange,
    ) -> BeanStreamingResult {
        // Take the body out so the stream can be extracted before
        // run_concurrent. The caller gates on is_streaming, but defend
        // anyway: restore the body and fail rather than abort the host.
        let taken_body = std::mem::replace(&mut exchange.input.body, Body::Empty);
        let stream_body = match taken_body {
            Body::Stream(sb) => sb,
            other => {
                exchange.input.body = other;
                return Err(WasmError::GuestPanic(
                    "invoke_streaming called with non-stream body".into(),
                ));
            }
        };
        let (stream, metadata) = crate::stream_bridge::extract_stream_body(stream_body).await;
        let mut stream_parts = Some((stream, metadata));

        let progress_notify = Arc::new(Notify::new());
        let cancel = CancellationToken::new();
        let max_bytes = self.max_bytes;
        let no_progress_timeout = self.no_progress_timeout;

        let processing = async {
            let run_result = store
                .as_context_mut()
                .run_concurrent(async |accessor| {
                    let (stream_opt, metadata) = stream_parts
                        .take()
                        .expect("stream_parts consumed once");
                    // Build the bean exchange with body=Empty via the
                    // cross-binding `From` (safe for non-stream variants),
                    // then attach the stream handle directly in the
                    // `bean_bindings` namespace. The cross-binding `From`
                    // cannot carry a `Stream` variant (the live
                    // StreamReader/FutureReader handle is not re-wrappable),
                    // so assemble_stream_body_bean builds it natively as a
                    // bean_bindings::WasmBody.
                    let mut bean_exchange: crate::bean_bindings::camel::plugin::types::WasmExchange =
                        crate::serde_bridge::exchange_to_wasm(exchange)
                            .map_err(|e| wasmtime::Error::msg(e.to_string()))?
                            .into();
                    bean_exchange.input.body = match stream_opt {
                        Some(stream) => crate::stream_bridge::assemble_stream_body_bean(
                            accessor,
                            stream,
                            &metadata,
                            cancel.clone(),
                            max_bytes,
                            progress_notify.clone(),
                        )?,
                        None => {
                            return Err(wasmtime::Error::msg(
                                "wasm: stream body already consumed before bean invocation",
                            ));
                        }
                    };
                    plugin.call_invoke(accessor, method, bean_exchange).await
                })
                .await;

            crate::error::peel_concurrent(
                run_result,
                |e| self.ctx.classify_error(e),
                |e| WasmError::GuestPanic(format!("bean method trapped: {e}")),
            )
        };

        let result = crate::runtime::WasmRuntime::drive_with_watchdog(
            processing,
            &progress_notify,
            no_progress_timeout,
        )
        .await;
        // Mirror the producer: on timeout/guest-trap, signal the
        // BoxStreamProducer to release the upstream stream promptly
        // (drive_with_watchdog only races the future; it does not cancel it).
        if result.is_err() {
            cancel.cancel();
        }
        result
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
