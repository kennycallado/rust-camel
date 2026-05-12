use camel_component_direct::DirectComponent;
use camel_component_llm::LlmComponent;
use camel_component_log::LogComponent;
use camel_component_timer::TimerComponent;
use camel_core::context::CamelContext;
use camel_dsl::parse_yaml;

const ROUTES_YAML: &str = include_str!("../routes.yaml");

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_target(false)
        .with_env_filter("info")
        .init();

    let mut ctx = CamelContext::builder().build().await?;
    ctx.register_component(DirectComponent::new());
    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());
    ctx.register_component(LlmComponent);

    let routes = parse_yaml(ROUTES_YAML)?;
    println!("Loaded {} route(s) from YAML", routes.len());

    for route in routes {
        let id = route.route_id().to_string();
        ctx.add_route_definition(route).await?;
        println!("Added route: {}", id);
    }

    ctx.start().await?;

    println!();
    println!("ai-ticket-router: Customer Support Pipeline Demo");
    println!("================================================");
    println!("Sends 5 realistic support tickets through:");
    println!("  1. ai_extract  -> structured data extraction");
    println!("  2. ai_classify -> category classification");
    println!("  3. choice      -> route to appropriate team");
    println!();
    println!("Requires: Ollama at localhost:11434 with qwen3.5:4b");
    println!("Press Ctrl+C to stop.");

    tokio::signal::ctrl_c().await?;
    ctx.stop().await?;
    Ok(())
}
