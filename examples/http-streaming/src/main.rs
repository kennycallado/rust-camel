//! HTTP Streaming Example
//!
//! Demonstrates native streaming in the HTTP Consumer:
//! - Axis A (response streaming): GET /stream returns Body::Stream with multiple chunks
//! - Axis B (request streaming): POST /upload receives Body::Stream, materializes it, echoes byte count

use bytes::Bytes;
use camel_api::body::{Body, StreamBody, StreamMetadata};
use camel_api::CamelError;
use camel_component::{Component, ConsumerContext, ExchangeEnvelope};
use camel_component_http::HttpComponent;
use futures::stream;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};

const PORT: u16 = 3030;

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    tracing_subscriber::fmt().with_target(false).init();

    println!("╔════════════════════════════════════════════════════════════╗");
    println!("║          HTTP Consumer Streaming Example                   ║");
    println!("╚════════════════════════════════════════════════════════════╝");
    println!();

    // Create the HTTP component and consumers for both routes
    let component = HttpComponent::new();

    // --- Route 1: GET /stream (response streaming) ---
    let stream_endpoint = component
        .create_endpoint(&format!("http://127.0.0.1:{PORT}/stream"))
        .map_err(|e| CamelError::EndpointCreationFailed(e.to_string()))?;
    let mut stream_consumer = stream_endpoint
        .create_consumer()
        .map_err(|e| CamelError::EndpointCreationFailed(e.to_string()))?;

    let (stream_tx, mut stream_rx) = mpsc::channel::<ExchangeEnvelope>(16);
    let stream_token = tokio_util::sync::CancellationToken::new();
    let stream_ctx = ConsumerContext::new(stream_tx, stream_token.clone());

    // Spawn the stream consumer
    tokio::spawn(async move {
        let _ = stream_consumer.start(stream_ctx).await;
    });

    // --- Route 2: POST /upload (request streaming) ---
    let upload_endpoint = component
        .create_endpoint(&format!("http://127.0.0.1:{PORT}/upload"))
        .map_err(|e| CamelError::EndpointCreationFailed(e.to_string()))?;
    let mut upload_consumer = upload_endpoint
        .create_consumer()
        .map_err(|e| CamelError::EndpointCreationFailed(e.to_string()))?;

    let (upload_tx, mut upload_rx) = mpsc::channel::<ExchangeEnvelope>(16);
    let upload_token = tokio_util::sync::CancellationToken::new();
    let upload_ctx = ConsumerContext::new(upload_tx, upload_token.clone());

    // Spawn the upload consumer
    tokio::spawn(async move {
        let _ = upload_consumer.start(upload_ctx).await;
    });

    // Wait for servers to start
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    println!("Server started on http://127.0.0.1:{PORT}");
    println!();

    // Spawn handlers for both consumers
    let stream_handler = tokio::spawn(async move {
        while let Some(mut envelope) = stream_rx.recv().await {
            // Create streaming response with 3 chunks
            let chunks: Vec<Result<Bytes, CamelError>> = vec![
                Ok(Bytes::from("Line 1\n")),
                Ok(Bytes::from("Line 2\n")),
                Ok(Bytes::from("Line 3\n")),
            ];
            let stream = Box::pin(stream::iter(chunks));
            envelope.exchange.input.body = Body::Stream(StreamBody {
                stream: Arc::new(Mutex::new(Some(stream))),
                metadata: StreamMetadata::default(),
            });
            if let Some(reply_tx) = envelope.reply_tx {
                let _ = reply_tx.send(Ok(envelope.exchange));
            }
        }
    });

    let upload_handler = tokio::spawn(async move {
        while let Some(mut envelope) = upload_rx.recv().await {
            // Materialize the incoming stream and echo byte count
            let byte_count = match envelope.exchange.input.body.into_bytes(1024 * 1024).await {
                Ok(bytes) => bytes.len(),
                Err(e) => {
                    envelope.exchange.input.body = Body::Text(format!("Error: {e}"));
                    if let Some(reply_tx) = envelope.reply_tx {
                        let _ = reply_tx.send(Ok(envelope.exchange));
                    }
                    continue;
                }
            };
            envelope.exchange.input.body = Body::Text(format!("Received {byte_count} bytes"));
            if let Some(reply_tx) = envelope.reply_tx {
                let _ = reply_tx.send(Ok(envelope.exchange));
            }
        }
    });

    // Run test requests
    let client = reqwest::Client::new();

    // --- Test Axis A: Response streaming ---
    println!("=== Axis A: Response Streaming (GET /stream) ===");
    println!("Requesting http://127.0.0.1:{PORT}/stream ...");

    let resp = client
        .get(format!("http://127.0.0.1:{PORT}/stream"))
        .send()
        .await
        .map_err(|e| CamelError::Io(e.to_string()))?;

    println!("Response status: {}", resp.status());
    println!("Transfer-Encoding: {:?}", resp.headers().get("transfer-encoding"));

    let body = resp
        .text()
        .await
        .map_err(|e| CamelError::Io(e.to_string()))?;
    println!("Received body:\n{body}");
    println!();

    // --- Test Axis B: Request streaming ---
    println!("=== Axis B: Request Streaming (POST /upload) ===");
    println!("Posting to http://127.0.0.1:{PORT}/upload ...");

    let resp = client
        .post(format!("http://127.0.0.1:{PORT}/upload"))
        .body("hello streaming world")
        .send()
        .await
        .map_err(|e| CamelError::Io(e.to_string()))?;

    println!("Response status: {}", resp.status());

    let body = resp
        .text()
        .await
        .map_err(|e| CamelError::Io(e.to_string()))?;
    println!("Response: {body}");
    println!();

    // Clean shutdown
    println!("Shutting down...");
    stream_token.cancel();
    upload_token.cancel();

    // Wait for handlers to finish (they will exit when channels close)
    stream_handler.abort();
    upload_handler.abort();

    println!("Done.");
    Ok(())
}
