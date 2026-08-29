# Tasks: camel-master-reconcile-hygiene

## Phase 1: supervision-loop behavior (camel-component-master)

### camel-component-master

#### Task 1.1: epoch-idempotent StartedLeading guard

**Files:**
- `crates/components/camel-master/src/consumer.rs` (modified)
- `crates/components/camel-master/src/leadership.rs` (modified)
- `crates/components/camel-master/src/tests.rs` (modified)

**Steps:**
1. Add `epoch: u64` to `DelegateState::Active` (consumer.rs:51-58, alongside
   `run_token`/`handle`/`bridge_handle`).
2. In `reconcile_event`'s `StartedLeading` arm (leadership.rs ~L230), at
   arm entry BEFORE any drain: load
   `let current_epoch = ctx.leader_epoch.load(Ordering::Acquire);` once.
   If the state matches `DelegateState::Active { epoch, .. }` and
   `epoch == current_epoch`, return `Ok(())` early — no drain, no
   recreate, no lifecycle emission. Nothing else changes at arm entry in
   this task (Task 1.2 captures the term-bump local).
3. At the `*state = DelegateState::Active { .. }` write (leadership.rs:300),
   store the epoch already snapshotted for the bridge (`my_epoch`,
   leadership.rs:278) in the new field.
4. In tests.rs, stop `FakeLeadershipService::start` (tests.rs:352) from
   handing a throwaway `Arc::new(AtomicU64::new(1))` to
   `LeadershipHandle::new`: add a `leader_epoch: Arc<AtomicU64>` field
   initialized to 1, pass `Arc::clone` into the handle, and expose
   `fn leader_epoch(&self) -> Arc<AtomicU64>` for bump-from-test access.
5. Rewrite `repeated_identical_delivery_does_not_reemit` (tests.rs:1616):
   Phase A currently WAITS for `create_consumer_calls >= 2` (5s poll) —
   under the guard that never happens and the test hangs. Replace with a
   bounded stability assertion (see Tests).
6. Do NOT touch the budget/backoff logic, the tick arm, or
   `StoppedLeading` handling — that is Tasks 1.2/1.3.

**Tests:** (all under `cargo test -p camel-master --lib`)
- `repeated_identical_delivery_does_not_reemit` (rewritten): FakeLeadershipService initial `StartedLeading`, healthy delegate → baseline one envelope + one `acquired` transition → `leadership.emit(StartedLeading)` → poll 500ms → assert `create_consumer_calls == 1`, lifecycle counters show NO new `stopped`/`started` pair after the baseline, transitions stay 1.
- `term_bump_while_active_reconciles_once`: initial `StartedLeading` (epoch 1, delegate active, baseline `create_consumer_calls == 1`) → `leadership.leader_epoch().store(2, Release)` then `leadership.emit(StartedLeading)` → await `create_consumer_calls == 2` (5s) → assert exactly one `stopped` + one `started` lifecycle observation added after the bump and the next received envelope carries property `x-camel-leader-epoch` = `"2"`.
- Existing edge tests stay green unchanged: `leadership lost` stop/drain tests, `transition_counted_despite_*`, permanent-error fail-fast (real edges still reconcile exactly once).

**Acceptance:**
- `cargo test -p camel-master --lib` green including the two rewritten/new tests above.
- `grep` confirms no remaining `create_consumer_calls >= 2` wait in `repeated_identical_delivery_does_not_reemit`.
- `cargo clippy -p camel-master --all-targets -- -D warnings` exits 0.
- `cargo fmt --check -p camel-master` exits 0.

- [x] 1.1

#### Task 1.2: exact acquisition budget (in-arm counting)

**Files:**
- `crates/components/camel-master/src/leadership.rs` (modified)
- `crates/components/camel-master/src/supervision.rs` (modified)
- `crates/components/camel-master/src/tests.rs` (modified)

**Steps:**
1. Add `attempts: std::sync::atomic::AtomicU32` to `ReconcileContext`
   (leadership.rs; the context is held across `.await`s inside the
   `tokio::spawn`ed supervision task — `Cell<u32>` is `!Sync` and would
   make the future `!Send`; `AtomicU32` with `Relaxed` ordering provides
   the interior mutability; single-task access means no contention).
2. Two insertions in the `StartedLeading` arm. FIRST, at arm entry
   immediately after the Task-1.1 guard check and BEFORE the drain
   (`stop_delegate`, leadership.rs:212-219 — the drain destroys the Active
   state and its epoch, so the mismatch must be captured first):
   `let term_bump = matches!(&*state, DelegateState::Active { .. });`
   (state was Active with a differing epoch). SECOND, the counting block
   immediately before the `create_endpoint` call (leadership.rs:227,
   i.e. after the drain — the consult must follow the drain per the
   no-zombie ordering), in exactly this order: (a) if `term_bump` is
   true, reset `ctx.attempts.swap(0, Relaxed)`; (b)
   read `let count = ctx.attempts.load(Relaxed);` and if
   `!ctx.reconnect.should_retry(count)` return `Ok(())` without creating —
   the consult sees the pre-increment value; (c)
   `ctx.attempts.store(count + 1, Relaxed)` unconditionally (the counter
   counts deliveries); (d) proceed to create. Thread the reconnect policy
   into `ReconcileContext` as a `reconnect: NetworkRetryPolicy` field (the
   supervision loop already owns `reconnect`).
3. supervision.rs: drop the `delegate_attempts` local, the `:179`
   retry-arm increment, and pass `AtomicU32::new(0)` + the policy into the
   context. Keep the snapshot reset (at :76) and the observed-edge reset
   (at :108) — they become `rctx.attempts.swap(0, Relaxed)`.
4. Adjust the retry-arm backoff gate (supervision.rs:170) from
   `delegate_attempts > 0 → delay_for(delegate_attempts - 1)` to
   `rctx.attempts.load(Relaxed) > 1 → delay_for(rctx.attempts.load(Relaxed) - 2)`
   — preserves today's schedule exactly (first retry delay-free).
5. The retry-arm budget check (supervision.rs:161) reads the counter once
   via `let attempts = rctx.attempts.load(Relaxed);` and uses that value
   for both `should_retry` and the `warn!` attempts field.
6. Update the two N+1-ratifying tests (see Tests) and their comments to
   name the old quirk as fixed.

**Tests:** (all under `cargo test -p camel-master --lib`)
- `create_error_endpoint_transient` (updated): `max_attempts = 1` via `build_error_delegate_master_with_metrics` → await task exit → assert `lifecycle.len() == 1` (`create_error`/`transient`), `create_endpoint_calls == 1`. Update its comment: the old two-observation assertion ratified the N+1 snapshot quirk, now fixed.
- `create_error_consumer_transient` (stays green UNCHANGED — `max_attempts = 3`, transient-then-success at the consumer layer, 2× `create_error` + 1 `started`, 3 creates): under in-arm counting consult(0)/consult(1)/consult(2) all pass at max=3, identical behavior; this test is the consumer-layer transient-recovery and backoff-parity regression — do NOT rewrite it.
- `term_bump_at_exhausted_budget_reacquires_fresh` (new, covers the spec scenario): `max_attempts = 1`, healthy first create (budget exhausted at count 1), delegate Active at epoch 1 → `leadership.leader_epoch().store(2, Release)` then `emit(StartedLeading)` (delivery-driven, live delegate) → await `create_consumer_calls == 2` (5s) → assert the leadership task did NOT exit and the next envelope is stamped `x-camel-leader-epoch` = `"2"`.
- `persistent_transient_at_max_two_attempts_exactly_twice` (new): `max_attempts = 2`, transient endpoint error → await task exit → assert exactly two `create_error` observations, `create_endpoint_calls == 2`.
- `exhausted_budget_refuses_duplicate_delivery` (new): `max_attempts = 1`, transient endpoint error, `emit(StartedLeading)` duplicate right after start → await task exit → assert `create_endpoint_calls == 1` and `lifecycle.len() == 1` (the duplicate counted but performed no create).
- `disabled_policy_creates_nothing` (new): construct `MasterConsumer` directly with a reconnect policy `NetworkRetryPolicy { enabled: false, ..NetworkRetryPolicy::default() }` (do NOT extend `build_error_delegate_master_with_metrics`) → await task exit at first tick → assert `create_endpoint_calls == 0`, `create_consumer_calls == 0`, zero lifecycle observations.
- `unlimited_default_keeps_retrying` (new): `max_attempts = 0`, transient endpoint error → poll until ≥ 3 `create_error` observations → `sleep(Duration::from_secs(2))` then `assert!(!master.leadership_task.as_ref().is_some_and(|h| h.is_finished()))` (field is pub(crate), same crate) — the task must still be retrying.

**Acceptance:**
- `cargo test -p camel-master --lib` green, all seven listed tests included (six new/updated plus the unchanged `create_error_consumer_transient`).
- `grep -n 'delegate_attempts' crates/components/camel-master/src/supervision.rs` returns no hits.
- `cargo clippy -p camel-master --all-targets -- -D warnings` exits 0; `cargo fmt --check -p camel-master` exits 0.

- [x] 1.2

#### Task 1.3: tick-driven stale-stamp detection with pre-teardown ordering

**Files:**
- `crates/components/camel-master/src/supervision.rs` (modified)
- `crates/components/camel-master/src/tests.rs` (modified)

**Steps:**
1. In the retry-tick arm (supervision.rs:130-206), insert BEFORE the
   finished-handle teardown branch (:131): if `is_leading` and the state
   matches `DelegateState::Active { epoch, .. }` with
   `epoch != rctx.leader_epoch.load(Ordering::Acquire)`, dispatch
   `reconcile_event(StartedLeading, &mut state, &rctx)` — the Task-1.1/1.2
   guard+reset path drains (subsumes the finished-handle teardown for this
   tick), recreates, and restamps. Log at `debug!` (lock, old epoch, new
   epoch) before dispatching.
2. Keep the existing finished-handle teardown and Inactive-acquisition
   branches unchanged and in order AFTER the new branch (tick sequence:
   stale-stamp dispatch → finished teardown → Inactive retry).
3. Do not compare the epoch anywhere else; the dispatch condition is
   stamp ≠ published only.

**Tests:** (all under `cargo test -p camel-master --lib`)
- `tick_renews_epoch_advance_restamps_without_delivery` (new): initial `StartedLeading` (epoch 1, delegate active, baseline 1 create) → `leadership.leader_epoch().store(2, Release)` with NO `emit` → await `create_consumer_calls == 2` (5s) → assert the next received envelope carries `x-camel-leader-epoch` = `"2"` and exactly one `stopped`+`started` pair was added.
- `dead_delegate_stale_stamp_resets_budget` (new): `max_attempts = 1`, healthy create (budget exhausted at count 1), first delegate exits on a one-shot signal ONLY (extend the fake with an exit-on-signal knob scoped to create ordinal 1 — e.g. a `watch::Receiver<()>` the first `FakeDelegateConsumer` awaits; delegate #2 must keep running, otherwise the next tick tears it down and the Inactive consult at count ≥ 2 > max stops the consumer and fails the test). Test order to avoid the tick race: `leader_epoch().store(2, Release)` FIRST, then signal the first delegate to exit, then await `create_consumer_calls == 2` (5s) → assert the leadership task did NOT exit and the next envelope is stamped `"2"`.

**Acceptance:**
- `cargo test -p camel-master --lib` green including both new tests.
- The stale-stamp branch textually precedes the finished-handle branch in supervision.rs (reviewable in diff).
- `cargo clippy -p camel-master --all-targets -- -D warnings` and `cargo fmt --check -p camel-master` exit 0.
- `cargo test -p camel-test --test master_noop_test` green.

- [x] 1.3

## Phase 2: platform validation + docs

### camel-platform-kubernetes

#### Task 2.1: renewal-slack validation rule

**Files:**
- `crates/platforms/camel-platform-kubernetes/src/platform_service.rs` (modified)

**Steps:**
1. In `KubernetesPlatformConfig::validate` (platform_service.rs:50-63),
   add a third rule after the two ordering rules: if
   `self.lease_duration - self.renew_deadline < self.retry_period`,
   return `Err(PlatformError::Config(format!("lease_duration ({:?}) minus renew_deadline ({:?}) must be >= retry_period ({:?}) to leave one retry window of renewal slack", self.lease_duration, self.renew_deadline, self.retry_period)))`.
2. Update the `validate` doc comment (platform_service.rs:45-49) to state
   the slack rule alongside the two ordering rules.
3. Add the three unit tests below in the existing tests module of the same
   file, next to the current validate tests.

**Tests:** (under `cargo test -p camel-platform-kubernetes --lib`)
- `validate_defaults_pass`: `KubernetesPlatformConfig` defaults (15s/10s/2s, slack 5s) → `validate()` → `Ok(())`.
- `validate_insufficient_slack_rejected`: lease 12s, renew 11s, retry 2s (slack 1s) → `validate()` → `Err` whose message contains "lease_duration", "renew_deadline", and "retry_period".
- `validate_slack_equal_to_retry_period_passes`: lease 12s, renew 10s, retry 2s → `validate()` → `Ok(())`.

**Acceptance:**
- `cargo test -p camel-platform-kubernetes --lib` green including the three new tests.
- `cargo clippy -p camel-platform-kubernetes --all-targets -- -D warnings` exits 0; `cargo fmt --check -p camel-platform-kubernetes` exits 0.

- [x] 2.1

### camel-component-master (docs)

#### Task 2.2: operator docs — drain/fencing, lock sharing, stale default

**Files:**
- `crates/components/camel-master/README.md` (modified)
- `crates/components/camel-master/src/config.rs` (modified)
- `crates/components/camel-master/src/leadership.rs` (modified)

**Steps:**
1. README "How It Works" (after the drain_timeout sentence at README:78):
   add a short paragraph — delegate drain runs after leadership is already
   lost, so it can overlap a successor's lease; the `x-camel-leader-epoch`
   fencing token enables split-brain safety when a sink opts into
   rejecting stale epochs (enforcement is opt-in per sink, ADR-0035), not
   drain-before-takeover ordering.
2. Same README: add a "Lock sharing" note, per backend — on Kubernetes,
   same-lock routes share one cached elector: mutual exclusion is
   per-process, so when this process leads, both routes' delegates run
   and only one process holds the lease; the default Noop platform
   reports every route as leader and provides no cross-route or
   cross-process exclusion.
3. README options table (README:48): fix the stale default for
   `delegate_retry_max_attempts` from `30` to `0` (`0` = unlimited;
   real default per `master_reconnect_default`, config.rs:31-35).
4. config.rs `drain_timeout_ms` field doc (config.rs:50-51): append one
   sentence cross-referencing the README drain/lease-overlap paragraph.
5. leadership.rs:87: fix the stale citation `specs/master-component/spec.md`
   → `openspec/specs/master-component/spec.md`.

**Tests:** (non-Rust — verification commands)
- `grep -n "x-camel-leader-epoch" crates/components/camel-master/README.md` returns ≥ 1 hit.
- `grep -n "share lock" crates/components/camel-master/README.md` returns 1 hit.
- `grep -n "| \`delegate_retry_max_attempts\`" crates/components/camel-master/README.md` shows the default column as `0`.
- `cargo xtask lint-context-citations` exits 0 (validates the fixed spec path).

**Acceptance:**
- All three grep checks above pass, plus `cargo xtask lint-context-citations` exiting 0 (fourth check).
- `cargo fmt --check -p camel-master` and `cargo clippy -p camel-master --all-targets -- -D warnings` still exit 0 (comment-only change).

- [x] 2.2
