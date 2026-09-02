//! T2 scenario fixture - rust-camel-lib (Pair A partner, bd rc-p9ki Task 3).
//!
//! Mirrors the v1 rust-camel-lib (`benchmarks/scenarios/startup-minimal/
//! rust-camel-lib/src/main.rs`) but implements the spec §4.1 T2 route:
//! timer -> set_body -> set_header -> filter -> choice.when/otherwise ->
//! marker. The marker `BENCH_ROUTE_READY body=pong-bench` is the harness's
//! exact grep target — the `body=pong-bench` suffix proves the choice/when
//! branch executed (vs `pong-other` if otherwise was wrongly taken).
//!
//! # Pair A predicate deviation (intentional, per spec §4.1)
//!
//! Apache Camel Pair A uses Simple-language predicates
//! (`simple("${body} == 'ping'")`, `simple("${header.source} == 'bench'")`).
//! rust-camel-lib Pair A uses **closure predicates**:
//! - `|ex| ex.input.body.as_text() == Some("ping")`
//! - `|ex| matches!(ex.input.header("source"), Some(Value::String(s)) if s == "bench")`
//!
//! Closure predicates are the idiomatic public `RouteBuilder::filter` /
//! `.when` API in rust-camel (see `crates/camel-builder/src/lib.rs:412`,
//! `:1243`). They are NOT language-subsystem-equivalent to Simple: the
//! closure evaluates Rust code directly, while Simple parses a
//! `${...}` expression string via the `camel-language-simple` crate.
//!
//! T2 Pair A therefore measures "overall EIP pipeline overhead at each
//! framework's idiomatic surface", NOT language-subsystem equivalence.
//! Pair B (YAML) IS language-subsystem-equivalent (both sides use
//! `${body}` / `${header.X}` Simple — see `rust-camel-cli/routes/
//! t2-realistic-eip.yaml` and the Apache Camel YAML fixtures).
//!
//! # Marker emission
//!
//! rust-camel's `log(message, level)` step is **static-only** — the
//! `message` is a baked `String` compiled into `LogProcessor` (see
//! `crates/camel-processor/src/log.rs:17-22` and
//! `crates/camel-core/src/lifecycle/adapters/step_compilers/core.rs:123`).
//! For Pair A's closure-based body, dynamic body interpolation is
//! achieved via a `process` step that formats the marker from the
//! current body and emits it via `tracing::info!` — same observable
//! stdout as Apache Camel's `.log("BENCH_ROUTE_READY body=${body}")`
//! via Simple. (Pair B uses the declarative log path with a Simple
//! `${body}` expression — see `rust-camel-cli` + the YAML fixtures.)
//!
//! # Tick mode (OpenSpec change `bench-consol-tick` task 2.2)
//!
//! The timer is the repeating warm-tick form
//! `timer:bench?period=10&repeatCount=10000` (verbatim, matching the
//! `xsd-validation-bridge` reference): the SAME EIP pipeline runs per
//! exchange. The static `.log` marker step is GONE — `LogProcessor`
//! is static-only and would repeat the bare line on every one of the
//! 10000 ticks. Both marker lines (bare `BENCH_ROUTE_READY`, then
//! `BENCH_ROUTE_READY body=<body>`, same order as the old pair) are
//! now emitted by one `process` step gated to the FIRST completed
//! exchange — exactly one marker pair per process lifetime. A
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

use camel_api::CamelError;
use camel_api::Value;
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_timer::TimerComponent;
use camel_core::context::CamelContext;
use tokio::fs::OpenOptions;
use tokio::io::AsyncWriteExt;

/// Exchange extension key under which the pre-pipeline `Instant` is
/// stored. Extension (not property) because `Instant` is not
/// serializable (same mechanism as `xsd-validation-bridge.rs:33`).
const BENCH_START: &str = "BenchStart";

/// Verbatim port of the per-scenario fixture's `main()`.
pub fn run() -> i32 {
    match main_async() {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("t2-realistic-eip: {e:?}");
            1
        }
    }
}

#[tokio::main]
async fn main_async() -> Result<(), CamelError> {
    // Initialize the tracing subscriber first so every subsequent log line
    // (including the route's marker emission) lands on stdout.
    tracing_subscriber::fmt().with_target(false).init();

    // 1. Construct the CamelContext via the public builder.
    let mut ctx = CamelContext::builder().build().await?;

    // 2. Register the only component the route touches. The T2 route's
    //    marker-emitting `process` step (Pair A, this fixture) resolves
    //    to an internal processor directly — no `log:` endpoint
    //    component needed, mirroring v1's dead-weight-removal fix.
    ctx.register_component(TimerComponent::new());

    // Tick-mode latency sink — read ONCE at branch start, mirroring
    // the reference exactly (xsd-validation-bridge.rs:58-59): when the
    // env is unset, fall back to the CANONICAL harness path so the M2
    // protocol B reader finds the log without scenario-specific wiring
    // (the lib cell argv is bare — run.sh resolve_all_cells does not
    // pass BENCH_LATENCY_FILE for the T2 scenarios).
    let latency_file = std::env::var("BENCH_LATENCY_FILE")
        .unwrap_or_else(|_| "/tmp/v3-protocol-b-t2-realistic-eip_rust-camel-lib.log".to_string());
    if let Some(parent) = std::path::Path::new(&latency_file).parent() {
        tokio::fs::create_dir_all(parent).await.ok();
    }
    _ = tokio::fs::File::create(&latency_file).await;

    let tick_counter = Arc::new(AtomicU64::new(0));
    let marker_fired = Arc::new(AtomicBool::new(false));
    let latency_file_arc = Arc::new(latency_file);

    let rc_for_route = Arc::clone(&tick_counter);
    let mf_for_route = Arc::clone(&marker_fired);
    let lf_for_route = Arc::clone(&latency_file_arc);

    // 3. Build the T2 route programmatically (Pair A — no YAML/DSL
    //    parsing). See file-level comment for the Pair A predicate
    //    deviation rationale (closure predicates, not Simple).
    let route = RouteBuilder::from("timer:bench?period=10&repeatCount=10000")
        .route_id("bench-route")
        // Bracket the per-exchange EIP pipeline (filter →
        // choice/when → marker) with Instant t_start.
        // Extension-stored because Instant is not serializable; read
        // back in the trailing latency step.
        .process(|mut exchange| async move {
            exchange.set_extension(BENCH_START, Arc::new(Instant::now()));
            Ok(exchange)
        })
        .set_body("ping")
        .set_header("source", "bench")
        // Filter predicate: closure form (NOT Simple). Body has just been
        // set to the literal `"ping"` above, so this is always true and
        // the choice/when branch below always runs.
        .filter(|ex| ex.input.body.as_text() == Some("ping"))
        // The filter is always-true under the T2 route, but the type
        // system requires `.end_filter()` to return to `RouteBuilder`
        // (so the next `.choice()` call is available — `choice` is on
        // RouteBuilder, not FilterBuilder). When the predicate would
        // be false in a real route, the choice/when block is skipped
        // entirely; the harness's marker grep still matches because
        // the gated `process` step after `end_choice()` still fired on
        // the first completed exchange.
        .end_filter()
        .choice()
        .when(|ex| {
            matches!(
                ex.input.header("source"),
                Some(Value::String(s)) if s == "bench"
            )
        })
        .set_body("pong-bench")
        .end_when()
        .otherwise()
        .set_body("pong-other")
        .end_otherwise()
        .end_choice()
        // Marker pair, gated to the FIRST completed exchange: the
        // static line first (same order as the old `.log` + `process`
        // pair — the static line is identical across T1/T2/Pair-A/
        // Pair-B), followed by the dynamic body-suffixed line that
        // proves T2 semantic correctness (body=pong-bench). Tick mode
        // repeats this step per tick; the marker contract is exactly
        // one pair.
        .process(move |ex| {
            let mf = Arc::clone(&mf_for_route);
            async move {
                if !mf.swap(true, Ordering::Relaxed) {
                    tracing::info!("BENCH_ROUTE_READY");
                    let body = ex.input.body.as_text().unwrap_or("").to_string();
                    tracing::info!("BENCH_ROUTE_READY body={}", body);
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

    // 4. Register the route and start the context.
    ctx.add_route_definition(route).await?;
    ctx.start().await?;

    // 5. Keep the CamelContext alive until killed by the benchmark
    //    harness. Mirrors v1 rust-camel-lib behavior — the marker pair
    //    fired on the first completed exchange, and tick mode keeps the
    //    context up (ticking) until SIGKILL.
    tokio::signal::ctrl_c().await.ok();
    ctx.stop().await?;
    Ok(())
}
