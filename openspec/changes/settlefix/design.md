# Design: settlefix

## Approach

Replace the sampled-quiet-window settle in the unit-tier test runner with a
notification-based settle. Two independent signals, one per mode.

**D1 — In-flight gauge (completion signal).** The drainclaim counter has no
wakeup today. Introduce `InFlightGauge` in `camel-api/src/in_flight.rs`:
`{ count: AtomicU64, idle: tokio::sync::Notify }` behind the existing
counting methods plus `total()`. `InFlightClaim::attach` takes the gauge;
`Drop` decrements and, on the last release (1→0), calls
`idle.notify_waiters()` (sync-callable from `Drop`). Counting behavior,
ordering, and `total_in_flight()` are unchanged — the notify is purely
additive. The handle type swap `Arc<AtomicU64>` → `Arc<InFlightGauge>` is
mechanical (`Deref` to the atomic keeps `fetch_*` call sites compiling)
across the enumerated surfaces (see proposal): camel-core plumbing and
adapters, the `in_flight_counter()` trait in camel-component-api, stored
handles/mint sites in camel-processor, camel-component-seda/-ws/-grpc,
camel-http, camel-master, camel-cli job batch tests.

**D2 — Mock arrival notification (stability signal).** Timer fires hold no
claim between fires, so in-flight quiescence is NOT completion for timer
documents, and 1→0 transitions do not reproduce today's exact window-reset
observations (a count can change while the counter stays nonzero; the
counter can hit zero without any expected count changing). Stability mode
must observe what today's algorithm observes: changes in the expected
endpoints' `received_count`. camel-mock gains a per-receive arrival
notification: the COMPONENT holds notification slots keyed by endpoint
name, exposed as `ensure_arrival_notify(name) -> Arc<Notify>` — a slot
exists from first request, independent of endpoint creation (endpoints
materialize when created — e.g. by route wiring — which may precede any
receive, so registration must not depend on endpoint existence). The receive
path pings the slot (creating it if absent) after recording the exchange;
the runner re-samples counts on wake.

**D3 — Mode split by structure.** The runner already scans route `from:`
URIs (seda consumer collection); the same scan classifies the document:

- **Completion mode** — no route consumes from a self-firing source (in the
  lean registry: `timer:`; direct, log, mock, seda are demand-driven). All
  traffic is input-triggered; settle completes on the first observed
  in-flight zero. Soundness inherits from drainclaim's lifecycle-covering
  claims — no observable zero-gap mid-chain, the same property that makes
  the batch runner's single zero read a verdict. Timeout = declared
  `settle:` value (default 5s), anchored at settle entry (after input
  delivery) so delivery time never consumes the settle budget. Deadline
  precedence: the deadline is checked before any idle acceptance — at
  entry and on every wake — so an idle counter with an unexpired deadline
  completes immediately, while an expired deadline errors even if the
  counter reads zero (a release racing the deadline resolved too late).
- **Stability mode** — legacy semantics verbatim, event-driven: quiet
  window default 250ms / `settle:` override; any change in the expected
  endpoints' counts (an arrival notification followed by a differing
  sample) resets the window; deadline = route-start + quiet + 5s
  instability budget anchored at route-execution begin (unchanged
  formula); `SAMPLE_INTERVAL` sampling is deleted. Only expected
  endpoints' counts participate (exactly today's rule — unrelated traffic
  never reset the window).

**D4 — Race-free wait patterns.** Both loops register-before-check:
pin the `notified()` future and `enable()` it (registering with the
`Notify` without awaiting), then read state, then `select!` the future
against the deadline sleep; on wake, re-read state before deciding. This
closes the check-to-registration window in which a release could fire
`notify_waiters` unobserved. Stability mode registers arrival slots via
`ensure_arrival_notify` for EVERY expected endpoint name (including
not-yet-created endpoints) before each sampling pass, and re-samples all
counts on window expiry as a backstop — a change that no notification
observed can only delay the window restart, never cause a premature
settle.

**D5 — Error contract.** Both modes fail with a settle-timeout document
error (exit 1) at the deadline — never hang. Messages keep the
`settle timeout:` prefix; the body names the mode's bound.

## Affected crates

- `camel-api`: `in_flight.rs` — gauge composite + notifying `Drop`;
  `exchange.rs` doc touch-ups.
- `camel-core`: gauge construction/plumbing (`context.rs`,
  `context_builder.rs`, `lifecycle/adapters/*`); gauge accessor for the
  runner; `total_in_flight()` unchanged.
- `camel-component-api`: `in_flight_counter()` trait + impls return the
  gauge (mechanical).
- `camel-component-mock`: per-endpoint arrival notification on receive.
- `camel-cli`: `commands/test/runner.rs` — settle rewrite (mode detection,
  two event loops, sampler deleted); `commands/job/batch.rs` tests adapt.
- `camel-processor`, `camel-component-seda`, `camel-component-ws`,
  `camel-component-grpc`, `camel-http`, `camel-master`: mechanical
  counter-handle type swap at stored fields and claim-mint sites.
- `openspec/specs/mock-testkit` (delta) + `docs/src/testing/index.md`.

## Architecture boundaries

Runtime (camel-core) owns the gauge and its notify — a runtime
observability primitive; the mock arrival notify is testkit observation
surface in camel-mock. The runner (camel-cli) is the sole consumer of both
for settle. No DSL, service, or language changes. The lean-registry
invariant (timer is the only self-firing source) is recorded in the spec.

## Alternatives considered

- **Lower the sampler floor**: still polling; the quiet window dominates
  lean wall time. Rejected (mission bans polling).
- **1→0 transitions for stability mode**: cannot reproduce count-change
  resets under a nonzero counter; resets without count changes alter
  timeout outcomes. Rejected (e_gpt, first blessing).
- **Count-satisfied early exit**: turns over-emission failures (timer
  emits 5, expects 3) into false passes. Rejected.
- **Reply-based completion only**: ignores seda continuation legs that
  outlive replies. Rejected.
- **Deadline anchored at route start for completion mode**: delivery time
  would consume the `settle:` budget — a `settle: 50ms` doc with slow
  delivery would fail where it settles today. Anchored at settle entry
  instead (e_gpt, first blessing).
