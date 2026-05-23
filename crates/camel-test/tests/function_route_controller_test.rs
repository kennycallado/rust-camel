use std::sync::Arc;

use camel_api::{FunctionDefinition, FunctionId, Lifecycle, NoopPlatformService};
use camel_core::DefaultRouteController;
use camel_core::{BuilderStep, Registry, RouteDefinition};
use camel_function::provider::fake::{FakeCall, FakeProvider, FakeProviderConfig};
use camel_function::{FunctionConfig, FunctionRuntimeService};

#[tokio::test(flavor = "multi_thread")]
async fn compile_route_definition_does_not_contaminate_staging_for_later_add_route() {
    let provider = std::sync::Arc::new(FakeProvider::new(FakeProviderConfig::default()));
    let mut service =
        FunctionRuntimeService::with_fake_provider(FunctionConfig::default(), provider.clone());
    Lifecycle::start(&mut service).await.unwrap();
    let invoker = service.invoker();

    let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
    let mut controller = DefaultRouteController::new(
        Arc::clone(&registry),
        Arc::new(NoopPlatformService::default()),
    );
    controller.set_function_invoker(invoker.clone());

    let compile_fn_id = FunctionId::compute("deno", "compile-only", 5000);
    let compile_def = RouteDefinition::new(
        "timer:tick",
        vec![BuilderStep::DeclarativeFunction {
            definition: FunctionDefinition {
                id: compile_fn_id.clone(),
                runtime: "fake".into(),
                source: "compile-only".into(),
                timeout_ms: 5000,
                route_id: None,
                step_index: None,
            },
        }],
    )
    .with_route_id("compile-route");

    let _pipeline = controller.compile_route_definition(compile_def).unwrap();

    let real_fn_id = FunctionId::compute("deno", "real-function", 5000);
    let add_def = RouteDefinition::new(
        "timer:tick",
        vec![BuilderStep::DeclarativeFunction {
            definition: FunctionDefinition {
                id: real_fn_id.clone(),
                runtime: "fake".into(),
                source: "real-function".into(),
                timeout_ms: 5000,
                route_id: None,
                step_index: None,
            },
        }],
    )
    .with_route_id("real-route");

    controller.add_route(add_def).await.unwrap();

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

    assert_eq!(
        register_calls.len(),
        1,
        "only real-route function should be registered via provider"
    );
    assert_eq!(
        register_calls[0], real_fn_id,
        "compile_route_definition's function must NOT leak into register calls"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn failed_add_route_discards_direct_add_function_staging() {
    let provider = std::sync::Arc::new(FakeProvider::new(FakeProviderConfig::default()));
    let mut service =
        FunctionRuntimeService::with_fake_provider(FunctionConfig::default(), provider.clone());
    Lifecycle::start(&mut service).await.unwrap();
    let invoker = service.invoker();

    let registry = Arc::new(std::sync::Mutex::new(Registry::new()));
    {
        let mut guard = registry.lock().expect("registry lock");
        guard.register(std::sync::Arc::new(
            camel_component_timer::TimerComponent::new(),
        ));
        guard.register(std::sync::Arc::new(
            camel_component_mock::MockComponent::new(),
        ));
    }
    let mut controller = DefaultRouteController::new(
        Arc::clone(&registry),
        Arc::new(NoopPlatformService::default()),
    );
    controller.set_function_invoker(invoker.clone());

    let failed_fn_id = FunctionId::compute("deno", "fn-failed-route", 5000);
    let failed_def = RouteDefinition::new(
        "timer:tick?period=1s",
        vec![
            BuilderStep::DeclarativeFunction {
                definition: FunctionDefinition {
                    id: failed_fn_id.clone(),
                    runtime: "fake".into(),
                    source: "fn-failed-route".into(),
                    timeout_ms: 5000,
                    route_id: None,
                    step_index: None,
                },
            },
            BuilderStep::To("unknown:out".into()),
        ],
    )
    .with_route_id("failed-route");

    let err = controller
        .add_route(failed_def)
        .await
        .expect_err("failed route must return error");
    assert!(!err.to_string().is_empty());

    let failed_staged = invoker.staged_refs_for_route("failed-route", 0);
    assert!(
        failed_staged.is_empty(),
        "failed-route staging[0] must be discarded, got: {:?}",
        failed_staged
    );

    let valid_fn_id = FunctionId::compute("deno", "fn-valid-route", 5000);
    let valid_def = RouteDefinition::new(
        "timer:tick?period=1s",
        vec![BuilderStep::DeclarativeFunction {
            definition: FunctionDefinition {
                id: valid_fn_id.clone(),
                runtime: "fake".into(),
                source: "fn-valid-route".into(),
                timeout_ms: 5000,
                route_id: None,
                step_index: None,
            },
        }],
    )
    .with_route_id("valid-route");

    controller.add_route(valid_def).await.unwrap();

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
        register_calls.contains(&valid_fn_id),
        "valid-route function must be registered"
    );
    assert!(
        !register_calls.contains(&failed_fn_id),
        "failed-route function must not be registered"
    );
}
