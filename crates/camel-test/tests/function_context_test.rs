use camel_api::{FunctionDefinition, FunctionId};
use camel_core::CamelContext;
use camel_core::{BuilderStep, RouteDefinition};
use camel_function::provider::fake::{FakeProvider, FakeProviderConfig};
use camel_function::{FunctionConfig, FunctionRuntimeService};

#[tokio::test]
async fn with_lifecycle_after_build_propagates_function_invoker() {
    let provider = std::sync::Arc::new(FakeProvider::new(FakeProviderConfig::default()));
    let service = FunctionRuntimeService::with_fake_provider(FunctionConfig::default(), provider);

    let ctx = CamelContext::builder().build().await.unwrap();
    let ctx = ctx.with_lifecycle(service);

    let def = FunctionDefinition {
        id: FunctionId::compute("fake", "test", 5000),
        runtime: "fake".into(),
        source: "test".into(),
        timeout_ms: 5000,
        route_id: Some("r1".into()),
        step_index: Some(0),
    };
    let route_def = RouteDefinition::new(
        "mock:in",
        vec![BuilderStep::DeclarativeFunction { definition: def }],
    )
    .with_route_id("test-fn-route");

    let result = ctx.add_route_definition(route_def).await;
    if let Err(e) = result {
        let msg = e.to_string();
        assert!(
            !msg.contains("requires FunctionRuntimeService"),
            "function invoker was not propagated: {msg}"
        );
    }
}
