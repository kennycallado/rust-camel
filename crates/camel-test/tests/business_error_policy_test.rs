//! Business-error policy tests (business-error-model task 1.6).
//!
//! Pins the phase-1 (non-seam) policy surface:
//!
//! - raise-site / handle-site / propagate-site logging with `code` + `kind`
//!   structured and no `data` payload in any record;
//! - declarative redelivery skipped for `Business` errors (clause-level and
//!   route-level retry);
//! - non-seam dead-letter parity (business errors keep DLC dispatch);
//! - wildcard clause match without redelivery.
//!
//! Log-capture tests follow the existing `do_try_test.rs` pattern: a
//! fmt-json subscriber installed as a thread-local scoped default around a
//! current-thread-runtime block_on of the whole route run. The process-wide
//! registry default is installed once at binary start so tracing callsite
//! interest is cached as enabled before any sibling test reaches the
//! callsites without a subscriber (rc-img5 / rc-6jarb).

use std::future::Future;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use camel_api::error_handler::{
    ErrorHandlerConfig, ExceptionDisposition, ExceptionPolicy, HEADER_REDELIVERED, RedeliveryPolicy,
};
use camel_api::{BoxProcessor, BoxProcessorExt, CamelError, Exchange, SyncBoxProcessor};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_api::test_support::acquire_deadline;
use camel_dsl::parse_yaml;
use camel_processor::ErrorHandlerService;
use camel_test::CamelTestContext;
use tower::ServiceExt;

/// Install one process-global registry default so the production tracing
/// callsites cache interest as enabled even when a sibling test executes
/// them without a capture subscriber (guard contract in
/// `camel_component_api::test_support::log_capture`).
#[ctor::ctor(unsafe)]
fn install_global_tracing_registry() {
    let _ = tracing::subscriber::set_global_default(tracing_subscriber::registry());
}

fn test_rt() -> Arc<dyn camel_component_api::RuntimeObservability> {
    Arc::new(camel_component_api::NoOpComponentContext)
}

fn direct_not_registered(err: &CamelError) -> bool {
    matches!(
        err,
        CamelError::EndpointCreationFailed(msg)
            if msg.starts_with("direct endpoint '") && msg.ends_with("' not registered")
    )
}

async fn build_direct_producer(
    h: &CamelTestContext,
    endpoint_uri: &str,
    context_label: &str,
) -> BoxProcessor {
    let ctx = acquire_deadline(h.ctx(), context_label, Duration::from_secs(10)).await;
    let producer_ctx = ctx.producer_context();
    let registry = ctx.registry();
    let component = registry
        .get("direct")
        .expect("direct component not registered");
    let endpoint = component
        .create_endpoint(endpoint_uri, &*ctx)
        .expect("failed to create direct endpoint");
    endpoint
        .create_producer(test_rt(), &producer_ctx)
        .expect("failed to create direct producer")
}

/// Send to a `direct:` endpoint, retrying while the consumer registration
/// task lags `start()` (no post-start sleep; same helper as do_try_test).
async fn send_when_registered(
    h: &CamelTestContext,
    endpoint_uri: &str,
    exchange: Exchange,
) -> Result<Exchange, CamelError> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let attempt = match tokio::time::timeout_at(deadline, async {
            let producer =
                build_direct_producer(h, endpoint_uri, "camel context (send_when_registered)")
                    .await;
            producer.oneshot(exchange.clone()).await
        })
        .await
        {
            Ok(result) => result,
            Err(_elapsed) => {
                return Err(CamelError::ProcessorError(format!(
                    "direct endpoint '{endpoint_uri}': send_when_registered deadline elapsed"
                )));
            }
        };
        match attempt {
            Err(err) if direct_not_registered(&err) && tokio::time::Instant::now() < deadline => {
                tokio::task::yield_now().await;
            }
            other => return other,
        }
    }
}

// ── Log capture ─────────────────────────────────────────────────────────

#[derive(Clone)]
struct JsonLogBuffer(Arc<Mutex<Vec<u8>>>);

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
/// current thread and return its result plus one parsed record per event.
fn capture_debug_records<T>(f: impl FnOnce() -> T) -> (T, Vec<serde_json::Value>) {
    let buffer = JsonLogBuffer(Arc::new(Mutex::new(Vec::new())));
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

    let raw = buffer
        .0
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    let records = raw
        .split(|&b| b == b'\n')
        .filter(|line| !line.iter().all(|b| b.is_ascii_whitespace()))
        .map(|line| {
            serde_json::from_slice(line).unwrap_or_else(|e| {
                panic!(
                    "captured log line is not JSON ({e}): {}",
                    String::from_utf8_lossy(line)
                )
            })
        })
        .collect();
    (output, records)
}

fn block_on<T>(future: impl Future<Output = T>) -> T {
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

fn records_with_code<'a>(
    records: &'a [serde_json::Value],
    code: &str,
) -> Vec<&'a serde_json::Value> {
    records
        .iter()
        .filter(|record| field_str(record, "code") == Some(code))
        .collect()
}

fn assert_no_payload(records: &[serde_json::Value], marker: &str) {
    for record in records {
        let rendered = record.to_string();
        assert!(
            !rendered.contains(marker),
            "captured log record leaked the data payload marker: {record}"
        );
    }
}

/// Count the records carrying `code` at exactly `level`. Business-error
/// policy records are one-per-outcome, so any count above one is a duplicate.
fn code_level_count(records: &[serde_json::Value], code: &str, level: &str) -> usize {
    records_with_code(records, code)
        .iter()
        .filter(|record| level_of(record) == Some(level))
        .count()
}

/// Business errors follow the info/warn logging policy: no ERROR record may
/// exist for the exchange (the generic terminal site is suppressed).
fn assert_no_error_records(records: &[serde_json::Value]) {
    for record in records {
        assert_ne!(
            level_of(record),
            Some("ERROR"),
            "business errors must never produce ERROR records: {record}"
        );
    }
}

/// Error-handler-scoped variant for the propagate scenario: the generic
/// terminal site (`send_to_handler(None)`) must stay silent, but the
/// transport-owned direct-consumer ERROR (`log-policy: outside-contract`,
/// ADR-0012) is a separate, pre-existing signal that task 1.6 step 3 keeps
/// out of the error handler's scope.
fn assert_no_handler_error_records(records: &[serde_json::Value]) {
    for record in records {
        if level_of(record) != Some("ERROR") {
            continue;
        }
        let target = record
            .get("target")
            .and_then(|t| t.as_str())
            .unwrap_or_default();
        assert!(
            !target.starts_with("camel_processor"),
            "business errors must never produce error-handler ERROR records: {record}"
        );
    }
}

fn business_step(counter: Arc<AtomicU32>, code: &'static str) -> BoxProcessor {
    BoxProcessor::from_fn(move |_ex: Exchange| {
        let counter = Arc::clone(&counter);
        Box::pin(async move {
            counter.fetch_add(1, Ordering::SeqCst);
            Err(CamelError::Business {
                code: code.into(),
                kind: "failed".into(),
                message: code.into(),
                data: None,
            })
        })
    })
}

/// Fails once with a retryable non-business error, then with `Business` on
/// every subsequent attempt (retry-transition regression fixture).
fn system_then_business_step(counter: Arc<AtomicU32>, code: &'static str) -> BoxProcessor {
    BoxProcessor::from_fn(move |_ex: Exchange| {
        let counter = Arc::clone(&counter);
        Box::pin(async move {
            let attempt = counter.fetch_add(1, Ordering::SeqCst);
            if attempt == 0 {
                Err(CamelError::ProcessorError("system fault".into()))
            } else {
                Err(CamelError::Business {
                    code: code.into(),
                    kind: "failed".into(),
                    message: code.into(),
                    data: None,
                })
            }
        })
    })
}

// ── Logging: raise site ─────────────────────────────────────────────────

#[test]
fn raise_logs_debug_with_code_kind_no_data() {
    let (outcome, records) = capture_debug_records(|| {
        block_on(async {
            let h = CamelTestContext::builder().with_direct().build().await;
            let yaml = r#"
routes:
  - id: "raise-log"
    from: "direct:raise-log"
    steps:
      - throw:
          code: "RAISE_LOG_X"
          kind: "not-found"
          message: "Order missing"
          data: { secret: "LEAKY_PAYLOAD_MARKER_RAISE" }
"#;
            for route in parse_yaml(yaml).expect("YAML parse failed") {
                h.add_route(route).await.unwrap();
            }
            h.start().await;
            let outcome = send_when_registered(&h, "direct:raise-log", Exchange::default()).await;
            h.stop().await;
            outcome
        })
    });

    assert!(
        matches!(
            outcome,
            Err(CamelError::Business { ref code, .. }) if code == "RAISE_LOG_X"
        ),
        "route must fail with the raised business error, got: {outcome:?}"
    );

    let raise_records = records_with_code(&records, "RAISE_LOG_X");
    assert_eq!(
        raise_records.len(),
        1,
        "exactly one record must carry the raise-site code, got {records:?}"
    );
    let record = raise_records[0];
    assert_eq!(level_of(record), Some("DEBUG"), "raise log must be debug");
    assert_eq!(field_str(record, "kind"), Some("not-found"));
    assert!(
        field_str(record, "data").is_none(),
        "raise log must not carry a data field: {record}"
    );
    assert_no_payload(&records, "LEAKY_PAYLOAD_MARKER_RAISE");
}

// ── Logging: handling sites ─────────────────────────────────────────────

#[test]
fn clause_handled_logs_info_no_data() {
    let (outcome, records) = capture_debug_records(|| {
        block_on(async {
            let h = CamelTestContext::builder().with_direct().build().await;
            let yaml = r#"
routes:
  - id: "clause-log"
    from: "direct:clause-log"
    error_handler:
      on_exceptions:
        - code: "CLAUSE_LOG_X"
          handled: true
    steps:
      - throw:
          code: "CLAUSE_LOG_X"
          kind: "conflict"
          data: { secret: "LEAKY_PAYLOAD_MARKER_CLAUSE" }
"#;
            for route in parse_yaml(yaml).expect("YAML parse failed") {
                h.add_route(route).await.unwrap();
            }
            h.start().await;
            let outcome = send_when_registered(&h, "direct:clause-log", Exchange::default()).await;
            h.stop().await;
            outcome
        })
    });

    assert!(
        outcome.is_ok(),
        "handled clause must complete the route, got: {outcome:?}"
    );

    let clause_records = records_with_code(&records, "CLAUSE_LOG_X");
    let info = clause_records
        .iter()
        .find(|record| level_of(record) == Some("INFO"))
        .unwrap_or_else(|| panic!("no info record for the handled clause among {records:?}"));
    assert_eq!(field_str(info, "kind"), Some("conflict"));
    assert!(
        field_str(info, "data").is_none(),
        "clause handling log must not carry a data field: {info}"
    );
    assert_eq!(
        code_level_count(&records, "CLAUSE_LOG_X", "DEBUG"),
        1,
        "exactly one raise-site debug record, got {records:?}"
    );
    assert_eq!(
        code_level_count(&records, "CLAUSE_LOG_X", "INFO"),
        1,
        "exactly one handled info record (no duplicate), got {records:?}"
    );
    assert_eq!(code_level_count(&records, "CLAUSE_LOG_X", "WARN"), 0);
    assert_eq!(code_level_count(&records, "CLAUSE_LOG_X", "ERROR"), 0);
    assert_no_error_records(&records);
    assert_no_payload(&records, "LEAKY_PAYLOAD_MARKER_CLAUSE");
}

#[test]
fn catch_handled_logs_info_no_data() {
    let (outcome, records) = capture_debug_records(|| {
        block_on(async {
            let h = CamelTestContext::builder().with_direct().build().await;
            let yaml = r#"
routes:
  - id: "catch-log"
    from: "direct:catch-log"
    steps:
      - do_try:
          steps:
            - throw:
                code: "CATCH_LOG_X"
                kind: "conflict"
                data: { secret: "LEAKY_PAYLOAD_MARKER_CATCH" }
          catch:
            - code: "CATCH_LOG_X"
              disposition: handled
              steps:
                - set_body: "caught"
"#;
            for route in parse_yaml(yaml).expect("YAML parse failed") {
                h.add_route(route).await.unwrap();
            }
            h.start().await;
            let outcome = send_when_registered(&h, "direct:catch-log", Exchange::default()).await;
            h.stop().await;
            outcome
        })
    });

    assert!(
        outcome.is_ok(),
        "handled catch must complete the route, got: {outcome:?}"
    );

    let catch_records = records_with_code(&records, "CATCH_LOG_X");
    let info = catch_records
        .iter()
        .find(|record| level_of(record) == Some("INFO"))
        .unwrap_or_else(|| panic!("no info record for the handled catch among {records:?}"));
    assert_eq!(field_str(info, "kind"), Some("conflict"));
    assert!(
        field_str(info, "data").is_none(),
        "catch handling log must not carry a data field: {info}"
    );
    assert_eq!(
        code_level_count(&records, "CATCH_LOG_X", "DEBUG"),
        1,
        "exactly one raise-site debug record, got {records:?}"
    );
    assert_eq!(
        code_level_count(&records, "CATCH_LOG_X", "INFO"),
        1,
        "exactly one catch-handled info record (no duplicate), got {records:?}"
    );
    assert_eq!(code_level_count(&records, "CATCH_LOG_X", "WARN"), 0);
    assert_eq!(code_level_count(&records, "CATCH_LOG_X", "ERROR"), 0);
    assert_no_error_records(&records);
    assert_no_payload(&records, "LEAKY_PAYLOAD_MARKER_CATCH");
}

#[test]
fn send_to_handler_logs_info_no_data() {
    let (outcome, records) = capture_debug_records(|| {
        block_on(async {
            let h = CamelTestContext::builder()
                .with_direct()
                .with_mock()
                .build()
                .await;
            let shaper = RouteBuilder::from("direct:delegate-shaper")
                .route_id("delegate-shaper-log")
                .set_body("shaped")
                .to("mock:delegate-out")
                .build()
                .unwrap();
            h.add_route(shaper).await.unwrap();
            let yaml = r#"
routes:
  - id: "delegate-log"
    from: "direct:delegate-log"
    error_handler:
      on_exceptions:
        - code: "DELEGATE_LOG_X"
          handled: true
          handled_by: "direct:delegate-shaper"
    steps:
      - throw:
          code: "DELEGATE_LOG_X"
          kind: "failed"
          data: { secret: "LEAKY_PAYLOAD_MARKER_DELEGATE" }
"#;
            for route in parse_yaml(yaml).expect("YAML parse failed") {
                h.add_route(route).await.unwrap();
            }
            h.start().await;
            let outcome =
                send_when_registered(&h, "direct:delegate-log", Exchange::default()).await;

            // The delegate producer must actually consume the business
            // exchange: the shaper mutates the body it was handed.
            let sink = h.mock().get_endpoint("delegate-out").unwrap();
            sink.assert_exchange_count(1).await;
            let received = sink.get_received_exchanges().await;
            assert!(
                matches!(
                    received[0].error,
                    Some(CamelError::Business { ref code, .. }) if code == "DELEGATE_LOG_X"
                ),
                "the delegate must receive the business exchange, got: {:?}",
                received[0].error
            );
            assert_eq!(received[0].input.body.as_text(), Some("shaped"));

            h.stop().await;
            outcome
        })
    });

    // The handled clause routes through the delegate: the shaper output is
    // the final result.
    let reply = outcome.expect("handled delegate must complete the route");
    assert_eq!(
        reply.input.body.as_text(),
        Some("shaped"),
        "the shaper output must be final"
    );

    let delegate_records = records_with_code(&records, "DELEGATE_LOG_X");
    let info: Vec<&serde_json::Value> = delegate_records
        .iter()
        .copied()
        .filter(|record| level_of(record) == Some("INFO"))
        .collect();
    assert_eq!(
        info.len(),
        1,
        "exactly one info record (the delegate dispatch site) must carry the code, got {records:?}"
    );
    let record = info[0];
    assert_eq!(field_str(record, "kind"), Some("failed"));
    assert_eq!(
        field_str(record, "message"),
        Some("business error sent to handler"),
        "the info record must come from the delegate dispatch site, not the \
         disposition path (no duplicate): {record}"
    );
    assert!(
        field_str(record, "data").is_none(),
        "delegate dispatch log must not carry a data field: {record}"
    );
    assert_no_payload(&records, "LEAKY_PAYLOAD_MARKER_DELEGATE");
}

#[test]
fn non_seam_route_propagate_logs_warn() {
    let (outcome, records) = capture_debug_records(|| {
        block_on(async {
            let h = CamelTestContext::builder().with_direct().build().await;
            let yaml = r#"
routes:
  - id: "propagate-log"
    from: "direct:propagate-log"
    error_handler: {}
    steps:
      - throw:
          code: "PROPAGATE_LOG_X"
          kind: "failed"
          data: { secret: "LEAKY_PAYLOAD_MARKER_PROPAGATE" }
"#;
            for route in parse_yaml(yaml).expect("YAML parse failed") {
                h.add_route(route).await.unwrap();
            }
            h.start().await;
            let outcome =
                send_when_registered(&h, "direct:propagate-log", Exchange::default()).await;
            h.stop().await;
            outcome
        })
    });

    assert!(
        matches!(
            outcome,
            Err(CamelError::Business { ref code, .. }) if code == "PROPAGATE_LOG_X"
        ),
        "unmatched business error must propagate, got: {outcome:?}"
    );

    let propagate_records = records_with_code(&records, "PROPAGATE_LOG_X");
    let warn = propagate_records
        .iter()
        .find(|record| level_of(record) == Some("WARN"))
        .unwrap_or_else(|| panic!("no warn record for the unmatched propagate among {records:?}"));
    assert_eq!(field_str(warn, "kind"), Some("failed"));
    assert!(
        field_str(warn, "data").is_none(),
        "propagate log must not carry a data field: {warn}"
    );
    assert_eq!(
        code_level_count(&records, "PROPAGATE_LOG_X", "DEBUG"),
        1,
        "exactly one raise-site debug record, got {records:?}"
    );
    assert_eq!(
        code_level_count(&records, "PROPAGATE_LOG_X", "WARN"),
        1,
        "exactly one propagate warn record (no duplicate), got {records:?}"
    );
    // A handled/propagated business error must not also emit the generic
    // terminal ERROR record (`send_to_handler(None)` suppression).
    assert_eq!(code_level_count(&records, "PROPAGATE_LOG_X", "INFO"), 0);
    assert_eq!(code_level_count(&records, "PROPAGATE_LOG_X", "ERROR"), 0);
    assert!(
        !records.iter().any(|record| record
            .to_string()
            .contains("Exchange failed with no error handler configured")),
        "the generic terminal ERROR site must stay silent for business errors: {records:?}"
    );
    assert_no_handler_error_records(&records);
    assert_no_payload(&records, "LEAKY_PAYLOAD_MARKER_PROPAGATE");
}

// ── Non-seam dead-letter parity (review C1) ─────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn seda_unmatched_business_error_reaches_dlc() {
    let h = CamelTestContext::builder()
        .with_direct()
        .with_seda()
        .with_mock()
        .build()
        .await;

    let yaml = r#"
routes:
  - id: "seda-consumer"
    from: "seda:jobs"
    error_handler:
      dead_letter_channel: "mock:dlc"
    steps:
      - throw: { code: "SEDA_DLC_X", kind: "failed" }
  - id: "seda-producer"
    from: "direct:seda-start"
    steps:
      - to: "seda:jobs"
"#;
    for route in parse_yaml(yaml).expect("YAML parse failed") {
        h.add_route(route).await.unwrap();
    }
    h.start().await;

    // InOnly: fire-and-forget, the consumer's failure is owned by its own
    // route error handler (dead-letter dispatch), not the producer.
    send_when_registered(&h, "direct:seda-start", Exchange::default())
        .await
        .expect("InOnly seda send must complete");

    let dlc = h.mock().get_endpoint("dlc").unwrap();
    dlc.await_exchanges(1, Duration::from_secs(5)).await;
    dlc.assert_exchange_count(1).await;
    let received = dlc.get_received_exchanges().await;
    assert!(
        matches!(
            received[0].error,
            Some(CamelError::Business { ref code, .. }) if code == "SEDA_DLC_X"
        ),
        "the dead-letter exchange must carry the business error, got: {:?}",
        received[0].error
    );

    h.stop().await;
}

// ── Redelivery skip ─────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn wildcard_matches_business_no_redelivery() {
    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .build()
        .await;

    let shaper = RouteBuilder::from("direct:wildcard-shaper")
        .route_id("wildcard-shaper")
        .set_body("shaped")
        .to("mock:wild-out")
        .build()
        .unwrap();
    h.add_route(shaper).await.unwrap();

    let yaml = r#"
routes:
  - id: "wildcard"
    from: "direct:wildcard"
    error_handler:
      on_exceptions:
        - kind: "*"
          handled: true
          handled_by: "direct:wildcard-shaper"
    steps:
      - throw: { code: "WILDCARD_X", kind: "failed" }
"#;
    for route in parse_yaml(yaml).expect("YAML parse failed") {
        h.add_route(route).await.unwrap();
    }
    h.start().await;

    let reply = send_when_registered(&h, "direct:wildcard", Exchange::default())
        .await
        .expect("wildcard match + handled_by must complete the route");
    assert_eq!(
        reply.input.body.as_text(),
        Some("shaped"),
        "shaper output must be the final result"
    );
    assert!(
        reply.input.header(HEADER_REDELIVERED).is_none(),
        "no redelivery occurred, so CamelRedelivered must be absent"
    );

    let sink = h.mock().get_endpoint("wild-out").unwrap();
    sink.assert_exchange_count(1).await;

    h.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn clause_retry_never_redelivers_business() {
    let step_runs = Arc::new(AtomicU32::new(0));
    let h = CamelTestContext::builder().with_direct().build().await;

    let route = RouteBuilder::from("direct:clause-retry")
        .route_id("clause-retry")
        .process_fn(business_step(Arc::clone(&step_runs), "CLAUSE_RETRY_X"))
        .error_handler(
            ErrorHandlerConfig::log_only()
                .on_exception(
                    |e| matches!(e, CamelError::Business { code, .. } if code == "CLAUSE_RETRY_X"),
                )
                // 1ms backoff is retry CONFIG, not a test sleep.
                .retry(3)
                .with_backoff(Duration::from_millis(1), 1.0, Duration::from_millis(1))
                .handled(true)
                .build(),
        )
        .build()
        .unwrap();
    h.add_route(route).await.unwrap();
    h.start().await;

    let reply = send_when_registered(&h, "direct:clause-retry", Exchange::default())
        .await
        .expect("handled clause must complete the route");

    assert_eq!(
        step_runs.load(Ordering::SeqCst),
        1,
        "a business error must never be redelivered by clause retry"
    );
    assert!(
        reply.input.header(HEADER_REDELIVERED).is_none(),
        "no redelivery occurred, so CamelRedelivered must be absent"
    );
    assert!(
        reply.error.is_none(),
        "the matching clause disposition (handled) must still run"
    );

    h.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn route_level_retry_suppressed_for_business() {
    let step_runs = Arc::new(AtomicU32::new(0));
    let h = CamelTestContext::builder()
        .with_direct()
        .with_mock()
        .build()
        .await;

    // `error_handler.retry` compiles to a catch-all policy with the retry
    // block; this mirrors that shape through the builder API.
    let route = RouteBuilder::from("direct:route-retry")
        .route_id("route-retry")
        .process_fn(business_step(Arc::clone(&step_runs), "ROUTE_RETRY_X"))
        .error_handler(
            ErrorHandlerConfig::dead_letter_channel("mock:route-retry-dlc")
                .on_exception(|_| true)
                .retry(3)
                .with_backoff(Duration::from_millis(1), 1.0, Duration::from_millis(1))
                .build(),
        )
        .build()
        .unwrap();
    h.add_route(route).await.unwrap();
    h.start().await;

    let outcome = send_when_registered(&h, "direct:route-retry", Exchange::default()).await;
    assert!(
        matches!(
            outcome,
            Err(CamelError::Business { ref code, .. }) if code == "ROUTE_RETRY_X"
        ),
        "route-level retry must not swallow the propagated business error, got: {outcome:?}"
    );

    assert_eq!(
        step_runs.load(Ordering::SeqCst),
        1,
        "a business error must never be redelivered by route-level retry"
    );

    let dlc = h.mock().get_endpoint("route-retry-dlc").unwrap();
    dlc.assert_exchange_count(1).await;
    let received = dlc.get_received_exchanges().await;
    assert!(
        received[0].input.header(HEADER_REDELIVERED).is_none(),
        "no redelivery occurred, so CamelRedelivered must be absent on the DLC exchange"
    );

    h.stop().await;
}

// ── Redelivery skip: retry transition (initial system error) ────────────

/// A clause retry that produces a business error stops the retry LOOP, not
/// just the entry check: the step executes exactly twice (one system failure,
/// one business failure), the disposition still applies, and the exchange the
/// disposition sees carries no `CamelRedelivered` header.
#[tokio::test(flavor = "multi_thread")]
async fn retry_stops_on_business_after_system_failure() {
    let step_runs = Arc::new(AtomicU32::new(0));
    let h = CamelTestContext::builder().with_direct().build().await;

    let route = RouteBuilder::from("direct:clause-transition")
        .route_id("clause-transition")
        .process_fn(system_then_business_step(
            Arc::clone(&step_runs),
            "CLAUSE_TRANSITION_X",
        ))
        .error_handler(
            ErrorHandlerConfig::log_only()
                .on_exception(|_| true)
                // 1ms backoff is retry CONFIG, not a test sleep.
                .retry(3)
                .with_backoff(Duration::from_millis(1), 1.0, Duration::from_millis(1))
                .handled(true)
                .build(),
        )
        .build()
        .unwrap();
    h.add_route(route).await.unwrap();
    h.start().await;

    let reply = send_when_registered(&h, "direct:clause-transition", Exchange::default())
        .await
        .expect("the handled disposition must still apply after the business stop");

    assert_eq!(
        step_runs.load(Ordering::SeqCst),
        2,
        "the loop must stop at the business failure (one system attempt, one business attempt)"
    );
    assert!(
        reply.input.header(HEADER_REDELIVERED).is_none(),
        "the disposition must see the pre-redelivery snapshot, so CamelRedelivered is absent"
    );
    assert!(
        !reply.has_error(),
        "the matching clause disposition (handled) must still run"
    );

    h.stop().await;
}

/// Service-layer (route-level) retry: a retry producing a business error
/// stops the loop immediately and the disposition path (here a DLC dispatch)
/// still runs exactly once, on the pre-redelivery snapshot.
#[tokio::test]
async fn route_retry_stops_on_business_after_system_failure() {
    let step_runs = Arc::new(AtomicU32::new(0));
    let received = Arc::new(Mutex::new(Vec::<Exchange>::new()));
    let sink = Arc::clone(&received);
    let dlc = BoxProcessor::from_fn(move |ex: Exchange| {
        let sink = Arc::clone(&sink);
        Box::pin(async move {
            sink.lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(ex.clone());
            Ok(ex)
        })
    });

    let policy = ExceptionPolicy {
        matches: Arc::new(|_| true),
        retry: Some(RedeliveryPolicy::new(3).with_initial_delay(Duration::from_millis(1))),
        handled_by: None,
        on_steps: None,
        disposition: ExceptionDisposition::Propagate,
    };
    let svc = ErrorHandlerService::new(
        system_then_business_step(Arc::clone(&step_runs), "ROUTE_TRANSITION_X"),
        Some(dlc),
        vec![(policy, None)],
    );

    let outcome = svc.oneshot(Exchange::default()).await;
    assert!(
        outcome.is_ok(),
        "the service absorbs pipeline errors, got: {outcome:?}"
    );
    assert_eq!(
        step_runs.load(Ordering::SeqCst),
        2,
        "the loop must stop at the business failure (one system attempt, one business attempt)"
    );

    let dispatched = received
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    assert_eq!(
        dispatched.len(),
        1,
        "the disposition must dispatch exactly once"
    );
    assert!(
        matches!(
            dispatched[0].error,
            Some(CamelError::Business { ref code, .. }) if code == "ROUTE_TRANSITION_X"
        ),
        "the DLC exchange must carry the business error, got: {:?}",
        dispatched[0].error
    );
    assert!(
        dispatched[0].input.header(HEADER_REDELIVERED).is_none(),
        "the disposition must see the pre-redelivery snapshot, so CamelRedelivered is absent"
    );
}

// ── Logging: service-layer handling site ────────────────────────────────

/// The service layer's on_steps success path is its own business-info site:
/// exactly one INFO record with code/kind, no data, and no ERROR record.
#[test]
fn service_layer_handled_logs_info() {
    let (outcome, records) = capture_debug_records(|| {
        block_on(async {
            let on_steps = BoxProcessor::from_fn(|ex: Exchange| Box::pin(async move { Ok(ex) }));
            let policy = ExceptionPolicy {
                matches: Arc::new(|_| true),
                retry: None,
                handled_by: None,
                on_steps: Some(SyncBoxProcessor::new(on_steps)),
                disposition: ExceptionDisposition::Handled,
            };
            let failing = BoxProcessor::from_fn(|_ex: Exchange| {
                Box::pin(async {
                    Err(CamelError::Business {
                        code: "SERVICE_LAYER_X".into(),
                        kind: "conflict".into(),
                        message: "service layer".into(),
                        data: None,
                    })
                })
            });
            let svc = ErrorHandlerService::new(failing, None, vec![(policy, None)]);
            svc.oneshot(Exchange::default()).await
        })
    });

    let exchange = outcome.expect("service-layer on_steps handling must absorb the error");
    assert!(
        !exchange.has_error(),
        "Handled disposition must clear the error"
    );

    assert_eq!(
        records.len(),
        1,
        "exactly one record (no duplicate) must be emitted: {records:?}"
    );
    let record = &records[0];
    assert_eq!(level_of(record), Some("INFO"), "layer log must be info");
    assert_eq!(field_str(record, "code"), Some("SERVICE_LAYER_X"));
    assert_eq!(field_str(record, "kind"), Some("conflict"));
    assert!(
        field_str(record, "data").is_none(),
        "layer handling log must not carry a data field: {record}"
    );
    assert_no_error_records(&records);
}
