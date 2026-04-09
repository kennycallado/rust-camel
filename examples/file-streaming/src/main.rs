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

use bytes::Bytes;
use camel_api::{
    CamelError, Exchange, Message,
    body::{Body, StreamBody, StreamMetadata},
    producer::ProducerContext,
};
use camel_component_api::Component;
use camel_component_file::FileComponent;
use tokio::sync::Mutex;
use tower::ServiceExt;
use tracing::info;

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
    let endpoint =
        component.create_endpoint(&write_uri, &camel_component_api::NoOpComponentContext)?;
    let ctx = ProducerContext::new();
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
