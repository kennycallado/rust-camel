use camel_component_embedding::EmbeddingComponent;
use camel_component_llm::LlmComponent;
use camel_component_log::LogComponent;
use camel_component_timer::TimerComponent;
use camel_component_vector::VectorComponent;
use camel_core::CamelContext;
use camel_core::CamelError;

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt().with_env_filter("info").init();

    let yaml = include_str!("../routes.yaml");

    let mut ctx = CamelContext::builder().build().await?;
    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());
    ctx.register_component(EmbeddingComponent);
    ctx.register_component(LlmComponent);
    ctx.register_component(VectorComponent);

    let routes = camel_dsl::parse_yaml(yaml)?;
    for route in routes {
        ctx.add_route_definition(route).await?;
    }

    ctx.start().await?;

    println!("rag-basic started.");
    println!("Route 1 ingests a sample doc at t+60s.");
    println!("Route 2 queries it at t+65s.");
    println!("(Requires Ollama + Qdrant — see docker-compose.yml)");
    println!("Press Ctrl+C to stop.");

    tokio::signal::ctrl_c().await?;
    ctx.stop().await?;
    Ok(())
}
