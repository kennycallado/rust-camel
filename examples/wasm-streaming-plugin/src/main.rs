//! WASM streaming body example.
//!
//! Runs a pre-built streaming plugin (`fixtures/streaming-plugin.wasm`) through a
//! timer → process → WASM → log route:
//! - Timer fires once
//! - Process step injects a 5 MB `Body::Stream` (repeating bytes)
//! - WASM plugin reads the stream incrementally and counts bytes
//! - Log component prints the result
//!
//! # Running
//!
//! ```bash
//! cargo run -p wasm-streaming-plugin
//! ```
//!
//! # Building your own plugin
//!
//! See `guest/` for the guest source and `fixtures/` for the pre-built WASM.

use std::sync::{Arc, Mutex};

use bytes::Bytes;
use camel_api::{Body, CamelError, StreamBody, StreamMetadata};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_log::LogComponent;
use camel_component_timer::TimerComponent;
use camel_component_wasm::WasmComponent;
use camel_core::Registry;
use camel_core::context::CamelContext;
use futures::stream;

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt().with_target(false).init();

    // Resolve fixtures/ relative to this source file so the example works
    // regardless of the working directory.
    let fixtures_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures");

    let mut ctx = CamelContext::builder().build().await.unwrap(); // allow-unwrap

    let registry = Arc::new(Mutex::new(Registry::new()));
    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());
    // base_dir = fixtures/ → "streaming-plugin.wasm" resolves to fixtures/streaming-plugin.wasm
    ctx.register_component(WasmComponent::new(registry, fixtures_dir));

    // Create a 5 MB stream of repeating bytes (0x42 = 'B')
    let chunk_size = 64 * 1024; // 64 KB chunks
    let total_size = 5 * 1024 * 1024; // 5 MB
    let num_chunks = total_size / chunk_size;

    let route = RouteBuilder::from("timer:tick?period=1000&repeatCount=1")
        .route_id("wasm-streaming-example")
        .process(move |mut exchange| {
            // Create a stream of 64 KB chunks
            let chunks: Vec<Result<Bytes, CamelError>> = (0..num_chunks)
                .map(|_| Ok(Bytes::from(vec![0x42u8; chunk_size])))
                .collect();

            let box_stream = stream::iter(chunks);
            exchange.input.body = Body::Stream(StreamBody {
                stream: Arc::new(tokio::sync::Mutex::new(Some(Box::pin(box_stream)))),
                metadata: StreamMetadata {
                    size_hint: Some(total_size as u64),
                    content_type: Some("application/octet-stream".to_string()),
                    origin: None,
                },
            });
            async move { Ok(exchange) }
        })
        .to("wasm:streaming-plugin.wasm?timeout=5&max-memory=10485760")
        .to("log:info")
        .build()?;

    ctx.add_route_definition(route).await?;
    ctx.start().await?;

    println!("WASM streaming example: 5 MB stream through byte-counter plugin, then exit.");
    tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;

    // Clean shutdown — stops all routes and consumers
    ctx.stop().await?;
    Ok(())
}
