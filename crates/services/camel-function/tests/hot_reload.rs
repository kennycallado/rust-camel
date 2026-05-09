use camel_api::Exchange;
use camel_api::function::*;
use camel_api::lifecycle::Lifecycle;
use camel_function::provider::fake::{FakeCall, FakeProvider, FakeProviderConfig};
use camel_function::{FunctionConfig, FunctionRuntimeService};
use std::sync::Arc;

fn make_service_with_config(
    config: FakeProviderConfig,
) -> (
    Arc<FakeProvider>,
    FunctionRuntimeService,
    Arc<dyn FunctionInvoker>,
) {
    let provider = Arc::new(FakeProvider::new(config));
    let mut service =
        FunctionRuntimeService::with_fake_provider(FunctionConfig::default(), provider.clone());
    let invoker = service.invoker();
    (provider, service, invoker)
}

fn fn_def(name: &str, route_id: Option<&str>) -> FunctionDefinition {
    FunctionDefinition {
        id: FunctionId::compute("deno", name, 5000),
        runtime: "fake".into(),
        source: name.into(),
        timeout_ms: 5000,
        route_id: route_id.map(|s| s.to_string()),
        step_index: None,
    }
}

fn fn_id(name: &str) -> FunctionId {
    FunctionId::compute("deno", name, 5000)
}

async fn started_service(
    config: FakeProviderConfig,
) -> (
    Arc<FakeProvider>,
    FunctionRuntimeService,
    Arc<dyn FunctionInvoker>,
) {
    let (provider, mut service, invoker) = make_service_with_config(config);
    Lifecycle::start(&mut service).await.unwrap();
    (provider, service, invoker)
}

#[tokio::test]
async fn prepare_reload_registers_added_functions() {
    let (provider, _service, invoker) = started_service(FakeProviderConfig::default()).await;

    let epoch = invoker.begin_reload();
    invoker.stage_pending(fn_def("fn-a", Some("r1")), Some("r1"), epoch);

    let diff = FunctionDiff {
        added: vec![(fn_def("fn-a", Some("r1")), Some("r1".into()))],
        removed: vec![],
        unchanged: vec![],
    };
    let token = invoker.prepare_reload(diff, epoch).await.unwrap();
    assert_eq!(token.registered.len(), 1);

    let calls = provider.calls.lock().expect("calls").clone();
    let register_count = calls
        .iter()
        .filter(|c| matches!(c, FakeCall::Register(_, id) if *id == fn_id("fn-a")))
        .count();
    assert_eq!(register_count, 1);
}

#[tokio::test]
async fn prepare_reload_rolls_back_on_register_failure() {
    let (_provider, _service, invoker) = started_service(FakeProviderConfig {
        fail_on_register: 1,
        ..Default::default()
    })
    .await;

    let def_a = fn_def("fn-a", Some("r1"));
    invoker.register(def_a.clone(), Some("r1")).await.unwrap();

    let epoch = invoker.begin_reload();
    let diff = FunctionDiff {
        added: vec![(fn_def("fn-b", Some("r1")), Some("r1".into()))],
        removed: vec![],
        unchanged: vec![],
    };
    let result = invoker.prepare_reload(diff, epoch).await;
    assert!(result.is_err());

    let invoke_a = invoker.invoke(&fn_id("fn-a"), &Exchange::default()).await;
    assert!(invoke_a.is_ok());

    let refs = invoker.function_refs_for_route("r1");
    assert!(refs.iter().any(|(id, _)| *id == fn_id("fn-a")));
    assert!(!refs.iter().any(|(id, _)| *id == fn_id("fn-b")));
}

#[tokio::test]
async fn finalize_reload_unregisters_removed() {
    let (provider, _service, invoker) = started_service(FakeProviderConfig::default()).await;

    let def_a = fn_def("fn-a", Some("r1"));
    invoker.register(def_a.clone(), Some("r1")).await.unwrap();

    let epoch = invoker.begin_reload();
    let diff = FunctionDiff {
        added: vec![],
        removed: vec![(fn_id("fn-a"), Some("r1".into()))],
        unchanged: vec![],
    };
    invoker.finalize_reload(&diff, epoch).await.unwrap();

    let calls = provider.calls.lock().expect("calls").clone();
    let unregister_count = calls
        .iter()
        .filter(|c| matches!(c, FakeCall::Unregister(_, id) if *id == fn_id("fn-a")))
        .count();
    assert_eq!(unregister_count, 1);

    let err = invoker
        .invoke(&fn_id("fn-a"), &Exchange::default())
        .await
        .unwrap_err();
    assert!(matches!(err, FunctionInvocationError::NotRegistered { .. }));
}

#[tokio::test]
async fn rollback_reload_unregisters_prepared_functions() {
    let (provider, _service, invoker) = started_service(FakeProviderConfig::default()).await;

    let epoch = invoker.begin_reload();
    let diff = FunctionDiff {
        added: vec![(fn_def("fn-new", Some("r1")), Some("r1".into()))],
        removed: vec![],
        unchanged: vec![],
    };
    let token = invoker.prepare_reload(diff, epoch).await.unwrap();
    invoker.rollback_reload(token, epoch).await.unwrap();

    let calls = provider.calls.lock().expect("calls").clone();
    let last_unregister = calls
        .iter()
        .rev()
        .find(|c| matches!(c, FakeCall::Unregister(_, id) if *id == fn_id("fn-new")));
    assert!(last_unregister.is_some());
}

#[tokio::test]
async fn stale_generation_rejected() {
    let (provider, _service, invoker) = started_service(FakeProviderConfig::default()).await;

    let epoch1 = invoker.begin_reload();
    let _epoch2 = invoker.begin_reload();

    let diff = FunctionDiff {
        added: vec![(fn_def("fn-a", Some("r1")), Some("r1".into()))],
        removed: vec![],
        unchanged: vec![],
    };
    let result = invoker.prepare_reload(diff, epoch1).await;
    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(err_msg.contains("stale generation"));

    let calls = provider.calls.lock().expect("calls").clone();
    let register_count = calls
        .iter()
        .filter(|c| matches!(c, FakeCall::Register(_, id) if *id == fn_id("fn-a")))
        .count();
    assert_eq!(register_count, 0);
}

#[tokio::test]
async fn stale_generation_buffer_purged() {
    let (_provider, _service, invoker) = started_service(FakeProviderConfig::default()).await;

    let epoch1 = invoker.begin_reload();
    invoker.stage_pending(fn_def("fn-a", Some("r1")), Some("r1"), epoch1);

    let epoch2 = invoker.begin_reload();
    invoker.stage_pending(fn_def("fn-b", Some("r2")), Some("r2"), epoch2);

    let diff = FunctionDiff {
        added: vec![(fn_def("fn-b", Some("r2")), Some("r2".into()))],
        removed: vec![],
        unchanged: vec![],
    };
    invoker.prepare_reload(diff, epoch2).await.unwrap();

    let staged_r1 = invoker.staged_refs_for_route("r1", epoch1);
    assert!(staged_r1.is_empty());
}

#[tokio::test]
async fn three_phase_swap_added_then_removed() {
    let (_provider, _service, invoker) = started_service(FakeProviderConfig::default()).await;

    let def_old = fn_def("fn-old", Some("r1"));
    invoker.register(def_old.clone(), Some("r1")).await.unwrap();

    let epoch = invoker.begin_reload();
    let def_new = fn_def("fn-new", Some("r1"));
    invoker.stage_pending(def_new.clone(), Some("r1"), epoch);

    let diff = FunctionDiff {
        added: vec![(def_new.clone(), Some("r1".into()))],
        removed: vec![(fn_id("fn-old"), Some("r1".into()))],
        unchanged: vec![],
    };
    let token = invoker.prepare_reload(diff.clone(), epoch).await.unwrap();
    assert_eq!(token.registered.len(), 1);

    invoker.finalize_reload(&diff, epoch).await.unwrap();

    let invoke_new = invoker.invoke(&fn_id("fn-new"), &Exchange::default()).await;
    assert!(invoke_new.is_ok());

    let err = invoker
        .invoke(&fn_id("fn-old"), &Exchange::default())
        .await
        .unwrap_err();
    assert!(matches!(err, FunctionInvocationError::NotRegistered { .. }));
}

#[tokio::test]
async fn swap_failure_triggers_rollback_old_untouched() {
    let (provider, _service, invoker) = started_service(FakeProviderConfig::default()).await;

    let def_old = fn_def("fn-old", Some("r1"));
    invoker.register(def_old.clone(), Some("r1")).await.unwrap();

    let epoch = invoker.begin_reload();
    let def_new = fn_def("fn-new", Some("r1"));
    let diff = FunctionDiff {
        added: vec![(def_new.clone(), Some("r1".into()))],
        removed: vec![(fn_id("fn-old"), Some("r1".into()))],
        unchanged: vec![],
    };
    let token = invoker.prepare_reload(diff, epoch).await.unwrap();
    invoker.rollback_reload(token, epoch).await.unwrap();

    let invoke_old = invoker.invoke(&fn_id("fn-old"), &Exchange::default()).await;
    assert!(invoke_old.is_ok());

    let err = invoker
        .invoke(&fn_id("fn-new"), &Exchange::default())
        .await
        .unwrap_err();
    assert!(matches!(err, FunctionInvocationError::NotRegistered { .. }));

    let calls = provider.calls.lock().expect("calls").clone();
    let unreg_new = calls
        .iter()
        .filter(|c| matches!(c, FakeCall::Unregister(_, id) if *id == fn_id("fn-new")))
        .count();
    assert_eq!(unreg_new, 1);

    let unreg_old = calls
        .iter()
        .filter(|c| matches!(c, FakeCall::Unregister(_, id) if *id == fn_id("fn-old")))
        .count();
    assert_eq!(unreg_old, 0);
}

#[tokio::test]
async fn added_failure_rolls_back_old_untouched() {
    let (_provider, _service, invoker) = started_service(FakeProviderConfig {
        fail_on_register: 1,
        ..Default::default()
    })
    .await;

    let def_old = fn_def("fn-old", Some("r1"));
    invoker.register(def_old.clone(), Some("r1")).await.unwrap();

    let epoch = invoker.begin_reload();
    let def_new = fn_def("fn-new", Some("r1"));
    let diff = FunctionDiff {
        added: vec![(def_new.clone(), Some("r1".into()))],
        removed: vec![(fn_id("fn-old"), Some("r1".into()))],
        unchanged: vec![],
    };
    let result = invoker.prepare_reload(diff, epoch).await;
    assert!(result.is_err());

    let invoke_old = invoker.invoke(&fn_id("fn-old"), &Exchange::default()).await;
    assert!(invoke_old.is_ok());
}

#[tokio::test]
async fn partial_added_failure_rolls_back_all() {
    let (provider, _service, invoker) = started_service(FakeProviderConfig {
        fail_on_register: 1,
        ..Default::default()
    })
    .await;

    let epoch = invoker.begin_reload();
    let def_a = fn_def("fn-a", Some("r1"));
    let def_b = fn_def("fn-b", Some("r1"));
    let diff = FunctionDiff {
        added: vec![
            (def_a.clone(), Some("r1".into())),
            (def_b.clone(), Some("r1".into())),
        ],
        removed: vec![],
        unchanged: vec![],
    };
    let result = invoker.prepare_reload(diff, epoch).await;
    assert!(result.is_err());

    let calls = provider.calls.lock().expect("calls").clone();
    let unreg_a = calls
        .iter()
        .filter(|c| matches!(c, FakeCall::Unregister(_, id) if *id == fn_id("fn-a")))
        .count();
    assert_eq!(unreg_a, 1);

    let err_a = invoker
        .invoke(&fn_id("fn-a"), &Exchange::default())
        .await
        .unwrap_err();
    assert!(matches!(
        err_a,
        FunctionInvocationError::NotRegistered { .. }
    ));

    let err_b = invoker
        .invoke(&fn_id("fn-b"), &Exchange::default())
        .await
        .unwrap_err();
    assert!(matches!(
        err_b,
        FunctionInvocationError::NotRegistered { .. }
    ));
}

#[tokio::test]
async fn discard_staging_drops_buffer_current_untouched() {
    let (_provider, _service, invoker) = started_service(FakeProviderConfig::default()).await;

    let def_a = fn_def("fn-a", Some("r1"));
    invoker.register(def_a.clone(), Some("r1")).await.unwrap();

    let epoch = invoker.begin_reload();
    invoker.stage_pending(fn_def("fn-b", Some("r1")), Some("r1"), epoch);

    invoker.discard_staging(epoch);
    let staged = invoker.staged_refs_for_route("r1", epoch);
    assert!(staged.is_empty());

    let invoke_a = invoker.invoke(&fn_id("fn-a"), &Exchange::default()).await;
    assert!(invoke_a.is_ok());
}
