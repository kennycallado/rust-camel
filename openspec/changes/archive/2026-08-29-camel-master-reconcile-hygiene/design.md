# Design: camel-master-reconcile-hygiene

## Approach

Three surgical edits in the Master supervision path plus one platform
validator and docs.

**1. Epoch-aware idempotence (rc-2f08).** `DelegateState::Active` gains
`epoch: u64`. `reconcile_event`'s `StartedLeading` arm (leadership.rs)
already snapshots `my_epoch = ctx.leader_epoch.load(Ordering::Acquire)` for
the epoch-stamping bridge (leadership.rs:278) — store that same value in the
new field at the `*state = DelegateState::Active { .. }` write
(leadership.rs:300). At arm entry, if the state is
`Active { epoch, .. } && epoch == ctx.leader_epoch.load(Ordering::Acquire)`,
return early (no drain, no recreate, no lifecycle emission). Otherwise run
the existing path. Semantics:

- Same-term duplicate (watch re-delivery without a takeover): skip — this is
  the churn case. A stale synthetic retry re-dispatch arriving while Active
  is also a no-op (same guard), keeping the canonical "synthetic retry
  re-dispatch does not emit" scenario green.
- Term bump (coalesced flap across a real takeover, or same-process
  re-acquisition at a higher lease term): epoch differs → full drain +
  recreate with a bridge restamped at the new epoch — required by ADR-0035
  fencing. A term bump found by the guard RESETS the acquisition counter
  before the arm counts: a new fencing epoch is a new acquisition edge, so
  restamping stays unconditional even under an exhausted bounded budget
  (otherwise a term bump at `max_attempts = N` after the final successful
  create would leave a zombie delegate stamped at the old epoch — every
  envelope fenced off downstream — or kill it without recreation).
- Stale-stamp detection is ALSO tick-driven, not only delivery-driven: the
  renewal path (`ContinueLeading`) clamp-adopts higher out-of-band lease
  terms into the published epoch WITHOUT emitting any event
  (platform_service.rs:263-273, `clamp_epoch` leadership_fsm.rs:75-80), so
  a delivery may never arrive. The retry tick therefore dispatches a
  synthetic `StartedLeading` reconciliation when `is_leading` and the
  Active delegate's stamp differs from the published epoch; the guard's
  term-bump path (reset, drain, recreate, restamp) handles it. This does
  NOT contradict the no-spurious-poll rule: the dispatch condition is
  stamp ≠ published (monotonic single-writer — fires only after genuine
  advancement requiring a restamp), never "epoch changed recently".
  TICK ORDERING MATTERS: the stale-stamp check runs BEFORE the
  finished-handle teardown. Otherwise a dead Active delegate at epoch E
  with an exhausted budget and a published E' > E would be torn down
  first — erasing E from the state — and the acquisition branch would
  then consult the stale exhausted budget and stop the consumer instead
  of resetting it. Tick sequence: (1) stale-stamp dispatch if
  `is_leading && Active{epoch} ≠ published`; (2) finished-handle
  teardown (unchanged) when no dispatch fired; (3) Inactive acquisition
  retry (unchanged). A finished handle with a stale stamp is drained by
  the dispatch's own term-bump path, subsuming the teardown.
- The epoch is compared in exactly TWO places: inside `reconcile_event`
  at dispatch time, and in the retry tick's stale-stamp dispatch condition
  (Active stamp vs published). Renewal-path updates
  (`apply_renewal_epoch`, platform_service.rs:436) clamp-monotonic and can
  bump mid-term; a dispatch keyed on "epoch changed recently" without the
  stamp comparison would introduce spurious churn — comparing the STAMP
  against the published epoch is what makes both dispatch sites safe.

**2. Exact acquisition budget (rc-h5s8).** One counting point, inside
`reconcile_event`'s `StartedLeading` arm, placed after the epoch guard and
immediately before endpoint construction (after the drain — the consult
follows the drain per the no-zombie ordering): the attempt counter reaches
the arm as an `AtomicU32` inside `ReconcileContext` (the context is held
across `.await`s inside the `tokio::spawn`ed supervision task; `Cell<u32>`
is `!Sync` and would make the future `!Send` — `AtomicU32` with `Relaxed`
ordering provides the interior mutability; single-task access means no
contention). The
exhaustion consult evaluates `!should_retry(count)` on the PRE-increment
value — the verdict sees the attempts made before this delivery, so a
fresh epoch performs its first create at `max_attempts = 1` — and the
counter then increments unconditionally regardless of the verdict (it
counts deliveries); a refused consult performs no create. An acquisition
epoch begins at an observed not-leading→leading edge, at the initial
snapshot, or at a guard-detected term bump — each resets the counter (§1);
the budget bounds attempts WITHIN one acquisition epoch. The supervision
loop
keeps only the edge/snapshot resets (supervision.rs:76, :108) and DROPS
the retry-arm increment (:179) — no double counting. Consequences:

- A duplicate `StartedLeading` watch delivery while Inactive-and-failing
  (coalesced flap with no observed bool edge) reaches the arm, counts, and
  cannot push attempts past N — the budget is checked before the create.
- Guard-skips are structurally uncounted (the skip returns before the
  increment).
- `max_attempts_absolute` total-attempt capping (network_retry.rs:193-201)
  now observes snapshot/edge attempts too, instead of only retry-tick ones.
- Delegate-death respawn (retry tick finds `Active` handle finished, stops
  it, re-enters the acquisition path with NO reset) consumes the same edge
  budget: at bounded `max_attempts = N` a dead delegate's respawns count
  toward N and can stop the consumer. Under the default unlimited policy
  this is invisible.

`max_attempts = N` (enabled, bounded) → AT MOST N create attempts per
acquisition epoch, and exactly N under persistent transient failure before
further creates are refused (an epoch whose first attempt succeeds
performs 1 ≤ N). `max_attempts = 0` (default) → unlimited, unchanged.
The counter counts
DELIVERIES, not successful creates: at refusal it can exceed N, so the
retry arm's exhaustion warning may log `attempts = 2` at
`max_attempts = 1` — truthful (two deliveries, one create); test
assertions on counts target `create_error` observations, not the raw
counter.

One deliberate behavior change is ratified here: a reconnect policy with
`enabled = false` previously performed one create per edge (the budget
gate lived only in the retry arm); under the in-arm consult it performs
zero creates — the consumer stops at the first tick. "Disabled = no
delegate" is the cleaner semantics for that non-default configuration.
Epoch-mismatch ordering is total and fixed: guard → (mismatch ⇒ reset the
acquisition epoch) → drain if a stale Active delegate exists → consult the
budget → recreate, or remain Inactive when the consult refuses (a disabled
policy drains a stale delegate and never recreates; no zombie — a drained
delegate cannot emit).

Backoff timing is preserved bit-for-bit: the retry-arm delay gate becomes
`delegate_attempts > 1 → delay_for(delegate_attempts - 2)` (today the
acquisition dispatch does not increment, so `> 0 → delay_for(n - 1)`
shifts by one once snapshot/edge count; the adjusted gate restores the
identical schedule — first retry delay-free, each subsequent retry one
backoff step later, exactly as before).

Tests: `create_error_endpoint_transient` updates from two `create_error`
observations to exactly one at `max_attempts = 1` (the old pair ratified
the N+1 snapshot quirk). `create_error_consumer_transient`
(`max_attempts = 3`, transient-then-success) stays green unchanged —
under in-arm counting its consults all pass at max=3, so it doubles as
the backoff-parity regression. New: a bounded-budget positive path
(`max_attempts = 2` → exactly two, then stop), the exhausted-duplicate
refusal, the disabled no-create, the unlimited default, and the
term-bump-at-exhausted re-acquisition.

**3. Renewal slack validation (rc-ys57 b).** `KubernetesPlatformConfig::
validate` (platform_service.rs:50) gains a third rule after the two ordering
rules: `lease_duration - renew_deadline >= retry_period`, else
`PlatformError::Config` with the three values in the message. No new field:
the slack IS one retry window, covering NTP skew and renew-jitter. Defaults
(15s/10s/2s → slack 5s ≥ 2s) pass.

**4. Docs (rc-ys57 a+c).** README "How It Works" gains: (i) drain runs after
leadership is already lost — overlap with a successor's lease is possible,
and the `x-camel-leader-epoch` fencing token enables split-brain safety
when a sink opts into rejecting stale epochs (enforcement is opt-in per
sink, ADR-0035), not drain-before-takeover ordering; (ii) how two
`master:X:` routes in one process coordinate, per backend: on Kubernetes
they share one cached elector per lock name — mutual exclusion is
per-process, so when this process leads, both routes' delegates run and
only one process holds the lease; the default Noop platform reports every
route as leader and provides no cross-route or cross-process exclusion.
The `drain_timeout_ms` field doc (camel-master config.rs:50)
cross-references the README paragraph. The stale
`delegate_retry_max_attempts` default row ("30") is corrected to
`0 = unlimited`. The stale citation `specs/master-component/spec.md`
(leadership.rs:87) is fixed to `openspec/specs/master-component/spec.md`.

## Affected crates

- `camel-component-master`: supervision.rs (budget), leadership.rs +
  consumer.rs (epoch field + guard), tests.rs (rewrites + new regression
  tests), config.rs (comment), README.md.
- `camel-platform-kubernetes`: platform_service.rs `validate` (slack rule) +
  unit tests. No FSM changes.

## Architecture boundaries

Components layer only consumes the `camel-api` leadership contract
(`LeadershipEvent`, `leader_epoch_arc()` — camel-api/src/platform.rs:64);
no trait or handle shape changes. The kubernetes platform keeps its FSM and
epoch monotonicity untouched — the new guard READS the published epoch, it
never writes it. camel-config is not modified (its `[platform]` zero-checks
stay; the semantic slack rule lives with `KubernetesPlatformConfig`, which
owns those invariants). DSL, Runtime, Languages, Functions: untouched.

## Phases

### Phase 1: supervision-loop behavior (camel-component-master)

- **Goal:** epoch-idempotent reconciliation + exact acquisition budget, with
  regression coverage and the master-component delta spec.
- **Dependencies:** none (ADR-0035 epoch semantics already shipped).
- **Externally-visible types/interfaces:** none (crate-internal
  `DelegateState` shape only).
- **Deliverable:** code + tests + delta spec; `cargo test -p
  camel-component-master` green including rewritten rc-tpv4 metric tests.
- **Exit-criteria:** new regression test — same-term duplicate leaves
  `create_consumer_calls == 1` and lifecycle counters unchanged; epoch-bump
  test re-reconciles once with restamped bridge (both delivery-driven and
  tick-driven, the latter mutating the fake epoch Arc without a watch
  delivery); tick-ordering test — dead Active delegate + stale stamp +
  exhausted budget resets and recreates instead of stopping;
  `max_attempts=1` transient test asserts exactly one `create_error`;
  bounded-budget positive path — `max_attempts=2` with persistent
  transient failure yields exactly two `create_error` observations then
  the budget-exhausted stop.

### Phase 2: platform validation + docs (camel-platform-kubernetes, README)

- **Goal:** renewal-slack rule, operator docs, stale-default and stale-path
  fixes.
- **Dependencies:** Phase 1 (README epoch-fencing paragraph cites shipped
  behavior).
- **Externally-visible types/interfaces:** `KubernetesPlatformConfig::
  validate` error surface only (new rejection class).
- **Deliverable:** validator + unit tests + README/field-comment updates +
  kubernetes-leadership delta spec.
- **Exit-criteria:** slack-violating config rejected with a message naming
  all three values; defaults pass; `cargo test -p
  camel-platform-kubernetes` green; lint-context-citations passes on the
  fixed path.

## Alternatives considered

- **Boolean edge-gate on `is_leading`** (skip when already leading):
  rejected — swallows coalesced flaps whose term bumped, losing mandatory
  epoch restamping; stale-state hazard.
- **Suppress duplicates upstream in `leadership_fsm`**: rejected — the FSM
  cannot know whether the consumer-side delegate needs re-reconciliation;
  blast radius crosses the platform/component boundary for no gain.
- **Document N+1 instead of fixing (rc-h5s8 option b):** rejected — README
  and spec already state "max attempts"; the quirk contradicts both.
- **New `skew_margin` config field (rc-ys57 b):** rejected — one retry
  window of slack is the operative bound; a second knob invites
  contradictory settings.
