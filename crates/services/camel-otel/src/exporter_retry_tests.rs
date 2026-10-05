//! Focused regression tests for OTLP exporter retry behavior (bd rc-5q3ox).
//!
//! opentelemetry-otlp 0.33 enables the "recommended" retry policy (3
//! retries) by default for both HTTP and gRPC exporters. rust-camel's
//! pre-0.33 behavior was a single export attempt, so the production
//! exporters must pin `RetryPolicy::disabled()`.
//!
//! Each test arranges a mock collector that fails with a *retryable*
//! status (HTTP 503 / gRPC UNAVAILABLE), invokes exactly one export per
//! signal, and asserts the collector observed exactly one network
//! attempt. Without the disabled policy the collector sees four attempts
//! (initial + 3 retries), so these tests fail on the unfixed builders.

use super::*;
use opentelemetry_sdk::logs::LogBatch;
use opentelemetry_sdk::metrics::data::ResourceMetrics;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

/// One `OtelService` configured for the given endpoint and transport.
fn service(endpoint: &str, protocol: OtelProtocol) -> OtelService {
    OtelService::new(OtelConfig::new(endpoint, "retry-policy-test").with_protocol(protocol))
}

/// Bounded cleanup guard for the blocking HTTP/1.1 collector mock.
///
/// Counts accepted connections and answers every request with `status`
/// and `Connection: close`, so each retry opens a fresh connection (one
/// connection == one attempt). The listener runs on a std thread with no
/// Tokio runtime: the default HTTP transport is `reqwest-blocking-client`,
/// whose client owns a runtime that must not be created or dropped in an
/// async context. `Drop` stops and joins the thread (also during an
/// assertion panic), so no detached listener outlives the test.
struct HttpMock {
    port: u16,
    attempts: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl HttpMock {
    fn start(status: u16) -> Self {
        use std::io::{Read, Write};

        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind http mock");
        listener
            .set_nonblocking(true)
            .expect("nonblocking http mock");
        let port = listener.local_addr().expect("addr").port();
        let attempts = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let counter = Arc::clone(&attempts);
        let stop_flag = Arc::clone(&stop);

        let join = std::thread::spawn(move || {
            loop {
                if stop_flag.load(Ordering::SeqCst) {
                    break;
                }
                match listener.accept() {
                    Ok((mut socket, _)) => {
                        counter.fetch_add(1, Ordering::SeqCst);
                        // Bound a half-open read so Drop's join cannot hang.
                        let _ = socket.set_read_timeout(Some(Duration::from_millis(500)));
                        let mut buf = [0u8; 4096];
                        let _ = socket.read(&mut buf);
                        let response = format!(
                            "HTTP/1.1 {status} Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                        );
                        let _ = socket.write_all(response.as_bytes());
                        let _ = socket.flush();
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    Err(_) => break,
                }
            }
        });

        Self {
            port,
            attempts,
            stop,
            join: Some(join),
        }
    }

    fn attempts(&self) -> usize {
        self.attempts.load(Ordering::SeqCst)
    }
}

impl Drop for HttpMock {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

/// Minimal gRPC collector mock: counts RPCs and answers all three OTLP
/// services with UNAVAILABLE (a retryable status per the OTLP spec).
async fn start_grpc_mock() -> (u16, Arc<AtomicUsize>) {
    use opentelemetry_proto::tonic::collector::logs::v1::{
        ExportLogsServiceRequest, ExportLogsServiceResponse,
        logs_service_server::{LogsService, LogsServiceServer},
    };
    use opentelemetry_proto::tonic::collector::metrics::v1::{
        ExportMetricsServiceRequest, ExportMetricsServiceResponse,
        metrics_service_server::{MetricsService, MetricsServiceServer},
    };
    use opentelemetry_proto::tonic::collector::trace::v1::{
        ExportTraceServiceRequest, ExportTraceServiceResponse,
        trace_service_server::{TraceService, TraceServiceServer},
    };
    use tonic::{Request, Response, Status};

    #[derive(Clone)]
    struct MockCollector {
        attempts: Arc<AtomicUsize>,
    }

    #[tonic::async_trait]
    impl TraceService for MockCollector {
        async fn export(
            &self,
            _request: Request<ExportTraceServiceRequest>,
        ) -> Result<Response<ExportTraceServiceResponse>, Status> {
            self.attempts.fetch_add(1, Ordering::SeqCst);
            Err(Status::unavailable("retryable"))
        }
    }

    #[tonic::async_trait]
    impl MetricsService for MockCollector {
        async fn export(
            &self,
            _request: Request<ExportMetricsServiceRequest>,
        ) -> Result<Response<ExportMetricsServiceResponse>, Status> {
            self.attempts.fetch_add(1, Ordering::SeqCst);
            Err(Status::unavailable("retryable"))
        }
    }

    #[tonic::async_trait]
    impl LogsService for MockCollector {
        async fn export(
            &self,
            _request: Request<ExportLogsServiceRequest>,
        ) -> Result<Response<ExportLogsServiceResponse>, Status> {
            self.attempts.fetch_add(1, Ordering::SeqCst);
            Err(Status::unavailable("retryable"))
        }
    }

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind grpc mock");
    let port = listener.local_addr().expect("addr").port();
    let attempts = Arc::new(AtomicUsize::new(0));
    let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);
    let collector = MockCollector {
        attempts: Arc::clone(&attempts),
    };

    tokio::spawn(async move {
        let _ = tonic::transport::Server::builder()
            .add_service(TraceServiceServer::new(collector.clone()))
            .add_service(MetricsServiceServer::new(collector.clone()))
            .add_service(LogsServiceServer::new(collector))
            .serve_with_incoming(incoming)
            .await;
    });

    // No readiness sleep: the socket is already bound here, so the kernel
    // queues the client's connection until the server task accepts it.
    (port, attempts)
}

#[test]
fn http_exporters_make_single_attempt_per_signal() {
    let mock = HttpMock::start(503);
    let endpoint = format!("http://127.0.0.1:{}", mock.port);

    let span_exporter = service(&endpoint, OtelProtocol::HttpProtobuf)
        .build_span_exporter()
        .expect("span exporter");
    let _ = futures::executor::block_on(opentelemetry_sdk::trace::SpanExporter::export(
        &span_exporter,
        Vec::new(),
    ));
    assert_eq!(
        mock.attempts(),
        1,
        "span exporter must make exactly one HTTP attempt on a retryable failure"
    );
    drop(span_exporter);

    let metric_exporter = service(&endpoint, OtelProtocol::HttpProtobuf)
        .build_metric_exporter()
        .expect("metric exporter");
    let resource_metrics = ResourceMetrics::default();
    let _ = futures::executor::block_on(
        opentelemetry_sdk::metrics::exporter::PushMetricExporter::export(
            &metric_exporter,
            &resource_metrics,
        ),
    );
    assert_eq!(
        mock.attempts(),
        2,
        "metric exporter must make exactly one HTTP attempt on a retryable failure"
    );
    drop(metric_exporter);

    let log_exporter = service(&endpoint, OtelProtocol::HttpProtobuf)
        .build_log_exporter()
        .expect("log exporter");
    let _ = futures::executor::block_on(opentelemetry_sdk::logs::LogExporter::export(
        &log_exporter,
        LogBatch::new(&[]),
    ));
    assert_eq!(
        mock.attempts(),
        3,
        "log exporter must make exactly one HTTP attempt on a retryable failure"
    );
    drop(log_exporter);
}

/// Same assertion over the gRPC/tonic production builders.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn grpc_exporters_make_single_attempt_per_signal() {
    let (port, attempts) = start_grpc_mock().await;
    let endpoint = format!("http://127.0.0.1:{port}");

    let svc = service(&endpoint, OtelProtocol::Grpc);
    let span_exporter = svc.build_span_exporter().expect("span exporter");
    let _ = opentelemetry_sdk::trace::SpanExporter::export(&span_exporter, Vec::new()).await;
    assert_eq!(
        attempts.load(Ordering::SeqCst),
        1,
        "span exporter must make exactly one gRPC attempt on a retryable failure"
    );

    let svc = service(&endpoint, OtelProtocol::Grpc);
    let metric_exporter = svc.build_metric_exporter().expect("metric exporter");
    let resource_metrics = ResourceMetrics::default();
    let _ = opentelemetry_sdk::metrics::exporter::PushMetricExporter::export(
        &metric_exporter,
        &resource_metrics,
    )
    .await;
    assert_eq!(
        attempts.load(Ordering::SeqCst),
        2,
        "metric exporter must make exactly one gRPC attempt on a retryable failure"
    );

    let svc = service(&endpoint, OtelProtocol::Grpc);
    let log_exporter = svc.build_log_exporter().expect("log exporter");
    let _ = opentelemetry_sdk::logs::LogExporter::export(&log_exporter, LogBatch::new(&[])).await;
    assert_eq!(
        attempts.load(Ordering::SeqCst),
        3,
        "log exporter must make exactly one gRPC attempt on a retryable failure"
    );
}
