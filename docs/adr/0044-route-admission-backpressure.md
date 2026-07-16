# ADR-0044: Route-admission back-pressure

## Status
Accepted

## Context
The pipeline's `poll_ready` only checks the first step's readiness (Tower semantics).
The Concurrent model's pipeline task acquires the Semaphore permit AFTER dequeue
(`rx.recv()`) and spawn — the consumer buffers unbounded in-flight work internally
before the permit gate. The original spec proposal (poll all steps upfront) was
REJECTED by the oracle: (1) Tower readiness reserves capacity for the immediately
following `call`; (2) ADR-0019 allows composing `Pending` but never routing
readiness `Err`.

## Decision

### 1. Fix permit acquisition: BEFORE dequeue (behavior change)
In the Concurrent model, restructure so the Semaphore permit is acquired BEFORE
`rx.recv()`. This ensures the consumer's `tx.send()` back-pressures when permits
are exhausted — the consumer cannot buffer unbounded in-flight work.

Implementation: acquire permit (owned) with cancel-awareness, then dequeue:
```rust
// Concurrent model — fixed admission (ADR-0044):
loop {
    // B2: acquire permit BEFORE dequeue, with cancel-awareness.
    let permit = tokio::select! {
        permit = async {
            match &sem {
                Some(s) => Arc::clone(s).acquire_owned().await.map(Some),
                None => Ok(None),
            }
        } => permit.expect("semaphore closed"),
        _ = pipeline_cancel.cancelled() => return,
    };
    let envelope = tokio::select! {
        envelope = rx.recv() => match envelope { Some(e) => e, None => return },
        _ = pipeline_cancel.cancelled() => return,
    };
    // Spawn per-exchange task WITH the permit moved in (preserves parallelism).
    let pipe_ref = Arc::clone(&pipeline);
    let cancel = pipeline_cancel.clone();
    tokio::spawn(async move {
        let _permit = permit; // held for exchange lifetime — RAII release
        // ... process exchange (load pipeline, ready_with_backoff, call)
    });
}
```

> **Cancel-awareness:** permit acquisition is wrapped in `select!` against `pipeline_cancel.cancelled()` so route stop is not blocked waiting for a permit.

> **`acquire_owned()`** returns an owned `OwnedSemaphorePermit` that can be moved into a spawned task. This preserves per-exchange parallelism while ensuring the permit is held for the exchange's entire lifetime.

> **Sequential model:** already correct — processes one at a time, natural back-pressure via `mpsc::channel(256)`.

### 2. Per-step readiness inside run_steps (existing behavior, documented)
`RetryableStep for BoxProcessor::invoke()` routes readiness errors into the
invocation path — `poll_ready::Err` becomes `PipelineOutcome::Failed`. This is
the per-step readiness check point. B2 documents this; no code change needed.

### ADR-0019 compatibility proof
- Readiness `Pending`: the step's `.await` cooperatively yields — runtime waits.
- Readiness `Err`: becomes `PipelineOutcome::Failed` via `RetryableStep` — never
  escapes as `poll_ready::Err`. Error handler sees it and may retry/handle.
- No readiness state escapes the pipeline boundary.

### What B2 does NOT do
- Does NOT poll all steps upfront (rejected by oracle).
- Does NOT add `CompiledStep` backpressure-marker metadata.

## Consequences
- Concurrent model: consumer back-pressures when in-flight permits exhausted.
- Permit held for entire exchange lifetime (dequeue → process → reply).
- Sequential model unchanged.
