use camel_api::function::{FunctionDefinition, FunctionId};
use camel_api::lifecycle::Lifecycle;
use camel_api::{Exchange, Message};
use camel_function::provider::fake::{FakeCall, FakeProvider, FakeProviderConfig};
use camel_function::{FunctionConfig, FunctionRuntimeService};
use std::sync::Arc;

fn def(name: &str) -> FunctionDefinition {
    FunctionDefinition {
        id: FunctionId::compute("fake", name, 5000),
        runtime: "fake".into(),
        source: name.into(),
        timeout_ms: 5000,
        route_id: Some("r1".into()),
        step_index: Some(0),
    }
}

#[tokio::test]
async fn pre_start_pending_start_registers_and_invokes() {
    let provider = Arc::new(FakeProvider::new(FakeProviderConfig::default()));
    let mut service = FunctionRuntimeService::with_fake_provider(
        FunctionConfig::default(),
        Arc::clone(&provider),
    );
    let inv = service.invoker();
    let d1 = def("a");
    let d2 = def("b");
    inv.stage_pending(d1.clone(), Some("r1"), 0);
    inv.stage_pending(d2.clone(), Some("r1"), 0);
    service.start().await.unwrap();
    let _ = inv
        .invoke(&d1.id, &Exchange::new(Message::new("x")))
        .await
        .unwrap();
    let calls = provider.calls.lock().expect("calls").clone();
    assert!(
        calls
            .iter()
            .any(|c| matches!(c, FakeCall::Register(_, id) if *id == d1.id))
    );
    assert!(
        calls
            .iter()
            .any(|c| matches!(c, FakeCall::Register(_, id) if *id == d2.id))
    );
}

#[tokio::test]
async fn pre_start_register_failure_rolls_back_and_keeps_pending() {
    let provider = Arc::new(FakeProvider::new(FakeProviderConfig {
        fail_on_register: 1,
        ..Default::default()
    }));
    let mut service = FunctionRuntimeService::with_fake_provider(
        FunctionConfig::default(),
        Arc::clone(&provider),
    );
    let inv = service.invoker();
    inv.stage_pending(def("a"), Some("r1"), 0);
    inv.stage_pending(def("b"), Some("r1"), 0);
    assert!(service.start().await.is_err());
    assert!(service.runner_state("fake").is_none());
    let calls = provider.calls.lock().expect("calls").clone();
    assert!(calls.iter().any(|c| matches!(c, FakeCall::Shutdown(_))));
}

#[tokio::test]
async fn start_is_idempotent() {
    let provider = Arc::new(FakeProvider::new(FakeProviderConfig::default()));
    let mut service =
        FunctionRuntimeService::with_fake_provider(FunctionConfig::default(), provider);
    let inv = service.invoker();
    inv.stage_pending(def("idem"), Some("r1"), 0);
    service.start().await.unwrap();
    service.start().await.unwrap();
    assert!(service.runner_state("fake").is_some());
}

#[tokio::test]
async fn post_start_register_is_immediate_and_invocable() {
    let provider = Arc::new(FakeProvider::new(FakeProviderConfig::default()));
    let mut service = FunctionRuntimeService::with_fake_provider(
        FunctionConfig::default(),
        Arc::clone(&provider),
    );
    service.start().await.unwrap();
    let inv = service.invoker();
    let d = def("p");
    inv.register(d.clone(), Some("r1")).await.unwrap();
    inv.invoke(&d.id, &Exchange::new(Message::new("x")))
        .await
        .unwrap();
}

#[tokio::test]
async fn boot_timeout_errors_and_no_leak() {
    let provider = Arc::new(FakeProvider::new(FakeProviderConfig {
        fail_on_health: true,
        ..Default::default()
    }));
    let cfg = FunctionConfig {
        health_interval: std::time::Duration::from_millis(20),
        boot_timeout: std::time::Duration::from_millis(80),
        ..Default::default()
    };
    let mut service = FunctionRuntimeService::with_fake_provider(cfg, Arc::clone(&provider));
    let inv = service.invoker();
    inv.stage_pending(def("a"), Some("r1"), 0);
    assert!(service.start().await.is_err());
    assert!(service.runner_state("fake").is_none());
}

#[tokio::test]
async fn stop_shutdowns_and_invoke_unavailable() {
    let provider = Arc::new(FakeProvider::new(FakeProviderConfig::default()));
    let mut service = FunctionRuntimeService::with_fake_provider(
        FunctionConfig::default(),
        Arc::clone(&provider),
    );
    let inv = service.invoker();
    let d = def("s");
    inv.stage_pending(d.clone(), Some("r1"), 0);
    service.start().await.unwrap();
    service.stop().await.unwrap();
    let err = inv
        .invoke(&d.id, &Exchange::new(Message::new("x")))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("not registered") || err.to_string().contains("unavailable"));
}

#[tokio::test]
async fn health_task_transitions_to_unhealthy() {
    let provider = Arc::new(FakeProvider::new(FakeProviderConfig::default()));
    let cfg = FunctionConfig {
        health_interval: std::time::Duration::from_millis(20),
        boot_timeout: std::time::Duration::from_millis(200),
        ..Default::default()
    };
    let mut service = FunctionRuntimeService::with_fake_provider(cfg, Arc::clone(&provider));
    let inv = service.invoker();
    inv.stage_pending(def("h"), Some("r1"), 0);
    service.start().await.unwrap();
    provider.config.lock().expect("config").fail_on_health = true;
    tokio::time::sleep(std::time::Duration::from_millis(70)).await;
    let state = service.runner_state("fake").unwrap();
    assert!(matches!(
        state,
        camel_function::RunnerState::Unhealthy { .. } | camel_function::RunnerState::Failed { .. }
    ));
}

#[tokio::test]
async fn failed_state_invoke_returns_unavailable() {
    let provider = Arc::new(FakeProvider::new(FakeProviderConfig::default()));
    let mut service = FunctionRuntimeService::with_fake_provider(
        FunctionConfig::default(),
        Arc::clone(&provider),
    );
    let inv = service.invoker();
    let d = def("f");
    inv.stage_pending(d.clone(), Some("r1"), 0);
    service.start().await.unwrap();
    service.force_runner_failed("fake", "boom");
    let err = inv
        .invoke(&d.id, &Exchange::new(Message::new("x")))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("runner unavailable"));
}
