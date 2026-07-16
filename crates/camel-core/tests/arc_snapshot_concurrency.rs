//! Concurrency tests for ArcSwap snapshot isolation.
//!
//! Verifies that atomic pipeline swap + concurrent exchange processing
//! never produces invalid intermediate states. Uses
//! `Arc<ArcSwap<SyncBoxProcessor>>` — the same pattern as the production
//! `SharedPipeline` (which wraps `SyncBoxProcessor` inside `PipelineAssembly`).

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use arc_swap::ArcSwap;
use camel_api::{BoxProcessor, BoxProcessorExt, Exchange, Message, SyncBoxProcessor, Value};
use camel_core::route::{CompiledStep, PipelineRuntimeCtx, compose_pipeline};
use tokio::sync::Barrier;
use tokio::time::timeout;
use tower::Service;

/// Build a pass-through CompiledStep that returns the exchange unchanged.
fn pass_through() -> CompiledStep {
    CompiledStep::Process {
        processor: BoxProcessor::from_fn(|ex: Exchange| Box::pin(async move { Ok(ex) })),
        body_contract: None,
        lifecycle: None,
    }
}

/// Verify that concurrent swap + call never produces invalid state.
///
/// Architecture: ArcSwap atomic pointer swap ensures each `load_full()`
/// returns a consistent snapshot (old or new). 8 workers + 1 swapper
/// coordinate via a 9-waiter barrier. Workers issue 50 calls before
/// and 50 after the swap (800 total), all must succeed.
#[tokio::test]
async fn concurrent_swap_and_call_no_invalid_state() {
    // S0: first step sets "phase: s0" marker, remainder pass-through.
    let s0 = compose_pipeline(
        std::iter::once(CompiledStep::Process {
            processor: BoxProcessor::from_fn(|mut ex: Exchange| {
                ex.input.set_header("phase", Value::String("s0".into()));
                async move { Ok(ex) }
            }),
            body_contract: None,
            lifecycle: None,
        })
        .chain((0..4).map(|_| pass_through()))
        .collect(),
        PipelineRuntimeCtx::compile_time(),
    );

    // S1: first step sets "phase: s1" marker, remainder pass-through.
    let s1 = compose_pipeline(
        std::iter::once(CompiledStep::Process {
            processor: BoxProcessor::from_fn(|mut ex: Exchange| {
                ex.input.set_header("phase", Value::String("s1".into()));
                async move { Ok(ex) }
            }),
            body_contract: None,
            lifecycle: None,
        })
        .chain((0..19).map(|_| pass_through()))
        .collect(),
        PipelineRuntimeCtx::compile_time(),
    );

    // SyncBoxProcessor wraps BoxProcessor behind Arc<Mutex<...>>,
    // making it Send+Sync so ArcSwap works cross-thread.
    let sw = Arc::new(ArcSwap::from_pointee(SyncBoxProcessor::new(s0)));
    let barrier = Arc::new(Barrier::new(9));

    let total_calls = Arc::new(AtomicUsize::new(0));
    let s0_count = Arc::new(AtomicUsize::new(0));
    let s1_count = Arc::new(AtomicUsize::new(0));

    let mut handles = Vec::with_capacity(9);

    // 8 worker tasks
    for _ in 0..8 {
        let sw = Arc::clone(&sw);
        let barrier = Arc::clone(&barrier);
        let total_calls = Arc::clone(&total_calls);
        let s0_count = Arc::clone(&s0_count);
        let s1_count = Arc::clone(&s1_count);
        handles.push(tokio::spawn(async move {
            // First batch: 50 calls on the initial snapshot (S0).
            for _ in 0..50 {
                let ex = Exchange::new(Message::new("test"));
                let mut proc = sw.load_full().as_ref().clone_inner();
                let ex = proc.call(ex).await.expect("first-batch call failed");
                total_calls.fetch_add(1, Ordering::Relaxed);
                track_phase(ex.input.header("phase"), &s0_count, &s1_count);
            }

            // Synchronize with swapper.
            barrier.wait().await;

            // Second batch: 50 calls — some may see S0, some S1
            // depending on timing. Both are valid pipelines.
            for _ in 0..50 {
                let ex = Exchange::new(Message::new("test"));
                let mut proc = sw.load_full().as_ref().clone_inner();
                let ex = proc.call(ex).await.expect("second-batch call failed");
                total_calls.fetch_add(1, Ordering::Relaxed);
                track_phase(ex.input.header("phase"), &s0_count, &s1_count);
            }
        }));
    }

    // Swapper task
    {
        let sw = Arc::clone(&sw);
        let barrier = Arc::clone(&barrier);
        handles.push(tokio::spawn(async move {
            barrier.wait().await;
            sw.store(Arc::new(SyncBoxProcessor::new(s1)));
        }));
    }

    // Await all tasks with timeout to prevent deadlock if the swapper
    // panics before reaching the barrier.
    for h in handles {
        timeout(Duration::from_secs(5), h)
            .await
            .expect("task timed out (swapper may have panicked)")
            .expect("task panicked");
    }

    assert_eq!(
        total_calls.load(Ordering::Relaxed),
        800,
        "total completed calls must be 800"
    );
    let s0 = s0_count.load(Ordering::Relaxed);
    let s1 = s1_count.load(Ordering::Relaxed);
    assert!(
        s0 >= 400,
        "S0 must have at least 400 calls (first batch), got {s0}"
    );
    assert!(
        s1 >= 1,
        "S1 must have at least 1 call (after swap), got {s1}"
    );
    assert_eq!(
        s0 + s1,
        total_calls.load(Ordering::Relaxed),
        "every call must carry a known phase marker"
    );
}

/// Increment the appropriate counter based on the phase header.
///
/// Panics if the header is missing or has an unexpected value — this is
/// intentional: any such exchange represents an invalid intermediate state.
fn track_phase(phase: Option<&Value>, s0_count: &AtomicUsize, s1_count: &AtomicUsize) {
    match phase {
        Some(Value::String(s)) if s == "s0" => s0_count.fetch_add(1, Ordering::Relaxed),
        Some(Value::String(s)) if s == "s1" => s1_count.fetch_add(1, Ordering::Relaxed),
        other => panic!("unexpected phase header: {other:?}"),
    };
}

/// Verify that an in-flight snapshot is isolated from a concurrent swap.
///
/// After acquiring a snapshot of S0 and swapping to S1, the old snapshot
/// must still process exchanges with S0's semantics (header marker present)
/// and must NOT show any S1 markers.
#[tokio::test]
async fn in_flight_call_completes_on_old_snapshot_after_swap() {
    // S0: a pipeline that marks the exchange with "s0-marker"
    let s0_proc = BoxProcessor::from_fn(|mut ex: Exchange| {
        ex.input
            .set_header("s0-marker", Value::String("yes".into()));
        async move { Ok(ex) }
    });
    let s0 = compose_pipeline(
        vec![CompiledStep::Process {
            processor: s0_proc,
            body_contract: None,
            lifecycle: None,
        }],
        PipelineRuntimeCtx::compile_time(),
    );

    // S1: a pipeline that marks the exchange with "s1-marker"
    let s1_proc = BoxProcessor::from_fn(|mut ex: Exchange| {
        ex.input
            .set_header("s1-marker", Value::String("yes".into()));
        async move { Ok(ex) }
    });
    let s1 = compose_pipeline(
        vec![CompiledStep::Process {
            processor: s1_proc,
            body_contract: None,
            lifecycle: None,
        }],
        PipelineRuntimeCtx::compile_time(),
    );

    let sw = Arc::new(ArcSwap::from_pointee(SyncBoxProcessor::new(s0)));

    // Acquire a snapshot of S0 before the swap.
    let mut proc_s0 = sw.load_full().as_ref().clone_inner();

    // Swap to S1.
    sw.store(Arc::new(SyncBoxProcessor::new(s1)));

    // Call the old S0 snapshot — must still carry S0's header marker
    // and must NOT have S1's marker.
    let ex = Exchange::new(Message::new("test"));
    let result = proc_s0.call(ex).await.expect("old snapshot call failed");

    assert_eq!(
        result.input.header("s0-marker"),
        Some(&Value::String("yes".into())),
        "old snapshot must preserve s0-marker"
    );
    assert_eq!(
        result.input.header("s1-marker"),
        None,
        "old snapshot must NOT carry s1-marker"
    );
}
