use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use camel_component_wasm::WasmConfig;
use camel_component_wasm::WasmSecurityPolicy;
use camel_core::Registry;

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("init-check.wasm")
}

fn make_registry() -> Arc<std::sync::Mutex<Registry>> {
    Arc::new(std::sync::Mutex::new(Registry::new()))
}

#[tokio::test]
async fn wasm_security_policy_init_receives_config() {
    let path = fixture_path();
    let registry = make_registry();
    let mut config = HashMap::new();
    config.insert("ldap_url".to_string(), "ldap://corp".to_string());
    config.insert("retry".to_string(), "3".to_string());

    let result = WasmSecurityPolicy::new(&path, WasmConfig::default(), registry, config).await;

    assert!(
        result.is_ok(),
        "expected Ok for matching config, got Err: {:?}",
        result.err()
    );
}

#[tokio::test]
async fn wasm_security_policy_init_rejects_wrong_config() {
    let path = fixture_path();
    let registry = make_registry();
    let mut config = HashMap::new();
    config.insert("unexpected".to_string(), "value".to_string());

    let result = WasmSecurityPolicy::new(&path, WasmConfig::default(), registry, config).await;

    assert!(result.is_err(), "expected Err for wrong config, got Ok");
    let err_msg = match result {
        Err(e) => format!("{e:?}"),
        Ok(_) => unreachable!(),
    };
    assert!(
        err_msg.contains("GuestPanic") || err_msg.contains("init()"),
        "error must indicate guest init() failure, got: {err_msg}"
    );
}
