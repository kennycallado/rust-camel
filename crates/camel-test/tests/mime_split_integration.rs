//! Integration tests for the MIME split EIP pattern.
//!
//! Verifies that a route using `format: mime` in a streaming split turns a
//! multipart attachment message into one fragment exchange per attachment,
//! that the MIME-only options (`max_parts`) reach the splitter, and that a
//! `pollEnrich`-sourced MIME file is not rolled back when the split fails.

use std::time::Duration;

use camel_api::body::DEFAULT_MATERIALIZE_LIMIT;
use camel_api::error_handler::ErrorHandlerConfig;
use camel_api::splitter::{AggregationStrategy, StreamSplitConfig, StreamSplitFormat};
use camel_api::{Body, Exchange};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_file::FileComponent;
use camel_core::BuilderStep;
use camel_processor::mime_splitter::{CAMEL_MIME_PART_INDEX, CAMEL_MIME_PART_NAME};
use camel_processor::splitter::CAMEL_SPLIT_SIZE;
use camel_test::CamelTestContext;

/// A multipart/mixed message with two attachments. The second one is
/// base64-encoded so the emitted body proves transfer decoding happened.
fn mime_two_attachments() -> Vec<u8> {
    concat!(
        "MIME-Version: 1.0\r\n",
        "Content-Type: multipart/mixed; boundary=\"test-boundary\"\r\n",
        "\r\n",
        "--test-boundary\r\n",
        "Content-Type: text/plain; name=\"alpha.txt\"\r\n",
        "Content-Disposition: attachment; filename=\"alpha.txt\"\r\n",
        "\r\n",
        "alpha\r\n",
        "--test-boundary\r\n",
        "Content-Type: application/octet-stream; name=\"beta.bin\"\r\n",
        "Content-Disposition: attachment; filename=\"beta.bin\"\r\n",
        "Content-Transfer-Encoding: base64\r\n",
        "\r\n",
        "YmV0YQ==\r\n",
        "--test-boundary--\r\n",
    )
    .as_bytes()
    .to_vec()
}

/// Replace the body with a materialized `Body::Bytes` payload.
fn set_mime_body(
    raw: Vec<u8>,
) -> impl Fn(Exchange) -> std::future::Ready<Result<Exchange, camel_api::CamelError>>
+ Send
+ Sync
+ 'static {
    move |mut exchange: Exchange| {
        exchange.input.body = Body::from(raw.clone());
        std::future::ready(Ok(exchange))
    }
}

/// Materialize a stream body (file consumer / pollEnrich) into `Body::Bytes`.
async fn materialize_body(mut exchange: Exchange) -> Result<Exchange, camel_api::CamelError> {
    let body = std::mem::replace(&mut exchange.input.body, Body::Empty);
    let bytes = body.into_bytes(DEFAULT_MATERIALIZE_LIMIT).await?;
    exchange.input.body = Body::Bytes(bytes);
    Ok(exchange)
}

fn push_mime_split(builder: &mut RouteBuilder, max_parts: Option<usize>, target: &str) {
    builder
        .steps_mut()
        .push(BuilderStep::DeclarativeStreamSplit {
            stream_config: StreamSplitConfig {
                format: StreamSplitFormat::Mime,
                max_parts,
                ..Default::default()
            },
            aggregation: AggregationStrategy::Original,
            stop_on_exception: false,
            steps: vec![BuilderStep::To(target.to_string())],
        });
}

fn mime_index(exchange: &Exchange) -> u64 {
    exchange
        .input
        .headers
        .get(CAMEL_MIME_PART_INDEX)
        .and_then(|value| value.as_u64())
        .unwrap_or(u64::MAX)
}

fn mime_name(exchange: &Exchange) -> Option<&str> {
    exchange
        .input
        .headers
        .get(CAMEL_MIME_PART_NAME)
        .and_then(|value| value.as_str())
}

fn body_bytes(exchange: &Exchange) -> &[u8] {
    match &exchange.input.body {
        Body::Bytes(bytes) => bytes.as_ref(),
        other => panic!("expected Body::Bytes, got {other:?}"),
    }
}

fn error_message(exchange: &Exchange) -> String {
    exchange
        .error
        .as_ref()
        .map(ToString::to_string)
        .expect("exchange should carry an error")
}

#[tokio::test(flavor = "multi_thread")]
async fn test_mime_split_multi_part() {
    let raw = mime_two_attachments();

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .build()
        .await;

    let mut builder = RouteBuilder::from("timer:mime-test?period=50&repeatCount=1")
        .route_id("mime-split-test")
        .process(set_mime_body(raw));
    push_mime_split(&mut builder, None, "mock:mime-part");

    h.add_route(builder.build().unwrap()).await.unwrap();
    h.start().await;

    let endpoint = h.mock().get_endpoint("mime-part").unwrap();
    endpoint.await_exchanges(2, Duration::from_secs(5)).await;
    h.stop().await;

    endpoint.assert_exchange_count(2).await;
    let mut sorted = endpoint.get_received_exchanges().await;
    sorted.sort_by_key(mime_index);

    assert_eq!(mime_index(&sorted[0]), 0, "first fragment index");
    assert_eq!(mime_name(&sorted[0]), Some("alpha.txt"), "first part name");
    assert_eq!(body_bytes(&sorted[0]), b"alpha", "first part decoded body");

    assert_eq!(mime_index(&sorted[1]), 1, "second fragment index");
    assert_eq!(mime_name(&sorted[1]), Some("beta.bin"), "second part name");
    assert_eq!(
        body_bytes(&sorted[1]),
        b"beta",
        "second part base64-decoded body"
    );

    assert_eq!(
        sorted[1]
            .property(CAMEL_SPLIT_SIZE)
            .and_then(|v| v.as_u64()),
        Some(2),
        "CamelSplitSize is stamped on the last fragment"
    );
    assert_eq!(
        sorted[0].property(CAMEL_SPLIT_SIZE),
        None,
        "CamelSplitSize is only known on the last fragment"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_mime_split_max_parts_plumbs() {
    let raw = mime_two_attachments();

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .build()
        .await;

    let mut builder = RouteBuilder::from("timer:mime-max-parts?period=50&repeatCount=1")
        .route_id("mime-max-parts-test")
        .error_handler(ErrorHandlerConfig::dead_letter_channel("mock:dlc"))
        .process(set_mime_body(raw));
    push_mime_split(&mut builder, Some(1), "mock:mime-part");

    h.add_route(builder.build().unwrap()).await.unwrap();
    h.start().await;

    let dlc = h
        .mock()
        .get_endpoint("dlc")
        .expect("DLC endpoint should be created by route compilation");
    dlc.await_exchanges(1, Duration::from_secs(5)).await;
    h.stop().await;

    dlc.assert_exchange_count(1).await;
    let failed = &dlc.get_received_exchanges().await[0];
    let message = error_message(failed);
    assert!(
        message.contains("max_parts"),
        "error should name max_parts, got {message:?}"
    );

    // The splitter streams: it emits attachment 0 (allowed by max_parts=1)
    // before it inspects attachment 1 and detects the overflow. The segment
    // pulls the next item before running the body, so exactly the one allowed
    // fragment reaches the mock; the error is the proof the option plumbed
    // through to the splitter.
    let success = h
        .mock()
        .get_endpoint("mime-part")
        .expect("mock endpoint should exist");
    success.assert_exchange_count(1).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_poll_enrich_mime_limit_does_not_roll_back() {
    let inbox = tempfile::tempdir().unwrap();
    let input_path = inbox.path().join("message.eml");
    let raw = mime_two_attachments();
    tokio::fs::write(&input_path, &raw).await.unwrap();

    let file_uri = format!("file:{}?noop=true", inbox.path().display());

    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .with_component(FileComponent::new())
        .build()
        .await;

    let mut builder = RouteBuilder::from("timer:mime-poll-enrich?period=50&repeatCount=1")
        .route_id("mime-poll-enrich-test")
        .error_handler(ErrorHandlerConfig::dead_letter_channel("mock:dlc"))
        .poll_enrich(&file_uri, 5000)
        .process(materialize_body);
    push_mime_split(&mut builder, Some(1), "mock:mime-part");

    h.add_route(builder.build().unwrap()).await.unwrap();
    h.start().await;

    let dlc = h
        .mock()
        .get_endpoint("dlc")
        .expect("DLC endpoint should be created by route compilation");
    dlc.await_exchanges(1, Duration::from_secs(5)).await;
    h.stop().await;

    dlc.assert_exchange_count(1).await;
    let failed = &dlc.get_received_exchanges().await[0];
    let message = error_message(failed);
    assert!(
        message.contains("max_parts"),
        "error should name max_parts, got {message:?}"
    );

    // noop=true leaves the source in place and the split failure must not
    // restore, move, or re-read it.
    assert!(input_path.exists(), "source file must remain in place");
    let after = tokio::fs::read(&input_path).await.unwrap();
    assert_eq!(after, raw, "source file content must be untouched");
    assert!(
        !inbox.path().join(".camel").exists(),
        "noop=true must not move the file into .camel"
    );
}
