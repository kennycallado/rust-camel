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

    let docs_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs");
    std::fs::create_dir_all(&docs_dir).ok();
    let docs_path = docs_dir.to_str().unwrap();

    let yaml = include_str!("../routes.yaml")
        .replace("file:docs", &format!("file:{}", docs_path));

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

    println!("rag-file started.");
    println!("Drop .txt files into {} to index them.", docs_path);
    println!("Ask questions: curl -d \"your question\" http://localhost:8080/ask");
    println!("(Requires Ollama + Qdrant — see docker-compose.yml)");
    println!("Press Ctrl+C to stop.");

    tokio::signal::ctrl_c().await?;
    ctx.stop().await?;
    Ok(())
}
