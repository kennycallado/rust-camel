# Tasks — direct-timeout-cancels-queued

## Task 1: write the contract the code already implements

**Files**
- `crates/camel-test/tests/direct_concurrent_admission_test.rs`
- `docs/src/components/direct.md`
- `crates/components/camel-direct/src/lib.rs` (comment only)
- `openspec/changes/direct-timeout-cancels-queued/specs/direct-dispatch/spec.md`

**Steps**
1. `direct_timeout_still_counts_admission_wait`: after release, settle with
   `await_no_in_flight` (bounded poll on `CamelContext::total_in_flight`) so
   the queued `p1` is provably dequeued, then assert the trace is
   `["begin:p0", "end:p0"]` and the mock received **zero** exchanges.
2. Add `abandoned_dispatch_stops_before_the_next_step`: a producer with
   `timeout_ms=100` dispatches into `concurrent(1)`; the caller polls once so
   the exchange enters and parks on a `Notify`; advance past the deadline and
   poll the caller's future to its timeout; release; settle; assert
   `["begin:only", "end:only"]` and zero mock exchanges — the running step
   finished, the next one never ran.
3. `docs/src/components/direct.md`: replace the "still executes" sentence with
   the channel-path boundary rule plus the inline clause (an abandoned inline
   call is dropped at once).
4. `crates/components/camel-direct/src/lib.rs`: the channel-path comment no
   longer claims the enqueued exchange survives its caller.

**Tests**
- `direct_timeout_still_counts_admission_wait` — arrange: parked `p0`,
  queued `p1`, one producer with `timeout_ms=100`; act: advance both
  deadlines; assert: canonical timeout text twice, no `begin:p1`, zero mock
  exchanges, in-flight settles to 0.
- `abandoned_dispatch_stops_before_the_next_step` — arrange: one admitted
  exchange parked in its step; act: caller deadline elapses; assert:
  `begin`/`end` recorded, zero mock exchanges.

**Acceptance**
- `cargo test -p camel-test --test direct_concurrent_admission_test` → 4
  passed, 0 failed.
- `cargo fmt --check --all` exit 0.
- `mdbook build` and `mdbook test` on `docs` exit 0.
- `openspec validate direct-timeout-cancels-queued --type change --json` valid.
- After archive, the canonical requirement "No producer-side serialization for
  Concurrent consumers" carries all three scenarios and no other page or
  comment still claims the retracted behaviour.

- [ ] 1.1
