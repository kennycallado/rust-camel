# Design: k8s-leader-self-fencing

## Approach

One mechanism replaces both failure modes: a renewal budget with deadline
accounting, plus a pure decision seam for testability.

1. **Reconcile verdict refined.** `reconcile_lease` currently returns
   `Ok((bool, Option<u64>))`, conflating "server answered: a valid foreign
   holder owns the lease" with "our renewal hit a 409 optimistic conflict"
   (the 409 arm at platform_service.rs:602-605 also returns
   `Ok((false, None))`). Its return contract becomes a verdict enum:

   ```rust
   enum ReconcileVerdict {
       Acquired { term: u64 },  // lease now held by us, fresh acquisition
       Renewed  { term: u64 },  // renewal succeeded
       ForeignHolder,           // server answered: valid foreign ownership
       Conflict,                // 409 on renewal — stale generation only
   }
   ```

   Transport errors stay `Err(kube::Error)`. `ForeignHolder` is
   server-answered truth; `Conflict` proves nothing about ownership.

2. **Budget accounting (pure).** The loop keeps
   `last_success: Option<Instant>` (None until first acquisition). Pure
   helpers:

   - `fn remaining_budget(last_success: Option<Instant>, config: &KubernetesPlatformConfig, now: Instant) -> Option<Duration>`
     — `None` while not leading; while leading,
     `Some(renew_deadline.saturating_sub(elapsed))`.
   - `fn budget_exhausted(last_success: Option<Instant>, config, now) -> bool`
     — true while leading with zero remaining budget. The loop checks this
     BEFORE each attempt: exhausted → step down without attempting.

3. **Bounded attempts.** Each reconcile call is wrapped in
   `tokio::time::timeout(budget, reconcile_future)` where budget is the
   remaining budget while leading, and `renew_deadline` for a non-leading
   contender (caps hanging acquire attempts; a contender has nothing to
   fence, so it retries at jittered `retry_period`). A hang while leading
   surfaces as a failed attempt exactly at the budget boundary.

4. **Decision seam (pure, deterministic).** The loop maps
   `Result<ReconcileVerdict, kube::Error>` + timeout outcomes to a
   `CycleOutcome`, then a pure function owns every transition. Jitter is
   injected — the loop pre-computes `retry_sleep = jittered_duration(retry_period, jitter_factor)`
   and passes it in, so `decide` is deterministic:

   ```rust
   enum CycleOutcome {
       Acquired { term: u64 },
       Renewed  { term: u64 },
       Lost,     // ForeignHolder verdict
       Conflict, // 409 verdict
       Failed,   // transport error or bounded-await timeout
   }

   enum CycleAction {
       BecomeLeader   { term: u64, sleep: Duration },
       ContinueLeading { term: Option<u64>, sleep: Duration }, // term = defensive epoch update
       StepDown       { reason: StepDownReason },
       SleepAcquiring { sleep: Duration },
   }

   enum StepDownReason { BudgetExhausted, LostLease }

   fn decide(
       state: &mut LoopState,      // { currently_leader: bool, last_success: Option<Instant> }
       outcome: CycleOutcome,
       config: &KubernetesPlatformConfig,
       retry_sleep: Duration,      // pre-jittered by the loop
       now: Instant,
   ) -> CycleAction
   ```

   Decision table (sleep while leading is ALWAYS
   `min(retry_sleep, remaining_budget)` — a failure at T+9.5s with 2s
   `retry_period` sleeps 0.5s, never fencing past T+10s):

   | state | outcome | action |
   |---|---|---|
   | not leading | Acquired/Renewed | `BecomeLeader{term, sleep: retry_sleep}` |
   | not leading | Lost/Conflict/Failed | `SleepAcquiring{retry_sleep}` |
   | leading | Acquired/Renewed | `ContinueLeading{term: Some, sleep: min(...)}`, budget reset |
   | leading | Conflict/Failed, budget remains | `ContinueLeading{term: None, sleep: min(retry_sleep, remaining)}` — no event, keep leading |
   | leading | Conflict/Failed, budget exhausted | `StepDown{BudgetExhausted}` |
   | leading | Lost | `StepDown{LostLease}` — immediate, no budget grace (server-answered truth) |

   Event emission, epoch storage, and `is_leader` updates stay in the loop
   task, applied from `CycleAction` as data. `BecomeLeader` emits
   `StartedLeading` and stores the term; `StepDown` clears `last_success`,
   emits `StoppedLeading`, and clears `is_leader`; `ContinueLeading` updates
   the epoch defensively when a term is carried.

5. **Removed/changed helpers.** `next_cycle_sleep` is superseded by `decide`;
   its three unit tests are replaced by the decision-table tests below.

## Testing

All scenarios unit-test the pure seam — no cluster, no mock server (names
land verbatim in tasks.md):

- `decide_transient_failure_within_budget_keeps_leading` — Failed at T+2s,
  10s budget → `ContinueLeading`, no event, sleep = jittered retry_period.
- `decide_failure_sleep_capped_by_remaining_budget` — Failed at T+9.5s,
  retry_sleep 2s, 10s budget → sleep is 0.5s.
- `decide_budget_exhaustion_steps_down` — Failed processed at T+10s →
  `StepDown{BudgetExhausted}`.
- `budget_exhausted_true_at_deadline` / `_false_within_budget` — pre-attempt
  check boundaries.
- `decide_conflict_within_budget_keeps_leading` — Conflict at T+2s →
  `ContinueLeading` (409 is not lease loss).
- `decide_lost_steps_down_immediately` — Lost at T+1s →
  `StepDown{LostLease}` with 9s budget remaining.
- `decide_acquired_while_not_leading_becomes_leader` — BecomeLeader carries
  term + event data.
- `decide_renewed_resets_budget` — success at T+4s after failures → budget
  resets from T+4s.
- `bound_attempt_times_out_at_budget` — a never-ready future under
  `timeout(budget, ...)` with paused Tokio time resolves as failed at the
  boundary (injectable helper `bound_attempt` wraps the reconcile call).

## Affected crates

- `camel-platform-kubernetes`: loop rewrite in `platform_service.rs`
  (`start()` spawned task), `ReconcileVerdict` on `reconcile_lease`, new
  `CycleOutcome`/`LoopState`/`CycleAction`/`StepDownReason`/`decide`/
  `remaining_budget`/`budget_exhausted`/`bound_attempt`, `next_cycle_sleep`
  removed. `KubernetesPlatformConfig` and validation unchanged (the
  `renew_deadline < lease_duration` invariant already holds; with holder
  wall-clock Lease timestamps, the ordering guarantee is modulo Kubernetes
  clock skew).
- `camel-master`: no change — it already reacts to `StoppedLeading` by
  draining the delegate within `drain_timeout` (leadership.rs:231-235).
- `docs`: `docs/src/platforms/kubernetes.md` leader-election section gains
  the self-fencing paragraph (budget semantics, flap tolerance, conflict
  semantics).

## Architecture boundaries

Components → Services respected: the change lives entirely inside
`KubernetesLeadershipService`'s spawned loop. `LeadershipService` trait,
`LeadershipEvent` enum, epoch protocol (ADR-0035), and `lease_is_expired`
semantics are untouched. The pure seam keeps policy decisions compilable and
reviewable separately from I/O.

Single-phase: one crate, one loop, one doc page; no milestone grouping.

## Alternatives considered

- **Keep renew_deadline cadence, add timeout only.** Rejected: one renewal
  attempt per lease cycle means a single failure exhausts the budget — flap
  persists in effect; no room for retries within the budget.
- **Step down on first failure (current fast-fail behavior).** Rejected:
  availability churn on transient blips; client-go, etcd, and Java Camel
  `KubernetesClusterView` all use deadline-budget semantics instead of
  single-attempt fencing.
- **Treat renewal 409 as lease loss.** Rejected: a 409 proves only a stale
  resourceVersion — it is not evidence of foreign ownership. Budget grace
  lets the next cycle read the true state; if a takeover really happened,
  the next verdict is `ForeignHolder` → immediate step-down.
- **Partition simulation in k3s tests.** Rejected: testcontainers cannot
  black-hole pod networking reliably; the pure seam makes failure sequences
  unit-testable deterministically, and CI keeps the happy-path lease tests.
- **Watch-based lease observation.** Out of scope: different failure model,
  larger surface; the polling loop with a budget meets the requirement.
