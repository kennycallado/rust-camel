//! Integration test for the ZIP split EIP pattern.
//!
//! Verifies that a route using `format: zip` in a streaming split correctly
//! processes a multi-entry ZIP archive, producing one fragment exchange per
//! entry with the expected CamelZipEntry* headers and body content.

use std::time::Duration;

use camel_api::Exchange;
use camel_api::body::Body;
use camel_api::splitter::{AggregationStrategy, StreamSplitConfig, StreamSplitFormat};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_core::BuilderStep;
use camel_processor::zip_splitter::{
    CAMEL_ZIP_ENTRY_COMPRESSED_SIZE, CAMEL_ZIP_ENTRY_CRC32, CAMEL_ZIP_ENTRY_INDEX,
    CAMEL_ZIP_ENTRY_IS_DIRECTORY, CAMEL_ZIP_ENTRY_NAME, CAMEL_ZIP_ENTRY_PATH, CAMEL_ZIP_ENTRY_SIZE,
};
use camel_test::CamelTestContext;

/// Build a multi-entry ZIP archive in memory.
fn make_multi_entry_zip(files: Vec<(&str, &[u8])>) -> Vec<u8> {
    let mut buf = Vec::new();
    {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for (name, data) in &files {
            // allow-unwrap: test helper, invalid ZIP creation is a test bug
            writer.start_file(name, options).unwrap();
            // allow-unwrap: test helper
            std::io::Write::write_all(&mut writer, data).unwrap();
        }
        // allow-unwrap: test helper
        writer.finish().unwrap();
    }
    buf
}

#[tokio::test(flavor = "multi_thread")]
async fn test_zip_split_multi_entry() {
    // ── Build a multi-entry ZIP ────────────────────────────────────────────
    let zip_data = make_multi_entry_zip(vec![
        ("data/file1.txt", b"hello"),
        ("data/file2.txt", b"world"),
    ]);

    // ── Set up test harness ────────────────────────────────────────────────
    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .build()
        .await;

    // ── Build route: timer → set ZIP body → streaming split (zip) → mock ──
    // We cannot use the fluent `.split()` API because streaming split does not
    // have a dedicated builder method, so we push BuilderStep directly.
    let captured_data = zip_data.clone();
    let mut builder = RouteBuilder::from("timer:zip-test?period=50&repeatCount=1")
        .route_id("zip-split-test")
        .process(move |mut ex: Exchange| {
            let data = captured_data.clone();
            async move {
                ex.input.body = Body::from(data);
                Ok(ex)
            }
        });

    builder
        .steps_mut()
        .push(BuilderStep::DeclarativeStreamSplit {
            stream_config: StreamSplitConfig {
                format: StreamSplitFormat::Zip,
                ..Default::default()
            },
            aggregation: AggregationStrategy::Original,
            stop_on_exception: false,
            steps: vec![BuilderStep::To("mock:zip-entry".to_string())],
        });

    let route = builder.build().unwrap();
    h.add_route(route).await.unwrap();

    // ── Run ────────────────────────────────────────────────────────────────
    h.start().await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    h.stop().await;

    // ── Assert ─────────────────────────────────────────────────────────────
    let entry_ep = h.mock().get_endpoint("zip-entry").unwrap();
    entry_ep.assert_exchange_count(2).await;

    let exchanges = entry_ep.get_received_exchanges().await;

    // Sort by index for deterministic assertions (ZIP entries may not arrive
    // in insertion order depending on the splitting implementation).
    let mut sorted: Vec<_> = exchanges.into_iter().collect();
    sorted.sort_by_key(|ex| {
        ex.input
            .headers
            .get(CAMEL_ZIP_ENTRY_INDEX)
            .and_then(|v| v.as_u64())
            .unwrap_or(0)
    });

    // ── Entry 0: data/file1.txt ────────────────────────────────────────────
    assert_eq!(
        sorted[0]
            .input
            .headers
            .get(CAMEL_ZIP_ENTRY_NAME)
            .and_then(|v| v.as_str()),
        Some("file1.txt"),
        "Entry 0 name should be file1.txt"
    );
    assert_eq!(
        sorted[0]
            .input
            .headers
            .get(CAMEL_ZIP_ENTRY_PATH)
            .and_then(|v| v.as_str()),
        Some("data/file1.txt"),
        "Entry 0 path should be data/file1.txt"
    );
    assert_eq!(
        sorted[0]
            .input
            .headers
            .get(CAMEL_ZIP_ENTRY_INDEX)
            .and_then(|v| v.as_u64()),
        Some(0),
        "Entry 0 index should be 0"
    );
    assert_eq!(
        sorted[0]
            .input
            .headers
            .get(CAMEL_ZIP_ENTRY_SIZE)
            .and_then(|v| v.as_u64()),
        Some(5),
        "Entry 0 size should be 5"
    );
    assert_eq!(
        sorted[0]
            .input
            .headers
            .get(CAMEL_ZIP_ENTRY_IS_DIRECTORY)
            .and_then(|v| v.as_bool()),
        Some(false),
        "Entry 0 should not be a directory"
    );
    // Compressed size will vary by compression, just assert it's present.
    assert!(
        sorted[0]
            .input
            .headers
            .contains_key(CAMEL_ZIP_ENTRY_COMPRESSED_SIZE),
        "Entry 0 should have compressed size"
    );
    // CRC32 should be present.
    assert!(
        sorted[0].input.headers.contains_key(CAMEL_ZIP_ENTRY_CRC32),
        "Entry 0 should have CRC32"
    );
    match &sorted[0].input.body {
        Body::Bytes(b) => assert_eq!(&b[..], b"hello", "Entry 0 body should be 'hello'"),
        other => panic!("Entry 0: expected Body::Bytes, got {other:?}"),
    }

    // ── Entry 1: data/file2.txt ────────────────────────────────────────────
    assert_eq!(
        sorted[1]
            .input
            .headers
            .get(CAMEL_ZIP_ENTRY_NAME)
            .and_then(|v| v.as_str()),
        Some("file2.txt"),
        "Entry 1 name should be file2.txt"
    );
    assert_eq!(
        sorted[1]
            .input
            .headers
            .get(CAMEL_ZIP_ENTRY_PATH)
            .and_then(|v| v.as_str()),
        Some("data/file2.txt"),
        "Entry 1 path should be data/file2.txt"
    );
    assert_eq!(
        sorted[1]
            .input
            .headers
            .get(CAMEL_ZIP_ENTRY_INDEX)
            .and_then(|v| v.as_u64()),
        Some(1),
        "Entry 1 index should be 1"
    );
    assert_eq!(
        sorted[1]
            .input
            .headers
            .get(CAMEL_ZIP_ENTRY_SIZE)
            .and_then(|v| v.as_u64()),
        Some(5),
        "Entry 1 size should be 5"
    );
    assert_eq!(
        sorted[1]
            .input
            .headers
            .get(CAMEL_ZIP_ENTRY_IS_DIRECTORY)
            .and_then(|v| v.as_bool()),
        Some(false),
        "Entry 1 should not be a directory"
    );
    assert!(
        sorted[1]
            .input
            .headers
            .contains_key(CAMEL_ZIP_ENTRY_COMPRESSED_SIZE),
        "Entry 1 should have compressed size"
    );
    assert!(
        sorted[1].input.headers.contains_key(CAMEL_ZIP_ENTRY_CRC32),
        "Entry 1 should have CRC32"
    );
    match &sorted[1].input.body {
        Body::Bytes(b) => assert_eq!(&b[..], b"world", "Entry 1 body should be 'world'"),
        other => panic!("Entry 1: expected Body::Bytes, got {other:?}"),
    }
}
