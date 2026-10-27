//! # streaming-split
//!
//! Demonstrates the Streaming Split EIP — splitting a `Body::Stream` containing
//! NDJSON into individual exchanges through a Camel route built with
//! `RouteBuilder::stream_split` and an NDJSON `StreamSplitCodec`.
//!
//! Run with:
//!   cargo run -p streaming-split

use std::sync::Arc;

use bytes::Bytes;
use camel_api::{
    CamelError, Exchange,
    body::{Body, StreamBody, StreamMetadata},
    splitter::{AggregationStrategy, StreamSplitConfig},
};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_log::LogComponent;
use camel_component_timer::TimerComponent;
use camel_core::context::CamelContext;
use camel_processor::LogLevel;
use camel_processor::stream_codec::{StreamSplitInput, resolve_format, resolve_incremental_codec};
use tokio::sync::Mutex;

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
                let err = camel_api::streaming_split_type_error(&exchange.input.body);
                return Box::pin(futures::stream::once(async move { Err(err) }));
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
        let codec = resolve_incremental_codec(&resolved_format);
        let codec = match codec {
            Ok(c) => c,
            Err(e) => return Box::pin(futures::stream::once(async { Err(e) })),
        };
        codec.split(input, config)
    })
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt().with_target(false).init();

    let mut ctx = CamelContext::builder().build().await?;
    ctx.register_component(TimerComponent::new());
    ctx.register_component(LogComponent::new());

    // ANCHOR: streaming-split-route
    let ndjson = vec![
        r#"{"user":"alice","action":"login"}"#,
        r#"{"user":"bob","action":"purchase"}"#,
        r#"{"user":"charlie","action":"logout"}"#,
    ];

    let route = RouteBuilder::from("timer:stream-split?period=3000&repeatCount=1")
        .route_id("streaming-split-route")
        // Fresh stream per timer firing: never reuse an already-consumed body.
        .set_body_fn(move |_ex: &Exchange| make_ndjson_stream(ndjson.clone()))
        .stream_split(make_streaming_expression())
        .aggregation(AggregationStrategy::CollectAll)
        .log("Streaming split fragment", LogLevel::Info)
        .end_stream_split()
        .log("Aggregated result body", LogLevel::Info)
        .build()?;
    // ANCHOR_END: streaming-split-route

    ctx.add_route_definition(route).await?;
    ctx.start().await?;

    println!("Streaming Split example");
    println!("Input: Body::Stream with 3 NDJSON lines");
    println!();
    println!("Each NDJSON line was split into a separate exchange,");
    println!("processed by the sub-pipeline (log), then aggregated.");
    println!();
    println!("Press Ctrl+C to stop.");

    tokio::signal::ctrl_c().await.ok();

    ctx.stop().await?;
    println!("Done.");
    Ok(())
}
