use std::collections::HashMap;
use std::sync::Arc;
use std::sync::OnceLock;

use camel_component_file::FileComponent;
use camel_component_wasm::bindings::camel::plugin::host::Host;
use camel_component_wasm::runtime::WasmHostState;
use camel_core::Registry;
use wasmtime::component::ResourceTable;
use wasmtime_wasi::WasiCtxBuilder;

fn test_tokio_handle() -> tokio::runtime::Handle {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| tokio::runtime::Runtime::new().expect("test runtime"))
        .handle()
        .clone()
}

fn make_state_with_registry(registry: Registry, call_depth: u32) -> WasmHostState {
    WasmHostState {
        table: ResourceTable::new(),
        wasi: WasiCtxBuilder::new().inherit_stderr().build(),
        properties: HashMap::new(),
        registry: Arc::new(std::sync::Mutex::new(registry)),
        call_depth,
        limits: wasmtime::StoreLimits::default(),
        state_store: camel_component_wasm::StateStore::new(),
        tokio_handle: test_tokio_handle(),
    }
}

#[test]
fn camel_poll_with_file_component_returns_file_content() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dir = tmp.path().to_str().unwrap();
    let content = "hello from poll";

    // Write a file to the temp dir
    std::fs::write(tmp.path().join("input.txt"), content).expect("write test file");

    // Register FileComponent under scheme "file"
    let mut registry = Registry::new();
    registry
        .register(Arc::new(FileComponent::default()) as Arc<dyn camel_component_api::Component>);

    let uri = format!("file:{dir}?fileName=input.txt&noop=true");
    let mut state = make_state_with_registry(registry, 0);

    let result = Host::camel_poll(&mut state, uri, 1000);
    assert!(result.is_ok(), "expected Ok, got: {:?}", result);
    assert_eq!(result.unwrap(), content);
}

#[test]
fn camel_poll_with_unknown_scheme_returns_error() {
    let registry = Registry::new();
    let mut state = make_state_with_registry(registry, 0);

    let result = Host::camel_poll(&mut state, "nosuch://foo".to_string(), 100);
    assert!(result.is_err(), "expected Err for unknown scheme");
    let err = result.unwrap_err();
    let err_str = format!("{:?}", err);
    assert!(
        err_str.contains("component not found"),
        "error should mention 'component not found', got: {}",
        err_str
    );
}

#[test]
fn camel_poll_recursion_guard_blocks_nested_calls() {
    let registry = Registry::new();
    let mut state = make_state_with_registry(registry, 1); // call_depth already > 0

    let result = Host::camel_poll(&mut state, "file:anything".to_string(), 100);
    assert!(result.is_err(), "expected Err for recursive call");
    let err = result.unwrap_err();
    let err_str = format!("{:?}", err);
    assert!(
        err_str.contains("recursive wasm calls"),
        "error should mention 'recursive wasm calls', got: {}",
        err_str
    );
}

#[test]
fn camel_poll_with_empty_uri_scheme_returns_error() {
    let registry = Registry::new();
    let mut state = make_state_with_registry(registry, 0);

    let result = Host::camel_poll(&mut state, String::new(), 100);
    assert!(result.is_err(), "expected Err for URI with no scheme");
    let err = result.unwrap_err();
    let err_str = format!("{:?}", err);
    assert!(
        err_str.contains("invalid URI"),
        "error should mention 'invalid URI', got: {}",
        err_str
    );
}

#[test]
fn camel_poll_with_non_pollable_scheme_returns_error() {
    use camel_component_log::LogComponent;

    let mut registry = Registry::new();
    registry.register(Arc::new(LogComponent::new()) as Arc<dyn camel_component_api::Component>);
    let mut state = make_state_with_registry(registry, 0);

    let result = Host::camel_poll(&mut state, "log:test".to_string(), 100);
    assert!(result.is_err(), "expected Err for non-pollable component");
    let err = result.unwrap_err();
    let err_str = format!("{:?}", err);
    assert!(
        err_str.contains("camel_poll requires"),
        "error should mention 'camel_poll requires', got: {}",
        err_str
    );
}
