//! Unit tests for the shared tracing log-capture harness.
//!
//! The `capture_logs` / `capture_logs_async` and `capture_warns` families
//! exist at this point; the `capture_debugs_with_span_records` and `traced`
//! families arrive with the module's full assembly.

#[test]
fn capture_logs_captures_warn_with_fields() {
    let captured = super::capture_logs(|| {
        tracing::warn!(bind = "0.0.0.0:1", public_routes = 1_u64, "gate warned");
    });
    assert!(
        captured.contains("0.0.0.0:1"),
        "field value must appear in the capture: {captured}"
    );
    assert!(
        captured.contains("public_routes=1"),
        "numeric field must appear as key=value: {captured}"
    );
    assert!(
        captured.to_lowercase().contains("warn"),
        "level must be rendered: {captured}"
    );
    assert!(
        captured.contains("gate warned"),
        "message must appear in the capture: {captured}"
    );
}

#[test]
fn capture_logs_empty_when_closure_is_silent() {
    let captured = super::capture_logs(|| ());
    assert!(
        captured.is_empty(),
        "silent closure must produce no output: {captured}"
    );
}

#[tokio::test]
async fn capture_logs_async_passes_output_and_captures() {
    let (out, captured) = super::capture_logs_async(async {
        tracing::warn!("async side fired");
        7_u8
    })
    .await;
    assert_eq!(out, 7);
    assert!(
        captured.contains("async side fired"),
        "event inside the future must be captured: {captured}"
    );
}

#[tokio::test]
async fn capture_logs_async_silent_future_yields_empty_capture() {
    let (out, captured) = super::capture_logs_async(async { 1_u8 }).await;
    assert!(
        captured.is_empty(),
        "silent future must produce no output: {captured}"
    );
    assert_eq!(out, 1);
}

#[test]
fn capture_warns_returns_output_and_captures_field_pairs() {
    let (out, warns) = super::capture_warns(|| {
        tracing::warn!(path = "/srv", "disk quota low");
        3_i32
    });
    assert_eq!(out, 3);
    assert!(
        !warns.is_empty(),
        "the event must be captured: {} entries — {warns:?}",
        warns.len()
    );
    let first = &warns[0];
    // The verbatim visitor renders the message field through `format_args!`
    // (no quotes) and `&str` fields through `Debug` (quoted). Message first,
    // then fields: don't "fix" either — the captured text is contract.
    assert!(
        first.contains("message=disk quota low"),
        "message must render as message=text: {first}"
    );
    assert!(
        first.contains("path=\"/srv\""),
        "field must render as field=\"value\": {first}"
    );
}

#[test]
fn capture_warns_two_events_preserve_emission_order() {
    let (_, warns) = super::capture_warns(|| {
        tracing::warn!("first");
        tracing::warn!("second");
    });
    assert!(
        warns.len() >= 2,
        "both events must be captured: {} entries — {warns:?}",
        warns.len()
    );
    let first_pos = warns
        .iter()
        .position(|entry| entry.contains("first"))
        .expect("first event must be captured");
    let second_pos = warns
        .iter()
        .position(|entry| entry.contains("second"))
        .expect("second event must be captured");
    assert!(
        first_pos < second_pos,
        "emission order must be preserved: first at {first_pos}, second at {second_pos} — {warns:?}"
    );
}

#[test]
fn capture_debugs_captures_debug_and_more_severe_events() {
    let ((), events, span_records) = super::capture_debugs_with_span_records(|| {
        tracing::debug!("probe debug");
        tracing::warn!("probe warn");
        let span = tracing::info_span!("probe_span", attempt = tracing::field::Empty);
        let _e = span.enter();
        tracing::Span::current().record("attempt", 2_i64);
    });
    assert!(
        events.iter().any(|entry| entry.contains("probe debug")),
        "debug event must be captured: {events:?}"
    );
    assert!(
        events.iter().any(|entry| entry.contains("probe warn")),
        "warn event must be captured: {events:?}"
    );
    assert!(
        span_records.iter().any(|entry| entry.contains("attempt=2")),
        "span record must be captured: {span_records:?}"
    );
}

#[test]
fn captured_field_ends_at_next_field_boundary() {
    assert_eq!(
        super::captured_field("k=\"1\" next=\"2 3\"", "k"),
        Some("\"1\"")
    );
    assert_eq!(
        super::captured_field("k=\"1\" next=\"2 3\"", "next"),
        Some("\"2 3\"")
    );
    assert_eq!(super::captured_field("k=\"1\"", "missing"), None);
}

#[test]
fn record_field_finds_message_part_then_field() {
    let lines = vec!["attempt=2 message=\"send failed\"".to_string()];
    assert_eq!(
        super::record_field(&lines, "send failed", "attempt"),
        Some("2")
    );
    assert_eq!(super::record_field(&lines, "absent text", "attempt"), None);
}
