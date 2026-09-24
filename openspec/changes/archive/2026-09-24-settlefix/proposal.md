# Proposal: settlefix

## Why

`camel test` settle is 98% of lean per-document wall time: every document
pays a quiet window (default 250ms) plus 50ms sample quantization before
evaluation, even when its traffic completed in microseconds. Measured on the
camel-cache team's 218-document lean batch: 258ms/doc default, 52.7ms/doc
with the interim `settle: 50ms` mitigation (4.9x). The root cause is the
sampled-quiet-window algorithm in `runner.rs`: a polling loop with a 50ms
floor (`SAMPLE_INTERVAL`) that can only detect stability by waiting a full
window after the last change.

The context already owns the authoritative completion signal:
`CamelContext::total_in_flight()` — the drainclaim counter whose RAII
`InFlightClaim`s cover the whole exchange lifecycle (seda queue residency,
dispatch, route-pipeline residency). The job batch runner already treats a
single zero read as a linearizable drained verdict. The testkit's settle
simply never used it.

## What Changes

- **Completion signal (partner signal)**: the in-flight counter gains a
  zero-transition notification — `InFlightClaim`'s `Drop` wakes waiters when
  the gauge releases its last claim (1→0). Counting semantics and
  `total_in_flight()` are unchanged; the notify is purely additive.
- **Stability signal**: mock endpoints gain a per-receive arrival
  notification (camel-mock), so timer documents reproduce today's exact
  observable semantics (quiet window over expected endpoints' counts)
  event-driven, without the 50ms sampler.
- **Settle rewrite in `runner.rs`**: mode split by structure.
  - Completion mode (no route consumes from a self-firing source): settle
    completes when the zero-transition notification arrives — no quiet
    window, no floor. `settle:` becomes the settle timeout (default 5s),
    anchored at settle entry (after input delivery), so delivery time never
    eats the settle budget.
  - Stability mode (`timer:` consumers — the only self-firing source in the
    lean registry): legacy quiet-window semantics verbatim (default 250ms,
    `settle:` override, any expected-count change resets the window,
    deadline = quiet + 5s budget anchored at route-execution begin),
    implemented with arrival notifications instead of sampling.
- **Spec delta**: mock-testkit "Settling before assertion" requirement
  rewritten for notification semantics; deadline/never-hang preserved.
- **Docs**: `docs/src/testing/index.md` — `settle:` reinterpreted as a
  deadline for completion-mode documents; the interim `settle: 50ms` note
  is superseded (existing configs keep working).
- Out of scope: `--jobs`/parallelism (spec mandates sequential docs), error
  handlers, compile/embed paths.

## Touched surfaces (lease disclosure)

The mission lease is "camel-cli test settle machinery + mock-testkit spec".
The notification seam requires mechanical, additive type changes to the
shared in-flight plumbing — enumerated exhaustively (no overlap with
mission 243 compile/embed or mission 248 error-handler):

- `camel-api` (`in_flight.rs`, `exchange.rs`): `InFlightGauge` composite
  (counter + idle notify); claim carries the gauge.
- `camel-core` (`context.rs`, `context_builder.rs`, 6 lifecycle adapter
  files): gauge construction/plumbing; new gauge accessor; runner wait
  seam.
- `camel-component-api` (`component_context.rs`, `consumer.rs`,
  `runtime_observability.rs`): `in_flight_counter()` returns the gauge.
- `camel-component-mock`: per-receive arrival notification on mock
  endpoints.
- Counter-handle type swap (mechanical, `Deref`-assisted): `camel-cli`
  (`job/batch.rs`), `camel-processor` (aggregator/resequencer claim sites),
  `camel-component-seda`, `camel-component-ws`, `camel-component-grpc`,
  `camel-http`, `camel-master`.

## Acceptance criteria

- A lean document settles on the completion notification with no
  quiet-window floor (settle wall collapses from ~250ms to microseconds;
  ~50x on a 218-doc lean batch per the mission's measurement).
- Stability-mode (timer) documents keep today's observable semantics:
  timer routes settle before assertion; count changes reset the window;
  unstable traffic still fails at the deadline with a settle-timeout
  message and exit 1 — never hang.
- A stuck in-flight claim in completion mode hits the `settle:` timeout and
  fails with exit 1 (never-hang regression pin).
- Sequential document execution and exit-code mapping are unchanged.
- No periodic sampling remains anywhere in the settle path.
- Existing `settle:` configurations keep working (deadlines in completion
  mode; quiet windows in stability mode).
- Gates: fmt; clippy `-D warnings` on every touched crate; `cargo test -p
  camel-cli --lib` + battery.

## Risk budget

- Acceptable: the gauge/arrival-notify changes are mechanical and additive;
  stability-mode semantics preserved verbatim.
- Out of bounds: any change to sequential document execution, exit codes,
  `--jobs`, error-handler files (mission 248), compile/embed (mission 243),
  or any polling loop reintroduced under another name.

Bd: rc-kv7wa
