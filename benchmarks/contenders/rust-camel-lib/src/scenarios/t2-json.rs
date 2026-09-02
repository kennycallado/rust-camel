//! T2-JSON scenario fixture — rust-camel-lib (Pair A partner, OpenSpec
//! change `bench-missing-cells` task 2.1).
//!
//! Route: `timer -> set_body -> unmarshal("json") -> filter -> transform
//! -> marshal("json") -> process assert`. The input is the canonical JSON
//! document built by `bench-loadgen`'s [`canonical_json_body`]
//! (task 1.2): `{"id":"bench","seq":<tick>,"fill":"<K×'b'>"}`, exactly
//! `BENCH_PAYLOAD_BYTES` bytes, validated against the payload axis
//! ([`VALID_PAYLOAD_SIZES`]). Before the context starts the fixture logs
//! `BENCH_INPUT_SHA256=<digest>` where the digest is
//! `canonical_body_sha256(size, CANONICAL_SELFTEST_TICK)` — a pure
//! function of (size, tick), identical across every contender in the
//! scenario (see the README golden table).
//!
//! # What unmarshal("json") actually produces (recorded per task 2.1)
//!
//! `camel_processor`'s JSON data format converts `Body::Text` into
//! `Body::Json(serde_json::Value)` — after unmarshal the body IS the
//! parsed structured value, not text plus a typed header. Two facts
//! drive the transform mechanism chosen here:
//!
//! 1. The rhai language binds `body` as TEXT ONLY
//!    (`camel_api::body::Body::as_text`). A `Body::Json` body reads as
//!    the empty string in rhai, and a rhai mutating script can only
//!    write `Body::Text` back — a script that re-serialized the
//!    document internally would break the single-serialization
//!    invariant (marshal must be the ONLY serializer).
//! 2. rust-camel-lib's idiomatic surface for structured body work is a
//!    Rust closure — the same "Pair A predicate deviation" the
//!    `t2-realistic-eip` fixture documents. The transform here is a
//!    closure over `Body`: it inserts the `"bench": true` member into
//!    the parsed MAP and returns the MAP (`Body::Json`), never a
//!    string.
//!
//! `marshal("json")` performs the single serialization
//! (`serde_json::to_string`); the +13 delta is exactly the inserted
//! `,"bench":true` member.
//!
//! # Marker contract
//!
//! The final `process` step asserts BOTH output invariants before the
//! marker: exact length `size + 13` AND parsed semantic equality
//! (`id == "bench"`, `seq` present, `fill` all 'b', `bench == true`),
//! parsing the marshaled text with serde_json inside the assert. An
//! assert failure returns an error — the route fails before the marker
//! is printed, so the cell fails. The harness's exact grep target is
//! one stdout line: `BENCH_ROUTE_READY bytes=<len>`.
//!
//! # Tick mode (OpenSpec change `bench-consol-tick` task 2.2)
//!
//! The timer is the repeating warm-tick form
//! `timer:bench?period=10&repeatCount=10000` (verbatim, matching the
//! `xsd-validation-bridge` reference): the SAME body-building/parity
//! pipeline runs per exchange. The marker keeps its exact code-path
//! position (the assert step) but is gated to the FIRST completed
//! exchange — exactly one marker line per process lifetime. A
//! `BENCH_START` extension brackets each exchange; the trailing step
//! appends `BENCH_LATENCY <id> <duration_ns>` to `$BENCH_LATENCY_FILE`
//! per exchange (env read ONCE at branch start; when unset the
//! canonical harness path is used, exactly like the reference — the
//! lib cell argv is bare, so the default is what makes the M2
//! protocol B reader find the log). Pattern mirrors
//! `xsd-validation-bridge.rs:58,111`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

use bench_loadgen::payload::{
    CANONICAL_SELFTEST_TICK, VALID_PAYLOAD_SIZES, canonical_body_sha256, canonical_json_body,
    validate_payload_size,
};
use camel_api::body::Body;
use camel_api::{CamelError, Value};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_timer::TimerComponent;
use camel_core::context::CamelContext;
use tokio::fs::OpenOptions;
use tokio::io::AsyncWriteExt;

/// Default payload size (bytes) — same default as the harness.
const DEFAULT_PAYLOAD_BYTES: usize = 32768;

/// Exact byte delta added by the transform: the `,"bench":true` member.
const BENCH_MEMBER_DELTA: usize = 13;

/// Exchange extension key under which the pre-pipeline `Instant` is
/// stored. Extension (not property) because `Instant` is not
/// serializable (same mechanism as `xsd-validation-bridge.rs:33`).
const BENCH_START: &str = "BenchStart";

/// Resolve `BENCH_PAYLOAD_BYTES` (default 32768), validated against the
/// payload axis. Invalid values abort before any marker is printed.
fn bench_payload_bytes() -> usize {
    let Ok(raw) = std::env::var("BENCH_PAYLOAD_BYTES") else {
        return DEFAULT_PAYLOAD_BYTES;
    };
    let parsed = raw.trim().parse::<usize>().unwrap_or_else(|_| {
        panic!("BENCH_PAYLOAD_BYTES='{raw}' is not a usize; valid sizes: {VALID_PAYLOAD_SIZES:?}")
    });
    validate_payload_size(parsed).unwrap_or_else(|e| panic!("BENCH_PAYLOAD_BYTES {e}"))
}

/// Transform step: operate on the PARSED structured body — insert the
/// `"bench": true` member and return the MAP (never a serialized
/// string). `marshal("json")` downstream is the single serializer.
fn insert_bench_member(body: Body) -> Body {
    match body {
        Body::Json(mut v) => {
            if let Some(obj) = v.as_object_mut() {
                obj.insert("bench".to_string(), Value::Bool(true));
            }
            Body::Json(v)
        }
        other => other,
    }
}

/// Output assert — both invariants: exact `size + 13` length AND parsed
/// semantic equality (`id == "bench"`, `seq` present, `fill` all 'b',
/// `bench == true`). Returns the expected output length on success; the
/// `Err` string becomes a route error (no marker) in the process step.
fn assert_bench_output(size: usize, text: &str) -> Result<usize, String> {
    let expected = size + BENCH_MEMBER_DELTA;
    if text.len() != expected {
        return Err(format!(
            "t2-json output length {} != expected {expected}",
            text.len()
        ));
    }
    let v: Value =
        serde_json::from_str(text).map_err(|e| format!("t2-json output is not valid JSON: {e}"))?;
    let obj = v.as_object().ok_or("t2-json output is not a JSON object")?;
    if obj.get("id").and_then(Value::as_str) != Some("bench") {
        return Err("t2-json output id != \"bench\"".to_string());
    }
    if !obj.contains_key("seq") {
        return Err("t2-json output seq member missing".to_string());
    }
    let fill = obj
        .get("fill")
        .and_then(Value::as_str)
        .ok_or("t2-json output fill member missing or non-string")?;
    if !fill.bytes().all(|b| b == b'b') {
        return Err("t2-json output fill is not all 'b'".to_string());
    }
    if obj.get("bench") != Some(&Value::Bool(true)) {
        return Err("t2-json output bench != true".to_string());
    }
    Ok(expected)
}

/// Verbatim port of the per-scenario fixture's `main()`.
pub fn run() -> i32 {
    match main_async() {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("t2-json: {e:?}");
            1
        }
    }
}

#[tokio::main]
async fn main_async() -> Result<(), CamelError> {
    // Initialize the tracing subscriber first so the SHA line and the
    // route's marker emission land on stdout.
    tracing_subscriber::fmt().with_target(false).init();

    let size = bench_payload_bytes();
    let body = canonical_json_body(size, CANONICAL_SELFTEST_TICK);

    // Input provenance: a pure function of (size, tick), identical
    // across every t2-json contender (README golden table).
    tracing::info!(
        "BENCH_INPUT_SHA256={}",
        canonical_body_sha256(size, CANONICAL_SELFTEST_TICK)
    );

    // Tick-mode latency sink — read ONCE at branch start, mirroring
    // the reference exactly (xsd-validation-bridge.rs:58-59): when the
    // env is unset, fall back to the CANONICAL harness path so the M2
    // protocol B reader finds the log without scenario-specific wiring
    // (the lib cell argv is bare — run.sh resolve_all_cells does not
    // pass BENCH_LATENCY_FILE for the T2 scenarios).
    let latency_file = std::env::var("BENCH_LATENCY_FILE")
        .unwrap_or_else(|_| "/tmp/v3-protocol-b-t2-json_rust-camel-lib.log".to_string());
    if let Some(parent) = std::path::Path::new(&latency_file).parent() {
        tokio::fs::create_dir_all(parent).await.ok();
    }
    _ = tokio::fs::File::create(&latency_file).await;

    let mut ctx = CamelContext::builder().build().await?;
    ctx.register_component(TimerComponent::new());

    let tick_counter = Arc::new(AtomicU64::new(0));
    let marker_fired = Arc::new(AtomicBool::new(false));
    let latency_file_arc = Arc::new(latency_file);

    let rc_for_route = Arc::clone(&tick_counter);
    let mf_for_route = Arc::clone(&marker_fired);
    let lf_for_route = Arc::clone(&latency_file_arc);

    let route = RouteBuilder::from("timer:bench?period=10&repeatCount=10000")
        .route_id("bench-route")
        .set_body(body)
        // Bracket the per-exchange pipeline (unmarshal → filter →
        // transform → marshal → assert) with Instant t_start.
        // Extension-stored because Instant is not serializable; read
        // back in the trailing latency step.
        .process(|mut exchange| async move {
            exchange.set_extension(BENCH_START, Arc::new(Instant::now()));
            Ok(exchange)
        })
        .unmarshal("json")?
        // Pair A idiomatic closure predicate: the structured body's id
        // member equals "bench" (the language-subsystem equivalent of
        // the task's `$.id == 'bench'` intent — see module doc).
        .filter(|ex| {
            matches!(
                &ex.input.body,
                Body::Json(v) if v.get("id").and_then(Value::as_str) == Some("bench")
            )
        })
        .map_body(insert_bench_member)
        .end_filter()
        .marshal("json")?
        .process(move |ex| {
            let size = size;
            let mf = Arc::clone(&mf_for_route);
            async move {
                let text = ex.input.body.as_text().unwrap_or("").to_string();
                let len = assert_bench_output(size, &text).map_err(CamelError::ProcessorError)?;
                // Marker fires on the FIRST completed exchange only —
                // tick mode repeats this step per tick, the marker
                // contract is exactly one line.
                if !mf.swap(true, Ordering::Relaxed) {
                    tracing::info!("BENCH_ROUTE_READY bytes={len}");
                }
                Ok(ex)
            }
        })
        .process(move |exchange| {
            let rc = Arc::clone(&rc_for_route);
            let lf = Arc::clone(&lf_for_route);
            async move {
                let id = rc.fetch_add(1, Ordering::Relaxed) + 1;
                let duration_ns = exchange
                    .get_extension::<Instant>(BENCH_START)
                    .map(|instant| instant.elapsed().as_nanos() as u64)
                    .unwrap_or(0);
                let line = format!("BENCH_LATENCY {id} {duration_ns}\n");
                if let Ok(mut f) = OpenOptions::new().append(true).open(lf.as_str()).await {
                    let _ = f.write_all(line.as_bytes()).await;
                }
                Ok(exchange)
            }
        })
        .build()?;

    ctx.add_route_definition(route).await?;
    ctx.start().await?;

    // Keep the context alive until killed by the smoke/harness (tick
    // mode: the route keeps ticking; the marker fired on the first
    // completed exchange).
    tokio::signal::ctrl_c().await.ok();
    ctx.stop().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lowercase hex SHA-256 of `data`. Mirrors `canonical_body_sha256`
    /// but accepts arbitrary (possibly corrupted) bytes — the corruption
    /// test relies on this.
    fn sha256_hex(data: &[u8]) -> String {
        use sha2::Digest;
        let mut hasher = sha2::Sha256::new();
        hasher.update(data);
        hex::encode(hasher.finalize())
    }

    /// Digest-check helper: true when `body` hashes to the canonical
    /// `(size, CANONICAL_SELFTEST_TICK)` golden digest.
    fn digest_matches(size: usize, body: &str) -> bool {
        sha256_hex(body.as_bytes()) == canonical_body_sha256(size, CANONICAL_SELFTEST_TICK)
    }

    /// (32768, 0) golden from bench-loadgen's table — the smoke's
    /// BENCH_PAYLOAD_BYTES=32768 class (bytes=32781).
    #[test]
    fn golden_digest_32768_tick0() {
        assert_eq!(
            canonical_body_sha256(32768, CANONICAL_SELFTEST_TICK),
            "a0db69e1146a29b0b25ca22435e51f39e271ecb1ac4ec1cee0ead3212eae10e9"
        );
        assert!(digest_matches(
            32768,
            &canonical_json_body(32768, CANONICAL_SELFTEST_TICK)
        ));
    }

    /// (1024, 0) golden — the per-class smoke variant (bytes=1037).
    #[test]
    fn golden_digest_1024_tick0() {
        assert_eq!(
            canonical_body_sha256(1024, CANONICAL_SELFTEST_TICK),
            "5abe5f00068356cad4e72f4d5e5e0a5d15d4a5cc9df8d0f22e22bf1448891b0f"
        );
    }

    /// Corrupting ONE byte flips the digest-check helper to false.
    #[test]
    fn digest_check_detects_single_byte_corruption() {
        let size = 1024;
        let canonical = canonical_json_body(size, CANONICAL_SELFTEST_TICK);
        assert!(digest_matches(size, &canonical));
        // Flip the first fill byte 'b' -> 'c' (structurally valid JSON,
        // semantically different document).
        let mut corrupted = canonical.clone();
        let fill_start = "{\"id\":\"bench\",\"seq\":".len() + 1 + ",\"fill\":\"".len();
        corrupted.replace_range(fill_start..fill_start + 1, "c");
        assert_ne!(corrupted, canonical);
        assert!(!digest_matches(size, &corrupted));
    }

    /// Full output contract at 1024: parse (unmarshal equivalent),
    /// insert the bench member (transform equivalent), serialize ONCE
    /// (marshal equivalent — `serde_json::to_string`, exactly what the
    /// json data format does). Length is exactly 1037 and the semantic
    /// assert passes.
    #[test]
    fn transform_marshal_contract_1024() {
        let size = 1024;
        let input = canonical_json_body(size, CANONICAL_SELFTEST_TICK);
        let v: Value = serde_json::from_str(&input)
            .unwrap_or_else(|e| panic!("fixture input must be valid JSON: {e}"));
        let out = insert_bench_member(Body::Json(v));
        let Body::Json(parsed) = out else {
            panic!("insert_bench_member must keep the body structured");
        };
        let text = serde_json::to_string(&parsed)
            .unwrap_or_else(|e| panic!("marshal equivalent must serialize: {e}"));
        let len = assert_bench_output(size, &text).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(len, 1037);
        assert!(text.contains("\"bench\":true"));
    }

    /// A semantically wrong output (bench absent) must fail the assert —
    /// the route would die before the marker.
    #[test]
    fn assert_rejects_missing_bench_member() {
        let size = 1024;
        let input = canonical_json_body(size, CANONICAL_SELFTEST_TICK);
        let err = assert_bench_output(size, &input).unwrap_err();
        assert!(err.contains("length"), "length fires first: {err}");
    }
}
