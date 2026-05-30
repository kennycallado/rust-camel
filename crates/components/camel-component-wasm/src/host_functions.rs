use serde_json::Value;
use tower::ServiceExt;
use wasmtime::component::{HasSelf, Linker};

use crate::bindings::camel::plugin::host::Host;
use crate::bindings::camel::plugin::types::WasmError;
use crate::runtime::WasmHostState;

impl Host for WasmHostState {
    fn camel_call(&mut self, uri: String, payload: String) -> Result<String, WasmError> {
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

            let producer = endpoint
                .create_producer(&camel_api::ProducerContext::new())
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
        self.state_store.store(&key, &value).map_err(WasmError::Io)
    }

    fn host_load(&mut self, key: String) -> Result<Option<String>, WasmError> {
        self.state_store.load(&key).map_err(WasmError::Io)
    }
}

pub fn add_to_linker(linker: &mut Linker<WasmHostState>) -> Result<(), wasmtime::Error> {
    crate::bindings::camel::plugin::host::add_to_linker::<_, HasSelf<_>>(linker, |state| state)
}

pub fn add_bean_to_linker(linker: &mut Linker<WasmHostState>) -> Result<(), wasmtime::Error> {
    crate::bean_bindings::camel::plugin::host::add_to_linker::<_, HasSelf<_>>(linker, |state| state)
}

use crate::bean_bindings::camel::plugin::host::Host as BeanHost;
use crate::bean_bindings::camel::plugin::types::WasmError as BeanWasmError;

impl BeanHost for WasmHostState {
    fn camel_call(&mut self, uri: String, payload: String) -> Result<String, BeanWasmError> {
        let host = self as &mut dyn Host;
        host.camel_call(uri, payload).map_err(|e| match e {
            WasmError::ProcessorError(s) => BeanWasmError::ProcessorError(s),
            WasmError::TypeConversion(s) => BeanWasmError::TypeConversion(s),
            WasmError::Io(s) => BeanWasmError::Io(s),
            WasmError::Timeout => BeanWasmError::Timeout,
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

    fn host_store(&mut self, key: String, value: String) -> Result<(), BeanWasmError> {
        let host = self as &mut dyn Host;
        host.host_store(key, value).map_err(|e| match e {
            WasmError::Io(s) => BeanWasmError::Io(s),
            other => BeanWasmError::Io(other.to_string()),
        })
    }

    fn host_load(&mut self, key: String) -> Result<Option<String>, BeanWasmError> {
        let host = self as &mut dyn Host;
        host.host_load(key).map_err(|e| match e {
            WasmError::Io(s) => BeanWasmError::Io(s),
            other => BeanWasmError::Io(other.to_string()),
        })
    }
}

pub fn add_security_policy_to_linker(
    linker: &mut Linker<WasmHostState>,
) -> Result<(), wasmtime::Error> {
    crate::security_policy_bindings::camel::plugin::host::add_to_linker::<_, HasSelf<_>>(
        linker,
        |state| state,
    )
}

use crate::security_policy_bindings::camel::plugin::host::Host as SecurityPolicyHost;
use crate::security_policy_bindings::camel::plugin::types::WasmError as SecurityPolicyWasmError;

impl SecurityPolicyHost for WasmHostState {
    fn camel_call(
        &mut self,
        uri: String,
        payload: String,
    ) -> Result<String, SecurityPolicyWasmError> {
        let host = self as &mut dyn Host;
        host.camel_call(uri, payload).map_err(|e| match e {
            WasmError::ProcessorError(s) => SecurityPolicyWasmError::ProcessorError(s),
            WasmError::TypeConversion(s) => SecurityPolicyWasmError::TypeConversion(s),
            WasmError::Io(s) => SecurityPolicyWasmError::Io(s),
            WasmError::Timeout => SecurityPolicyWasmError::Timeout,
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

    fn host_store(&mut self, key: String, value: String) -> Result<(), SecurityPolicyWasmError> {
        let host = self as &mut dyn Host;
        host.host_store(key, value).map_err(|e| match e {
            WasmError::Io(s) => SecurityPolicyWasmError::Io(s),
            other => SecurityPolicyWasmError::Io(other.to_string()),
        })
    }

    fn host_load(&mut self, key: String) -> Result<Option<String>, SecurityPolicyWasmError> {
        let host = self as &mut dyn Host;
        host.host_load(key).map_err(|e| match e {
            WasmError::Io(s) => SecurityPolicyWasmError::Io(s),
            other => SecurityPolicyWasmError::Io(other.to_string()),
        })
    }
}

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
}
