//! Adversarial pipeline tests for the rc-ucemm job-send contract,
//! exercised END-TO-END through `send_with_startup_retry` on a REAL
//! booted pipeline (`direct:jobs -> mock:counted -> seda:worker`, no
//! route consuming the seda endpoint).
//!
//! The contract under test: the SEDA no-active-consumers gate (single
//! mode and fanout mode) must be NON-retryable so the send fails fast on
//! the first attempt — a retry would replay already-executed route steps
//! and duplicate the pre-SEDA side effect, so the mock endpoint must
//! record the exchange EXACTLY once. The discard-if-no-consumers path
//! never produces a gate error and must complete with the same
//! exactly-once side effect.
//!
//! Plain `#[tokio::test]` ONLY: `start_paused` would auto-advance the
//! retry sleeps and compress the old 3 s spin under the elapsed bound,
//! voiding the fail-fast discriminator (elapsed < 1 s vs the 3 s
//! retry window).
//!
//! The RETRY-POSITIVE complement is pinned by
//! `direct_race_retries_until_consumer_registers`: the direct
//! registration race must keep the loop retrying until a consumer
//! route registers mid-send.

use std::sync::Arc;
use std::time::{Duration, Instant};

use camel_api::{CamelError, Exchange};
use camel_component_direct::DirectComponent;
use camel_component_mock::MockComponent;
use camel_component_seda::{SedaComponent, is_no_active_consumers_gate};
use camel_core::CamelContext;

use super::SendError;
use super::document::{JobBody, JobSendAction};
use super::send_with_startup_retry;

/// Boot a started context with the direct, mock, and seda components and
/// one route `direct:jobs -> mock:counted -> <seda_step>`. NO route
/// consumes the seda endpoint, so a plain (or fanout) seda producer
/// rejects with the no-active-consumers gate while the discard variant
/// succeeds. Returns the context and the mock handle for assertions.
async fn booted_gate_pipeline(seda_step: &str) -> (CamelContext, MockComponent) {
    let mut ctx = CamelContext::builder()
        .build()
        .await
        .expect("context build");
    let mock = MockComponent::new();
    ctx.register_component(mock.clone());
    ctx.register_component(DirectComponent::new());
    ctx.register_component(SedaComponent::new());

    let yaml = format!(
        r#"
routes:
  - id: gate
    from: "direct:jobs"
    steps:
      - to: "mock:counted"
      - to: "{seda_step}"
"#
    );
    let defs = camel_dsl::parse_routes_with_env(&yaml, &|_| None).expect("routes parse");
    assert_eq!(defs.len(), 1, "fixture declares exactly one route");
    for def in defs {
        ctx.add_route_definition(def).await.expect("add route");
    }
    ctx.start().await.expect("context start");
    (ctx, mock)
}

/// The job send action targeting the entry route.
fn tick_send() -> JobSendAction {
    JobSendAction {
        to: "direct:jobs".to_string(),
        body: Some(JobBody::Text("tick".to_string())),
        headers: None,
    }
}

/// Assert the send failed as a pipeline failure carrying the SEDA
/// no-active-consumers gate, within the fail-fast budget: the gate must
/// return on the FIRST attempt (elapsed < 1 s), never after spinning the
/// 3 s retry window. Returns the error so callers can pin the exact gate
/// wording (`is_no_active_consumers_gate` matches both the single-mode
/// "has no active consumers" and the fanout "has no active subscribers"
/// forms).
fn assert_gate_failure(result: Result<Exchange, SendError>, elapsed: Duration) -> CamelError {
    let e = match result {
        Err(SendError::Pipeline(e)) => e,
        Err(SendError::Transport(detail)) => {
            panic!("expected the SEDA gate as a pipeline failure, got transport: {detail}")
        }
        Ok(_) => panic!("expected the SEDA gate as a pipeline failure, got Ok"),
    };
    assert!(
        is_no_active_consumers_gate(&e),
        "the pipeline failure must be the SEDA no-active-consumers gate: {e}"
    );
    assert!(
        elapsed < Duration::from_secs(1),
        "gate failure must fail fast on the first attempt (the pre-fix send loop \
         spun the full 3 s retry window); took {elapsed:?}"
    );
    e
}

/// Assert the pre-SEDA side effect executed EXACTLY once: a retry of the
/// pipeline would replay the already-run mock step and duplicate it.
async fn assert_exactly_one_receipt(mock: &MockComponent) {
    let counted = mock.get_endpoint("counted").expect("counted endpoint");
    counted.expect_count(1);
    counted.assert_satisfied().await;
}

#[tokio::test]
async fn gate_fails_fast_with_exactly_one_side_effect() {
    let (ctx, mock) = booted_gate_pipeline("seda:worker").await;
    let send = tick_send();

    let started = Instant::now();
    let result = send_with_startup_retry(&ctx, &send, "direct:jobs").await;
    let elapsed = started.elapsed();

    assert_gate_failure(result, elapsed);
    assert_exactly_one_receipt(&mock).await;
}

#[tokio::test]
async fn fanout_gate_fails_fast_with_exactly_one_side_effect() {
    // Fanout mode (`multipleConsumers=true`) with no subscribers fires
    // the no-active-SUBSCRIBERS gate wording; the send loop must treat
    // it identically: first-attempt pipeline failure, no replay.
    let (ctx, mock) = booted_gate_pipeline("seda:worker?multipleConsumers=true").await;
    let send = tick_send();

    let started = Instant::now();
    let result = send_with_startup_retry(&ctx, &send, "direct:jobs").await;
    let elapsed = started.elapsed();

    let e = assert_gate_failure(result, elapsed);
    // Pin the FANOUT wording: the shared predicate above also matches
    // the single-mode "has no active consumers" form, so only a wording
    // assertion proves the fanout endpoint fired its own gate text.
    assert!(
        e.to_string().contains("has no active subscribers"),
        "fanout endpoint must fire the no-active-subscribers gate wording: {e}"
    );
    assert_exactly_one_receipt(&mock).await;
}

#[tokio::test]
async fn discard_if_no_consumers_proceeds_without_gate_error() {
    // The discard variant never produces `EndpointCreationFailed` — the
    // exchange is discarded at the seda producer — so the retry
    // classifier is never consulted and the send completes.
    let (ctx, mock) = booted_gate_pipeline("seda:worker?discardIfNoConsumers=true").await;
    let send = tick_send();

    let result = send_with_startup_retry(&ctx, &send, "direct:jobs").await;
    match result {
        Ok(_) => {}
        Err(SendError::Pipeline(e)) => {
            panic!("discard path must not surface a pipeline error: {e}")
        }
        Err(SendError::Transport(detail)) => {
            panic!("discard path must not surface a transport error: {detail}")
        }
    }
    assert_exactly_one_receipt(&mock).await;
}

#[tokio::test]
async fn direct_race_retries_until_consumer_registers() {
    // RETRY-POSITIVE complement: the direct registration race (non-gate
    // `EndpointCreationFailed`) must be retried until the consumer
    // route starts. The consumer registers 100 ms into the send, so a
    // loop that never retries fails on the first attempt and this test
    // goes red.
    let mut ctx = CamelContext::builder()
        .build()
        .await
        .expect("context build");
    let mock = MockComponent::new();
    ctx.register_component(mock.clone());
    ctx.register_component(DirectComponent::new());
    ctx.register_component(SedaComponent::new());
    // The consumer route does NOT exist yet: the send below races it.
    ctx.start().await.expect("context start");
    let ctx = Arc::new(ctx);

    let racer_ctx = Arc::clone(&ctx);
    let racer = tokio::spawn(async move {
        // Real (non-paused) delay so the send's first attempts hit the
        // unregistered direct endpoint while the loop is retrying.
        tokio::time::sleep(Duration::from_millis(100)).await;
        let yaml = r#"
routes:
  - id: consumer
    from: "direct:jobs"
    steps:
      - to: "mock:counted"
"#;
        let defs = camel_dsl::parse_routes_with_env(yaml, &|_| None).expect("routes parse");
        assert_eq!(defs.len(), 1, "fixture declares exactly one route");
        let def = defs.into_iter().next().expect("one route definition");
        let route_id = def.route_id().to_string();
        racer_ctx
            .add_route_definition(def)
            .await
            .expect("add consumer route");
        racer_ctx
            .runtime()
            .execute(camel_api::RuntimeCommand::StartRoute {
                route_id,
                command_id: "test:direct_race_retries:add-start".to_string(),
                causation_id: None,
            })
            .await
            .expect("start consumer route");
    });

    let send = tick_send();
    let started = Instant::now();
    let result = send_with_startup_retry(&ctx, &send, "direct:jobs").await;
    let elapsed = started.elapsed();

    tokio::time::timeout(Duration::from_secs(2), racer)
        .await
        .expect("route-registration task within 2s")
        .expect("route-registration task");
    match result {
        Ok(_) => {}
        Err(SendError::Pipeline(e)) => {
            panic!(
                "the send loop must retry the direct race until the consumer registers, got pipeline error: {e}"
            )
        }
        Err(SendError::Transport(detail)) => {
            panic!(
                "the send loop must retry the direct race until the consumer registers, got transport error: {detail}"
            )
        }
    }
    assert!(
        elapsed < Duration::from_secs(2),
        "send must succeed once the consumer registers (~100 ms), took {elapsed:?}"
    );
    assert_exactly_one_receipt(&mock).await;
}
