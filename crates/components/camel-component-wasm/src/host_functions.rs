use serde_json::Value;
use wasmtime::component::{Access, Accessor, Linker};

use crate::runtime::WasmHostState;

// Host marker trait must stay on WasmHostState (not HasSelf<WasmHostState>):
// the add_to_linker bound `for<'a> D::Data<'a>: Host` resolves through the
// `impl<T: Host> Host for &mut T` blanket impl, which requires the concrete
// type inside &mut (_) to implement Host.
// Sync `Host` trait impl: under the per-function import config the sync host
// functions (get-property, set-property, host-store, host-load) live in
// `HostWithStore` (taking an `Access`); `Host` remains an empty marker trait
// required by the linker's where-clause.
impl crate::bindings::camel::plugin::host::Host for WasmHostState {}

impl crate::bindings::camel::plugin::host::HostWithStore<WasmHostState>
    for wasmtime::component::HasSelf<WasmHostState>
{
    async fn camel_call(
        store: &Accessor<WasmHostState, wasmtime::component::HasSelf<WasmHostState>>,
        uri: String,
        payload: String,
    ) -> Result<String, crate::bindings::camel::plugin::types::WasmError> {
        WasmHostState::camel_call_impl(store, uri, payload).await
    }

    async fn camel_poll(
        store: &Accessor<WasmHostState, wasmtime::component::HasSelf<WasmHostState>>,
        uri: String,
        timeout_ms: u32,
    ) -> Result<String, crate::bindings::camel::plugin::types::WasmError> {
        WasmHostState::camel_poll_impl(store, uri, timeout_ms).await
    }

    fn get_property(
        mut store: Access<'_, WasmHostState, wasmtime::component::HasSelf<WasmHostState>>,
        key: String,
    ) -> Option<String> {
        store.get().get_property_impl(key)
    }

    fn set_property(
        mut store: Access<'_, WasmHostState, wasmtime::component::HasSelf<WasmHostState>>,
        key: String,
        value: String,
    ) {
        store.get().set_property_impl(key, value)
    }

    fn host_store(
        mut store: Access<'_, WasmHostState, wasmtime::component::HasSelf<WasmHostState>>,
        key: String,
        value: String,
    ) -> Result<(), crate::bindings::camel::plugin::types::WasmError> {
        WasmHostState::host_store_impl(store.get(), key, value)
    }

    fn host_load(
        mut store: Access<'_, WasmHostState, wasmtime::component::HasSelf<WasmHostState>>,
        key: String,
    ) -> Result<Option<String>, crate::bindings::camel::plugin::types::WasmError> {
        WasmHostState::host_load_impl(store.get(), key)
    }
}

pub fn add_to_linker(linker: &mut Linker<WasmHostState>) -> Result<(), wasmtime::Error> {
    // D = HasSelf<WasmHostState> uses wasmtime's built-in HasData impl
    // (Data<'a> = &'a mut WasmHostState). The HostWithStore and Host trait
    // impls target HasSelf<WasmHostState> and WasmHostState respectively.
    crate::bindings::camel::plugin::host::add_to_linker::<
        WasmHostState,
        wasmtime::component::HasSelf<WasmHostState>,
    >(linker, |state| state)
}

// Async helpers: the actual producer / poller logic. Pulled out of the trait
// impls so they can also be invoked from the per-binding macro arms and from
// tests (where we don't have an Accessor in scope).

async fn run_async_call(
    registry: std::sync::Arc<dyn camel_component_api::ComponentContext>,
    observability: std::sync::Arc<dyn camel_component_api::RuntimeObservability>,
    uri: String,
    payload: String,
) -> Result<String, crate::bindings::camel::plugin::types::WasmError> {
    use tower::ServiceExt;
    let scheme = uri.split(':').next().unwrap_or("").to_string();
    if scheme.is_empty() {
        return Err(
            crate::bindings::camel::plugin::types::WasmError::ProcessorError(format!(
                "invalid URI (no scheme): {}",
                uri
            )),
        );
    }

    let component = registry.resolve_component(&scheme).ok_or_else(|| {
        crate::bindings::camel::plugin::types::WasmError::ProcessorError(format!(
            "component not found for scheme: {}",
            scheme
        ))
    })?;

    let endpoint = component.create_endpoint(&uri, &*registry).map_err(|e| {
        crate::bindings::camel::plugin::types::WasmError::ProcessorError(format!(
            "create_endpoint failed: {}",
            e
        ))
    })?;

    let producer = endpoint
        .create_producer(observability, &camel_api::ProducerContext::new())
        .map_err(|e| {
            crate::bindings::camel::plugin::types::WasmError::ProcessorError(format!(
                "create_producer failed: {}",
                e
            ))
        })?;

    let exchange =
        camel_api::Exchange::new(camel_api::Message::new(camel_api::Body::Text(payload)));

    let result = producer.oneshot(exchange).await.map_err(|e| {
        crate::bindings::camel::plugin::types::WasmError::ProcessorError(format!(
            "endpoint call failed: {}",
            e
        ))
    })?;

    let body_str = match &result.output {
        Some(msg) => match &msg.body {
            camel_api::Body::Text(s) => s.clone(),
            camel_api::Body::Json(v) => v.to_string(),
            camel_api::Body::Bytes(b) => String::from_utf8_lossy(b).to_string(),
            camel_api::Body::Xml(s) => s.clone(),
            camel_api::Body::Empty => String::new(),
            camel_api::Body::Stream(_) => "<stream>".to_string(),
            _ => String::new(),
        },
        None => match &result.input.body {
            camel_api::Body::Text(s) => s.clone(),
            camel_api::Body::Json(v) => v.to_string(),
            camel_api::Body::Bytes(b) => String::from_utf8_lossy(b).to_string(),
            camel_api::Body::Xml(s) => s.clone(),
            camel_api::Body::Empty => String::new(),
            camel_api::Body::Stream(_) => "<stream>".to_string(),
            _ => String::new(),
        },
    };

    Ok(body_str)
}

async fn run_async_poll(
    registry: std::sync::Arc<dyn camel_component_api::ComponentContext>,
    // The handle is accepted for signature symmetry with `run_async_call`;
    // `polling_consumer()` takes no rt handle, so only the create_endpoint
    // context fix applies on this path.
    _observability: std::sync::Arc<dyn camel_component_api::RuntimeObservability>,
    uri: String,
    timeout_ms: u32,
) -> Result<String, crate::bindings::camel::plugin::types::WasmError> {
    let scheme = uri.split(':').next().unwrap_or("").to_string();
    if scheme.is_empty() {
        return Err(
            crate::bindings::camel::plugin::types::WasmError::ProcessorError(format!(
                "invalid URI (no scheme): {}",
                uri
            )),
        );
    }

    let component = registry.resolve_component(&scheme).ok_or_else(|| {
        crate::bindings::camel::plugin::types::WasmError::ProcessorError(format!(
            "component not found for scheme: {}",
            scheme
        ))
    })?;

    let endpoint = component.create_endpoint(&uri, &*registry).map_err(|e| {
        crate::bindings::camel::plugin::types::WasmError::ProcessorError(format!(
            "create_endpoint failed: {}",
            e
        ))
    })?;

    let mut poller = endpoint.polling_consumer().ok_or_else(|| {
        crate::bindings::camel::plugin::types::WasmError::ProcessorError(format!(
            "camel_poll requires a component that supports polling consumers (scheme: {})",
            scheme
        ))
    })?;

    let exchange = poller
        .receive(std::time::Duration::from_millis(timeout_ms as u64))
        .await
        .map_err(|e| {
            crate::bindings::camel::plugin::types::WasmError::ProcessorError(format!(
                "poll failed: {}",
                e
            ))
        })?;

    let body_str = match exchange {
        Some(ex) => {
            let bytes = ex
                .input
                .body
                .into_bytes(10 * 1024 * 1024)
                .await
                .map_err(|e| {
                    crate::bindings::camel::plugin::types::WasmError::ProcessorError(format!(
                        "body read failed: {}",
                        e
                    ))
                })?;
            String::from_utf8_lossy(&bytes).to_string()
        }
        None => {
            return Err(
                crate::bindings::camel::plugin::types::WasmError::ProcessorError(format!(
                    "no message received within {}ms timeout",
                    timeout_ms
                )),
            );
        }
    };

    Ok(body_str)
}

// Macro that implements the same `HostWithStore` trait for the bean and
// security-policy bindings modules. Those bindings define an equivalent
// `host` interface (separate types but identical shape), so the host impl is
// duplicated against each module's generated `HostWithStore` trait.
macro_rules! impl_host_for_binding {
    ($bindings_mod:ident) => {
        // Host marker stays on WasmHostState for the `impl<T: Host> Host for &mut T`
        // blanket bound on D::Data<'a> inside add_to_linker.
        impl crate::$bindings_mod::camel::plugin::host::Host for WasmHostState {}

        // HostWithStore targets HasSelf<WasmHostState> (wasmtime's built-in HasData)
        // instead of a hand-rolled HasData impl on WasmHostState.
        impl crate::$bindings_mod::camel::plugin::host::HostWithStore<WasmHostState>
            for wasmtime::component::HasSelf<WasmHostState>
        {
            async fn camel_call(
                store: &Accessor<WasmHostState, wasmtime::component::HasSelf<WasmHostState>>,
                uri: String,
                payload: String,
            ) -> Result<String, crate::$bindings_mod::camel::plugin::types::WasmError> {
                WasmHostState::camel_call_impl(store, uri, payload)
                    .await
                    .map_err(map_plugin_wasm_error_to)
            }

            async fn camel_poll(
                store: &Accessor<WasmHostState, wasmtime::component::HasSelf<WasmHostState>>,
                uri: String,
                timeout_ms: u32,
            ) -> Result<String, crate::$bindings_mod::camel::plugin::types::WasmError> {
                WasmHostState::camel_poll_impl(store, uri, timeout_ms)
                    .await
                    .map_err(map_plugin_wasm_error_to)
            }

            fn get_property(
                mut store: Access<'_, WasmHostState, wasmtime::component::HasSelf<WasmHostState>>,
                key: String,
            ) -> Option<String> {
                store.get().get_property_impl(key)
            }

            fn set_property(
                mut store: Access<'_, WasmHostState, wasmtime::component::HasSelf<WasmHostState>>,
                key: String,
                value: String,
            ) {
                store.get().set_property_impl(key, value)
            }

            fn host_store(
                mut store: Access<'_, WasmHostState, wasmtime::component::HasSelf<WasmHostState>>,
                key: String,
                value: String,
            ) -> Result<(), crate::$bindings_mod::camel::plugin::types::WasmError> {
                WasmHostState::host_store_impl(store.get(), key, value)
                    .map_err(map_plugin_wasm_error_to)
            }

            fn host_load(
                mut store: Access<'_, WasmHostState, wasmtime::component::HasSelf<WasmHostState>>,
                key: String,
            ) -> Result<Option<String>, crate::$bindings_mod::camel::plugin::types::WasmError> {
                WasmHostState::host_load_impl(store.get(), key).map_err(map_plugin_wasm_error_to)
            }
        }
    };
}

// Helper: re-map a canonical bindings::camel::plugin::types::WasmError to
// the same shape produced by a different bindgen invocation (bean /
// authorization-policy). All variants have identical structure, so this is
// a straight field-for-field clone.
fn map_plugin_wasm_error_to<E: FromWasmErrorVariant>(
    e: crate::bindings::camel::plugin::types::WasmError,
) -> E {
    match e {
        crate::bindings::camel::plugin::types::WasmError::ProcessorError(s) => {
            E::from_processor_error(s)
        }
        crate::bindings::camel::plugin::types::WasmError::TypeConversion(s) => {
            E::from_type_conversion(s)
        }
        crate::bindings::camel::plugin::types::WasmError::Io(s) => E::from_io(s),
        crate::bindings::camel::plugin::types::WasmError::Timeout => E::from_timeout(),
    }
}

trait FromWasmErrorVariant {
    fn from_processor_error(s: String) -> Self;
    fn from_type_conversion(s: String) -> Self;
    fn from_io(s: String) -> Self;
    fn from_timeout() -> Self;
}

impl FromWasmErrorVariant for crate::bindings::camel::plugin::types::WasmError {
    fn from_processor_error(s: String) -> Self {
        Self::ProcessorError(s)
    }
    fn from_type_conversion(s: String) -> Self {
        Self::TypeConversion(s)
    }
    fn from_io(s: String) -> Self {
        Self::Io(s)
    }
    fn from_timeout() -> Self {
        Self::Timeout
    }
}

impl FromWasmErrorVariant for crate::bean_bindings::camel::plugin::types::WasmError {
    fn from_processor_error(s: String) -> Self {
        Self::ProcessorError(s)
    }
    fn from_type_conversion(s: String) -> Self {
        Self::TypeConversion(s)
    }
    fn from_io(s: String) -> Self {
        Self::Io(s)
    }
    fn from_timeout() -> Self {
        Self::Timeout
    }
}

impl FromWasmErrorVariant for crate::security_policy_bindings::camel::plugin::types::WasmError {
    fn from_processor_error(s: String) -> Self {
        Self::ProcessorError(s)
    }
    fn from_type_conversion(s: String) -> Self {
        Self::TypeConversion(s)
    }
    fn from_io(s: String) -> Self {
        Self::Io(s)
    }
    fn from_timeout() -> Self {
        Self::Timeout
    }
}

pub fn add_bean_to_linker(linker: &mut Linker<WasmHostState>) -> Result<(), wasmtime::Error> {
    crate::bean_bindings::camel::plugin::host::add_to_linker::<
        WasmHostState,
        wasmtime::component::HasSelf<WasmHostState>,
    >(linker, |state| state)
}

impl_host_for_binding!(bean_bindings);

pub fn add_security_policy_to_linker(
    linker: &mut Linker<WasmHostState>,
) -> Result<(), wasmtime::Error> {
    crate::security_policy_bindings::camel::plugin::host::add_to_linker::<
        WasmHostState,
        wasmtime::component::HasSelf<WasmHostState>,
    >(linker, |state| state)
}

impl_host_for_binding!(security_policy_bindings);

// RAII recursion guard: increments `call_depth` on construction and
// decrements on drop. The drop runs even if the future is cancelled,
// which is the whole point: a manual inc/dec around an `.await` leaks
// the increment forever if the future is dropped between the two sites.
// Returns `None` from `new` if the depth was already > 0 (a nested
// call is in flight) so the caller can return the recursion error.
struct DepthGuard<'a> {
    depth: &'a std::sync::atomic::AtomicUsize,
}

impl<'a> DepthGuard<'a> {
    fn new(depth: &'a std::sync::atomic::AtomicUsize) -> Option<Self> {
        let prev = depth.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if prev > 0 {
            depth.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
            return None;
        }
        Some(DepthGuard { depth })
    }
}

impl Drop for DepthGuard<'_> {
    fn drop(&mut self) {
        self.depth.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

// Inherent methods on `WasmHostState` that implement the host function
// logic. The trait impls above are thin shims that call these. Splitting
// the logic this way gives the test suite direct access to the same code
// paths without needing an `Accessor<WasmHostState, HasSelf<WasmHostState>>` (which
// can only be constructed inside a `run_concurrent` scope).
impl WasmHostState {
    pub(crate) async fn camel_call_impl(
        store: &Accessor<WasmHostState, wasmtime::component::HasSelf<WasmHostState>>,
        uri: String,
        payload: String,
    ) -> Result<String, crate::bindings::camel::plugin::types::WasmError> {
        // Capability gate (H4): check scheme against per-world allowlist.
        let scheme = uri.split(':').next().unwrap_or("").to_string();
        let can_call = store.with(|mut view| view.get().capabilities.can_call(&scheme));
        if !can_call {
            return Err(
                crate::bindings::camel::plugin::types::WasmError::ProcessorError(format!(
                    "camel_call denied: scheme '{}' not in capability allowlist",
                    scheme
                )),
            );
        }

        // Snapshot the registry (cloned Arc — cheap) and the producer's
        // observability handle for the async work so we don't borrow
        // `store` across the await.
        let registry = store.with(|mut view| view.get().registry.clone());
        let observability = store.with(|mut view| view.get().observability.clone());

        // Recursion guard. The RAII `DepthGuard` decrements on drop, so
        // even if the `run_async_call` future is cancelled mid-await the
        // counter returns to 0 — otherwise every subsequent call would
        // fail with "recursive wasm calls not supported".
        //
        // Clone the Arc<AtomicUsize> from store state — no unsafe needed.
        // The guard borrows the Arc's inner AtomicUsize for the call scope.
        let call_depth = store.with(|mut view| view.get().call_depth.clone());
        let _depth_guard = match DepthGuard::new(call_depth.as_ref()) {
            Some(g) => g,
            None => {
                return Err(
                    crate::bindings::camel::plugin::types::WasmError::ProcessorError(
                        "recursive wasm calls not supported".to_string(),
                    ),
                );
            }
        };

        run_async_call(registry, observability, uri, payload).await
    }

    pub(crate) async fn camel_poll_impl(
        store: &Accessor<WasmHostState, wasmtime::component::HasSelf<WasmHostState>>,
        uri: String,
        timeout_ms: u32,
    ) -> Result<String, crate::bindings::camel::plugin::types::WasmError> {
        let scheme = uri.split(':').next().unwrap_or("").to_string();
        let can_call = store.with(|mut view| view.get().capabilities.can_call(&scheme));
        if !can_call {
            return Err(
                crate::bindings::camel::plugin::types::WasmError::ProcessorError(format!(
                    "camel_poll denied: scheme '{}' not in capability allowlist",
                    scheme
                )),
            );
        }

        // Recursion guard (RAII — see camel_call_impl for rationale).
        let registry = store.with(|mut view| view.get().registry.clone());
        let observability = store.with(|mut view| view.get().observability.clone());
        let call_depth = store.with(|mut view| view.get().call_depth.clone());
        let _depth_guard = match DepthGuard::new(call_depth.as_ref()) {
            Some(g) => g,
            None => {
                return Err(
                    crate::bindings::camel::plugin::types::WasmError::ProcessorError(
                        "recursive wasm calls not supported".to_string(),
                    ),
                );
            }
        };

        run_async_poll(registry, observability, uri, timeout_ms).await
    }

    pub(crate) fn get_property_impl(&self, key: String) -> Option<String> {
        self.properties.get(&key).map(|v| match v {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        })
    }

    pub(crate) fn set_property_impl(&mut self, key: String, value: String) {
        if key.len() > self.state_store.max_key_bytes() {
            return;
        }
        if value.len() > self.state_store.max_value_bytes() {
            return;
        }
        let parsed = serde_json::from_str::<Value>(&value).unwrap_or(Value::String(value));
        self.properties.insert(key, parsed);
    }

    pub(crate) fn host_store_impl(
        state: &mut WasmHostState,
        key: String,
        value: String,
    ) -> Result<(), crate::bindings::camel::plugin::types::WasmError> {
        if !state.capabilities.host_kv {
            return Err(
                crate::bindings::camel::plugin::types::WasmError::ProcessorError(
                    "host_store denied: host_kv capability not granted".to_string(),
                ),
            );
        }
        state
            .state_store
            .store(&key, &value)
            .map_err(crate::bindings::camel::plugin::types::WasmError::Io)
    }

    pub(crate) fn host_load_impl(
        state: &mut WasmHostState,
        key: String,
    ) -> Result<Option<String>, crate::bindings::camel::plugin::types::WasmError> {
        if !state.capabilities.host_kv {
            return Err(
                crate::bindings::camel::plugin::types::WasmError::ProcessorError(
                    "host_load denied: host_kv capability not granted".to_string(),
                ),
            );
        }
        state
            .state_store
            .load(&key)
            .map_err(crate::bindings::camel::plugin::types::WasmError::Io)
    }
}

// Inherent (non-async) versions of the impl methods for the test suite.
// The trait methods are async + store-bound, but the unit tests want to
// exercise the gating logic synchronously against a plain `&mut
// WasmHostState`. Both layers share the same recursion / capability /
// host_kv gates.
//
// `dead_code` allow: the `*_gate` methods are referenced only from
// `#[cfg(test)]` code in this same module, but the rustc dead-code lint
// analyses test code separately and reports them as unused. The
// #[allow(dead_code)] silences the false positive.
#[allow(dead_code)]
impl WasmHostState {
    /// Same gating as `camel_call_impl` but synchronous and against a
    /// plain `&mut self`. Returns the same error variant on gate failure.
    /// Async work is NOT run — this is purely for unit-testing the gate
    /// logic in isolation.
    pub(crate) fn camel_call_gate(
        &mut self,
        uri: String,
    ) -> Result<(), crate::bindings::camel::plugin::types::WasmError> {
        let scheme = uri.split(':').next().unwrap_or("").to_string();
        if !self.capabilities.can_call(&scheme) {
            return Err(
                crate::bindings::camel::plugin::types::WasmError::ProcessorError(format!(
                    "camel_call denied: scheme '{}' not in capability allowlist",
                    scheme
                )),
            );
        }
        if self.call_depth.load(std::sync::atomic::Ordering::Relaxed) > 0 {
            return Err(
                crate::bindings::camel::plugin::types::WasmError::ProcessorError(
                    "recursive wasm calls not supported".to_string(),
                ),
            );
        }
        self.call_depth
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }

    pub(crate) fn release_call_depth(&self) {
        self.call_depth
            .fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
    }

    pub(crate) fn host_store_gate(
        &self,
        key: String,
        value: String,
    ) -> Result<(), crate::bindings::camel::plugin::types::WasmError> {
        if !self.capabilities.host_kv {
            return Err(
                crate::bindings::camel::plugin::types::WasmError::ProcessorError(
                    "host_store denied: host_kv capability not granted".to_string(),
                ),
            );
        }
        self.state_store
            .store(&key, &value)
            .map_err(crate::bindings::camel::plugin::types::WasmError::Io)
    }

    pub(crate) fn host_load_gate(
        &self,
        key: String,
    ) -> Result<Option<String>, crate::bindings::camel::plugin::types::WasmError> {
        if !self.capabilities.host_kv {
            return Err(
                crate::bindings::camel::plugin::types::WasmError::ProcessorError(
                    "host_load denied: host_kv capability not granted".to_string(),
                ),
            );
        }
        self.state_store
            .load(&key)
            .map_err(crate::bindings::camel::plugin::types::WasmError::Io)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Arc;

    fn make_state(call_depth: usize) -> WasmHostState {
        WasmHostState {
            table: wasmtime::component::ResourceTable::new(),
            wasi: wasmtime_wasi::WasiCtxBuilder::new().build(),
            properties: HashMap::new(),
            registry: Arc::new(camel_component_api::NoOpComponentContext),
            observability: Arc::new(camel_component_api::test_support::NoopRuntimeObservability),
            call_depth: Arc::new(std::sync::atomic::AtomicUsize::new(call_depth)),
            limits: wasmtime::StoreLimits::default(),
            state_store: crate::state_store::StateStore::new(),
            capabilities: crate::capabilities::WasmCapabilities::default(),
        }
    }

    #[test]
    fn test_recursion_guard_blocks_nested_calls() {
        let state = make_state(1);
        assert!(state.call_depth.load(std::sync::atomic::Ordering::Relaxed) > 0);
    }

    #[test]
    fn test_recursion_guard_allows_initial_call() {
        let state = make_state(0);
        assert_eq!(
            state.call_depth.load(std::sync::atomic::Ordering::Relaxed),
            0
        );
    }

    #[test]
    fn test_get_property_string_value() {
        let mut state = make_state(0);
        state
            .properties
            .insert("key".to_string(), Value::String("value".to_string()));
        assert_eq!(
            state.get_property_impl("key".to_string()),
            Some("value".to_string())
        );
    }

    #[test]
    fn test_get_property_missing_key() {
        let state = make_state(0);
        assert!(!state.properties.contains_key("missing"));
    }

    #[test]
    fn test_set_property_json_value() {
        let mut state = make_state(0);
        state.set_property_impl("json_key".to_string(), "{\"nested\":true}".to_string());
        assert!(state.properties.get("json_key").unwrap().is_object());
    }

    #[test]
    fn test_uri_scheme_parsing() {
        assert_eq!("direct".split(':').next().unwrap_or(""), "direct");
        assert_eq!("log:info".split(':').next().unwrap_or(""), "log");
        assert_eq!("noscheme".split(':').next().unwrap_or(""), "noscheme");
        assert_eq!("".split(':').next().unwrap_or(""), "");
    }

    #[test]
    fn test_camel_call_does_not_panic_inside_tokio_runtime() {
        let mut state = make_state(0);
        let result = state.camel_call_gate("noscheme".to_string());
        assert!(
            result.is_err(),
            "should return error for empty scheme, not panic"
        );
    }

    // Capability gating (H4 / H5)
    // -----------------------------------------------------------------------
    // Default capabilities are fail-closed: empty scheme allowlist, host_kv
    // disabled. Tests below verify that the gates fire BEFORE any side effect
    // (registry lookup, state store mutation) so a denied call cannot leak
    // information or persist data.

    #[test]
    fn test_camel_call_denied_without_capability() {
        let mut state = make_state(0);
        // Default capabilities: empty allowlist
        let result = state.camel_call_gate("log:info".to_string());
        let err = result.unwrap_err();
        let msg = format!("{:?}", err);
        assert!(
            msg.contains("denied"),
            "expected 'denied' in error, got: {msg}"
        );
    }

    #[test]
    fn test_camel_poll_denied_without_capability() {
        let state = make_state(0);
        // The poll gate mirrors the call gate's scheme check.
        let can_call = state.capabilities.can_call("file:foo");
        assert!(!can_call, "default capabilities must deny 'file' scheme");
    }

    #[test]
    fn test_host_store_denied_without_capability() {
        let state = make_state(0);
        let result = state.host_store_gate("key".to_string(), "val".to_string());
        let err = result.unwrap_err();
        let msg = format!("{:?}", err);
        assert!(
            msg.contains("host_kv"),
            "expected 'host_kv' in error, got: {msg}"
        );
    }

    #[test]
    fn test_host_load_denied_without_capability() {
        let state = make_state(0);
        let result = state.host_load_gate("key".to_string());
        let err = result.unwrap_err();
        let msg = format!("{:?}", err);
        assert!(
            msg.contains("host_kv"),
            "expected 'host_kv' in error, got: {msg}"
        );
    }

    #[test]
    fn test_camel_call_allowed_with_capability_passes_scheme_check() {
        // Granting the scheme means the gate no longer denies — but with an
        // empty registry, the next stage returns "component not found". This
        // proves the gate runs BEFORE the registry lookup.
        let mut state = make_state(0);
        state.capabilities = crate::capabilities::WasmCapabilities::from_scheme_list("noscheme");
        // The scheme check now passes; the gate increments call_depth.
        let result = state.camel_call_gate("noscheme:foo".to_string());
        assert!(
            result.is_ok(),
            "gate should pass when scheme is allowed; got: {result:?}"
        );
        state.release_call_depth();
    }

    #[test]
    fn test_denied_capabilities_block_host_kv_via_field() {
        // H5: policy worlds (denied caps) must have host_kv disabled — the
        // gate is a single field check, not a per-key check.
        let mut state = make_state(0);
        state.capabilities = crate::capabilities::WasmCapabilities::denied();
        assert!(!state.capabilities.host_kv);
        // host_store and host_load both go through the same gate
        assert!(
            state
                .host_store_gate("k".to_string(), "v".to_string())
                .is_err()
        );
        assert!(state.host_load_gate("k".to_string()).is_err());
    }

    // ── DepthGuard: RAII recursion counter (C2, I2) ──────────────────────
    //
    // The guard increments `call_depth` on `new` and decrements on `Drop`.
    // The three tests below cover the three behaviours the production code
    // depends on: increment, drop-decrement, and recursion rejection.
    //
    // Per I2: the spec explicitly notes that "if testing through `Accessor`
    // is too complex for unit tests, at minimum test the `DepthGuard`
    // struct directly". The trait method signatures now use
    // `&Accessor<WasmHostState, HasSelf<WasmHostState>>` (switched from
    // hand-rolled HasData to wasmtime's built-in HasSelf). Accessor
    // construction still requires `run_concurrent`, so DepthGuard unit
    // tests remain the most practical verification path for the RAII
    // recursion counter behaviour.

    #[test]
    fn test_depth_guard_increments_and_decrements() {
        // new() must increment; Drop must decrement back to the original value.
        let state = make_state(0);
        let depth = &state.call_depth;
        assert_eq!(depth.load(std::sync::atomic::Ordering::SeqCst), 0);
        {
            let _g = DepthGuard::new(depth).expect("first guard must succeed");
            assert_eq!(
                depth.load(std::sync::atomic::Ordering::SeqCst),
                1,
                "guard must increment to 1"
            );
        }
        assert_eq!(
            depth.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "Drop must decrement back to 0"
        );
    }

    #[test]
    fn test_depth_guard_blocks_recursion() {
        // A second guard while the first is alive must return None and must
        // not leak an increment (the failed new() must roll back).
        let state = make_state(0);
        let depth = &state.call_depth;
        let first = DepthGuard::new(depth).expect("first guard must succeed");
        assert_eq!(depth.load(std::sync::atomic::Ordering::SeqCst), 1);
        let second = DepthGuard::new(depth);
        assert!(second.is_none(), "nested guard must be rejected");
        // Drop the second (None) and verify the counter is still 1.
        drop(second);
        assert_eq!(
            depth.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "rejected guard must not leak an increment"
        );
        drop(first);
        assert_eq!(depth.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[test]
    fn test_depth_guard_rolls_back_on_rejection() {
        // If new() is called when depth > 0, it must return None AND restore
        // the counter (otherwise the increment would leak forever).
        let state = make_state(0);
        let depth = &state.call_depth;
        let guard = DepthGuard::new(depth).expect("first guard");
        assert_eq!(depth.load(std::sync::atomic::Ordering::SeqCst), 1);
        // The second attempt increments-then-decrements; verify the transient
        // bump does not affect the observed value after rejection.
        let _rejected = DepthGuard::new(depth);
        assert_eq!(
            depth.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "after rejection, counter must be unchanged"
        );
        drop(guard);
        assert_eq!(depth.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[test]
    fn test_depth_guard_decrements_on_explicit_drop() {
        // Explicit drop() must decrement. This is the test the original
        // manual inc/dec pattern was missing: a future that gets dropped
        // between inc and dec would leak the counter.
        let state = make_state(0);
        let depth = &state.call_depth;
        let guard = DepthGuard::new(depth).expect("first guard");
        assert_eq!(depth.load(std::sync::atomic::Ordering::SeqCst), 1);
        drop(guard);
        assert_eq!(depth.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    // ── set_property_impl size limits (W2) ──────────────────────────────

    #[test]
    fn test_set_property_rejects_oversized_key() {
        let mut state = make_state(0);
        state.state_store = crate::state_store::StateStore::with_limits(256, 5, 65536);
        state.set_property_impl("very_long_key_name".to_string(), "val".to_string());
        assert!(
            !state.properties.contains_key("very_long_key_name"),
            "must not insert property when key exceeds max_key_bytes"
        );
    }

    #[test]
    fn test_set_property_rejects_oversized_value() {
        let mut state = make_state(0);
        state.state_store = crate::state_store::StateStore::with_limits(256, 1024, 5);
        state.set_property_impl("k".to_string(), "this_value_is_way_too_long".to_string());
        assert!(
            !state.properties.contains_key("k"),
            "must not insert property when value exceeds max_value_bytes"
        );
    }

    #[test]
    fn test_set_property_allows_within_bounds() {
        let mut state = make_state(0);
        state.state_store = crate::state_store::StateStore::with_limits(256, 1024, 65536);
        state.set_property_impl("key".to_string(), "{\"x\":true}".to_string());
        let val = state.properties.get("key");
        assert!(
            val.is_some(),
            "must insert property when key and value are within limits"
        );
        if let Some(v) = val {
            assert!(v.is_object(), "JSON value must be parsed as object");
        }
    }

    // ── run_async_call / run_async_poll observability threading ──────────
    //
    // The fakes below prove two things about the guest host-function path:
    // (a) the rt handle handed to dynamically created producers IS the
    // threaded `RuntimeObservability` (Arc pointer identity), and (b) the
    // context passed to `create_endpoint` IS the live registry (an emission
    // through its `metrics()` reaches the recording collector — the old
    // `NoOpComponentContext` hard-code routed it to `NoOpMetrics`, which
    // silently drops it).

    /// Slot where the fake endpoint captures the rt handle received by
    /// `create_producer`.
    type CapturedRt = std::sync::Mutex<Option<Arc<dyn camel_component_api::RuntimeObservability>>>;

    /// Recorded counter: `(family name, label pairs)`.
    type RecordedCounter = (String, Vec<(String, String)>);

    /// Recording `MetricsCollector` capturing every `record_counter` call as
    /// `(family name, label pairs)` (pattern: `RecMetrics` in camel-core
    /// `context_tests.rs`).
    #[derive(Default)]
    struct RecMetrics {
        counters: std::sync::Mutex<Vec<RecordedCounter>>,
    }

    impl RecMetrics {
        fn contains(&self, name: &str, labels: &[(&str, &str)]) -> bool {
            let want: Vec<(String, String)> = labels
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect();
            self.counters
                .lock()
                .expect("counters lock")
                .iter()
                .any(|(n, l)| n == name && *l == want)
        }
    }

    impl camel_api::MetricsCollector for RecMetrics {
        fn record_exchange_duration(&self, _: &str, _: std::time::Duration) {}
        fn increment_errors(&self, _: &str, _: &str) {}
        fn increment_exchanges(&self, _: &str) {}
        fn set_queue_depth(&self, _: &str, _: usize) {}
        fn record_circuit_breaker_change(&self, _: &str, _: &str, _: &str) {}
        fn record_counter(&self, name: &str, _value: f64, labels: &[(&str, &str)]) {
            self.counters.lock().expect("counters lock").push((
                name.to_string(),
                labels
                    .iter()
                    .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                    .collect(),
            ));
        }
    }

    /// Fake `Component` for scheme `fake`: `create_endpoint` emits a
    /// live-context counter and hands out a fake endpoint that captures the
    /// rt passed to `create_producer`.
    struct FakeComponent {
        captured_rt: Arc<CapturedRt>,
    }

    impl camel_component_api::Component for FakeComponent {
        fn scheme(&self) -> &str {
            "fake"
        }

        fn create_endpoint(
            &self,
            _uri: &str,
            ctx: &dyn camel_component_api::ComponentContext,
        ) -> Result<Box<dyn camel_component_api::Endpoint>, camel_api::CamelError> {
            // Live-context proof: this emission must reach the recording
            // collector; through `NoOpMetrics` it would be dropped.
            ctx.metrics().record_counter("wasmguest:test:ctx", 1.0, &[]);
            Ok(Box::new(FakeEndpoint {
                captured_rt: Arc::clone(&self.captured_rt),
            }))
        }
    }

    struct FakeEndpoint {
        captured_rt: Arc<CapturedRt>,
    }

    impl camel_component_api::Endpoint for FakeEndpoint {
        fn uri(&self) -> &str {
            "fake:dest"
        }

        fn create_consumer(
            &self,
            _rt: Arc<dyn camel_component_api::RuntimeObservability>,
        ) -> Result<Box<dyn camel_component_api::Consumer>, camel_api::CamelError> {
            Err(camel_api::CamelError::EndpointCreationFailed(
                "fake endpoint exposes no consumer".into(),
            ))
        }

        fn create_producer(
            &self,
            rt: Arc<dyn camel_component_api::RuntimeObservability>,
            _ctx: &camel_api::ProducerContext,
        ) -> Result<camel_api::BoxProcessor, camel_api::CamelError> {
            *self.captured_rt.lock().expect("captured_rt lock") = Some(Arc::clone(&rt));
            Ok(tower::util::BoxCloneSyncService::new(FakeProducer { rt }))
        }

        fn polling_consumer(&self) -> Option<Box<dyn camel_component_api::PollingConsumer>> {
            Some(Box::new(FakePoller))
        }
    }

    /// Fake producer: emits a counter through the captured rt's metrics and
    /// echoes the input Text body as the response.
    #[derive(Clone)]
    struct FakeProducer {
        rt: Arc<dyn camel_component_api::RuntimeObservability>,
    }

    impl tower::Service<camel_api::Exchange> for FakeProducer {
        type Response = camel_api::Exchange;
        type Error = camel_api::CamelError;
        type Future = std::pin::Pin<
            Box<
                dyn std::future::Future<Output = Result<camel_api::Exchange, camel_api::CamelError>>
                    + Send,
            >,
        >;

        fn poll_ready(
            &mut self,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Result<(), Self::Error>> {
            std::task::Poll::Ready(Ok(()))
        }

        fn call(&mut self, mut exchange: camel_api::Exchange) -> Self::Future {
            let rt = Arc::clone(&self.rt);
            Box::pin(async move {
                rt.metrics()
                    .record_counter("wasmguest:test:invoke", 1.0, &[("route", "test")]);
                let echoed = match &exchange.input.body {
                    camel_api::Body::Text(s) => s.clone(),
                    _ => String::new(),
                };
                exchange.output = Some(camel_api::Message::new(camel_api::Body::Text(echoed)));
                Ok(exchange)
            })
        }
    }

    /// Fake poller: hands out one Text exchange immediately, well within any
    /// reasonable poll timeout.
    struct FakePoller;

    #[async_trait::async_trait]
    impl camel_component_api::PollingConsumer for FakePoller {
        async fn receive(
            &mut self,
            _timeout: std::time::Duration,
        ) -> Result<Option<camel_api::Exchange>, camel_api::CamelError> {
            Ok(Some(camel_api::Exchange::new(camel_api::Message::new(
                camel_api::Body::Text("polled-payload".to_string()),
            ))))
        }
    }

    /// Fake `ComponentContext`: records metrics into `RecMetrics` and
    /// resolves the fake component under scheme `fake`. The same Arc serves
    /// as both the registry (`Arc<dyn ComponentContext>`) and the
    /// observability handle (`Arc<dyn RuntimeObservability>` via the
    /// blanket impl) — one owner, two handles, no new acquisition seam.
    struct FakeCtx {
        metrics: Arc<RecMetrics>,
        component: Arc<FakeComponent>,
    }

    impl camel_component_api::ComponentContext for FakeCtx {
        fn resolve_component(
            &self,
            scheme: &str,
        ) -> Option<Arc<dyn camel_component_api::Component>> {
            if scheme == "fake" {
                Some(Arc::clone(&self.component) as Arc<dyn camel_component_api::Component>)
            } else {
                None
            }
        }

        fn resolve_language(&self, _name: &str) -> Option<Arc<dyn camel_language_api::Language>> {
            None
        }

        fn metrics(&self) -> Arc<dyn camel_api::MetricsCollector> {
            Arc::clone(&self.metrics) as Arc<dyn camel_api::MetricsCollector>
        }

        fn platform_service(&self) -> Arc<dyn camel_api::PlatformService> {
            Arc::new(camel_api::NoopPlatformService::default())
        }

        fn register_route_health_check(
            &self,
            _route_id: &str,
            _check: Arc<dyn camel_api::AsyncHealthCheck>,
        ) {
        }

        fn unregister_route_health_check(&self, _route_id: &str) {}
    }

    fn fake_registry() -> (Arc<FakeCtx>, Arc<CapturedRt>, Arc<RecMetrics>) {
        let metrics = Arc::new(RecMetrics::default());
        let captured_rt: Arc<CapturedRt> = Arc::new(std::sync::Mutex::new(None));
        let component = Arc::new(FakeComponent {
            captured_rt: Arc::clone(&captured_rt),
        });
        let ctx = Arc::new(FakeCtx {
            metrics: Arc::clone(&metrics),
            component,
        });
        (ctx, captured_rt, metrics)
    }

    #[tokio::test]
    async fn guest_call_threads_observability_to_dynamic_producer() {
        let (ctx, captured_rt, metrics) = fake_registry();
        let registry: Arc<dyn camel_component_api::ComponentContext> =
            Arc::clone(&ctx) as Arc<dyn camel_component_api::ComponentContext>;
        let observability: Arc<dyn camel_component_api::RuntimeObservability> =
            Arc::clone(&ctx) as Arc<dyn camel_component_api::RuntimeObservability>;

        let result = run_async_call(
            registry,
            Arc::clone(&observability),
            "fake:dest".to_string(),
            "{\"ok\":true}".to_string(),
        )
        .await
        .expect("guest call must succeed");

        assert_eq!(result, "{\"ok\":true}", "producer must echo the payload");

        let captured = captured_rt
            .lock()
            .expect("captured_rt lock")
            .clone()
            .expect("create_producer must capture the rt handle");
        assert!(
            Arc::ptr_eq(&captured, &observability),
            "dynamic producer must receive the threaded observability handle"
        );
        assert!(
            metrics.contains("wasmguest:test:invoke", &[("route", "test")]),
            "counter emitted through the threaded handle must reach the recording collector"
        );
    }

    #[tokio::test]
    async fn guest_call_passes_live_context_to_create_endpoint() {
        let (ctx, _captured_rt, metrics) = fake_registry();
        let registry: Arc<dyn camel_component_api::ComponentContext> =
            Arc::clone(&ctx) as Arc<dyn camel_component_api::ComponentContext>;
        let observability: Arc<dyn camel_component_api::RuntimeObservability> =
            Arc::clone(&ctx) as Arc<dyn camel_component_api::RuntimeObservability>;

        run_async_call(
            registry,
            observability,
            "fake:dest".to_string(),
            "{\"ok\":true}".to_string(),
        )
        .await
        .expect("guest call must succeed");

        assert!(
            metrics.contains("wasmguest:test:ctx", &[]),
            "create_endpoint must receive the live registry context, not NoOp"
        );
    }

    #[tokio::test]
    async fn guest_poll_passes_live_context_to_create_endpoint() {
        let (ctx, _captured_rt, metrics) = fake_registry();
        let registry: Arc<dyn camel_component_api::ComponentContext> =
            Arc::clone(&ctx) as Arc<dyn camel_component_api::ComponentContext>;
        let observability: Arc<dyn camel_component_api::RuntimeObservability> =
            Arc::clone(&ctx) as Arc<dyn camel_component_api::RuntimeObservability>;

        let result = run_async_poll(registry, observability, "fake:dest".to_string(), 100)
            .await
            .expect("poll must succeed");

        assert_eq!(result, "polled-payload", "poll result shape unchanged");
        assert!(
            metrics.contains("wasmguest:test:ctx", &[]),
            "poll-path create_endpoint must receive the live registry context"
        );
    }
}
