//! Emission-proof task 2.1: wasm invoke emission legs. Each test drives
//! the wasm producer through a full invoke and asserts what the
//! component emits on the [`RuntimeObservability`] port: the
//! `("wasm", "invoke", "success")` operation on the happy path, the
//! `("wasm", "e:wasm:invoke")` error family plus
//! `("wasm", "invoke", "failure")` operation on a broken guest, and the
//! error family alone when the components lever is off.

use std::{fs, path::PathBuf, sync::Arc};

use camel_api::{CamelError, Exchange, Message, ProducerContext};
use camel_component_api::test_support::RecordingRuntimeObservability;
use camel_component_api::{Component, NoOpComponentContext, RuntimeObservability};
use camel_component_wasm::WasmComponent;
use tempfile::tempdir;
use tower::ServiceExt;

/// Pre-built echo guest shared with `camel-integration-test` (the same
/// module `wasm_boot_test.rs` drives end to end).
fn guest_src() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../camel-integration-test/tests/fixtures/wasm/echo.wasm")
}

/// Drives one wasm producer call end to end against `base`:
/// component → endpoint → producer → a single `hello-wasm` oneshot
/// (the exact payload shape the echo guest processes in
/// `crates/camel-integration-test/tests/wasm_boot_test.rs`).
async fn drive(
    base: &std::path::Path,
    guest_name: &str,
    rt: Arc<dyn RuntimeObservability>,
) -> Result<Exchange, CamelError> {
    let component = WasmComponent::new(Arc::new(NoOpComponentContext), base.to_path_buf());
    let endpoint = component
        .create_endpoint(&format!("wasm:{guest_name}"), &NoOpComponentContext)
        .expect("wasm endpoint must be created for an existing module file");
    let producer = endpoint
        .create_producer(rt, &ProducerContext::new())
        .expect("wasm producer must be created for the source world");
    producer
        .clone()
        .oneshot(Exchange::new(Message::new("hello-wasm")))
        .await
}

#[tokio::test]
async fn wasm_invoke_success_emits_component_operation() {
    let base = tempdir().expect("temp dir");
    fs::copy(guest_src(), base.path().join("echo.wasm")).expect("copy echo guest");
    let rt = RecordingRuntimeObservability::new(true);

    let result = drive(base.path(), "echo.wasm", rt.clone()).await;

    assert!(result.is_ok(), "echo guest invoke must succeed: {result:?}");
    assert_eq!(
        rt.ops(),
        vec![(
            "wasm".to_string(),
            "invoke".to_string(),
            "success".to_string()
        )]
    );
    assert!(rt.errors().is_empty());
}

#[tokio::test]
async fn wasm_invoke_failure_emits_error_family_and_failure_op() {
    let base = tempdir().expect("temp dir");
    fs::write(
        base.path().join("not-module.wasm"),
        b"this-is-not-a-wasm-component",
    )
    .expect("write invalid module");
    let rt = RecordingRuntimeObservability::new(true);

    let result = drive(base.path(), "not-module.wasm", rt.clone()).await;

    assert!(
        matches!(&result, Err(CamelError::Config(msg)) if msg.contains("wasm compilation failed")),
        "invalid module must surface CamelError::Config: {result:?}"
    );
    assert!(
        rt.errors()
            .contains(&("wasm".to_string(), "e:wasm:invoke".to_string())),
        "error family must be emitted with the lever on: {:?}",
        rt.errors()
    );
    assert!(
        rt.ops().contains(&(
            "wasm".to_string(),
            "invoke".to_string(),
            "failure".to_string()
        )),
        "failure operation must be emitted with the lever on: {:?}",
        rt.ops()
    );
}

#[tokio::test]
async fn wasm_invoke_failure_with_lever_off_still_emits_error_family() {
    let base = tempdir().expect("temp dir");
    fs::write(
        base.path().join("not-module.wasm"),
        b"this-is-not-a-wasm-component",
    )
    .expect("write invalid module");
    let rt = RecordingRuntimeObservability::new(false);

    let result = drive(base.path(), "not-module.wasm", rt.clone()).await;

    assert!(
        matches!(&result, Err(CamelError::Config(msg)) if msg.contains("wasm compilation failed")),
        "invalid module must surface CamelError::Config: {result:?}"
    );
    assert!(
        rt.errors()
            .contains(&("wasm".to_string(), "e:wasm:invoke".to_string())),
        "error family must be emitted even with the lever off: {:?}",
        rt.errors()
    );
    assert!(
        rt.ops().is_empty(),
        "success/failure operations must be suppressed with the lever off: {:?}",
        rt.ops()
    );
}
