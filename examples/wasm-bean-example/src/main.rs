use std::sync::Arc;

use camel_bean::{BeanProcessor, BeanRegistry};
use camel_builder::StepAccumulator;
use camel_component_log::LogComponent;
use camel_component_timer::TimerComponent;
use camel_component_wasm::bean::WasmBean;
use camel_core::context::CamelContext;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt().with_target(false).init();

    let fixtures_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures");

    let mut bean_registry = BeanRegistry::new();

    let wasm_path = fixtures_dir.join("text_utils.wasm");
    let registry_arc = Arc::new(std::sync::Mutex::new(camel_core::Registry::new()));
    let bean_config: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let wasm_bean = WasmBean::new(
        &wasm_path,
        camel_component_wasm::WasmConfig::default(),
        registry_arc,
        bean_config,
    )
    .await?;

    println!("bean methods: {:?}", wasm_bean.methods());

    bean_registry.register("text-utils", wasm_bean)?;
    let beans = Arc::new(std::sync::Mutex::new(bean_registry));

    let mut ctx = CamelContext::builder().beans(beans).build().await.unwrap();

    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());

    let pick_text_js = r#"
        var texts = ["hello", "world", "camel", "wasm", "bean", "rust"];
        camel.body = texts[Math.floor(Math.random() * texts.length)];
    "#;

    let route_transform =
        camel_builder::RouteBuilder::from("timer:tick?period=150&repeatCount=200")
            .route_id("transform-route")
            .script("js", pick_text_js)
            .bean("text-utils", "upper")
            .build()?;

    let route_last = camel_builder::RouteBuilder::from("timer:tick?period=300&repeatCount=100")
        .route_id("last-route")
        .bean("text-utils", "last")
        .to("log:info")
        .build()?;

    ctx.add_route_definition(route_transform).await?;
    ctx.add_route_definition(route_last).await?;
    ctx.start().await?;

    println!("\n=== WASM Bean Example ===");
    println!("Route 1 (transform): timer → js(set random text) → bean:upper → log");
    println!("Route 2 (last):      timer → bean:last → log");
    println!("Running for ~10s…\n");

    tokio::time::sleep(tokio::time::Duration::from_secs(12)).await;

    ctx.stop().await?;
    println!("\nDone.");
    Ok(())
}
