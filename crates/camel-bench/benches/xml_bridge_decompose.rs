//! XML bridge overhead decomposition (xmlperf-bridge-overhead): four
//! criterion phases attributing the end-to-end `Transform` latency —
//! non-blocking transport floor (`health_mtls`), blocking dispatch
//! without XML-engine work (`transform_dispatch_mtls`), the full
//! transform (`transform_mtls`), and the client-side prost floor
//! (`proto_serde`). See the change's design.md "Measured surfaces".

use std::collections::HashMap;
use std::time::Duration;

use camel_bench::{resolve_bridge_binary, xmlperf_evidence_run};
use camel_bridge::process::{BridgeProcess, BridgeProcessConfig};
use criterion::{Criterion, criterion_group, criterion_main};
use prost::Message;
use sha2::{Digest, Sha256};

pub mod proto {
    tonic::include_proto!("xml_bridge");
}

use proto::{
    CompileStylesheetRequest, HealthCheckRequest, TransformRequest, TransformResponse,
    bridge_error::Kind, health_client::HealthClient,
    xslt_transformer_client::XsltTransformerClient,
};

/// Mirrors camel-xslt's `xslt_bridge_decode_limit`.
const DECODE_LIMIT: usize = 17 * 1024 * 1024;
/// Unknown id: dispatch measures the blocking path with zero XML work.
const UNKNOWN_STYLESHEET_ID: &str = "xslt-unknown-id-bench";
const HEALTH_PROBE_TIMEOUT: Duration = Duration::from_secs(10);
const HEALTH_DEADLINE: Duration = Duration::from_secs(60);
const WARM_CALLS: usize = 200;

const IDENTITY_XSL: &str =
    include_str!("../../../benchmarks/scenarios/xslt-bridge/shared/identity-transform.xsl");
const PAYLOAD: &str =
    include_str!("../../../benchmarks/scenarios/xslt-bridge/shared/bench-payload.xml");

/// `XsltBridgeClient::stylesheet_id_for` algorithm computed locally:
/// sha256 hex, `xslt-{hex}`.
fn stylesheet_id_for(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(hex, "{byte:02x}");
    }
    format!("xslt-{hex}")
}

fn transform_request(stylesheet_id: String) -> TransformRequest {
    TransformRequest {
        stylesheet_id,
        document: PAYLOAD.as_bytes().to_vec(),
        parameters: HashMap::new(),
        output_method: "xml".to_string(),
    }
}

fn bench_xml_bridge(c: &mut Criterion) {
    match std::fs::read_to_string("/proc/loadavg") {
        Ok(loadavg) => println!("/proc/loadavg: {loadavg}"),
        Err(_) => println!("loadavg unavailable"),
    }

    let binary = match resolve_bridge_binary() {
        Ok(binary) => binary,
        Err(err) => {
            if xmlperf_evidence_run() {
                eprintln!("{err}");
                std::process::exit(2);
            }
            println!("skipping xml_bridge_decompose: {err}");
            return;
        }
    };

    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        // tokio ≥1.53: IO/time drivers are off by default on manual
        // builders; the bridge subprocess (piped stdout) and the TCP
        // channel need the IO driver.
        .enable_all()
        .build()
        .expect("tokio runtime");
    // Idempotent — discard the Err when a provider is already installed.
    let _ = rustls::crypto::ring::default_provider().install_default();
    // RUST_LOG is set only in diagnostic runs; without a subscriber it
    // configures nothing, and with one it would pollute timed iterations,
    // so install conditionally BEFORE any channel construction.
    if std::env::var("RUST_LOG").is_ok() {
        tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
            .with_writer(std::io::stderr)
            .init();
    }

    let (process, channel) = rt
        .block_on(BridgeProcess::start_and_connect(&BridgeProcessConfig::xml(
            binary, 60_000,
        )))
        .expect("bridge start_and_connect");

    let mut health = HealthClient::new(channel.clone()).max_decoding_message_size(DECODE_LIMIT);

    // Health readiness: poll Health.Check until SERVING or deadline
    // (loop shape from camel-bridge/src/health.rs).
    rt.block_on(async {
        let deadline = tokio::time::Instant::now() + HEALTH_DEADLINE;
        loop {
            let probe = tokio::time::timeout(
                HEALTH_PROBE_TIMEOUT,
                health.check(tonic::Request::new(HealthCheckRequest {})),
            )
            .await;
            let serving = match &probe {
                Ok(Ok(response)) => response.get_ref().status == "SERVING",
                _ => false,
            };
            if serving {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "xml bridge health check did not reach SERVING within 60s"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    });
    println!("XMLPERF_BRIDGE port={}", process.grpc_port());

    let mut xslt = XsltTransformerClient::new(channel).max_decoding_message_size(DECODE_LIMIT);
    let stylesheet_id = stylesheet_id_for(IDENTITY_XSL.as_bytes());
    rt.block_on(async {
        let response = xslt
            .compile_stylesheet(tonic::Request::new(CompileStylesheetRequest {
                stylesheet_id: stylesheet_id.clone(),
                stylesheet: IDENTITY_XSL.as_bytes().to_vec(),
            }))
            .await
            .expect("compile_stylesheet rpc")
            .into_inner();
        assert!(
            response.error.is_none(),
            "identity stylesheet compile failed: {:?}",
            response.error
        );
    });

    // Warm: ≥200 iterations of health, dispatch-transform, and transform
    // calls before criterion samples.
    rt.block_on(async {
        for _ in 0..WARM_CALLS {
            let response = health
                .check(tonic::Request::new(HealthCheckRequest {}))
                .await
                .expect("warm health rpc");
            assert_eq!(response.into_inner().status, "SERVING");

            let response = xslt
                .transform(tonic::Request::new(transform_request(
                    UNKNOWN_STYLESHEET_ID.to_string(),
                )))
                .await
                .expect("warm dispatch transform rpc")
                .into_inner();
            let error = response
                .error
                .expect("dispatch transform must error on unknown id");
            assert_eq!(error.kind, Kind::ResourceNotFound as i32);

            let response = xslt
                .transform(tonic::Request::new(transform_request(
                    stylesheet_id.clone(),
                )))
                .await
                .expect("warm transform rpc")
                .into_inner();
            assert!(
                response.error.is_none(),
                "warm transform errored: {:?}",
                response.error
            );
            assert!(!response.result.is_empty());
        }
    });

    c.bench_function("xml_bridge/health_mtls", |b| {
        b.to_async(&rt).iter(|| {
            let mut health = health.clone();
            async move {
                let response = health
                    .check(tonic::Request::new(HealthCheckRequest {}))
                    .await
                    .expect("health_mtls rpc");
                assert_eq!(response.into_inner().status, "SERVING");
            }
        })
    });

    c.bench_function("xml_bridge/transform_dispatch_mtls", |b| {
        b.to_async(&rt).iter(|| {
            let mut xslt = xslt.clone();
            async move {
                let response = xslt
                    .transform(tonic::Request::new(transform_request(
                        UNKNOWN_STYLESHEET_ID.to_string(),
                    )))
                    .await
                    .expect("transform_dispatch_mtls rpc")
                    .into_inner();
                let error = response
                    .error
                    .expect("transform_dispatch_mtls must error on unknown id");
                assert_eq!(error.kind, Kind::ResourceNotFound as i32);
            }
        })
    });

    c.bench_function("xml_bridge/transform_mtls", |b| {
        b.to_async(&rt).iter(|| {
            let mut xslt = xslt.clone();
            let request = transform_request(stylesheet_id.clone());
            async move {
                let response = xslt
                    .transform(tonic::Request::new(request))
                    .await
                    .expect("transform_mtls rpc")
                    .into_inner();
                assert!(
                    response.error.is_none(),
                    "transform_mtls errored: {:?}",
                    response.error
                );
                assert!(!response.result.is_empty());
            }
        })
    });

    let encoded_response = TransformResponse {
        result: PAYLOAD.as_bytes().to_vec(),
        error: None,
    }
    .encode_to_vec();
    c.bench_function("xml_bridge/proto_serde", |b| {
        b.iter(|| {
            let encoded = transform_request(stylesheet_id.clone()).encode_to_vec();
            let decoded =
                TransformResponse::decode(encoded_response.as_slice()).expect("decode response");
            std::hint::black_box(encoded.len());
            std::hint::black_box(decoded.result.len());
        })
    });

    rt.block_on(process.stop())
        .expect("bridge stop after benches");
}

criterion_group!(benches, bench_xml_bridge);
criterion_main!(benches);
