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

use camel_api::CamelError;
use camel_api::Value;
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_timer::TimerComponent;
use camel_core::context::CamelContext;
use camel_processor::LogLevel;

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    // Initialize the tracing subscriber first so every subsequent log line
    // (including the route's marker emission) lands on stdout.
    tracing_subscriber::fmt().with_target(false).init();

    // 1. Construct the CamelContext via the public builder.
    let mut ctx = CamelContext::builder().build().await?;

    // 2. Register the only component the route touches. The T2 route's
    //    `.log(...)` step (Pair B equivalent) and the marker-emitting
    //    `process` step (Pair A, this fixture) both resolve to internal
    //    processors directly — no `log:` endpoint component needed,
    //    mirroring v1's dead-weight-removal fix.
    ctx.register_component(TimerComponent::new());

    // 3. Build the T2 route programmatically (Pair A — no YAML/DSL
    //    parsing). See file-level comment for the Pair A predicate
    //    deviation rationale (closure predicates, not Simple).
    let route = RouteBuilder::from("timer:bench?repeatCount=1&delay=0")
        .route_id("bench-route")
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
        // the `process` step after `end_choice()` is unconditional.
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
        // Static log first (matches v1's `BENCH_ROUTE_READY` baseline so
        // the harness `BENCH_ROUTE_READY` substring grep still matches),
        // followed by a dynamic `process` step that emits the
        // body-suffixed marker. The two lines are paired: the static
        // line is identical across T1/T2/Pair-A/Pair-B; the dynamic
        // line is what proves T2 semantic correctness (body=pong-bench).
        .log("BENCH_ROUTE_READY", LogLevel::Info)
        .process(|ex| async move {
            let body = ex.input.body.as_text().unwrap_or("").to_string();
            tracing::info!("BENCH_ROUTE_READY body={}", body);
            Ok(ex)
        })
        .build()?;

    // 4. Register the route and start the context.
    ctx.add_route_definition(route).await?;
    ctx.start().await?;

    // 5. Keep the CamelContext alive until killed by the benchmark harness.
    //    Mirrors v1 rust-camel-lib behavior — the marker has already
    //    fired (repeatCount=1) but the context stays up until SIGKILL.
    tokio::signal::ctrl_c().await.ok();
    ctx.stop().await?;
    Ok(())
}
