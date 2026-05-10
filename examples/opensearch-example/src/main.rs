//! OpenSearch example for rust-camel.
//!
//! Demonstrates:
//!   - Route 1 (INDEX): timer → set_body(json doc) → set_header(CamelOpenSearch.Id) → opensearch INDEX → log
//!   - Route 2 (SEARCH): timer → set_body(query json) → opensearch SEARCH → log
//!
//! An OpenSearch instance is started automatically via testcontainers (requires Docker).
//! No external infrastructure needed — just run:
//!
//!   cargo run -p opensearch-example
//!
//! Press Ctrl+C to stop.

use camel_api::{CamelError, Value};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_log::LogComponent;
use camel_component_opensearch::OpenSearchComponent;
use camel_component_timer::TimerComponent;
use camel_core::context::CamelContext;
use std::time::Duration;
use testcontainers::{
    Image,
    core::{ContainerPort, WaitFor},
    runners::AsyncRunner,
};
use tokio::time::sleep;

const OPENSEARCH_HTTP_PORT: u16 = 9200;

#[derive(Debug, Clone)]
struct OpenSearchImage;

impl Image for OpenSearchImage {
    fn name(&self) -> &str {
        "opensearchproject/opensearch"
    }

    fn tag(&self) -> &str {
        "2.18.0"
    }

    fn env_vars(
        &self,
    ) -> impl IntoIterator<
        Item = (
            impl Into<std::borrow::Cow<'_, str>>,
            impl Into<std::borrow::Cow<'_, str>>,
        ),
    > {
        vec![
            ("discovery.type".to_string(), "single-node".to_string()),
            (
                "OPENSEARCH_JAVA_OPTS".to_string(),
                "-Xms512m -Xmx512m".to_string(),
            ),
            ("DISABLE_SECURITY_PLUGIN".to_string(), "true".to_string()),
        ]
    }

    fn expose_ports(&self) -> &[ContainerPort] {
        &[ContainerPort::Tcp(OPENSEARCH_HTTP_PORT)]
    }

    fn ready_conditions(&self) -> Vec<WaitFor> {
        vec![WaitFor::Nothing]
    }
}

async fn wait_for_opensearch(host: &str, port: u16) {
    let client = reqwest::Client::new();
    let url = format!("http://{}:{}/", host, port);
    println!("Waiting for OpenSearch at {}...", url);
    for _ in 0..120 {
        if client.get(&url).send().await.is_ok() {
            println!("OpenSearch is ready!");
            return;
        }
        sleep(Duration::from_millis(500)).await;
    }
    panic!("OpenSearch did not become ready in 60s");
}

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt()
        .with_env_filter("info")
        .with_target(false)
        .init();

    println!("Starting OpenSearch via testcontainers (requires Docker)...");
    let container = OpenSearchImage
        .start()
        .await
        .expect("Failed to start OpenSearch container — is Docker running?");
    let port = container
        .get_host_port_ipv4(OPENSEARCH_HTTP_PORT)
        .await
        .expect("Failed to get OpenSearch port");
    let host_port = format!("127.0.0.1:{}", port);

    wait_for_opensearch("127.0.0.1", port).await;
    println!("OpenSearch available at {}\n", host_port);

    let mut ctx = CamelContext::builder().build().await?;
    ctx.register_component(OpenSearchComponent::new());
    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());

    let index_uri = format!("opensearch://{}/documents?operation=INDEX", host_port);
    let search_uri = format!("opensearch://{}/documents?operation=SEARCH", host_port);

    let index_route = RouteBuilder::from("timer:index?period=5000&repeatCount=3")
        .route_id("opensearch-index-producer")
        .set_body(Value::String(
            serde_json::json!({
                "title": "Test Document",
                "content": "Hello from rust-camel!",
                "timestamp": chrono::Utc::now().to_rfc3339()
            })
            .to_string(),
        ))
        .set_header("CamelOpenSearch.Id", Value::String("doc-1".into()))
        .to(&index_uri)
        .to("log:info?showHeaders=true")
        .build()?;

    let search_route = RouteBuilder::from("timer:search?period=10000&repeatCount=2")
        .route_id("opensearch-search-producer")
        .set_body(Value::String(
            serde_json::json!({
                "query": { "match_all": {} }
            })
            .to_string(),
        ))
        .to(&search_uri)
        .to("log:info?showAll=true")
        .build()?;

    ctx.add_route_definition(index_route).await?;
    ctx.add_route_definition(search_route).await?;

    println!("Starting OpenSearch example... Press Ctrl+C to stop.\n");

    ctx.start().await?;

    tokio::signal::ctrl_c().await.ok();
    println!("\nShutting down...");
    ctx.stop().await?;
    println!("OpenSearch example stopped.");
    Ok(())
}
