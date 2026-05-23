use std::sync::Arc;
use std::time::Duration;

use camel_api::function::FunctionId;
use camel_api::{RuntimeQuery, RuntimeQueryResult};
use camel_core::CamelContext;
use camel_core::{
    BuilderStep, FunctionReloadContext, ReloadAction, RouteDefinition, execute_reload_actions,
};
use camel_function::provider::fake::{FakeCall, FakeProvider, FakeProviderConfig};
use camel_function::{FunctionConfig, FunctionRuntimeService};

fn fn_def(name: &str, route_id: Option<&str>) -> camel_api::function::FunctionDefinition {
    camel_api::function::FunctionDefinition {
        id: camel_api::function::FunctionId::compute("deno", name, 5000),
        runtime: "fake".into(),
        source: name.into(),
        timeout_ms: 5000,
        route_id: route_id.map(|s| s.to_string()),
        step_index: None,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_execute_swap_with_function_steps_replaces_function() {
    let provider = Arc::new(FakeProvider::new(FakeProviderConfig::default()));
    let function_service =
        FunctionRuntimeService::with_fake_provider(FunctionConfig::default(), provider.clone());
    let invoker = function_service.invoker();

    let mut ctx = CamelContext::builder()
        .with_lifecycle(function_service)
        .build()
        .await
        .unwrap();
    ctx.register_component(camel_component_timer::TimerComponent::new());
    ctx.start().await.unwrap();

    let old_fn_id = FunctionId::compute("deno", "fn-old", 5000);
    let old_def = RouteDefinition::new(
        "timer:tick?period=50&repeatCount=1",
        vec![BuilderStep::DeclarativeFunction {
            definition: fn_def("fn-old", Some("swap-fn-route")),
        }],
    )
    .with_route_id("swap-fn-route");
    ctx.add_route_definition(old_def).await.unwrap();

    let register_calls: Vec<_> = provider
        .calls
        .lock()
        .unwrap()
        .iter()
        .filter_map(|c| match c {
            FakeCall::Register(_, id) => Some(id.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(register_calls.len(), 1);
    assert_eq!(register_calls[0], old_fn_id);

    let generation = invoker.begin_reload();
    invoker.stage_pending(
        fn_def("fn-new", Some("swap-fn-route")),
        Some("swap-fn-route"),
        generation,
    );

    let new_def = RouteDefinition::new(
        "timer:tick?period=50&repeatCount=1",
        vec![BuilderStep::DeclarativeFunction {
            definition: fn_def("fn-new", Some("swap-fn-route")),
        }],
    )
    .with_route_id("swap-fn-route");

    let actions = vec![ReloadAction::Swap {
        route_id: "swap-fn-route".into(),
    }];
    let function_ctx = FunctionReloadContext {
        invoker: invoker.clone(),
        generation,
    };
    let errors = execute_reload_actions(
        actions,
        vec![new_def],
        &ctx.runtime_execution_handle(),
        Duration::from_secs(10),
        Some(&function_ctx),
    )
    .await;
    assert!(errors.is_empty(), "Expected no errors, got: {:?}", errors);

    let register_calls: Vec<_> = provider
        .calls
        .lock()
        .unwrap()
        .iter()
        .filter_map(|c| match c {
            FakeCall::Register(_, id) => Some(id.clone()),
            _ => None,
        })
        .collect();
    let new_fn_id = FunctionId::compute("deno", "fn-new", 5000);
    assert!(
        register_calls.contains(&new_fn_id),
        "fn-new should be registered, calls: {:?}",
        register_calls
    );

    let unregister_calls: Vec<_> = provider
        .calls
        .lock()
        .unwrap()
        .iter()
        .filter_map(|c| match c {
            FakeCall::Unregister(_, id) => Some(id.clone()),
            _ => None,
        })
        .collect();
    assert!(
        unregister_calls.contains(&old_fn_id),
        "fn-old should be unregistered, calls: {:?}",
        unregister_calls
    );

    ctx.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn test_execute_add_with_function_step_registers_function() {
    let provider = Arc::new(FakeProvider::new(FakeProviderConfig::default()));
    let function_service =
        FunctionRuntimeService::with_fake_provider(FunctionConfig::default(), provider.clone());
    let invoker = function_service.invoker();

    let mut ctx = CamelContext::builder()
        .with_lifecycle(function_service)
        .build()
        .await
        .unwrap();
    ctx.register_component(camel_component_timer::TimerComponent::new());
    ctx.start().await.unwrap();

    let generation = invoker.begin_reload();
    invoker.stage_pending(
        fn_def("fn-a", Some("add-fn-route")),
        Some("add-fn-route"),
        generation,
    );

    let new_def = RouteDefinition::new(
        "timer:tick?period=50&repeatCount=1",
        vec![BuilderStep::DeclarativeFunction {
            definition: fn_def("fn-a", Some("add-fn-route")),
        }],
    )
    .with_route_id("add-fn-route");

    let actions = vec![ReloadAction::Add {
        route_id: "add-fn-route".into(),
    }];
    let function_ctx = FunctionReloadContext {
        invoker: invoker.clone(),
        generation,
    };
    let errors = execute_reload_actions(
        actions,
        vec![new_def],
        &ctx.runtime_execution_handle(),
        Duration::from_secs(10),
        Some(&function_ctx),
    )
    .await;
    assert!(errors.is_empty(), "Expected no errors, got: {:?}", errors);

    let fn_a_id = FunctionId::compute("deno", "fn-a", 5000);
    let register_calls: Vec<_> = provider
        .calls
        .lock()
        .unwrap()
        .iter()
        .filter_map(|c| match c {
            FakeCall::Register(_, id) => Some(id.clone()),
            _ => None,
        })
        .collect();
    assert!(
        register_calls.contains(&fn_a_id),
        "fn-a should be registered, calls: {:?}",
        register_calls
    );

    ctx.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn test_execute_remove_with_function_step_unregisters_function() {
    let provider = Arc::new(FakeProvider::new(FakeProviderConfig::default()));
    let function_service =
        FunctionRuntimeService::with_fake_provider(FunctionConfig::default(), provider.clone());
    let invoker = function_service.invoker();

    let mut ctx = CamelContext::builder()
        .with_lifecycle(function_service)
        .build()
        .await
        .unwrap();
    ctx.register_component(camel_component_timer::TimerComponent::new());
    ctx.start().await.unwrap();

    let old_fn_id = FunctionId::compute("deno", "fn-old", 5000);
    let def = RouteDefinition::new(
        "timer:tick?period=50&repeatCount=1",
        vec![BuilderStep::DeclarativeFunction {
            definition: fn_def("fn-old", Some("remove-fn-route")),
        }],
    )
    .with_route_id("remove-fn-route");
    ctx.add_route_definition(def).await.unwrap();

    let register_calls: Vec<_> = provider
        .calls
        .lock()
        .unwrap()
        .iter()
        .filter_map(|c| match c {
            FakeCall::Register(_, id) => Some(id.clone()),
            _ => None,
        })
        .collect();
    assert!(
        register_calls.contains(&old_fn_id),
        "fn-old should be registered initially"
    );

    let generation = invoker.begin_reload();

    let actions = vec![ReloadAction::Remove {
        route_id: "remove-fn-route".into(),
    }];
    let function_ctx = FunctionReloadContext {
        invoker: invoker.clone(),
        generation,
    };
    let errors = execute_reload_actions(
        actions,
        vec![],
        &ctx.runtime_execution_handle(),
        Duration::from_secs(10),
        Some(&function_ctx),
    )
    .await;
    assert!(errors.is_empty(), "Expected no errors, got: {:?}", errors);

    let unregister_calls: Vec<_> = provider
        .calls
        .lock()
        .unwrap()
        .iter()
        .filter_map(|c| match c {
            FakeCall::Unregister(_, id) => Some(id.clone()),
            _ => None,
        })
        .collect();
    assert!(
        unregister_calls.contains(&old_fn_id),
        "fn-old should be unregistered after remove, calls: {:?}",
        unregister_calls
    );

    ctx.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn test_swap_registers_new_function_before_pipeline_swap() {
    let provider = Arc::new(FakeProvider::new(FakeProviderConfig::default()));
    let function_service =
        FunctionRuntimeService::with_fake_provider(FunctionConfig::default(), provider.clone());
    let invoker = function_service.invoker();

    let mut ctx = CamelContext::builder()
        .with_lifecycle(function_service)
        .build()
        .await
        .unwrap();
    ctx.register_component(camel_component_timer::TimerComponent::new());
    ctx.start().await.unwrap();

    let old_fn_id = FunctionId::compute("deno", "fn-old", 5000);
    let new_fn_id = FunctionId::compute("deno", "fn-new", 5000);

    let old_def = RouteDefinition::new(
        "timer:tick?period=50&repeatCount=1",
        vec![BuilderStep::DeclarativeFunction {
            definition: fn_def("fn-old", Some("swap-order-route")),
        }],
    )
    .with_route_id("swap-order-route");
    ctx.add_route_definition(old_def).await.unwrap();

    provider.calls.lock().unwrap().clear();

    let generation = invoker.begin_reload();
    invoker.stage_pending(
        fn_def("fn-new", Some("swap-order-route")),
        Some("swap-order-route"),
        generation,
    );

    let new_def = RouteDefinition::new(
        "timer:tick?period=50&repeatCount=1",
        vec![BuilderStep::DeclarativeFunction {
            definition: fn_def("fn-new", Some("swap-order-route")),
        }],
    )
    .with_route_id("swap-order-route");

    let actions = vec![ReloadAction::Swap {
        route_id: "swap-order-route".into(),
    }];
    let function_ctx = FunctionReloadContext {
        invoker: invoker.clone(),
        generation,
    };
    let errors = execute_reload_actions(
        actions,
        vec![new_def],
        &ctx.runtime_execution_handle(),
        Duration::from_secs(10),
        Some(&function_ctx),
    )
    .await;
    assert!(errors.is_empty(), "Expected no errors, got: {:?}", errors);

    let calls = provider.calls.lock().unwrap().clone();
    let register_new_idx = calls
        .iter()
        .position(|c| matches!(c, FakeCall::Register(_, id) if *id == new_fn_id))
        .expect("fn-new should be registered");
    let unregister_old_idx = calls
        .iter()
        .position(|c| matches!(c, FakeCall::Unregister(_, id) if *id == old_fn_id))
        .expect("fn-old should be unregistered");
    assert!(
        register_new_idx < unregister_old_idx,
        "Register(fn-new) at {} should come before Unregister(fn-old) at {}",
        register_new_idx,
        unregister_old_idx
    );

    ctx.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn test_swap_rollback_on_pipeline_failure() {
    let provider = Arc::new(FakeProvider::new(FakeProviderConfig::default()));
    let function_service =
        FunctionRuntimeService::with_fake_provider(FunctionConfig::default(), provider.clone());
    let invoker = function_service.invoker();

    let mut ctx = CamelContext::builder()
        .with_lifecycle(function_service)
        .build()
        .await
        .unwrap();
    ctx.register_component(camel_component_timer::TimerComponent::new());
    ctx.start().await.unwrap();

    let new_fn_id = FunctionId::compute("deno", "fn-ghost", 5000);

    let generation = invoker.begin_reload();
    invoker.stage_pending(
        fn_def("fn-ghost", Some("nonexistent-route")),
        Some("nonexistent-route"),
        generation,
    );

    let new_def = RouteDefinition::new(
        "timer:tick?period=50&repeatCount=1",
        vec![BuilderStep::DeclarativeFunction {
            definition: fn_def("fn-ghost", Some("nonexistent-route")),
        }],
    )
    .with_route_id("nonexistent-route");

    let actions = vec![ReloadAction::Swap {
        route_id: "nonexistent-route".into(),
    }];
    let function_ctx = FunctionReloadContext {
        invoker: invoker.clone(),
        generation,
    };
    let errors = execute_reload_actions(
        actions,
        vec![new_def],
        &ctx.runtime_execution_handle(),
        Duration::from_secs(10),
        Some(&function_ctx),
    )
    .await;
    assert!(
        !errors.is_empty(),
        "Expected errors from swapping nonexistent route"
    );

    let calls = provider.calls.lock().unwrap().clone();
    let register_idx = calls
        .iter()
        .position(|c| matches!(c, FakeCall::Register(_, id) if *id == new_fn_id));
    let unregister_idx = calls
        .iter()
        .position(|c| matches!(c, FakeCall::Unregister(_, id) if *id == new_fn_id));

    if let (Some(reg), Some(unreg)) = (register_idx, unregister_idx) {
        assert!(
            reg < unreg,
            "Register at {} should precede Unregister (rollback) at {}",
            reg,
            unreg
        );
    }

    ctx.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn test_add_via_hot_reload_uses_generation_not_staging_zero() {
    let provider = Arc::new(FakeProvider::new(FakeProviderConfig::default()));
    let function_service =
        FunctionRuntimeService::with_fake_provider(FunctionConfig::default(), provider.clone());
    let invoker = function_service.invoker();

    let mut ctx = CamelContext::builder()
        .with_lifecycle(function_service)
        .build()
        .await
        .unwrap();
    ctx.register_component(camel_component_timer::TimerComponent::new());
    ctx.start().await.unwrap();

    let fn_a_id = FunctionId::compute("deno", "fn-addgen", 5000);

    let generation = invoker.begin_reload();
    invoker.stage_pending(
        fn_def("fn-addgen", Some("add-gen-route")),
        Some("add-gen-route"),
        generation,
    );

    let new_def = RouteDefinition::new(
        "timer:tick?period=50&repeatCount=1",
        vec![BuilderStep::DeclarativeFunction {
            definition: fn_def("fn-addgen", Some("add-gen-route")),
        }],
    )
    .with_route_id("add-gen-route");

    let actions = vec![ReloadAction::Add {
        route_id: "add-gen-route".into(),
    }];
    let function_ctx = FunctionReloadContext {
        invoker: invoker.clone(),
        generation,
    };
    let errors = execute_reload_actions(
        actions,
        vec![new_def],
        &ctx.runtime_execution_handle(),
        Duration::from_secs(10),
        Some(&function_ctx),
    )
    .await;
    assert!(errors.is_empty(), "Expected no errors, got: {:?}", errors);

    let calls = provider.calls.lock().unwrap().clone();
    let registered = calls
        .iter()
        .any(|c| matches!(c, FakeCall::Register(_, id) if *id == fn_a_id));
    assert!(registered, "fn-addgen should be registered");

    let staging0_refs = invoker.staged_refs_for_route("add-gen-route", 0);
    assert!(
        staging0_refs.is_empty(),
        "staging[0] should be empty, got: {:?}",
        staging0_refs
    );

    ctx.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn test_restart_registers_added_before_removing_old() {
    let provider = Arc::new(FakeProvider::new(FakeProviderConfig::default()));
    let function_service =
        FunctionRuntimeService::with_fake_provider(FunctionConfig::default(), provider.clone());
    let invoker = function_service.invoker();

    let mut ctx = CamelContext::builder()
        .with_lifecycle(function_service)
        .build()
        .await
        .unwrap();
    ctx.register_component(camel_component_timer::TimerComponent::new());
    ctx.start().await.unwrap();

    let old_fn_id = FunctionId::compute("deno", "fn-restart-old", 5000);
    let new_fn_id = FunctionId::compute("deno", "fn-restart-new", 5000);

    let old_def = RouteDefinition::new(
        "timer:tick?period=50&repeatCount=1",
        vec![BuilderStep::DeclarativeFunction {
            definition: fn_def("fn-restart-old", Some("restart-order-route")),
        }],
    )
    .with_route_id("restart-order-route");
    ctx.add_route_definition(old_def).await.unwrap();

    provider.calls.lock().unwrap().clear();

    let generation = invoker.begin_reload();
    invoker.stage_pending(
        fn_def("fn-restart-new", Some("restart-order-route")),
        Some("restart-order-route"),
        generation,
    );

    let new_def = RouteDefinition::new(
        "timer:tock?period=50&repeatCount=1",
        vec![BuilderStep::DeclarativeFunction {
            definition: fn_def("fn-restart-new", Some("restart-order-route")),
        }],
    )
    .with_route_id("restart-order-route");

    let actions = vec![ReloadAction::Restart {
        route_id: "restart-order-route".into(),
    }];
    let function_ctx = FunctionReloadContext {
        invoker: invoker.clone(),
        generation,
    };
    let errors = execute_reload_actions(
        actions,
        vec![new_def],
        &ctx.runtime_execution_handle(),
        Duration::from_secs(10),
        Some(&function_ctx),
    )
    .await;
    assert!(errors.is_empty(), "Expected no errors, got: {:?}", errors);

    let calls = provider.calls.lock().unwrap().clone();
    let register_new_idx = calls
        .iter()
        .position(|c| matches!(c, FakeCall::Register(_, id) if *id == new_fn_id))
        .expect("fn-restart-new should be registered");
    let unregister_old_idx = calls
        .iter()
        .position(|c| matches!(c, FakeCall::Unregister(_, id) if *id == old_fn_id))
        .expect("fn-restart-old should be unregistered");
    assert!(
        register_new_idx < unregister_old_idx,
        "Register(fn-restart-new) at {} must precede Unregister(fn-restart-old) at {}",
        register_new_idx,
        unregister_old_idx
    );

    ctx.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn test_restart_with_function_change_full_lifecycle() {
    let provider = Arc::new(FakeProvider::new(FakeProviderConfig::default()));
    let function_service =
        FunctionRuntimeService::with_fake_provider(FunctionConfig::default(), provider.clone());
    let invoker = function_service.invoker();

    let mut ctx = CamelContext::builder()
        .with_lifecycle(function_service)
        .build()
        .await
        .unwrap();
    ctx.register_component(camel_component_timer::TimerComponent::new());
    ctx.start().await.unwrap();

    let old_fn_id = FunctionId::compute("deno", "fn-life-old", 5000);
    let new_fn_id = FunctionId::compute("deno", "fn-life-new", 5000);

    let old_def = RouteDefinition::new(
        "timer:tick?period=50&repeatCount=1",
        vec![BuilderStep::DeclarativeFunction {
            definition: fn_def("fn-life-old", Some("restart-life-route")),
        }],
    )
    .with_route_id("restart-life-route");
    ctx.add_route_definition(old_def).await.unwrap();

    let initial_calls = provider.calls.lock().unwrap().clone();
    let initial_register = initial_calls
        .iter()
        .any(|c| matches!(c, FakeCall::Register(_, id) if *id == old_fn_id));
    assert!(
        initial_register,
        "fn-life-old should be registered initially"
    );

    provider.calls.lock().unwrap().clear();

    let generation = invoker.begin_reload();
    invoker.stage_pending(
        fn_def("fn-life-new", Some("restart-life-route")),
        Some("restart-life-route"),
        generation,
    );

    let new_def = RouteDefinition::new(
        "timer:tock?period=50&repeatCount=1",
        vec![BuilderStep::DeclarativeFunction {
            definition: fn_def("fn-life-new", Some("restart-life-route")),
        }],
    )
    .with_route_id("restart-life-route");

    let actions = vec![ReloadAction::Restart {
        route_id: "restart-life-route".into(),
    }];
    let function_ctx = FunctionReloadContext {
        invoker: invoker.clone(),
        generation,
    };
    let errors = execute_reload_actions(
        actions,
        vec![new_def],
        &ctx.runtime_execution_handle(),
        Duration::from_secs(10),
        Some(&function_ctx),
    )
    .await;
    assert!(errors.is_empty(), "Expected no errors, got: {:?}", errors);

    let calls = provider.calls.lock().unwrap().clone();
    let register_new = calls
        .iter()
        .any(|c| matches!(c, FakeCall::Register(_, id) if *id == new_fn_id));
    assert!(
        register_new,
        "fn-life-new should be registered after restart"
    );

    let unregister_old = calls
        .iter()
        .any(|c| matches!(c, FakeCall::Unregister(_, id) if *id == old_fn_id));
    assert!(
        unregister_old,
        "fn-life-old should be unregistered after restart"
    );

    drop(calls);

    let after = ctx
        .runtime()
        .ask(RuntimeQuery::GetRouteStatus {
            route_id: "restart-life-route".into(),
        })
        .await
        .unwrap();
    match after {
        RuntimeQueryResult::RouteStatus { status, .. } => {
            assert_eq!(status, "Registered", "route should be in Registered state");
        }
        other => panic!("unexpected query result: {other:?}"),
    }

    ctx.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn test_add_with_prepare_failure_never_inserts_route() {
    let provider = Arc::new(FakeProvider::new(FakeProviderConfig::default()));
    let function_service =
        FunctionRuntimeService::with_fake_provider(FunctionConfig::default(), provider.clone());
    let invoker = function_service.invoker();

    let mut ctx = CamelContext::builder()
        .with_lifecycle(function_service)
        .build()
        .await
        .unwrap();
    ctx.register_component(camel_component_timer::TimerComponent::new());
    ctx.start().await.unwrap();

    provider.config.lock().unwrap().fail_on_register = 1;

    let generation = invoker.begin_reload();
    invoker.stage_pending(
        fn_def("fn-first", Some("add-fail-route")),
        Some("add-fail-route"),
        generation,
    );
    invoker.stage_pending(
        fn_def("fn-second", Some("add-fail-route")),
        Some("add-fail-route"),
        generation,
    );

    let new_def = RouteDefinition::new(
        "timer:tick?period=50&repeatCount=1",
        vec![
            BuilderStep::DeclarativeFunction {
                definition: fn_def("fn-first", Some("add-fail-route")),
            },
            BuilderStep::DeclarativeFunction {
                definition: fn_def("fn-second", Some("add-fail-route")),
            },
        ],
    )
    .with_route_id("add-fail-route");

    let actions = vec![ReloadAction::Add {
        route_id: "add-fail-route".into(),
    }];
    let function_ctx = FunctionReloadContext {
        invoker: invoker.clone(),
        generation,
    };
    let errors = execute_reload_actions(
        actions,
        vec![new_def],
        &ctx.runtime_execution_handle(),
        Duration::from_secs(10),
        Some(&function_ctx),
    )
    .await;
    assert_eq!(errors.len(), 1, "Expected one error, got: {:?}", errors);
    assert!(
        errors[0].action.contains("prepare"),
        "Expected prepare error, got: {}",
        errors[0].action
    );

    assert_eq!(
        ctx.runtime_execution_handle()
            .controller_route_count_for_test()
            .await,
        0,
        "Route should NOT exist after prepare failure"
    );

    ctx.stop().await.unwrap();
}
