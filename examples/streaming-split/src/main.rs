//! # streaming-split
//!
//! Demonstrates the Streaming Split EIP — splitting a `Body::Stream` containing
//! NDJSON into individual exchanges via `StreamingSplitterService` + `NdjsonCodec`.
//!
//! Run with:
//!   cargo run -p streaming-split

use std::sync::Arc;

use bytes::Bytes;
use camel_api::{
    BoxProcessor, BoxProcessorExt, CamelError, Exchange, Message,
    body::{Body, StreamBody, StreamMetadata},
    splitter::{AggregationStrategy, StreamSplitConfig},
};
use camel_processor::stream_codec::{StreamSplitInput, resolve_codec, resolve_format};
use camel_processor::streaming_splitter::StreamingSplitterService;
use tokio::sync::Mutex;
use tower::Service;

fn make_ndjson_stream(lines: Vec<&'static str>) -> Body {
    let chunks: Vec<Result<Bytes, CamelError>> = lines
        .into_iter()
        .map(|l| Ok(Bytes::from(format!("{}\n", l))))
        .collect();
    let stream = futures::stream::iter(chunks);
    Body::Stream(StreamBody {
        stream: Arc::new(Mutex::new(Some(Box::pin(stream)))),
        metadata: StreamMetadata {
            content_type: Some("application/x-ndjson".to_string()),
            size_hint: None,
            origin: Some("demo://ndjson-stream".to_string()),
        },
    })
}

fn make_streaming_expression() -> camel_api::StreamingSplitExpression {
    let config = StreamSplitConfig::default();
    Arc::new(move |exchange| {
        let config = config.clone();
        let (stream_body, parent) = match &exchange.input.body {
            Body::Stream(sb) => (sb.clone(), {
                let mut p = exchange.clone();
                p.input.body = Body::Empty;
                p
            }),
            _ => {
                return Box::pin(futures::stream::once(async {
                    Err(CamelError::ProcessorError(
                        "streaming split requires Body::Stream".into(),
                    ))
                }));
            }
        };
        let stream = match stream_body.stream.try_lock() {
            Ok(mut guard) => match guard.take() {
                Some(s) => s,
                None => {
                    return Box::pin(futures::stream::once(async {
                        Err(CamelError::ProcessorError(
                            "stream body already consumed".into(),
                        ))
                    }));
                }
            },
            Err(_) => {
                return Box::pin(futures::stream::once(async {
                    Err(CamelError::ProcessorError("stream body locked".into()))
                }));
            }
        };
        let input = StreamSplitInput {
            parent,
            stream,
            metadata: stream_body.metadata,
        };
        let resolved_format = match resolve_format(&config.format, &input.metadata) {
            Ok(f) => f,
            Err(e) => return Box::pin(futures::stream::once(async { Err(e) })),
        };
        let codec = resolve_codec(&resolved_format);
        codec.split(input, config)
    })
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt().with_target(false).init();

    let expression = make_streaming_expression();

    let sub_pipeline: BoxProcessor = BoxProcessor::from_fn(|ex| async move {
        tracing::info!("[fragment] {:?}", ex.input.body);
        Ok(ex)
    });

    let mut splitter = StreamingSplitterService::new(
        expression,
        sub_pipeline,
        AggregationStrategy::CollectAll,
        true,
    );

    let ndjson = vec![
        r#"{"user":"alice","action":"login"}"#,
        r#"{"user":"bob","action":"purchase"}"#,
        r#"{"user":"charlie","action":"logout"}"#,
    ];
    let exchange = Exchange::new(Message::new(make_ndjson_stream(ndjson)));

    println!("Streaming Split example");
    println!("Input: Body::Stream with 3 NDJSON lines");
    println!();

    let _ = splitter.poll_ready(&mut std::task::Context::from_waker(
        &futures::task::noop_waker(),
    ));
    let result = splitter.call(exchange).await?;

    println!();
    println!("Aggregated result body:");
    println!("  {:?}", result.input.body);

    println!();
    println!("Done. Each NDJSON line was split into a separate exchange,");
    println!("processed by the sub-pipeline (log), then aggregated.");

    Ok(())
}
