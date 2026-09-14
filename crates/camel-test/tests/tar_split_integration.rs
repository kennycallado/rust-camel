//! Integration test for the TAR and TAR.GZ split EIP patterns.
//!
//! Verifies that a route using `format: tar` or `format: tar.gz` in a
//! streaming split correctly processes a multi-entry archive, producing one
//! fragment exchange per regular-file entry with the expected CamelTarEntry*
//! headers and body content. TAR.GZ input is a single-member GZIP stream of
//! the same archive and must produce identical metadata and bodies.

use std::io::Write;
use std::time::Duration;

use camel_api::Exchange;
use camel_api::body::Body;
use camel_api::splitter::{AggregationStrategy, StreamSplitConfig, StreamSplitFormat};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_core::BuilderStep;
use camel_processor::tar_splitter::{
    CAMEL_TAR_ENTRY_INDEX, CAMEL_TAR_ENTRY_IS_DIRECTORY, CAMEL_TAR_ENTRY_NAME,
    CAMEL_TAR_ENTRY_PATH, CAMEL_TAR_ENTRY_SIZE,
};
use camel_test::CamelTestContext;
use flate2::Compression;
use flate2::write::GzEncoder;

/// Build a ustar header block for one entry (512 bytes, valid checksum).
fn tar_header(name: &str, size: u64, typeflag: u8) -> [u8; 512] {
    let mut h = [0u8; 512];
    h[..name.len()].copy_from_slice(name.as_bytes());
    h[124..136].copy_from_slice(format!("{size:011o}\0").as_bytes());
    // Checksum field must be spaces while the checksum is computed.
    h[148..156].copy_from_slice(b"        ");
    h[156] = typeflag;
    h[257..263].copy_from_slice(b"ustar\0");
    h[263..265].copy_from_slice(b"00");
    let sum: u32 = h.iter().map(|&b| u32::from(b)).sum();
    h[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
    h
}

/// Assemble headers, padded data blocks, and the two-block end marker
/// into a complete TAR archive.
fn tar_archive(entries: &[(&str, u8, &[u8])]) -> Vec<u8> {
    let mut out = Vec::new();
    for &(name, typeflag, data) in entries {
        out.extend_from_slice(&tar_header(name, data.len() as u64, typeflag));
        if !data.is_empty() {
            let mut block = data.to_vec();
            let rem = block.len() % 512;
            if rem != 0 {
                block.extend_from_slice(&vec![0u8; 512 - rem]);
            }
            out.extend_from_slice(&block);
        }
    }
    out.extend_from_slice(&[0u8; 1024]);
    out
}

/// Wrap raw bytes as a single-member GZIP stream.
fn gzip_bytes(raw: &[u8]) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    // allow-unwrap: test helper, invalid gzip creation is a test bug
    encoder.write_all(raw).expect("gzip write");
    // allow-unwrap: test helper
    encoder.finish().expect("gzip finish")
}

const TWO_FILES: &[(&str, u8, &[u8])] = &[
    ("data/file1.txt", b'0', b"hello".as_slice()),
    ("data/file2.txt", b'0', b"world".as_slice()),
];

/// Sort received exchanges by TAR entry index and assert the two expected
/// regular-file entries with their exact metadata and bodies. Shared by the
/// TAR and TAR.GZ runs, which must produce identical fragments.
fn assert_two_file_entries(exchanges: Vec<Exchange>) {
    // Sort by index for deterministic assertions (fragments may not arrive
    // in archive order depending on the splitting implementation).
    let mut sorted: Vec<_> = exchanges.into_iter().collect();
    sorted.sort_by_key(|ex| {
        ex.input
            .headers
            .get(CAMEL_TAR_ENTRY_INDEX)
            .and_then(|v| v.as_u64())
            .unwrap_or(0)
    });

    // ── Entry 0: data/file1.txt ────────────────────────────────────────────
    assert_eq!(
        sorted[0]
            .input
            .headers
            .get(CAMEL_TAR_ENTRY_NAME)
            .and_then(|v| v.as_str()),
        Some("file1.txt"),
        "Entry 0 name should be file1.txt"
    );
    assert_eq!(
        sorted[0]
            .input
            .headers
            .get(CAMEL_TAR_ENTRY_PATH)
            .and_then(|v| v.as_str()),
        Some("data/file1.txt"),
        "Entry 0 path should be data/file1.txt"
    );
    assert_eq!(
        sorted[0]
            .input
            .headers
            .get(CAMEL_TAR_ENTRY_INDEX)
            .and_then(|v| v.as_u64()),
        Some(0),
        "Entry 0 index should be 0"
    );
    assert_eq!(
        sorted[0]
            .input
            .headers
            .get(CAMEL_TAR_ENTRY_SIZE)
            .and_then(|v| v.as_u64()),
        Some(5),
        "Entry 0 size should be 5"
    );
    assert_eq!(
        sorted[0]
            .input
            .headers
            .get(CAMEL_TAR_ENTRY_IS_DIRECTORY)
            .and_then(|v| v.as_bool()),
        Some(false),
        "Entry 0 should not be a directory"
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
            .get(CAMEL_TAR_ENTRY_NAME)
            .and_then(|v| v.as_str()),
        Some("file2.txt"),
        "Entry 1 name should be file2.txt"
    );
    assert_eq!(
        sorted[1]
            .input
            .headers
            .get(CAMEL_TAR_ENTRY_PATH)
            .and_then(|v| v.as_str()),
        Some("data/file2.txt"),
        "Entry 1 path should be data/file2.txt"
    );
    assert_eq!(
        sorted[1]
            .input
            .headers
            .get(CAMEL_TAR_ENTRY_INDEX)
            .and_then(|v| v.as_u64()),
        Some(1),
        "Entry 1 index should be 1"
    );
    assert_eq!(
        sorted[1]
            .input
            .headers
            .get(CAMEL_TAR_ENTRY_SIZE)
            .and_then(|v| v.as_u64()),
        Some(5),
        "Entry 1 size should be 5"
    );
    assert_eq!(
        sorted[1]
            .input
            .headers
            .get(CAMEL_TAR_ENTRY_IS_DIRECTORY)
            .and_then(|v| v.as_bool()),
        Some(false),
        "Entry 1 should not be a directory"
    );
    match &sorted[1].input.body {
        Body::Bytes(b) => assert_eq!(&b[..], b"world", "Entry 1 body should be 'world'"),
        other => panic!("Entry 1: expected Body::Bytes, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_tar_split_multi_entry() {
    // ── Build a multi-entry TAR ────────────────────────────────────────────
    let tar_data = tar_archive(TWO_FILES);

    // ── Set up test harness ────────────────────────────────────────────────
    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .build()
        .await;

    // ── Build route: timer → set TAR body → streaming split (tar) → mock ──
    // We cannot use the fluent `.split()` API because streaming split does not
    // have a dedicated builder method, so we push BuilderStep directly.
    let captured_data = tar_data.clone();
    let mut builder = RouteBuilder::from("timer:tar-test?period=50&repeatCount=1")
        .route_id("tar-split-test")
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
                format: StreamSplitFormat::Tar,
                ..Default::default()
            },
            aggregation: AggregationStrategy::Original,
            stop_on_exception: false,
            steps: vec![BuilderStep::To("mock:tar-entry".to_string())],
        });

    let route = builder.build().unwrap();
    h.add_route(route).await.unwrap();

    // ── Run ────────────────────────────────────────────────────────────────
    h.start().await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    h.stop().await;

    // ── Assert ─────────────────────────────────────────────────────────────
    let entry_ep = h.mock().get_endpoint("tar-entry").unwrap();
    entry_ep.assert_exchange_count(2).await;

    assert_two_file_entries(entry_ep.get_received_exchanges().await);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_tar_gz_split_multi_entry() {
    // ── Build the equivalent single-member TAR.GZ ──────────────────────────
    let gz_data = gzip_bytes(&tar_archive(TWO_FILES));

    // ── Set up test harness ────────────────────────────────────────────────
    let h = CamelTestContext::builder()
        .with_timer()
        .with_mock()
        .build()
        .await;

    // ── Build route: timer → set TAR.GZ body → streaming split → mock ─────
    let captured_data = gz_data.clone();
    let mut builder = RouteBuilder::from("timer:tar-gz-test?period=50&repeatCount=1")
        .route_id("tar-gz-split-test")
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
                format: StreamSplitFormat::TarGz,
                ..Default::default()
            },
            aggregation: AggregationStrategy::Original,
            stop_on_exception: false,
            steps: vec![BuilderStep::To("mock:tar-gz-entry".to_string())],
        });

    let route = builder.build().unwrap();
    h.add_route(route).await.unwrap();

    // ── Run ────────────────────────────────────────────────────────────────
    h.start().await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    h.stop().await;

    // ── Assert ─────────────────────────────────────────────────────────────
    let entry_ep = h.mock().get_endpoint("tar-gz-entry").unwrap();
    entry_ep.assert_exchange_count(2).await;

    assert_two_file_entries(entry_ep.get_received_exchanges().await);
}
