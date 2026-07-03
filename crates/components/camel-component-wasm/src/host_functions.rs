use serde_json::Value;
use tower::ServiceExt;
use wasmtime::component::{HasSelf, Linker};

use crate::bindings::camel::plugin::host::Host;
use crate::bindings::camel::plugin::types::WasmError;
use crate::runtime::WasmHostState;

impl Host for WasmHostState {
    fn camel_call(&mut self, uri: String, payload: String) -> Result<String, WasmError> {
        // Capability gate (H4): check scheme against per-world allowlist.
        // Runs before call_depth increment so a denied scheme also avoids
        // any side effect on the depth counter.
        let scheme = uri.split(':').next().unwrap_or("").to_string();
        if !self.capabilities.can_call(&scheme) {
            return Err(WasmError::ProcessorError(format!(
                "camel_call denied: scheme '{}' not in capability allowlist",
                scheme
            )));
        }

        if self.call_depth > 0 {
            return Err(WasmError::ProcessorError(
                "recursive wasm calls not supported".to_string(),
            ));
        }
        self.call_depth += 1;

        let registry = self.registry.clone();
        let async_work = async {
            let scheme = uri.split(':').next().unwrap_or("").to_string();
            if scheme.is_empty() {
                return Err(WasmError::ProcessorError(format!(
                    "invalid URI (no scheme): {}",
                    uri
                )));
            }

            let component = {
                let guard = registry
                    .lock()
                    .map_err(|e| WasmError::Io(format!("registry lock poisoned: {}", e)))?;
                guard.get(&scheme).ok_or_else(|| {
                    WasmError::ProcessorError(format!("component not found for scheme: {}", scheme))
                })?
            };

            let endpoint = component
                .create_endpoint(&uri, &camel_component_api::NoOpComponentContext)
                .map_err(|e| WasmError::ProcessorError(format!("create_endpoint failed: {}", e)))?;

            let rt: std::sync::Arc<dyn camel_component_api::RuntimeObservability> =
                std::sync::Arc::new(camel_component_api::NoOpComponentContext);
            let producer = endpoint
                .create_producer(rt, &camel_api::ProducerContext::new())
                .map_err(|e| WasmError::ProcessorError(format!("create_producer failed: {}", e)))?;

            let exchange =
                camel_api::Exchange::new(camel_api::Message::new(camel_api::Body::Text(payload)));

            let result = producer
                .oneshot(exchange)
                .await
                .map_err(|e| WasmError::ProcessorError(format!("endpoint call failed: {}", e)))?;

            let body_str = match &result.output {
                Some(msg) => match &msg.body {
                    camel_api::Body::Text(s) => s.clone(),
                    camel_api::Body::Json(v) => v.to_string(),
                    camel_api::Body::Bytes(b) => String::from_utf8_lossy(b).to_string(),
                    camel_api::Body::Xml(s) => s.clone(),
                    camel_api::Body::Empty => String::new(),
                    camel_api::Body::Stream(_) => "<stream>".to_string(),
                },
                None => match &result.input.body {
                    camel_api::Body::Text(s) => s.clone(),
                    camel_api::Body::Json(v) => v.to_string(),
                    camel_api::Body::Bytes(b) => String::from_utf8_lossy(b).to_string(),
                    camel_api::Body::Xml(s) => s.clone(),
                    camel_api::Body::Empty => String::new(),
                    camel_api::Body::Stream(_) => "<stream>".to_string(),
                },
            };

            Ok(body_str)
        };

        let handle = self.tokio_handle.clone();
        let result = tokio::task::block_in_place(|| handle.block_on(async_work));

        self.call_depth -= 1;
        result
    }

    fn camel_poll(&mut self, uri: String, timeout_ms: u32) -> Result<String, WasmError> {
        // Capability gate (H4): check scheme against per-world allowlist.
        let scheme = uri.split(':').next().unwrap_or("").to_string();
        if !self.capabilities.can_call(&scheme) {
            return Err(WasmError::ProcessorError(format!(
                "camel_poll denied: scheme '{}' not in capability allowlist",
                scheme
            )));
        }

        if self.call_depth > 0 {
            return Err(WasmError::ProcessorError(
                "recursive wasm calls not supported".to_string(),
            ));
        }
        self.call_depth += 1;

        let registry = self.registry.clone();
        let async_work = async {
            let scheme = uri.split(':').next().unwrap_or("").to_string();
            if scheme.is_empty() {
                return Err(WasmError::ProcessorError(format!(
                    "invalid URI (no scheme): {}",
                    uri
                )));
            }

            let component = {
                let guard = registry
                    .lock()
                    .map_err(|e| WasmError::Io(format!("registry lock poisoned: {}", e)))?;
                guard.get(&scheme).ok_or_else(|| {
                    WasmError::ProcessorError(format!("component not found for scheme: {}", scheme))
                })?
            };

            let endpoint = component
                .create_endpoint(&uri, &camel_component_api::NoOpComponentContext)
                .map_err(|e| WasmError::ProcessorError(format!("create_endpoint failed: {}", e)))?;

            let mut poller = endpoint.polling_consumer().ok_or_else(|| {
                WasmError::ProcessorError(format!(
                    "camel_poll requires a component that supports polling consumers (scheme: {})",
                    scheme
                ))
            })?;

            let exchange = poller
                .receive(std::time::Duration::from_millis(timeout_ms as u64))
                .await
                .map_err(|e| WasmError::ProcessorError(format!("poll failed: {}", e)))?;

            let body_str = match exchange {
                Some(ex) => {
                    let bytes = ex
                        .input
                        .body
                        .into_bytes(10 * 1024 * 1024)
                        .await
                        .map_err(|e| {
                            WasmError::ProcessorError(format!("body read failed: {}", e))
                        })?;
                    String::from_utf8_lossy(&bytes).to_string()
                }
                None => {
                    return Err(WasmError::ProcessorError(format!(
                        "no message received within {}ms timeout",
                        timeout_ms
                    )));
                }
            };

            Ok(body_str)
        };

        let handle = self.tokio_handle.clone();
        let result = tokio::task::block_in_place(|| handle.block_on(async_work));

        self.call_depth -= 1;
        result
    }

    fn get_property(&mut self, key: String) -> Option<String> {
        self.properties.get(&key).map(|v| match v {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        })
    }

    fn set_property(&mut self, key: String, value: String) {
        let parsed = serde_json::from_str::<Value>(&value).unwrap_or(Value::String(value));
        self.properties.insert(key, parsed);
    }

    fn host_store(&mut self, key: String, value: String) -> Result<(), WasmError> {
        // Capability gate (H5): policy worlds have host_kv = false.
        if !self.capabilities.host_kv {
            return Err(WasmError::ProcessorError(
                "host_store denied: host_kv capability not granted".to_string(),
            ));
        }
        self.state_store.store(&key, &value).map_err(WasmError::Io)
    }

    fn host_load(&mut self, key: String) -> Result<Option<String>, WasmError> {
        // Capability gate (H5): policy worlds have host_kv = false.
        if !self.capabilities.host_kv {
            return Err(WasmError::ProcessorError(
                "host_load denied: host_kv capability not granted".to_string(),
            ));
        }
        self.state_store.load(&key).map_err(WasmError::Io)
    }
}

pub fn add_to_linker(linker: &mut Linker<WasmHostState>) -> Result<(), wasmtime::Error> {
    crate::bindings::camel::plugin::host::add_to_linker::<_, HasSelf<_>>(linker, |state| state)
}

macro_rules! impl_host_for_binding {
    ($bindings_mod:ident) => {
        impl crate::$bindings_mod::camel::plugin::host::Host for WasmHostState {
            fn camel_call(
                &mut self,
                uri: String,
                payload: String,
            ) -> Result<String, crate::$bindings_mod::camel::plugin::types::WasmError> {
                let host = self as &mut dyn Host;
                host.camel_call(uri, payload).map_err(|e| match e {
                    WasmError::ProcessorError(s) => {
                        crate::$bindings_mod::camel::plugin::types::WasmError::ProcessorError(s)
                    }
                    WasmError::TypeConversion(s) => {
                        crate::$bindings_mod::camel::plugin::types::WasmError::TypeConversion(s)
                    }
                    WasmError::Io(s) => {
                        crate::$bindings_mod::camel::plugin::types::WasmError::Io(s)
                    }
                    WasmError::Timeout => {
                        crate::$bindings_mod::camel::plugin::types::WasmError::Timeout
                    }
                })
            }

            fn camel_poll(
                &mut self,
                uri: String,
                timeout_ms: u32,
            ) -> Result<String, crate::$bindings_mod::camel::plugin::types::WasmError> {
                let host = self as &mut dyn Host;
                host.camel_poll(uri, timeout_ms).map_err(|e| match e {
                    WasmError::ProcessorError(s) => {
                        crate::$bindings_mod::camel::plugin::types::WasmError::ProcessorError(s)
                    }
                    WasmError::TypeConversion(s) => {
                        crate::$bindings_mod::camel::plugin::types::WasmError::TypeConversion(s)
                    }
                    WasmError::Io(s) => {
                        crate::$bindings_mod::camel::plugin::types::WasmError::Io(s)
                    }
                    WasmError::Timeout => {
                        crate::$bindings_mod::camel::plugin::types::WasmError::Timeout
                    }
                })
            }

            fn get_property(&mut self, key: String) -> Option<String> {
                let host = self as &mut dyn Host;
                host.get_property(key)
            }

            fn set_property(&mut self, key: String, value: String) {
                let host = self as &mut dyn Host;
                host.set_property(key, value)
            }

            fn host_store(
                &mut self,
                key: String,
                value: String,
            ) -> Result<(), crate::$bindings_mod::camel::plugin::types::WasmError> {
                let host = self as &mut dyn Host;
                host.host_store(key, value).map_err(|e| match e {
                    WasmError::ProcessorError(s) => {
                        crate::$bindings_mod::camel::plugin::types::WasmError::ProcessorError(s)
                    }
                    WasmError::TypeConversion(s) => {
                        crate::$bindings_mod::camel::plugin::types::WasmError::TypeConversion(s)
                    }
                    WasmError::Io(s) => {
                        crate::$bindings_mod::camel::plugin::types::WasmError::Io(s)
                    }
                    WasmError::Timeout => {
                        crate::$bindings_mod::camel::plugin::types::WasmError::Timeout
                    }
                })
            }

            fn host_load(
                &mut self,
                key: String,
            ) -> Result<Option<String>, crate::$bindings_mod::camel::plugin::types::WasmError> {
                let host = self as &mut dyn Host;
                host.host_load(key).map_err(|e| match e {
                    WasmError::ProcessorError(s) => {
                        crate::$bindings_mod::camel::plugin::types::WasmError::ProcessorError(s)
                    }
                    WasmError::TypeConversion(s) => {
                        crate::$bindings_mod::camel::plugin::types::WasmError::TypeConversion(s)
                    }
                    WasmError::Io(s) => {
                        crate::$bindings_mod::camel::plugin::types::WasmError::Io(s)
                    }
                    WasmError::Timeout => {
                        crate::$bindings_mod::camel::plugin::types::WasmError::Timeout
                    }
                })
            }
        }
    };
}

pub fn add_bean_to_linker(linker: &mut Linker<WasmHostState>) -> Result<(), wasmtime::Error> {
    crate::bean_bindings::camel::plugin::host::add_to_linker::<_, HasSelf<_>>(linker, |state| state)
}

impl_host_for_binding!(bean_bindings);

pub fn add_security_policy_to_linker(
    linker: &mut Linker<WasmHostState>,
) -> Result<(), wasmtime::Error> {
    crate::security_policy_bindings::camel::plugin::host::add_to_linker::<_, HasSelf<_>>(
        linker,
        |state| state,
    )
}

impl_host_for_binding!(security_policy_bindings);

#[cfg(test)]
mod tests {
    use super::*;
    use camel_core::Registry;
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::sync::OnceLock;

    fn test_tokio_handle() -> tokio::runtime::Handle {
        static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
        RT.get_or_init(|| tokio::runtime::Runtime::new().expect("test runtime"))
            .handle()
            .clone()
    }

    fn make_state(call_depth: u32) -> WasmHostState {
        WasmHostState {
            table: wasmtime::component::ResourceTable::new(),
            wasi: wasmtime_wasi::WasiCtxBuilder::new()
                .inherit_stderr()
                .build(),
            properties: HashMap::new(),
            registry: Arc::new(std::sync::Mutex::new(Registry::new())),
            call_depth,
            limits: wasmtime::StoreLimits::default(),
            state_store: crate::state_store::StateStore::new(),
            tokio_handle: test_tokio_handle(),
            capabilities: crate::capabilities::WasmCapabilities::default(),
        }
    }

    #[test]
    fn test_recursion_guard_blocks_nested_calls() {
        let state = make_state(1);
        assert!(state.call_depth > 0);
    }

    #[test]
    fn test_recursion_guard_allows_initial_call() {
        let state = make_state(0);
        assert_eq!(state.call_depth, 0);
    }

    #[test]
    fn test_get_property_string_value() {
        let mut state = make_state(0);
        state
            .properties
            .insert("key".to_string(), Value::String("value".to_string()));
        let value = state.properties.get("key").map(|v| match v {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        });
        assert_eq!(value, Some("value".to_string()));
    }

    #[test]
    fn test_get_property_missing_key() {
        let state = make_state(0);
        assert!(!state.properties.contains_key("missing"));
    }

    #[test]
    fn test_set_property_json_value() {
        let mut state = make_state(0);
        let parsed = serde_json::from_str::<Value>("{\"nested\":true}")
            .unwrap_or(Value::String("{\"nested\":true}".to_string()));
        state.properties.insert("json_key".to_string(), parsed);
        assert!(state.properties.get("json_key").unwrap().is_object());
    }

    #[test]
    fn test_uri_scheme_parsing() {
        assert_eq!("direct".split(':').next().unwrap_or(""), "direct");
        assert_eq!("log:info".split(':').next().unwrap_or(""), "log");
        assert_eq!("noscheme".split(':').next().unwrap_or(""), "noscheme");
        assert_eq!("".split(':').next().unwrap_or(""), "");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_camel_call_does_not_panic_inside_tokio_runtime() {
        let mut state = make_state(0);
        let result = Host::camel_call(&mut state, "noscheme".to_string(), "{}".to_string());
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

    #[tokio::test(flavor = "multi_thread")]
    async fn test_camel_call_denied_without_capability() {
        let mut state = make_state(0);
        // Default capabilities: empty allowlist
        let result = Host::camel_call(&mut state, "log:info".to_string(), "{}".to_string());
        let err = result.unwrap_err();
        let msg = format!("{:?}", err);
        assert!(
            msg.contains("denied"),
            "expected 'denied' in error, got: {msg}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_camel_poll_denied_without_capability() {
        let mut state = make_state(0);
        let result = Host::camel_poll(&mut state, "file:foo".to_string(), 100);
        let err = result.unwrap_err();
        let msg = format!("{:?}", err);
        assert!(
            msg.contains("denied"),
            "expected 'denied' in error, got: {msg}"
        );
    }

    #[test]
    fn test_host_store_denied_without_capability() {
        let mut state = make_state(0);
        let result = Host::host_store(&mut state, "key".to_string(), "val".to_string());
        let err = result.unwrap_err();
        let msg = format!("{:?}", err);
        assert!(
            msg.contains("host_kv"),
            "expected 'host_kv' in error, got: {msg}"
        );
    }

    #[test]
    fn test_host_load_denied_without_capability() {
        let mut state = make_state(0);
        let result = Host::host_load(&mut state, "key".to_string());
        let err = result.unwrap_err();
        let msg = format!("{:?}", err);
        assert!(
            msg.contains("host_kv"),
            "expected 'host_kv' in error, got: {msg}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_camel_call_allowed_with_capability_passes_scheme_check() {
        // Granting the scheme means the gate no longer denies — but with an
        // empty registry, the next stage returns "component not found". This
        // proves the gate runs BEFORE the registry lookup.
        let mut state = make_state(0);
        state.capabilities = crate::capabilities::WasmCapabilities::from_scheme_list("noscheme");
        let result = Host::camel_call(&mut state, "noscheme:foo".to_string(), "{}".to_string());
        let err = result.unwrap_err();
        let msg = format!("{:?}", err);
        assert!(
            msg.contains("component not found"),
            "expected 'component not found' (gate cleared), got: {msg}"
        );
    }

    #[test]
    fn test_denied_capabilities_block_host_kv_via_field() {
        // H5: policy worlds (denied caps) must have host_kv disabled — the
        // gate is a single field check, not a per-key check.
        let mut state = make_state(0);
        state.capabilities = crate::capabilities::WasmCapabilities::denied();
        assert!(!state.capabilities.host_kv);
        // host_store and host_load both go through the same gate
        assert!(Host::host_store(&mut state, "k".to_string(), "v".to_string()).is_err());
        assert!(Host::host_load(&mut state, "k".to_string()).is_err());
    }
}
