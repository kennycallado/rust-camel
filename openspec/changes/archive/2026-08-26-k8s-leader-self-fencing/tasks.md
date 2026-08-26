# Tasks: k8s-leader-self-fencing

## camel-platform-kubernetes

### Task 1.1: ReconcileVerdict type and reconcile_lease migration

**Files:**
- `crates/platforms/camel-platform-kubernetes/src/leadership_fsm.rs` (new)
- `crates/platforms/camel-platform-kubernetes/src/lib.rs` (modified — add
  `pub(crate) mod leadership_fsm;`)
- `crates/platforms/camel-platform-kubernetes/src/platform_service.rs` (modified)

**Steps:**
1. Create `src/leadership_fsm.rs` — the pure leadership decision module. Its
   first item is the verdict enum:
   `pub(crate) enum ReconcileVerdict { Acquired { term: u64 }, Renewed { term: Option<u64> }, ForeignHolder, Conflict }`
   with doc comments: `Acquired` — lease now held by us via create or expiry
   takeover, `term` never 0 (fallback 1 applied at the return site); `Renewed`
   — renewal succeeded, `None` means the server stripped the leader-term
   annotation (caller keeps the current epoch); `ForeignHolder` — server
   answered, a valid foreign holder owns the lease; `Conflict` —
   optimistic-concurrency 409, proves only a stale generation.
   NOTE for the implementer: this refines design.md §1's sketch (`Renewed`
   carries `Option<u64>`) to preserve today's only-if-Some defensive epoch
   update; the decision-table semantics are unchanged.
   Register the module in `lib.rs` as `pub(crate) mod leadership_fsm;` (the
   decision seam grows in Task 1.2; the loop consumes it in Task 1.3).
2. In `platform_service.rs`, change `reconcile_lease` to return
   `Result<leadership_fsm::ReconcileVerdict, kube::Error>` (same parameters)
   and map the seven return sites:
   - create `Ok` (line ~569) → `Ok(ReconcileVerdict::Acquired { term: extract_leader_term(&created).unwrap_or(1) })`
   - create 409 (line ~573) → `Ok(ReconcileVerdict::Conflict)`
   - renew `Ok` (line ~601) → `Ok(ReconcileVerdict::Renewed { term: extract_leader_term(&replaced) })`
   - renew 409 (line ~604) → `Ok(ReconcileVerdict::Conflict)`
   - takeover `Ok` (line ~648) → `Ok(ReconcileVerdict::Acquired { term: extract_leader_term(&replaced).unwrap_or(1) })`
   - takeover 409 (line ~651) → `Ok(ReconcileVerdict::Conflict)`
   - fall-through (line ~657, lease exists, not ours, not expired) →
     `Ok(ReconcileVerdict::ForeignHolder)`
3. Adapt the single call site in the `start()` spawned loop
   (lines ~242-255) BEHAVIOR-PRESERVING so the crate compiles green at this
   task boundary: map the verdict back to the old `(bool, Option<u64>)`
   shape — `Acquired{term}|Renewed{term}` → `(true, Some(term))` for
   Acquired, `(true, term)` for Renewed; `ForeignHolder|Conflict` →
   `(false, None)` — and leave all downstream loop logic untouched. The
   semantic rewrite is Task 1.3; this task only introduces the type.
4. Delete no tests; existing tests must pass unchanged.

**Tests:**
- Existing suite green (verdict mapping is exercised by compilation and the
  unchanged loop behavior; unit coverage of verdict semantics lands with the
  decision seam in Task 1.2).

Command: `cargo test -p camel-platform-kubernetes --lib` — expected: green
before and after (refactor-only task); red would indicate a mapping error.

**Acceptance:**
- `cargo test -p camel-platform-kubernetes --lib` exits 0.
- `cargo clippy -p camel-platform-kubernetes -- -D warnings` exits 0
  (`ReconcileVerdict` is consumed by `reconcile_lease`, so no dead_code).
- `rg -n '\(false, None\)' crates/platforms/camel-platform-kubernetes/src/platform_service.rs`
  shows no matches inside `reconcile_lease` (all seven sites now return
  verdicts).

- [x] 1.1

### Task 1.2: Pure decision seam — budget helpers, decide, bound_attempt

**Files:**
- `crates/platforms/camel-platform-kubernetes/src/leadership_fsm.rs` (modified — grows the seam below `ReconcileVerdict`)
- `crates/platforms/camel-platform-kubernetes/Cargo.toml` (modified — dev-dependencies)

NOTE: the seam items are unused by non-test code until Task 1.3 rewires the
loop, so `cargo clippy -D warnings` would fail on dead_code at this boundary.
Tasks 1.2 and 1.3 land in ONE commit; this task's acceptance gate is the
test run only. The clippy gate runs at the Task 1.3 boundary.

**Steps:** (all items below go into `leadership_fsm.rs`, `pub(crate)`, with
their tests in that module's `#[cfg(test)]` block; `ReconcileVerdict` from
Task 1.1 is already there)
0. In `Cargo.toml` `[dev-dependencies]`, change the tokio entry to
   `tokio = { workspace = true, features = ["macros", "rt-multi-thread", "test-util"] }`
   — `#[tokio::test(start_paused = true)]` requires the `test-util` feature.
1. Add `pub(crate) struct LoopState { pub(crate) currently_leader: bool, pub(crate) last_success: Option<std::time::Instant> }`.
2. Add `fn remaining_budget(last_success: Option<std::time::Instant>, config: &KubernetesPlatformConfig, now: std::time::Instant) -> Option<Duration>`
   — `None` when `last_success` is `None` (not leading); else
   `Some(config.renew_deadline.saturating_sub(now - last_success))`.
3. Add `fn budget_exhausted(last_success: Option<std::time::Instant>, config: &KubernetesPlatformConfig, now: std::time::Instant) -> bool`
   — true iff `remaining_budget` with those arguments returns
   `Some(Duration::ZERO)`.
4. Add
   `enum StepDownReason { BudgetExhausted, LostLease }` and
   `enum CycleAction { BecomeLeader { term: u64, sleep: Duration }, ContinueLeading { term: Option<u64>, sleep: Duration }, StepDown { reason: StepDownReason }, SleepAcquiring { sleep: Duration } }`
   with doc comments stating which side effects each action implies
   (BecomeLeader: store epoch, set is_leader, emit StartedLeading, set
   last_success; ContinueLeading: defensive epoch update when term is Some;
   StepDown: clear is_leader, emit StoppedLeading, clear last_success;
   SleepAcquiring: none).
5. Add `enum CycleOutcome { Acquired { term: u64 }, Renewed { term: Option<u64> }, Lost, Conflict, Failed }`.
6. Add the pure decision function:
   `fn decide(state: &mut LoopState, outcome: CycleOutcome, config: &KubernetesPlatformConfig, retry_sleep: Duration, now: std::time::Instant) -> CycleAction`
   implementing the blessed decision table:
   - not leading + Acquired/Renewed → `BecomeLeader { term: <u64 from outcome; Renewed uses term.unwrap_or(1)>, sleep: retry_sleep }`, set `currently_leader = true`, `last_success = Some(now)`
   - not leading + Lost/Conflict/Failed → `SleepAcquiring { sleep: retry_sleep }`
   - leading + Acquired/Renewed → `ContinueLeading { term: <Some when outcome carries one>, sleep: min(retry_sleep, remaining) }`, `last_success = Some(now)`
   - leading + Conflict/Failed with remaining budget > 0 → `ContinueLeading { term: None, sleep: min(retry_sleep, remaining) }` (no event)
   - leading + Conflict/Failed with budget exhausted → `StepDown { BudgetExhausted }`, set `currently_leader = false`, `last_success = None`
   - leading + Lost → `StepDown { LostLease }`, same state clears
   (`min` is `Duration::min`; remaining computed via `remaining_budget` from
   the PRE-update `last_success` for failure rows, from the new success for
   success rows).
7. Add the bounded-attempt helper:
   `async fn bound_attempt<F: Future<Output = Result<ReconcileVerdict, kube::Error>>>(fut: F, budget: Duration) -> Result<ReconcileVerdict, AttemptFailure>`
   with `enum AttemptFailure { Transport(kube::Error), Deadline }` — wraps
   `tokio::time::timeout(budget, fut)`; inner `Ok` passes through, inner
   `Err(e)` → `Transport(e)`, elapsed → `Deadline`.

**Tests:** (in the existing `#[cfg(test)]` module; `use std::time::{Duration, Instant};`,
config fixture `KubernetesPlatformConfig` with `renew_deadline = 10s`,
`retry_period = 2s`; construct `Instant`s via
`Instant::now().checked_add/sub` or tokio's paused clock is NOT needed here —
`decide` takes `now: Instant` as data)
- `decide_transient_failure_within_budget_keeps_leading`: state leading,
  last_success = now-2s, outcome Failed, retry_sleep 2s →
  `ContinueLeading { term: None, sleep: 2s }`, still leading.
- `decide_failure_sleep_capped_by_remaining_budget`: state leading,
  last_success = now-9.5s, outcome Failed, retry_sleep 2s →
  `ContinueLeading { sleep: 0.5s }` (capped, not 2s).
- `decide_budget_exhaustion_steps_down`: state leading, last_success =
  now-10s, outcome Failed → `StepDown { reason: BudgetExhausted }`, state
  cleared.
- `budget_exhausted_true_at_deadline`: last_success = now-10s → true.
- `budget_exhausted_false_within_budget`: last_success = now-2s → false.
- `budget_exhausted_false_when_never_led`: last_success None → false.
- `decide_conflict_within_budget_keeps_leading`: state leading, last_success
  = now-2s, outcome Conflict → `ContinueLeading { term: None }` (grace, not
  Lost).
- `decide_lost_steps_down_immediately`: state leading, last_success = now-1s
  (9s budget remaining), outcome Lost → `StepDown { reason: LostLease }`.
- `decide_acquired_while_not_leading_becomes_leader`: state not leading,
  outcome Acquired { term: 3 } → `BecomeLeader { term: 3, sleep: retry_sleep }`,
  state now leading with last_success set.
- `decide_renewed_resets_budget`: state leading, last_success = now-9s,
  outcome Renewed { term: Some(5) } → `ContinueLeading { term: Some(5), sleep: 2s }`
  (success-row sleep is the jittered retry_period — the
  cadence-between-renewals scenario) and a subsequent
  `budget_exhausted(now+9s)` with the updated state is false (budget measured
  from the new success).
- `bound_attempt_times_out_at_budget`: `#[tokio::test(start_paused = true)]`,
  `bound_attempt(std::future::pending(), Duration::from_secs(10)).await` →
  `Err(AttemptFailure::Deadline)` and, with the paused clock, assert
  `tokio::time::Instant::now() - start == Duration::from_secs(10)` (captured
  `start` before the call) — the timeout auto-fires exactly at the budget
  boundary without manual advancement.
- `bound_attempt_passes_transport_error`: construct the same `kube::Error`
  the existing test `conflict_classification_is_explicit_for_409_api_errors`
  builds (`kube::Error::Api(Box::new(Status { code: 409, reason:
  "Conflict", message: "conflict", .. }))`) and assert
  `bound_attempt(async { Err(that_err) }, Duration::from_secs(10)).await` →
  `Err(AttemptFailure::Transport(_))`.

Command: `cargo test -p camel-platform-kubernetes --lib leadership_fsm` —
expected: red before implementation (types absent → compile failure), green
after.

**Acceptance:**
- `cargo test -p camel-platform-kubernetes --lib leadership_fsm` exits 0 with
  the 12 new tests passing.
- No clippy gate at this boundary (see NOTE above — clippy runs at Task 1.3).

- [x] 1.2

### Task 1.3: Rewrite the start() loop around the decision seam

**Files:**
- `crates/platforms/camel-platform-kubernetes/src/platform_service.rs` (modified)

**Steps:**
1. Import the seam items from `crate::leadership_fsm` (`LoopState`,
   `remaining_budget`, `budget_exhausted`, `CycleOutcome`, `CycleAction`,
   `StepDownReason`, `ReconcileVerdict`, `bound_attempt`, `AttemptFailure`)
   and replace the body of the spawned loop (current lines ~234-306) with:
   a. `let mut state = LoopState { currently_leader: false, last_success: None };`
   b. Each cycle: if `cancel_task.is_cancelled()` → cancelled = true, break
      (unchanged).
   c. Compute `retry_sleep = jittered_duration(config.retry_period, config.jitter_factor)`.
   d. Pre-attempt fence: `if budget_exhausted(state.last_success, &config, Instant::now())`
      → apply `StepDown { BudgetExhausted }` side effects via the same
      applier (step g) AND clear the loop state
      (`state.currently_leader = false; state.last_success = None;` — decide()
      owns state clears on the outcome path; on the pre-attempt fence path the
      applier must do it, otherwise budget_exhausted stays true forever and
      the pod never re-enters the contender path), then skip the attempt
      (sleep `retry_sleep`, contender path).
   e. Attempt budget = `remaining_budget(state.last_success, &config, now).unwrap_or(config.renew_deadline)`
      (contender cap).
   f. `let attempt = bound_attempt(reconcile_lease(&leases, &lease_name, &config, &holder_identity), budget).await;`
      map: `Ok(ReconcileVerdict::Acquired{term})` → `CycleOutcome::Acquired{term}`;
      `Ok(Renewed{term})` → `Renewed{term}`; `Ok(ForeignHolder)` → `Lost`;
      `Ok(Conflict)` → `Conflict`; `Err(Transport(_)|Deadline)` → `Failed`
      (keep the existing `warn!` "leader election cycle failed" for Transport
      and Deadline).
   g. `let action = decide(&mut state, outcome, &config, retry_sleep, Instant::now());`
      then apply side effects from `CycleAction`:
      `BecomeLeader{term,..}` → `is_leader_task.store(true)`,
      `leader_epoch_task.store(term)`, send
      `LeadershipEvent::StartedLeading` (keep the existing debug! log);
      `ContinueLeading{term: Some(t),..}` → defensive epoch update (only if
      `t != current`); `StepDown{..}` → `is_leader_task.store(false)`, send
      `LeadershipEvent::StoppedLeading`; `SleepAcquiring` → nothing.
   h. Sleep: `BecomeLeader`, `ContinueLeading`, and `SleepAcquiring` use
      their carried `sleep`; `StepDown` (from decide or the pre-attempt
      fence) sleeps `retry_sleep` before re-entering the contender path. All
      sleeps run inside the existing `tokio::select!` on
      `cancel_task.cancelled()`.
2. Keep the shutdown path after the loop unchanged: on non-cancelled
   termination log the existing error!; on break while `state.currently_leader`
   → `release_lease` attempt + `is_leader=false` + final `StoppedLeading`
   (graceful shutdown release; self-fence step-down does NOT call
   release_lease — the partitioned holder cannot reach the server, and a
   fenced holder re-enters the contender path).
3. Delete `next_cycle_sleep` and its three tests
   (`next_cycle_sleep_subtracts_elapsed_when_leading`,
   `next_cycle_sleep_zero_when_elapsed_exceeds_deadline`,
   `next_cycle_sleep_uses_retry_period_when_not_leading`) — superseded by the
   Task 1.2 decision tests (design.md §5).
4. The `#[allow(unused_assignments)]` on `cancelled` stays only if still
   needed; remove it if the rewrite makes it dead.

**Tests:**
- No new unit tests (loop wiring is I/O-bound; decision coverage lives in
  Task 1.2; the k3s suite exercises the loop live in CI). Compile + existing
  suite green is this task's gate.

Command: `cargo test -p camel-platform-kubernetes` — expected: green after
the rewrite (all targets: lib tests including the twelve from Task 1.2, minus
the three deleted `next_cycle_sleep` tests, plus `tests/wiring_test.rs` which
exercises `start()`/handle wiring without a cluster).

**Acceptance:**
- `cargo test -p camel-platform-kubernetes` exits 0 (all targets).
- `cargo clippy -p camel-platform-kubernetes --all-targets -- -D warnings` exits 0
  (covers Tasks 1.2+1.3 jointly — the seam is now consumed).
- `cargo fmt --check` on the crate exits 0.
- `rg -n 'next_cycle_sleep' crates/` returns no matches.
- `cargo check -p camel-platform-kubernetes-example` exits 0 (downstream
  compile).

- [x] 1.3

## docs

### Task 1.4: Document self-fencing semantics

**Files:**
- `docs/src/platforms/kubernetes.md` (modified)

**Steps:**
1. Extend the "## Leader election" section (after the numbered loop
   description) with a "### Self-fencing" subsection stating: every renew
   attempt is bounded by the remaining renewal budget
   (`renew_deadline` since the last successful renewal); a leader that cannot
   renew within the budget steps down on its own — it emits
   `StoppedLeading` and stops its delegate — independent of the Lease state
   it observes; transient failures and optimistic-concurrency conflicts
   within the budget keep the leader in place (no step-down, retry at
   jittered `retry_period` cadence); the `renew_deadline < lease_duration`
   invariant means the holder fences itself before the lease can legally
   expire for peers, modulo Kubernetes clock skew on Lease timestamps.
2. STE style: short sentences, active voice, no fluff; match the section's
   existing formatting.

**Tests:**
- `docs_self_fencing_terms_present`: each command exits 0 —
  `rg -qF 'renewal budget' docs/src/platforms/kubernetes.md`;
  `rg -qF 'renew_deadline' docs/src/platforms/kubernetes.md`;
  `rg -qF 'retry_period' docs/src/platforms/kubernetes.md`;
  `rg -qF 'steps down' docs/src/platforms/kubernetes.md`;
  `rg -qF 'clock skew' docs/src/platforms/kubernetes.md`;
  `rg -qF 'optimistic-concurrency' docs/src/platforms/kubernetes.md`.
  Expected: red before the edit, green after.
- `cargo xtask lint-context-citations` exits 0.
- `nix shell nixpkgs#mdbook -c mdbook build docs` exits 0.
- `nix shell nixpkgs#mdbook -c mdbook test docs` exits 0.

**Acceptance:**
- All six `rg -qF` commands exit 0.
- `cargo xtask lint-context-citations` exits 0; mdbook build and mdbook test
  exit 0.

- [x] 1.4
