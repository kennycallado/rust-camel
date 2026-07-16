//! A2 gate: count heap allocations per pipeline invocation.
//!
//! Runs as a dedicated test binary because the global `#[global_allocator]`
//! hook affects the entire process. Other integration tests in `camel-core`
//! would otherwise pay a per-allocation bookkeeping cost.
//!
//! Gate: A2 must reduce per-call allocations (we expect ~3 fewer — one
//! per pipeline step — by removing the `Box::new(...) as Box<dyn RetryableStep>`
//! wrappers in `run_steps`).
//!
//! See `crate::lifecycle::adapters::route_compiler::run_steps` for the
//! hot path under test.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use camel_api::BoxProcessorExt;

static ALLOC_COUNT: AtomicU64 = AtomicU64::new(0);
static COUNTING: AtomicBool = AtomicBool::new(false);

struct CountingAllocator;

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if COUNTING.load(Ordering::Relaxed) {
            ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

fn make_exchange() -> camel_api::Exchange {
    camel_api::Exchange::new(camel_api::Message::new("alloc-test"))
}

/// Pass-through step: returns the exchange unchanged.
fn passthrough() -> camel_api::BoxProcessor {
    camel_api::BoxProcessor::from_fn(|ex| Box::pin(async move { Ok(ex) }))
}

/// Build a 3-step pass-through pipeline with no error handler. Going through
/// `compose_pipeline` (not `_with_handler`) keeps the hot path minimal so the
/// delta attributable to A2 is visible.
fn build_three_step_pipeline() -> camel_api::BoxProcessor {
    use camel_core::route::{CompiledStep, compose_pipeline};

    let processors: Vec<CompiledStep> = (0..3)
        .map(|_| CompiledStep::Process {
            processor: passthrough(),
            body_contract: None,
            lifecycle: None,
        })
        .collect();

    compose_pipeline(processors)
}

#[tokio::test(flavor = "current_thread")]
async fn allocations_drop_after_a2() {
    use camel_api::BoxProcessor;
    use tower::Service;
    use tower::ServiceExt;

    // ── Setup (outside measurement region) ────────────────────────────
    let pipeline = build_three_step_pipeline();
    let mut svc = BoxProcessor::new(pipeline);

    // Warm up: drives the first-time codegen and any one-shot bookkeeping
    // (Arc bumps, first-call lazy init). Two warmups are cheap insurance.
    for _ in 0..2 {
        let ex = make_exchange();
        let _ = svc.ready().await.unwrap().call(ex).await.unwrap();
    }

    // ── Measure ───────────────────────────────────────────────────────
    // The exchange must be created BEFORE we start counting — the call()
    // takes ownership of the exchange, so we build it ahead of time and
    // start the counter immediately before invoking the service.
    let ex = make_exchange();
    COUNTING.store(true, Ordering::Relaxed);
    ALLOC_COUNT.store(0, Ordering::Relaxed);

    let result = svc.ready().await.unwrap().call(ex).await.unwrap();

    let allocs = ALLOC_COUNT.load(Ordering::Relaxed);
    COUNTING.store(false, Ordering::Relaxed);

    let body_str = format!("{:?}", result.input.body);
    assert!(
        body_str.contains("alloc-test"),
        "exchange body mismatch: {body_str}"
    );

    // Regression gate — catches if the A2 optimization is reverted.
    // The pre-A2 baseline was 22 allocs/call for a 3-step pass-through
    // pipeline. Post-A2 removes the per-step `Box::new(...) as
    // Box<dyn RetryableStep>` wrapper (delta = 3, one per step).
    //
    // Baseline (pre-A2): 22 allocs/call for a 3-step pass-through pipeline.
    // Expected post-A2:  ~19 allocs/call (delta = 3, one per step).
    const ALLOCS_BEFORE_A2: u64 = 22;
    eprintln!("allocations per pipeline call: {allocs}");
    // Threshold 20 allows 1 alloc drift from tokio/Exchange.
    assert!(
        allocs <= ALLOCS_BEFORE_A2 - 2,
        "A2 allocation reduction insufficient: {allocs} > {} (baseline {ALLOCS_BEFORE_A2})",
        ALLOCS_BEFORE_A2 - 2
    );
}
