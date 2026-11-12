//! Consumer-side observable business-error-mapping tests (business-error-model
//! task 2.2).
//!
//! The snapshot proof lives in camel-test because component-crate tests cannot
//! reach `tests/support`. OBSERVABLE proof only: HTTP reply status/body and
//! mock-endpoint delivery counts — no private consumer-state inspection.
//!
//! Pins:
//!
//! - a seam rest route (lowering always sets `http_error_mapping: Some(_)`)
//!   with a global dead-letter channel: an unmatched business error is NOT
//!   dead-lettered, NOT consumed by the synthetic Handled catch-all (the error
//!   is not cleared → never 2xx), and the reply maps through the built-in
//!   default kind map;
//! - a matched on_exceptions clause without `handled_by` (which normally falls
//!   back to the DLC via `policy_producer.or(dlc)`) applies its disposition
//!   without the dead-letter fallback, and the reply maps;
//! - the error handler logs the seam propagate at DEBUG only (the info/warn
//!   propagate record is the HTTP finalizer's — no double record);
//! - two routes with DIFFERENT mappings reply with their own statuses
//!   (per-registration isolation) and a `suspend_route → resume_route` cycle
//!   preserves each route's own mapping (snapshot survives resume).

mod support;

use std::time::Duration;

use camel_api::circuit_breaker::CircuitBreakerConfig;
use camel_api::error_handler::ErrorHandlerConfig;
use camel_api::runtime::RuntimeCommand;
use camel_api::{BoxProcessor, BoxProcessorExt, CamelError, Exchange};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_http::HttpComponent;
use camel_core::route::RouteDefinition;
use camel_dsl::parse_yaml;
use camel_test::CamelTestContext;

use support::stage_http_listener;

/// Install one process-global registry default so the production tracing
/// callsites cache interest as enabled even when a sibling test executes
/// them without a capture subscriber (guard contract in
/// `camel_component_api::test_support::log_capture`; same pattern as
/// `business_error_policy_test.rs`).
#[ctor::ctor(unsafe)]
fn install_global_tracing_registry() {
    let _ = tracing::subscriber::set_global_default(tracing_subscriber::registry());
}

/// Same process-level rustls CryptoProvider installation as the gated
/// `support::install_crypto_provider` — that helper is feature-gated and this
/// test binary is not, so the body is repeated here (rustls is a plain
/// dev-dependency of camel-test).
fn install_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

const REQUEST_DEADLINE: Duration = Duration::from_secs(10);

/// A rest BLOCK body (list item under a single `rest:` key) whose single GET
/// operation throws a business error. `mapping` is the YAML fragment for the
/// block-level `error_mapping` table (already indented), `None` omits it.
fn rest_block(port: u16, base_path: &str, code: &str, mapping: Option<&str>) -> String {
    let mut block = format!("  - host: 127.0.0.1\n    port: {port}\n    path: \"{base_path}\"\n");
    if let Some(fragment) = mapping {
        block.push_str("    error_mapping:\n");
        block.push_str(fragment);
        block.push('\n');
    }
    block.push_str(&format!(
        "    operations:\n      - method: GET\n        path: \"orders\"\n        steps:\n          - throw: {{ code: \"{code}\", kind: \"not-found\", message: \"missing\" }}\n"
    ));
    block
}

/// A rest block whose single GET operation throws a business error, wrapped in
/// its own document. `mapping` is the YAML fragment for the block-level
/// `error_mapping` table (already indented), `None` omits the block entirely.
fn rest_yaml(port: u16, base_path: &str, code: &str, mapping: Option<&str>) -> String {
    format!("rest:\n{}", rest_block(port, base_path, code, mapping))
}
/// GET `url` and return `(status, body text)`. Bounded retry while the request
/// itself fails (consumer registration race); no sleeps, only yields.
async fn get_reply(url: &str) -> (u16, String) {
    let client = reqwest::Client::new();
    let deadline = tokio::time::Instant::now() + REQUEST_DEADLINE;
    loop {
        match client.get(url).send().await {
            Ok(resp) => {
                let status = resp.status().as_u16();
                let body = resp.text().await.unwrap_or_default();
                return (status, body);
            }
            Err(_) if tokio::time::Instant::now() < deadline => {
                tokio::task::yield_now().await;
            }
            Err(e) => panic!("request to {url} failed past the deadline: {e}"),
        }
    }
}

/// Like [`get_reply`] but keeps retrying while the route answers 404 (the
/// post-resume re-registration window), so the FIRST served reply proves the
/// mapping travelled with the resumed registration.
async fn get_reply_after_gap(url: &str) -> (u16, String) {
    let client = reqwest::Client::new();
    let deadline = tokio::time::Instant::now() + REQUEST_DEADLINE;
    loop {
        let outcome = client.get(url).send().await;
        match outcome {
            Ok(resp) if resp.status().as_u16() == 404 && tokio::time::Instant::now() < deadline => {
                tokio::task::yield_now().await;
            }
            Ok(resp) => {
                let status = resp.status().as_u16();
                let body = resp.text().await.unwrap_or_default();
                return (status, body);
            }
            Err(_) if tokio::time::Instant::now() < deadline => {
                tokio::task::yield_now().await;
            }
            Err(e) => panic!("request to {url} failed past the deadline: {e}"),
        }
    }
}

/// Two rest routes on separate listeners with DIFFERENT mappings for the SAME
/// kind (`not-found → 410` vs `not-found → 402`; neither equals the built-in
/// default 404, so both statuses prove the explicit table was served). Returns
/// the harness, the two ports, and the lowered route ids.
async fn isolation_fixture() -> (CamelTestContext, u16, u16, Vec<String>) {
    install_crypto_provider();
    let port_a = stage_http_listener("127.0.0.1").await;
    let port_b = stage_http_listener("127.0.0.1").await;
    let h = CamelTestContext::builder()
        .with_component(HttpComponent::new())
        .build()
        .await;

    let yaml = format!(
        "rest:\n{}{}",
        rest_block(
            port_a,
            "/alpha",
            "ISO_A",
            Some("      kinds: { \"not-found\": 410 }")
        ),
        rest_block(
            port_b,
            "/beta",
            "ISO_B",
            Some("      kinds: { \"not-found\": 402 }")
        ),
    );
    let definitions = parse_yaml(&yaml).expect("rest document must load");
    let route_ids: Vec<String> = definitions
        .iter()
        .map(|d| d.route_id().to_string())
        .collect();
    assert_eq!(route_ids.len(), 2, "two operations lower to two routes");
    for route in definitions {
        h.add_route(route).await.expect("route must stage");
    }
    h.start().await;
    (h, port_a, port_b, route_ids)
}

/// Suspend or resume one route through the runtime bus and wait for the
/// aggregate status to settle (`Suspended` / `Started`). Stopped routes cannot
/// resume — the suspend/resume pair is the only lifecycle cycle that re-runs
/// the consumer registration seam.
async fn cycle_route(h: &CamelTestContext, route_id: &str, suspend: bool) {
    // Unique command id per invocation: the runtime bus dedupes execute()
    // commands by `command_id`, so a repeated id would be a silent no-op.
    static COMMAND_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let expected = if suspend { "Suspended" } else { "Started" };
    {
        let ctx = h.ctx().lock().await; // allow-test-wait: harness ctx mutex is test-owned
        let seq = COMMAND_SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let command = if suspend {
            RuntimeCommand::SuspendRoute {
                route_id: route_id.to_string(),
                command_id: format!("bizerrors-suspend-{seq}"),
                causation_id: None,
            }
        } else {
            RuntimeCommand::ResumeRoute {
                route_id: route_id.to_string(),
                command_id: format!("bizerrors-resume-{seq}"),
                causation_id: None,
            }
        };
        ctx.runtime()
            .execute(command)
            .await
            .unwrap_or_else(|e| panic!("{route_id}: suspend/resume command failed: {e}"));
    }
    let deadline = tokio::time::Instant::now() + REQUEST_DEADLINE;
    loop {
        let status = {
            let ctx = h.ctx().lock().await; // allow-test-wait: harness ctx mutex is test-owned
            ctx.runtime_route_status(route_id).await.ok().flatten()
        };
        if status.as_deref() == Some(expected) {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "{route_id} never reached status {expected}, last: {status:?}"
        );
        tokio::task::yield_now().await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn seam_route_business_error_skips_dlc_and_synthetic_policy() {
    install_crypto_provider();
    let port = stage_http_listener("127.0.0.1").await;
    let h = CamelTestContext::builder()
        .with_component(HttpComponent::new())
        .with_mock()
        .build()
        .await;

    // Global dead-letter channel with NO explicit policies: the handler
    // resolves the synthetic catch-all Handled policy backed by the DLC —
    // exactly the configuration a seam route must bypass for business errors.
    h.ctx()
        .lock()
        .await // allow-test-wait: harness ctx mutex is test-owned
        .set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:dlc"))
        .await;

    let yaml = rest_yaml(port, "/seam", "SEAM_DLC_X", None);
    for route in parse_yaml(&yaml).expect("rest document must load") {
        h.add_route(route).await.expect("route must stage");
    }
    h.start().await;

    let (status, body) = get_reply(&format!("http://127.0.0.1:{port}/seam/orders")).await;
    assert_ne!(
        status / 100,
        2,
        "a business error must never answer 2xx, got {status} {body}"
    );
    assert_eq!(
        status, 404,
        "unmatched business error must map through the built-in default kind map \
         (not-found → 404), got {status} {body}"
    );
    let reply: serde_json::Value =
        serde_json::from_str(&body).unwrap_or_else(|e| panic!("reply body must be JSON: {e}"));
    assert_eq!(
        reply["error"], "SEAM_DLC_X",
        "reply body error code: {body}"
    );
    assert_eq!(reply["kind"], "not-found", "reply body kind: {body}");

    // NOT dispatched to the DLC: the synthetic Handled catch-all consumed
    // nothing, so the error was not cleared into a dead-letter side effect.
    // The reply has already arrived, so the dispatch decision is final.
    let dlc = h.mock().get_endpoint("dlc").expect("mock dlc endpoint");
    dlc.assert_exchange_count(0).await;

    h.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn matched_clause_without_handled_by_fallback_bypassed_on_seam() {
    install_crypto_provider();
    let port = stage_http_listener("127.0.0.1").await;
    let h = CamelTestContext::builder()
        .with_component(HttpComponent::new())
        .with_mock()
        .build()
        .await;

    // Global handler with an EXPLICIT clause matching the business code and no
    // `handled_by`: disposition defaults to Propagate, and the clause producer
    // falls back to the DLC (`policy_producer.or(dlc)`). On a seam route the
    // fallback must be suppressed while the clause's disposition still applies.
    let global = ErrorHandlerConfig::dead_letter_channel("mock:dlc")
        .on_exception(|e| matches!(e, CamelError::Business { code, .. } if code == "SEAM_CLAUSE_X"))
        .build();
    h.ctx()
        .lock()
        .await // allow-test-wait: harness ctx mutex is test-owned
        .set_error_handler(global)
        .await;

    let yaml = rest_yaml(port, "/clause", "SEAM_CLAUSE_X", None);
    for route in parse_yaml(&yaml).expect("rest document must load") {
        h.add_route(route).await.expect("route must stage");
    }
    h.start().await;

    let (status, body) = get_reply(&format!("http://127.0.0.1:{port}/clause/orders")).await;
    assert_eq!(
        status, 404,
        "the matched clause's Propagate disposition applies and the reply maps \
         through the default kind map, got {status} {body}"
    );

    // No dead-letter fallback for the business error on the seam route.
    let dlc = h.mock().get_endpoint("dlc").expect("mock dlc endpoint");
    dlc.assert_exchange_count(0).await;

    h.stop().await;
}

// ── Log capture (same pattern as business_error_policy_test.rs) ──────────
#[derive(Clone)]
struct JsonLogBuffer(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for JsonLogBuffer {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for JsonLogBuffer {
    type Writer = JsonLogBuffer;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// Run `f` with a DEBUG-level fmt-json capture subscriber installed on the
/// current thread and return one parsed record per event.
fn capture_debug_records<T>(f: impl FnOnce() -> T) -> (T, Vec<serde_json::Value>) {
    let buffer = JsonLogBuffer(std::sync::Arc::new(std::sync::Mutex::new(Vec::new())));
    let subscriber = tracing_subscriber::fmt()
        .json()
        .with_max_level(tracing_subscriber::filter::LevelFilter::DEBUG)
        .without_time()
        .with_writer(buffer.clone())
        .finish();

    let output = {
        use tracing_subscriber::util::SubscriberInitExt;
        let _guard = subscriber.set_default();
        f()
    };

    (output, parse_records(&buffer))
}

/// Like [`capture_debug_records`], but hands the capture buffer to `f` so a
/// test whose assertion is produced by a DETACHED route (seda, timer, …) can
/// wait, bounded and yield-only, for the record it needs before the subscriber
/// is torn down.
fn capture_debug_records_shared<T>(
    f: impl FnOnce(JsonLogBuffer) -> T,
) -> (T, Vec<serde_json::Value>) {
    let buffer = JsonLogBuffer(std::sync::Arc::new(std::sync::Mutex::new(Vec::new())));
    let subscriber = tracing_subscriber::fmt()
        .json()
        .with_max_level(tracing_subscriber::filter::LevelFilter::DEBUG)
        .without_time()
        .with_writer(buffer.clone())
        .finish();

    let output = {
        use tracing_subscriber::util::SubscriberInitExt;
        let _guard = subscriber.set_default();
        f(buffer.clone())
    };

    (output, parse_records(&buffer))
}

/// Parse one JSON record per captured line.
fn parse_records(buffer: &JsonLogBuffer) -> Vec<serde_json::Value> {
    let raw = buffer
        .0
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    raw.split(|&b| b == b'\n')
        .filter(|line| !line.iter().all(|b| b.is_ascii_whitespace()))
        .map(|line| {
            serde_json::from_slice(line).unwrap_or_else(|e| {
                panic!(
                    "captured log line is not JSON ({e}): {}",
                    String::from_utf8_lossy(line)
                )
            })
        })
        .collect()
}

fn block_on<T>(future: impl std::future::Future<Output = T>) -> T {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("current-thread runtime")
        .block_on(future)
}

/// Structured field lookup: fmt-json nests event fields under `fields`.
fn field_str<'a>(record: &'a serde_json::Value, name: &str) -> Option<&'a str> {
    record.get(name).and_then(|v| v.as_str()).or_else(|| {
        record
            .get("fields")
            .and_then(|f| f.get(name))
            .and_then(|v| v.as_str())
    })
}

fn level_of(record: &serde_json::Value) -> Option<&str> {
    record.get("level").and_then(|l| l.as_str())
}

fn target_of(record: &serde_json::Value) -> &str {
    record
        .get("target")
        .and_then(|t| t.as_str())
        .unwrap_or_default()
}

/// Records carrying `code` emitted by the error handler (camel_processor).
fn handler_records_with_code<'a>(
    records: &'a [serde_json::Value],
    code: &str,
) -> Vec<&'a serde_json::Value> {
    records
        .iter()
        .filter(|record| {
            target_of(record).starts_with("camel_processor")
                && field_str(record, "code") == Some(code)
        })
        .collect()
}

/// Records carrying `code` emitted by the HTTP finalizer.
fn finalizer_records_with_code<'a>(
    records: &'a [serde_json::Value],
    code: &str,
) -> Vec<&'a serde_json::Value> {
    records
        .iter()
        .filter(|record| {
            target_of(record).starts_with("camel_component_http")
                && field_str(record, "code") == Some(code)
        })
        .collect()
}

#[test]
fn seam_route_propagate_logs_debug_at_error_handler() {
    let ((), records) = capture_debug_records(|| {
        block_on(async {
            install_crypto_provider();
            let port = stage_http_listener("127.0.0.1").await;
            let h = CamelTestContext::builder()
                .with_component(HttpComponent::new())
                .with_mock()
                .build()
                .await;
            h.ctx()
                .lock()
                .await // allow-test-wait: harness ctx mutex is test-owned
                .set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:dlc"))
                .await;

            let yaml = rest_yaml(port, "/debug", "SEAM_DBG_X", None);
            for route in parse_yaml(&yaml).expect("rest document must load") {
                h.add_route(route).await.expect("route must stage");
            }
            h.start().await;

            let (status, body) = get_reply(&format!("http://127.0.0.1:{port}/debug/orders")).await;
            assert_eq!(
                status, 404,
                "seam business error must reach the mapped reply: {body}"
            );

            h.stop().await;
        })
    });

    // The error handler's ONLY record for the seam propagate is DEBUG — the
    // info/warn propagate record belongs to the HTTP finalizer (owner decision
    // Q7): no double record.
    let handler = handler_records_with_code(&records, "SEAM_DBG_X");
    assert_eq!(
        handler.len(),
        1,
        "exactly one error-handler record for the code, got: {records:?}"
    );
    assert_eq!(
        level_of(handler[0]),
        Some("DEBUG"),
        "the error-handler seam propagate record must be debug: {:?}",
        handler[0]
    );

    // The finalizer owns the single info record (status below 500).
    let finalizer = finalizer_records_with_code(&records, "SEAM_DBG_X");
    assert_eq!(
        finalizer.len(),
        1,
        "exactly one finalizer record for the code, got: {records:?}"
    );
    assert_eq!(
        level_of(finalizer[0]),
        Some("INFO"),
        "the finalizer propagate record below 500 is info: {:?}",
        finalizer[0]
    );

    // No warn records anywhere for the code, and no ERROR records at all from
    // the error handler.
    for record in &records {
        if field_str(record, "code") == Some("SEAM_DBG_X") {
            assert_ne!(
                level_of(record),
                Some("WARN"),
                "no warn record may carry the code: {record}"
            );
        }
        assert_ne!(
            level_of(record),
            Some("ERROR"),
            "business errors must never produce ERROR records: {record}"
        );
    }
}

/// bd rc-c3vds (STAGE-4 expert REJECT on double-logging): a PLAIN (non-rest)
/// HTTP route is not rest-lowered, so it carries no `http_error_mapping` and
/// the mapping-keyed DLC bypass cannot tell it is transport-replied. An
/// INSTALLED error handler whose policy chain produces no producer must NOT
/// emit its own unmatched-propagate WARN on such a route: the HTTP finalizer
/// owns the single propagate record (owner decision Q7 — transport-owned and
/// never doubled). Before the reply-seam flag the handler emitted a WARN and
/// the finalizer an INFO, so a 4xx failure alerted twice.
#[test]
fn plain_http_route_propagate_logs_once_finalizer_owned() {
    let ((), records) = capture_debug_records(|| {
        block_on(async {
            install_crypto_provider();
            let port = stage_http_listener("127.0.0.1").await;
            let h = CamelTestContext::builder()
                .with_component(HttpComponent::new())
                .build()
                .await;

            let yaml = format!(
                r#"routes:
  - id: "plain-reply-seam"
    from: "http://127.0.0.1:{port}/plain"
    error_handler: {{}}
    steps:
      - throw: {{ code: "PLAIN_HTTP_X", kind: "not-found", message: "missing" }}
"#
            );
            for route in parse_yaml(&yaml).expect("YAML parse failed") {
                h.add_route(route).await.expect("route must stage");
            }
            h.start().await;

            let (status, body) = get_reply(&format!("http://127.0.0.1:{port}/plain")).await;
            assert_eq!(
                status, 404,
                "the HTTP finalizer maps the business error: {body}"
            );

            h.stop().await;
        })
    });

    // The finalizer owns the single propagate record (built-in default kind
    // map: not-found → 404, below 500 → INFO).
    let finalizer = finalizer_records_with_code(&records, "PLAIN_HTTP_X");
    assert_eq!(
        finalizer.len(),
        1,
        "exactly one finalizer record for the code, got: {records:?}"
    );
    assert_eq!(
        level_of(finalizer[0]),
        Some("INFO"),
        "the finalizer propagate record below 500 is info: {:?}",
        finalizer[0]
    );

    // The error handler still logs the seam diagnostic, but at DEBUG only.
    let handler = handler_records_with_code(&records, "PLAIN_HTTP_X");
    assert_eq!(
        handler.len(),
        1,
        "exactly one error-handler record for the code, got: {records:?}"
    );
    assert_eq!(
        level_of(handler[0]),
        Some("DEBUG"),
        "the error-handler propagate record must be debug: {:?}",
        handler[0]
    );

    // EXACTLY ONE propagate record total: no doubled WARN/INFO for one failure.
    let propagate: Vec<&serde_json::Value> = records
        .iter()
        .filter(|record| field_str(record, "code") == Some("PLAIN_HTTP_X"))
        .filter(|record| matches!(level_of(record), Some("INFO") | Some("WARN")))
        .collect();
    assert_eq!(
        propagate.len(),
        1,
        "exactly one propagate record (the finalizer's), got: {records:?}"
    );
    for record in &records {
        if field_str(record, "code") == Some("PLAIN_HTTP_X") {
            assert_ne!(
                level_of(record),
                Some("WARN"),
                "no warn record may carry the code: {record}"
            );
        }
        assert_ne!(
            level_of(record),
            Some("ERROR"),
            "business errors must never produce ERROR records: {record}"
        );
    }
}

/// bd rc-c3vds (STAGE-4 expert REJECT on wrapper endpoints): a route whose
/// `from` URI is a DELEGATING wrapper (`master:<lock>:http://…`) replies
/// through the wrapped camel-http consumer's finalizer, yet the wrapper scheme
/// is not `http`, so a strict from-scheme check classified it non-HTTP and the
/// error handler logged its own WARN on top of the finalizer INFO (double
/// record). Classification now unwraps wrappers to the EFFECTIVE consumer, so
/// the handler leaves the single propagate record to the finalizer.
///
/// `master:` needs no cluster infra here: the context default is
/// `NoopPlatformService`, which grants leadership immediately, so the HTTP
/// delegate consumer starts and serves.
#[test]
fn wrapper_endpoint_classifies_http_reply_seam() {
    use camel_master::MasterComponent;
    use camel_master::config::MasterComponentConfig;

    let ((), records) = capture_debug_records(|| {
        block_on(async {
            install_crypto_provider();
            let port = stage_http_listener("127.0.0.1").await;
            let h = CamelTestContext::builder()
                .with_component(HttpComponent::new())
                .with_component(MasterComponent::new(MasterComponentConfig::default()))
                .build()
                .await;

            let yaml = format!(
                r#"routes:
  - id: "wrapped-reply-seam"
    from: "master:wrapped-lock:http://127.0.0.1:{port}/wrapped"
    error_handler: {{}}
    steps:
      - throw: {{ code: "WRAPPED_HTTP_X", kind: "not-found", message: "missing" }}
"#
            );
            for route in parse_yaml(&yaml).expect("YAML parse failed") {
                h.add_route(route).await.expect("route must stage");
            }
            h.start().await;

            let (status, body) = get_reply(&format!("http://127.0.0.1:{port}/wrapped")).await;
            assert_eq!(
                status, 404,
                "the wrapped HTTP finalizer maps the business error: {body}"
            );

            h.stop().await;
        })
    });

    // The wrapped consumer's finalizer owns the single propagate record.
    let finalizer = finalizer_records_with_code(&records, "WRAPPED_HTTP_X");
    assert_eq!(
        finalizer.len(),
        1,
        "exactly one finalizer record for the code, got: {records:?}"
    );
    assert_eq!(
        level_of(finalizer[0]),
        Some("INFO"),
        "the finalizer propagate record below 500 is info: {:?}",
        finalizer[0]
    );

    // The error handler logs the seam diagnostic at DEBUG only.
    let handler = handler_records_with_code(&records, "WRAPPED_HTTP_X");
    assert_eq!(
        handler.len(),
        1,
        "exactly one error-handler record for the code, got: {records:?}"
    );
    assert_eq!(
        level_of(handler[0]),
        Some("DEBUG"),
        "the error-handler propagate record must be debug: {:?}",
        handler[0]
    );

    // EXACTLY ONE propagate record total: no doubled WARN/INFO for one failure.
    let propagate: Vec<&serde_json::Value> = records
        .iter()
        .filter(|record| field_str(record, "code") == Some("WRAPPED_HTTP_X"))
        .filter(|record| matches!(level_of(record), Some("INFO") | Some("WARN")))
        .collect();
    assert_eq!(
        propagate.len(),
        1,
        "exactly one propagate record (the finalizer's), got: {records:?}"
    );
    for record in &records {
        if field_str(record, "code") == Some("WRAPPED_HTTP_X") {
            assert_ne!(
                level_of(record),
                Some("WARN"),
                "no warn record may carry the code: {record}"
            );
        }
    }
}

/// bd rc-c3vds (STAGE-4 expert REJECT): the HTTP reply-ownership marker must
/// NOT survive a detached handoff. An HTTP route that sends InOnly to
/// `seda:jobs` hands the exchange to a consumer route that no HTTP finalizer
/// will ever observe, yet the marker used to ride along — so the seda-side
/// installed-but-producer-less error handler suppressed its unmatched-propagate
/// WARN (silent business-error propagation, WARN count 0). The fire-and-forget
/// delivery clears the marker, so the detached route logs with its normal
/// ownership: exactly one WARN, from its own handler.
#[test]
fn seda_handoff_clears_http_reply_ownership() {
    let ((), records) = capture_debug_records_shared(|buffer| {
        block_on(async {
            install_crypto_provider();
            let port = stage_http_listener("127.0.0.1").await;
            let h = CamelTestContext::builder()
                .with_component(HttpComponent::new())
                .with_seda()
                .build()
                .await;

            let yaml = format!(
                r#"routes:
  - id: "detaching-http"
    from: "http://127.0.0.1:{port}/detach"
    steps:
      - to: "seda:jobs"
  - id: "detached-consumer"
    from: "seda:jobs"
    error_handler: {{}}
    steps:
      - throw: {{ code: "SEDA_DETACH_X", kind: "not-found", message: "missing" }}
"#
            );
            for route in parse_yaml(&yaml).expect("YAML parse failed") {
                h.add_route(route).await.expect("route must stage");
            }
            h.start().await;

            // Fire-and-forget: the HTTP route completes before the detached
            // consumer route runs, so the request itself succeeds.
            let (status, body) = get_reply(&format!("http://127.0.0.1:{port}/detach")).await;
            assert_eq!(
                status, 200,
                "the detached seda failure cannot reach the HTTP reply: {body}"
            );

            // The seda-side handler logs on another task: wait (bounded,
            // yield-only) for the record it must emit.
            let deadline = tokio::time::Instant::now() + REQUEST_DEADLINE;
            while tokio::time::Instant::now() < deadline {
                let warned = parse_records(&buffer).iter().any(|record| {
                    target_of(record).starts_with("camel_processor")
                        && field_str(record, "code") == Some("SEDA_DETACH_X")
                        && level_of(record) == Some("WARN")
                });
                if warned {
                    break;
                }
                tokio::task::yield_now().await;
            }

            h.stop().await;
        })
    });

    let handler = handler_records_with_code(&records, "SEDA_DETACH_X");
    let warn: Vec<&serde_json::Value> = handler
        .iter()
        .copied()
        .filter(|record| level_of(record) == Some("WARN"))
        .collect();
    assert_eq!(
        warn.len(),
        1,
        "the detached route's handler owns exactly one WARN (not zero: the marker \
         must not survive the handoff; not two: no double record), got: {records:?}"
    );
    assert_eq!(
        field_str(warn[0], "kind"),
        Some("not-found"),
        "the WARN carries the business error kind: {:?}",
        warn[0]
    );
    assert!(
        field_str(warn[0], "data").is_none(),
        "propagate log must not carry a data field: {:?}",
        warn[0]
    );
    assert!(
        handler
            .iter()
            .all(|record| level_of(record) != Some("DEBUG")
                || field_str(record, "message") != Some("business error propagating to the HTTP reply seam (finalizer owns the record)")),
        "the detached route must not take the seam-owned DEBUG suppression: {records:?}"
    );
    for record in &records {
        if field_str(record, "code") == Some("SEDA_DETACH_X") {
            assert_ne!(
                level_of(record),
                Some("ERROR"),
                "business errors must never produce ERROR records: {record}"
            );
        }
    }
}

/// bd rc-c3vds (STAGE-4 expert REJECT round 3): a wire tap is a DETACHED
/// delivery, so the HTTP reply-ownership marker must not ride along on the
/// tap copy. The wiretap dispatches the copy fire-and-forget and bypasses
/// `ConsumerContext::send`, so a marker surviving the fork makes the tap
/// route's installed-but-producer-less error handler suppress its
/// unmatched-propagate WARN (HTTP 200, WARN count 0, incorrect
/// finalizer-owned DEBUG) while no HTTP finalizer ever sees the tap-side
/// failure. The tap copy drops the marker; the main exchange keeps it and
/// still succeeds.
#[test]
fn wire_tap_clears_http_reply_ownership() {
    let ((), records) = capture_debug_records_shared(|buffer| {
        block_on(async {
            install_crypto_provider();
            let port = stage_http_listener("127.0.0.1").await;
            let h = CamelTestContext::builder()
                .with_component(HttpComponent::new())
                .with_direct()
                .build()
                .await;

            let yaml = format!(
                r#"routes:
  - id: "tapping-http"
    from: "http://127.0.0.1:{port}/tap"
    steps:
      - wire_tap: "direct:tap"
  - id: "tap-target"
    from: "direct:tap"
    error_handler: {{}}
    steps:
      - throw: {{ code: "WIRE_TAP_X", kind: "not-found", message: "missing" }}
"#
            );
            for route in parse_yaml(&yaml).expect("YAML parse failed") {
                h.add_route(route).await.expect("route must stage");
            }
            h.start().await;

            // The main path succeeds: the tap failure is detached and cannot
            // reach the HTTP reply.
            let (status, body) = get_reply(&format!("http://127.0.0.1:{port}/tap")).await;
            assert_eq!(
                status, 200,
                "the detached wire-tap failure cannot reach the HTTP reply: {body}"
            );

            // The tap route logs on another task: wait (bounded, yield-only)
            // for the WARN its own handler must emit.
            let deadline = tokio::time::Instant::now() + REQUEST_DEADLINE;
            while tokio::time::Instant::now() < deadline {
                let warned = parse_records(&buffer).iter().any(|record| {
                    target_of(record).starts_with("camel_processor")
                        && field_str(record, "code") == Some("WIRE_TAP_X")
                        && level_of(record) == Some("WARN")
                });
                if warned {
                    break;
                }
                tokio::task::yield_now().await;
            }

            h.stop().await;
        })
    });

    let handler = handler_records_with_code(&records, "WIRE_TAP_X");
    let warn: Vec<&serde_json::Value> = handler
        .iter()
        .copied()
        .filter(|record| level_of(record) == Some("WARN"))
        .collect();
    assert_eq!(
        warn.len(),
        1,
        "the tap route's handler owns exactly one WARN (not zero: the marker \
         must not survive the detached tap fork; not two: no double record), \
         got: {records:?}"
    );
    assert_eq!(
        field_str(warn[0], "kind"),
        Some("not-found"),
        "the WARN carries the business error kind: {:?}",
        warn[0]
    );
    assert!(
        handler.iter().all(|record| level_of(record) != Some("DEBUG")
            || field_str(record, "message")
                != Some("business error propagating to the HTTP reply seam (finalizer owns the record)")),
        "the tap copy must not take the seam-owned DEBUG suppression: {records:?}"
    );
    for record in &records {
        if field_str(record, "code") == Some("WIRE_TAP_X") {
            assert_ne!(
                level_of(record),
                Some("ERROR"),
                "business errors must never produce ERROR records: {record}"
            );
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn per_registration_isolation() {
    let (h, port_a, port_b, _) = isolation_fixture().await;

    let (status_a, body_a) = get_reply(&format!("http://127.0.0.1:{port_a}/alpha/orders")).await;
    let (status_b, body_b) = get_reply(&format!("http://127.0.0.1:{port_b}/beta/orders")).await;
    assert_eq!(
        status_a, 410,
        "route A must serve its own mapping (not-found → 410), got {status_a} {body_a}"
    );
    assert_eq!(
        status_b, 402,
        "route B must serve its own mapping (not-found → 402), got {status_b} {body_b}"
    );

    h.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn mapping_snapshot_survives_resume() {
    let (h, port_a, port_b, route_ids) = isolation_fixture().await;

    // Suspend → resume re-runs the consumer registration seam for BOTH routes;
    // each resumed consumer must carry its own mapping snapshot again.
    for route_id in &route_ids {
        cycle_route(&h, route_id, true).await;
    }
    for route_id in &route_ids {
        cycle_route(&h, route_id, false).await;
    }

    // A stopped route cannot resume — but a SUSPENDED route resumes through
    // the controller's resume path, so the replies keep the route's own
    // mapping (post-resume re-registration window tolerated on 404).
    let (status_a, body_a) =
        get_reply_after_gap(&format!("http://127.0.0.1:{port_a}/alpha/orders")).await;
    let (status_b, body_b) =
        get_reply_after_gap(&format!("http://127.0.0.1:{port_b}/beta/orders")).await;
    assert_eq!(
        status_a, 410,
        "route A's mapping must survive resume, got {status_a} {body_a}"
    );
    assert_eq!(
        status_b, 402,
        "route B's mapping must survive resume, got {status_b} {body_b}"
    );

    h.stop().await;
}

// ── Task 2.3: end-to-end mapping closure, DLC coexistence, rc-7lkp2 ──────

/// A rest BLOCK body whose `operations` YAML is emitted verbatim under
/// `operations:`, so a single block-level table can be shared by several
/// operations (rest inheritance). `mapping` is the YAML fragment for the
/// block-level `error_mapping` table (already indented), `None` omits it.
fn rest_block_with_operations(
    port: u16,
    base_path: &str,
    mapping: Option<&str>,
    operations: &str,
) -> String {
    let mut block = format!("  - host: 127.0.0.1\n    port: {port}\n    path: \"{base_path}\"\n");
    if let Some(fragment) = mapping {
        block.push_str("    error_mapping:\n");
        block.push_str(fragment);
        block.push('\n');
    }
    block.push_str("    operations:\n");
    block.push_str(operations);
    block
}

/// A complete `rest:` document around [`rest_block_with_operations`].
fn rest_yaml_with_operations(
    port: u16,
    base_path: &str,
    mapping: Option<&str>,
    operations: &str,
) -> String {
    format!(
        "rest:\n{}",
        rest_block_with_operations(port, base_path, mapping, operations)
    )
}

/// Setups (1) and (4): one block table shared by TWO operations. The first
/// raises `not-found` (the block's deliberate `410` override — the built-in
/// default is `404`, so a 404 expectation could not discriminate the table);
/// the second raises `conflict`, which is NOT in the block table and falls
/// through to the built-in default `409`.
const TWO_OPERATION_TABLE: &str = r#"      - method: GET
        path: "missing"
        steps:
          - throw: { code: "ORDER_NOT_FOUND", kind: "not-found", message: "missing" }
      - method: GET
        path: "conflict"
        steps:
          - throw: { code: "ORDER_CONFLICT", kind: "conflict", message: "stale" }
"#;

/// Setup (2): one rest block (one inherited table) with a business error
/// operation and a SYSTEM error operation. The system operation uses raw
/// binding so no response-marshal step runs after the handled system error.
const DLC_COEXISTENCE_OPERATIONS: &str = r#"      - method: GET
        path: "business"
        steps:
          - throw: { code: "BIZ_DLC", kind: "not-found", message: "missing" }
      - method: GET
        path: "system"
        binding: raw
        produces: "text/plain"
        steps:
          - to: "direct:biz-io-fail"
"#;

/// Setup (3): a doTry catches the business error by CODE and rewrites the
/// body; the exchange continues on the normal path, so the reply is a plain
/// 2xx and the business mapping never runs.
const CATCH_OPERATIONS: &str = r#"      - method: GET
        path: "orders"
        binding: raw
        produces: "text/plain"
        steps:
          - do_try:
              steps:
                - throw: { code: "ORDER_NOT_FOUND", kind: "not-found", message: "missing" }
              catch:
                - code: "ORDER_NOT_FOUND"
                  disposition: handled
                  steps:
                    - set_body: "caught-business-error"
"#;

/// Setup (5): the block declares NO table (empty seam tables) and the kind is
/// outside the built-in map → the 500 closure. The message and data values
/// are distinctive so the body can be proven to carry neither.
const UNMAPPED_OPERATIONS: &str = r#"      - method: GET
        path: "broken"
        steps:
          - throw: { code: "CUSTOM_CODE", kind: "custom-kind", message: "secret-message", data: { secret: "hunter2" } }
"#;

/// A programmatic `direct:` route whose only step fails with `error`. Drives
/// a SYSTEM error inside a rest operation pipeline (setup 2) and the
/// circuit-breaker open phase (setup 6) without any external dependency.
///
/// `error_handler` overrides the inherited global handler per route. Passing
/// `Some(ErrorHandlerConfig::log_only())` makes the route PROPAGATE the error
/// to its caller instead of absorbing it into the global dead-letter channel,
/// so the failure surfaces on the calling (seam) route.
fn failing_direct_route(
    route_id: &'static str,
    from: &'static str,
    error: CamelError,
    error_handler: Option<ErrorHandlerConfig>,
) -> RouteDefinition {
    let builder = RouteBuilder::from(from)
        .route_id(route_id)
        .process_fn(BoxProcessor::from_fn(move |_ex: Exchange| {
            let error = error.clone();
            Box::pin(async move { Err(error) })
        }));
    let builder = match error_handler {
        Some(config) => builder.error_handler(config),
        None => builder,
    };
    builder.build().expect("failing direct route builds")
}

/// Parse every route out of `yaml` and stage it on the harness.
async fn stage_rest_routes(h: &CamelTestContext, yaml: &str) {
    for route in parse_yaml(yaml).expect("rest document must load") {
        h.add_route(route).await.expect("route must stage");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn e2e_rest_mapped_status_via_block_table() {
    install_crypto_provider();
    let port = stage_http_listener("127.0.0.1").await;
    let h = CamelTestContext::builder()
        .with_component(HttpComponent::new())
        .build()
        .await;

    let yaml = rest_yaml_with_operations(
        port,
        "/inheritance",
        Some("      kinds: { \"not-found\": 410 }"),
        TWO_OPERATION_TABLE,
    );
    stage_rest_routes(&h, &yaml).await;
    h.start().await;

    let (status, body) = get_reply(&format!("http://127.0.0.1:{port}/inheritance/missing")).await;
    assert_eq!(
        status, 410,
        "the block table's `not-found → 410` override must beat the built-in \
         404, got {status} {body}"
    );
    assert_eq!(
        body, r#"{"error":"ORDER_NOT_FOUND","kind":"not-found"}"#,
        "the business reply body is exactly error+kind"
    );

    h.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn e2e_rest_global_dlc_and_mapping_coexist() {
    install_crypto_provider();
    let port = stage_http_listener("127.0.0.1").await;
    let h = CamelTestContext::builder()
        .with_component(HttpComponent::new())
        .with_mock()
        .with_direct()
        .build()
        .await;

    // Global dead-letter channel with NO explicit policies: the handler
    // resolves the synthetic Handled catch-all backed by the DLC. The seam
    // bypass removes it for business errors only — system errors still take
    // the dead-letter path.
    h.ctx()
        .lock()
        .await // allow-test-wait: harness ctx mutex is test-owned
        .set_error_handler(ErrorHandlerConfig::dead_letter_channel("mock:dlc"))
        .await;

    // The downstream route carries a LOG-ONLY route-level handler, so the Io
    // failure propagates back to the seam route instead of being absorbed by
    // the inherited global dead-letter channel. The seam route's own handler
    // is therefore the only dispatcher left — which is exactly what the
    // system-error leg must prove.
    h.add_route(failing_direct_route(
        "biz-io-fail",
        "direct:biz-io-fail",
        CamelError::Io("simulated disk failure".into()),
        Some(ErrorHandlerConfig::log_only()),
    ))
    .await
    .expect("system-error route must stage");

    let yaml = rest_yaml_with_operations(
        port,
        "/coexist",
        Some("      kinds: { \"not-found\": 410 }"),
        DLC_COEXISTENCE_OPERATIONS,
    );
    stage_rest_routes(&h, &yaml).await;
    h.start().await;

    let dlc = h.mock().get_endpoint("dlc").expect("mock dlc endpoint");

    // Business error: mapped reply, never 2xx, and NOT dead-lettered.
    let (status, body) = get_reply(&format!("http://127.0.0.1:{port}/coexist/business")).await;
    assert_ne!(
        status / 100,
        2,
        "a business error must never answer 2xx, got {status} {body}"
    );
    assert_eq!(
        status, 410,
        "the inherited block table maps the business error, got {status} {body}"
    );
    dlc.assert_exchange_count(0).await;

    // System error on the SAME rest block: the seam bypass does not cover it,
    // so the seam route's synthetic Handled catch-all forwards it to the DLC.
    // The downstream log-only handler dispatches nothing, so the single dead
    // letter can only come from the SEAM route's handler. The reply is awaited
    // first — dispatch happens before the reply is produced.
    let _ = get_reply(&format!("http://127.0.0.1:{port}/coexist/system")).await;
    dlc.await_exchanges(1, REQUEST_DEADLINE).await;
    dlc.assert_exchange_count(1).await;
    let received = dlc.get_received_exchanges().await;
    match &received[0].error {
        Some(CamelError::Io(message)) => assert!(
            message.contains("simulated disk failure"),
            "the seam-route dead letter carries the Io failure: {message}"
        ),
        other => panic!("the dead letter must carry the propagated Io error, got: {other:?}"),
    }

    h.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn e2e_caught_business_error_is_normal_response() {
    install_crypto_provider();
    let port = stage_http_listener("127.0.0.1").await;
    let h = CamelTestContext::builder()
        .with_component(HttpComponent::new())
        .build()
        .await;

    let yaml = rest_yaml_with_operations(port, "/catch", None, CATCH_OPERATIONS);
    stage_rest_routes(&h, &yaml).await;
    h.start().await;

    let (status, body) = get_reply(&format!("http://127.0.0.1:{port}/catch/orders")).await;
    assert_eq!(
        status / 100,
        2,
        "the doTry catch handled the exchange, so the normal path answers 2xx: \
         {status} {body}"
    );
    assert_eq!(
        body, "caught-business-error",
        "the catch clause's rewritten body is the reply body"
    );
    assert!(
        !body.contains("ORDER_NOT_FOUND"),
        "no business mapping ran once the route handled the error: {body}"
    );

    h.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn e2e_rest_inheritance_two_operations_one_table() {
    install_crypto_provider();
    let port = stage_http_listener("127.0.0.1").await;
    let h = CamelTestContext::builder()
        .with_component(HttpComponent::new())
        .build()
        .await;

    let yaml = rest_yaml_with_operations(
        port,
        "/inherit",
        Some("      kinds: { \"not-found\": 410 }"),
        TWO_OPERATION_TABLE,
    );
    stage_rest_routes(&h, &yaml).await;
    h.start().await;

    // Operation 1: the block table's explicit override.
    let (status_missing, body_missing) =
        get_reply(&format!("http://127.0.0.1:{port}/inherit/missing")).await;
    assert_eq!(
        status_missing, 410,
        "the first operation inherits the block table (not-found → 410), got \
         {status_missing} {body_missing}"
    );

    // Operation 2: not in the block table, so the built-in default kind map
    // applies BENEATH the block table (conflict → 409).
    let (status_conflict, body_conflict) =
        get_reply(&format!("http://127.0.0.1:{port}/inherit/conflict")).await;
    assert_eq!(
        status_conflict, 409,
        "the second operation falls through the block table to the built-in \
         conflict → 409, got {status_conflict} {body_conflict}"
    );
    assert_eq!(
        body_conflict, r#"{"error":"ORDER_CONFLICT","kind":"conflict"}"#,
        "the conflict reply body is exactly error+kind"
    );

    h.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn e2e_unmapped_rest_business_error_is_500() {
    install_crypto_provider();
    let port = stage_http_listener("127.0.0.1").await;
    let h = CamelTestContext::builder()
        .with_component(HttpComponent::new())
        .build()
        .await;

    // No block table: lowering still marks the route as a seam route with
    // empty tables, so the 500 closure applies to the unmapped kind.
    let yaml = rest_yaml_with_operations(port, "/unmapped", None, UNMAPPED_OPERATIONS);
    stage_rest_routes(&h, &yaml).await;
    h.start().await;

    let (status, body) = get_reply(&format!("http://127.0.0.1:{port}/unmapped/broken")).await;
    assert_ne!(
        status / 100,
        2,
        "an unmapped business error must never answer 2xx, got {status} {body}"
    );
    assert_eq!(
        status, 500,
        "a kind outside the built-in map closes at 500, got {status} {body}"
    );
    let reply: serde_json::Value =
        serde_json::from_str(&body).unwrap_or_else(|e| panic!("reply body must be JSON: {e}"));
    assert_eq!(
        reply,
        serde_json::json!({"error": "CUSTOM_CODE", "kind": "custom-kind"}),
        "the body carries exactly error+kind: {body}"
    );
    assert!(
        !body.contains("message") && !body.contains("secret-message"),
        "the throw message never enters the reply body: {body}"
    );
    assert!(
        !body.contains("data") && !body.contains("hunter2"),
        "throw data never enters the reply body: {body}"
    );

    h.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn e2e_ok_with_error_boundary_propagate_never_2xx() {
    install_crypto_provider();
    let port = stage_http_listener("127.0.0.1").await;
    let h = CamelTestContext::builder()
        .with_component(HttpComponent::new())
        .with_mock()
        .with_direct()
        .build()
        .await;

    h.add_route(failing_direct_route(
        "rc7lkp2-downstream",
        "direct:rc7lkp2-downstream",
        CamelError::ProcessorError("downstream failed".into()),
        None,
    ))
    .await
    .expect("downstream route must stage");

    // Handler-configured route: the explicit policy matches the breaker's
    // boundary error and PROPAGATES it (no `handled`), with the DLC as the
    // resolved handler — exactly the shape where `handle_boundary` returns
    // `Ok(exchange)` while the error is still present (rc-7lkp2).
    let route_uri = format!("http://127.0.0.1:{port}/rc7lkp2");
    let route = RouteBuilder::from(&route_uri)
        .route_id("rc7lkp2-boundary")
        .circuit_breaker(
            CircuitBreakerConfig::new()
                .failure_threshold(1)
                .open_duration(Duration::from_secs(60)),
        )
        .to("direct:rc7lkp2-downstream")
        .error_handler(
            ErrorHandlerConfig::dead_letter_channel("mock:dlc")
                .on_exception(|e| matches!(e, CamelError::CircuitOpen(_)))
                .build(),
        )
        .build()
        .expect("boundary-propagate route builds");
    h.add_route(route).await.expect("route must stage");
    h.start().await;

    let url = format!("http://127.0.0.1:{port}/rc7lkp2");

    // Closed phase: the downstream failure reaches the consumer as `Err` and
    // opens the breaker (threshold 1).
    let (first_status, first_body) = get_reply(&url).await;
    assert_eq!(
        first_status, 500,
        "the genuine downstream failure maps through the existing 500 closure \
         and opens the breaker, got {first_status} {first_body}"
    );

    // Open phase: the rejection is a BOUNDARY error. The matched Propagate
    // policy dispatches it to the DLC and `handle_boundary` returns
    // `Ok(exchange)` with the error still carried — the reply arm must map
    // that carried error and never answer 2xx.
    let (status, body) = get_reply(&url).await;
    assert_ne!(
        status / 100,
        2,
        "an Ok exchange still carrying an error must never answer 2xx, got \
         {status} {body}"
    );
    assert_eq!(
        status, 500,
        "the carried CircuitOpen error maps through the existing 500 closure, \
         got {status} {body}"
    );
    assert_eq!(
        body, "Internal Server Error",
        "the carried error uses the existing variant mapping, got {body}"
    );

    // Both failures were dispatched: the closed-phase step error (no policy
    // match, DLC fallback) and the open-phase BOUNDARY propagate — proving
    // the carried-error reply came from `handle_boundary`.
    let dlc = h.mock().get_endpoint("dlc").expect("mock dlc endpoint");
    dlc.await_exchanges(2, REQUEST_DEADLINE).await;
    dlc.assert_exchange_count(2).await;

    h.stop().await;
}
