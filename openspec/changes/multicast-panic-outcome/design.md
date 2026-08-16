# Design: multicast-panic-outcome

## Context

`parallel_multicast` (crates/camel-processor/src/multicast_segment.rs)
spawns one task per branch returning `(idx, Option<PipelineOutcome>)`,
then drains:

```rust
while let Some(res) = set.join_next().await {
    if let Ok((idx, Some(o))) = res {
        results.push((idx, o));
    }
}
```

`JoinError` (task panic) is dropped. `JoinError` does not carry the branch
index, so the drain cannot reconstruct which branch panicked.

## Chosen approach: catch_unwind inside the task (index-preserving)

Wrap the branch outcome future with `futures::FutureExt::catch_unwind`
(`AssertUnwindSafe`) inside the spawned task, before the timeout arm:

```rust
let outcome = AssertUnwindSafe(outcome).catch_unwind();
let outcome = if let Some(dur) = timeout {
    match tokio::time::timeout(dur, outcome).await {
        Ok(Ok(o)) => o,
        Ok(Err(panic_payload)) => {
            let failure = Failed(ProcessorError(format!(
                "multicast branch {idx} panicked")));
            std::mem::forget(panic_payload);
            failure
        }
        Err(_elapsed) => Failed(ProcessorError(format!(
            "multicast branch {idx} timed out after {dur:?}"))),
    }
} else {
    match outcome.await {
        Ok(outcome) => outcome,
        Err(panic_payload) => {
            let failure = Failed(ProcessorError(format!(
                "multicast branch {idx} panicked")));
            std::mem::forget(panic_payload);
            failure
        }
    }
};
```

Rationale vs. a drain-side `Err(join_err) if join_err.is_panic()` arm
(recipient_list's shape): recipient_list has no per-task index to recover.
Multicast has an index, but `JoinError` loses it. Catching inside the task
preserves `idx`, so the synthetic `Failed(idx, …)` enters `results` and
flows through all downstream paths unchanged:

- Stop-winner scan (`results.find(idx == winning_idx)`) still works.
- `branch_count` / `failed_branches` accounting becomes correct for free
  because a panicked branch is now a `Failed` slot.
- `last_error` highest-index rule picks the panic error naturally.
- Zero-success guard fires for all-panic runs. This closes the ADR-0058
  laundering hole.

`AssertUnwindSafe` is sound here because the branch and exchange are owned by
the task. The code drops the unwound future and does not reuse its state.
Shared Stop state uses atomics and does not depend on branch-local invariants.

The mapping constructs the representative error, then forgets the panic
payload. A payload can have a `Drop` implementation that panics. Dropping such
a payload would create a second panic outside `catch_unwind` and recreate the
dropped-`JoinError` defect. Forgetting the payload leaks it only on the panic
path and preserves the required outcome.

Cancellation semantics: a cancellation `JoinError` is not a panic. The drain
continues to exclude it from failure accounting. The current multicast Stop
path drains all in-flight tasks and does not call `abort_all`. Panics during
the timeout window are caught by the same wrapper in the `Ok(Err(payload))`
timeout arm.

This design handles unwinding panics. A build configured with `panic=abort`
terminates the process before Rust can produce a `PipelineOutcome`; no
in-process mapping can intercept that behavior.

## Alternatives rejected

- **Drain-side panic arm + separate counter** (recipient_list mirror):
  requires editing accounting, `last_error` selection, and the
  zero-success guard. It has higher regression risk on rc-b41j surfaces.
- **Do nothing / document**: violates the spec's zero-success invariant
  because all-panic `Completed([])` laundering is observable today.

## Affected crates and boundaries

- `crates/camel-processor` (Runtime/OutcomePipeline boundary). The
  `futures` crate is already a workspace dependency.
- No DSL, component, CLI, or schema surface changes.

## ADR references

- ADR-0058 (outcome-based composition): panic = zero-success attempted
  work; MUST NOT launder to success. Drives the zero-success guard fix.
- ADR-0025 §3 (Stop EIP): intentional halt precedence. `Stopped` still
  wins over panicked branches; unchanged by this design.
- Recipient-list precedent: `recipient_list.rs` L110-118 maps panics to a
  representative `ProcessorError` with an ADR-0058 comment.

## Testing strategy

Six tests in the `#[cfg(test)] mod tests` module (extracted at
implementation time to `multicast_segment_tests.rs` via
`#[cfg(test)] #[path]`, following the `claim_check.rs`/`multicast.rs`
crate precedent) in
`crates/camel-processor/src/multicast_segment.rs` (where the rc-b41j
scenario suite lives), mirroring the recipient_list panic test pattern
(spawned branch body that panics inside its future). Existing helpers
(`always_failed_body`, `always_stopped_body`, `tagged_completed_body`)
are reused; a new `panicking_body` helper is introduced:

1. A `Completed` branch plus a `Failed` branch and a panicked branch returns
   `Completed`. A captured partial-success diagnostic reports two failed
   branches.
2. All branches panic. The result is `Failed` with the highest-index branch's
   representative error, not `Completed([])`.
3. A `Failed` branch plus a higher-index panicked branch returns `Failed` with
   the panic error under the highest-index rule.
4. A `Stopped` branch plus a panicked branch propagates `Stopped`.
5. A panic inside the timeout window maps to `Failed`, not a dropped
   `JoinError`.
6. A panic payload with a panicking `Drop` implementation maps once to
   `Failed` and does not escape as a second task panic.

Sequential mode and non-panicking parallel behavior asserted unchanged by
the existing suite.

## Phases

Single-phase change: two implementation tasks (1.1 catch_unwind wiring
plus core accounting tests, 1.2 precedence and payload-safety tests) plus
the spec delta. No multi-phase decomposition needed.
