//! # file-streaming
//!
//! Demonstrates end-to-end streaming in the File component:
//!
//! - A `Body::Stream` (multi-chunk) is written to disk by `FileProducer` via
//!   `tokio::io::copy` — no RAM materialization, regardless of size.
//! - The written file is read back to verify correctness.
//!
//! Run with:
//!   cargo run -p file-streaming

use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use camel_api::{
    CamelError, Exchange, Message, RouteController, RouteStatus,
    body::{Body, StreamBody, StreamMetadata},
    producer::ProducerContext,
};
use camel_component::Component;
use camel_component_file::FileComponent;
use tokio::sync::Mutex;
use tower::ServiceExt;
use tracing::info;

// Minimal RouteController for standalone producer usage
struct NullRouteController;

#[async_trait]
impl RouteController for NullRouteController {
    async fn start_route(&mut self, _: &str) -> Result<(), CamelError> {
        Ok(())
    }
    async fn stop_route(&mut self, _: &str) -> Result<(), CamelError> {
        Ok(())
    }
    async fn restart_route(&mut self, _: &str) -> Result<(), CamelError> {
        Ok(())
    }
    async fn suspend_route(&mut self, _: &str) -> Result<(), CamelError> {
        Ok(())
    }
    async fn resume_route(&mut self, _: &str) -> Result<(), CamelError> {
        Ok(())
    }
    fn route_status(&self, _: &str) -> Option<RouteStatus> {
        None
    }
    async fn start_all_routes(&mut self) -> Result<(), CamelError> {
        Ok(())
    }
    async fn stop_all_routes(&mut self) -> Result<(), CamelError> {
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    let out_dir = tempfile::tempdir()?;
    let out_path = out_dir.path().to_str().unwrap().to_string();

    info!("Output directory: {out_path}");

    // Build a multi-chunk stream body (simulating a large file)
    let chunks: Vec<Result<Bytes, CamelError>> = vec![
        Ok(Bytes::from("chunk-1\n")),
        Ok(Bytes::from("chunk-2\n")),
        Ok(Bytes::from("chunk-3\n")),
    ];
    let stream = futures::stream::iter(chunks);
    let body = Body::Stream(StreamBody {
        stream: Arc::new(Mutex::new(Some(Box::pin(stream)))),
        metadata: StreamMetadata {
            size_hint: Some(24),
            content_type: Some("text/plain".to_string()),
            origin: None,
        },
    });

    // Write via FileProducer (streaming, no materialization)
    let write_uri = format!("file:{out_path}?fileName=output.txt");
    let component = FileComponent::new();
    let endpoint = component.create_endpoint(&write_uri)?;
    let ctx = ProducerContext::new(Arc::new(Mutex::new(NullRouteController)));
    let producer = endpoint.create_producer(&ctx)?;

    let exchange = Exchange::new(Message::new(body));
    let result = producer.oneshot(exchange).await?;
    let produced = result
        .input
        .header("CamelFileNameProduced")
        .and_then(|v: &camel_api::Value| v.as_str())
        .unwrap_or("(unknown)")
        .to_string();
    info!("Written to: {produced}");

    // Read back and verify
    let content = tokio::fs::read_to_string(format!("{out_path}/output.txt")).await?;
    info!("File content:\n{content}");
    assert_eq!(content, "chunk-1\nchunk-2\nchunk-3\n");
    info!("Content verified");

    Ok(())
}
