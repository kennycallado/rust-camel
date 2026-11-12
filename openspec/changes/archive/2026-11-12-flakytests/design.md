# Design: flakytests

## Approach

One test-stability fix lands (flake 1); the second flake is parked at
scoping with a production-seam requirement (flake 2, documented below).

### Flake 1 — JS amortization ratio (rc-86u4n) — IN SCOPE

Evidence: CI run 43033768484 attempt 6 measured steady-state
514.6 µs/eval (eval1 = 1993.9 µs, ratio 3.9x) against the 4x floor; the
dev-box calibration is 5.5–6.2x with steady ~350 µs. The CI number is
47% above the dev figure. ATTRIBUTION HYPOTHESIS (blessing expert
requirement): scheduler contention inflating the 999-eval aggregate —
the CI-vs-dev aggregates establish a difference, not its cause. The
mechanism claim is validated by recording ≥ 20 unloaded runs AND ≥ 20
runs under representative synthetic CPU load; both distributions go
into the test's doc comment.

Fix: restructure the measurement, keep the intent.

- eval1 unchanged: first job into a fresh `BoaEngine`, wall-clock, with
  the existing correctness sanity asserts.
- Explicit warm-up window: the next 100 evals run untimed.
- Steady state: 6 batches × 150 evals; per-batch µs/eval; the assertion
  goes through a pure helper `assert_amortization_ratio(eval1_us,
  &batches)` that computes the median (even-count median = mean of the
  two middle values) and asserts median < eval1/4 (the 4x floor,
  unchanged). Total 1001 evals ≈ today's 1000.
- Print eval1, every batch value, the median, and the ratio; the
  failure message carries the full distribution for triage.

Honesty labels: whether contention inflates only a minority of the 6
batches is a HYPOTHESIS; sustained whole-window contention still trips
the floor, which is the accepted false-positive direction for a ≥ 4x
intent.

Simulated cache regression, defined exactly and wall-clock-free: a
second `#[test]` feeds the pure helper a synthetic regressed
distribution (steady-state batches at ~eval1 cost) and asserts it
panics — the regression-detection property is pinned deterministically,
independent of any machine's timing.

Best-of-N was rejected: it accepts atypically favorable samples and
measures a noise floor rather than steady-state cost.

Spec note: the `js-engine-cache` scenario pins the methodology (1000
evals, mean). This change amends that scenario — methodology only; the
≥ 4x machine-independent ratio, the cold-eval1 definition, and the
dev-profile CI assertion are unchanged. Same amendment path the spec
records for its 2026-09-12 recalibration. Moving the timing assertion
to a bench/xtask target was rejected: it removes CI's continuous
protection. The amendment is the mission's stop-and-say-so item:
authorization is sought at the merge gate, in the park.

### Flake 2 — CLI second stop signal (rc-af1y3) — PARKED, OUT OF SCOPE

Evidence: attempt 7 got exit 0 where the force-exit arm must produce 1;
the child logged an orderly stop, so the blind `kill -INT; kill -TERM`
pair landed while teardown had already progressed past the guard. The
exact exit-0 mechanism is not statically reconcilable (the interrupted
path exits 2 or 1) — open question carried in `rc-af1y3`.

Three expert rounds rejected every test-only design:

1. INT alone → wait the child's `Stopping CamelContext` stop-start
   marker → TERM. Broken schedule: the trust marker precedes the send
   race; a buffered INT wins the biased signal-first select before the
   send is polled; empty-context teardown completes inside one 25 ms
   marker poll; TERM lands on an aborted guard. A teardown-duration
   barrier cannot fix a race whose adverse side controls teardown
   duration.
2. Park both signals pre-race (pair right after the armed marker,
   during boot). Broken schedule: one `sh -c` removes the typical exec
   gap, not worst-case scheduling between the two kills — a shell
   stall between INT and TERM reproduces the same late-TERM failure.
3. Marker-synchronized claim chain (INT after `CamelContext started`,
   TERM after `Stopping CamelContext`, drain hold from the claimed
   30 s-delay exchange). Broken assumption: "send polled before INT
   lands" rests on kill-spawn latency (ms) beating the race's first
   poll (µs after the marker flush) — a scheduling claim, not an
   observable; the signal-first select explicitly permits INT to win
   without polling the send, and polling the send first does not by
   itself prove worker claim. Requirement: an OBSERVED fixture marker
   proving worker pipeline entry into the teardown-holding work,
   before INT.

That requirement is unachievable test-only: the drain hold counts
claimed exchanges (`drain_in_flight`), and no production log line
observes exchange claim or pipeline entry at any level — the delayer
is silent (`delayer.rs` has no logging), seda logs only consumer
start/stop (`SEDA consumer started/stopped`), and the route runtime
logs no per-exchange events. Surfacing it needs a ~3-line env-opt-in
test seam at consumer dispatch, mirroring the existing
`CAMEL_JOB_SIGNAL_MARKER` / `CAMEL_JOB_SHUTDOWN_MARKER` precedent —
production code, forbidden by this mission's constraints.

Resolution: the CLI test ships unchanged; `rc-af1y3` stays open
carrying this analysis; a follow-up mission adds the seam (production,
test-default-off) and then the marker-synchronized test of design (3)
becomes deterministic: INT after the pipeline-entry marker (worker
claim observed), TERM after `Stopping CamelContext`, with the drain
hold (`max(remaining, 5 s)` budget vs the 30 s delay exchange) keeping
the guard window seconds wide.

## Affected crates

- camel-language-js: test-only (`tests/js_engine_cache_amortization.rs`).
- camel-cli: untouched by this change (flake 2 parked).

## Architecture boundaries

No production surface changes: Runtime, DSL, Components, Services,
Languages, Functions are untouched. The only spec delta is in the
`js-engine-cache` domain and covers measurement methodology of the
amortization scenario; the isolation, integrity, limits, and caching
requirements are not modified.
