use camel_component_embedding::EmbeddingComponent;
use camel_component_file::FileComponent;
use camel_component_http::HttpComponent;
use camel_component_llm::LlmComponent;
use camel_component_log::LogComponent;
use camel_component_vector::VectorComponent;
use camel_core::CamelContext;
use camel_core::CamelError;

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt().with_env_filter("info").init();

    let kb_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("kb");
    std::fs::create_dir_all(&kb_dir).ok();
    let kb_path = kb_dir.to_str().unwrap();

    let yaml = include_str!("../routes.yaml").replace("file:kb", &format!("file:{}", kb_path));

    let mut ctx = CamelContext::builder().build().await?;
    ctx.register_component(FileComponent::new());
    ctx.register_component(HttpComponent::new());
    ctx.register_component(LogComponent::new());
    ctx.register_component(EmbeddingComponent);
    ctx.register_component(LlmComponent);
    ctx.register_component(VectorComponent);

    let routes = camel_dsl::parse_yaml(&yaml)?;
    for route in routes {
        ctx.add_route_definition(route).await?;
    }

    ctx.start().await?;

    println!("RAG Knowledge Base: AcmeCorp Support Q&A");
    println!("=========================================");
    println!("Drop .txt files into the kb/ directory — they will be auto-indexed.");
    println!("Ask questions via HTTP:");
    println!("  curl -d \"How do I reset a password?\" http://localhost:8080/ask");
    println!();
    println!("Requires: Ollama (embeddinggemma + qwen3.5:4b) + Qdrant");
    println!("Press Ctrl+C to stop.");

    tokio::signal::ctrl_c().await?;
    ctx.stop().await?;
    Ok(())
}
