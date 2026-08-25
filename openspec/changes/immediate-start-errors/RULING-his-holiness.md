# His Holiness — Definitive Ruling: immediate-start-errors (rc-slvd)

**Status:** FINAL. Architecture converges here. Plan edits only.
**Verified against code** (not the summary): `runtime_bus.rs:179`, `commands.rs:262/389/526/574`, `route_runtime.rs:189/208/227`, `route_controller_trait.rs:220/566`, `runtime_execution.rs:34`, `route_helpers.rs:162/284`.

---

## 1. The Round-9 defect is REAL (confirmed independently)

The version machine, verified line-by-line:

- `begin_start()` bumps version → **Starting@V+1** (Phase 1 persist).
- `confirm_start()` **does NOT bump version** (route_runtime.rs:208-222). Phase 2a persists Started with `save_if_version(expected = V+1)`.
- `fail()` bumps version → **Failed@V+2**.
- `RuntimeBus::execute` is **not serialized across commands** — a detached watcher's `FailRoute` re-loads the aggregate and runs its own optimistic `save_if_version`.

**Hazard (Timeline B):** watcher `FailRoute` lands between Phase 2 (side effect done, err-latch set) and Phase 2a. It loads Starting@V+1, writes Failed@V+2. Then Phase 2a's `confirm_start` persist does `save_if_version(expected=V+1)` against a store now at V+2 → **conflict → `start_route` returns Err** → violates the spec's "lifecycle op SHALL return Ok". Same-id retries return `Duplicate` and cannot repair it. **Round 9 is correct.**

The proposed timing fixes ("prompt Err is often before the side-effect future returns") do not help: the race is on the **durable version**, not wall-clock. Deterministic ordering — not timing — is required. Correct diagnosis.

---

## 2. CHOSEN ARCHITECTURE — Option 3: make Phase 2a tolerate a terminal-Failed supersede

**Do NOT reorder via a post-confirm hook. Fix the victim, not the racer.**

Keep the async detached watcher exactly where the current design puts it (spawned inside the controller trait `start_route`/`resume_route`/aggregate path, after `await_startup`, never awaited). The watcher's `FailRoute` semantics are unchanged. The **only** change is in `handle_lifecycle_start` Phase 2a (commands.rs:525-566):

> When the `confirm_start` persist (`save_if_version` / `uow.persist_upsert`) returns an **optimistic-lock conflict**, RE-LOAD the aggregate. If the reloaded state is **terminal `Failed`**, the watcher has legitimately superseded this start: treat Phase 2a as a **no-op success** and return `Ok(RouteStateChanged { status: "Failed" })` (or a Started-superseded marker — see §5). If the reloaded state is anything else (e.g. a concurrent Stop/Suspend), fall through to the **existing** Err path unchanged.

### Why this is the correct ruling

1. **Deterministic, not timing.** The conflict is the signal. The discriminator (re-load → is it `Failed`?) is a total function over aggregate state, not a race window.
2. **Zero signature churn.** No change to `RuntimeExecutionPort::start_route` (stays `Result<(), DomainError>`), no change to the controller trait, the adapter, the aggregate path, or **any** of the ~15 `RuntimeExecutionPort`/repo test mocks. Blast radius = one function in `commands.rs`.
3. **No new hazard.** Option 1's post-confirm hook must fire the watcher even on the Phase-2a **Err** path (consumer already spawned) — and there the aggregate may be in an unexpected state for an unrelated reason, exactly the hazard your Question 1 raised. Option 3 has no such branch: the watcher is already running; Phase 2a merely *reads* the outcome it produced.
4. **Program-order guarantee preserved where it matters.** The route reaches `Failed` durably (by the watcher) OR `Started` durably (by Phase 2a) — never a lost update. The conflict path proves the watcher won; we honor it.
5. **`start_route` returns Ok** in the fire-and-forget contract sense: the operation was accepted and its side effect ran; the terminal outcome (Failed) is reflected in the projection, which is precisely what the spec's "eventually becomes Failed" scenario asserts.

### The concrete ordering guarantee (state this in design.md)

> `handle_lifecycle_start` and the detached watcher are concurrent writers to one optimistically-versioned aggregate. Exactly one of two durable outcomes obtains: **Started@V+1** (Phase 2a committed first; watcher's later `FailRoute` legally transitions Started→Failed@V+2 — the intended "fails loudly" result) or **Failed@V+2** (watcher committed first; Phase 2a's version conflict is caught, the reloaded terminal-Failed state is recognized as a supersede, and the operation returns Ok). The optimistic lock makes lost updates impossible; the re-load-on-conflict makes the supersede observable. No command is deferred, no actor yields, no port signature changes.

---

## 3. PHASE DECOMPOSITION — verdict: **SINGLE PHASE.**

Reject the split. The proposed Phase 1 (observability floor only: `error!` + CrashNotification, no lifecycle change) delivers **nothing the code does not already have** — verified: `spawn_consumer_task` already emits the `error!` (consumer_management.rs:314-317) and already sends `CrashNotification` (320-330) and already calls `publish_runtime_failure` (332). The observability floor is **present in-tree today**. A "Phase 1" that re-asserts it is a no-op PR.

The entire remaining value — and the entire difficulty — is the Failed transition + the ordering fix + the reentrancy non-regression. That is one indivisible correctness contract; splitting it ships a half-fix whose first half is already merged. The design.md is right: "Single-phase." Keep it.

(If the workflow needs task *grouping* inside the single change, group as: (A) watcher + latches wiring, (B) Phase-2a supersede tolerance, (C) tests. But that is task ordering within one phase, not a multi-phase delivery.)

---

## 4. SPEC VERDICT (hash 53c683f4)

The 2 Requirements / 10 scenarios **largely survive**, but three deltas are required to match Option 3 (they currently presume the *watcher-side* command must not conflict, which was the Option-1/2 framing):

- **Requirement 1** — keep as-is. "SHALL return Ok without waiting" is satisfied.
- **Requirement 2 ("Failure watcher retries projection synchronization")** — **reword**. The retry/sole-publisher machinery was scaffolding to survive the confirm race *on the watcher side*. Under Option 3 the confirm race is absorbed by Phase 2a, so the watcher's `FailRoute` no longer needs the "raced a pending confirmation → retry with same command_id" clause as a *correctness* mechanism. Retain **at most one** bounded retry as defense-in-depth against transient persist errors, but drop the normative "3 attempts to defeat the confirmation race" language — that race no longer reaches the watcher. Sole-publisher (one command_id, one executor) stays.
- **Scenario "Watcher retry after startup confirmation race"** — **rewrite** to target Phase 2a: "GIVEN a watcher FailRoute that commits before Phase 2a / WHEN Phase 2a's confirm persist conflicts / THEN Phase 2a recognizes the terminal-Failed supersede and the operation returns Ok and the route is Failed." This is the scenario that actually exercises the fix.
- **Scenario "No startup reentrancy regression"** — keep verbatim; the two-way barrier is exactly right and non-negotiable. This is the scenario that killed Design 1; it must stay as the regression gate.
- Add **one** scenario: "Phase 2a supersede is not swallowed for non-Failed conflicts" — GIVEN a concurrent Stop causes the Phase 2a conflict / THEN the operation returns Err (existing behavior), proving the discriminator is narrow.

---

## 5. Implementation notes (binding)

1. **Discriminator is terminal-Failed only.** On Phase 2a conflict, re-load; match `RouteRuntimeState::Failed(_)` → supersede-Ok. Any other state → existing Err. Do not broaden.
2. **UoW path parity.** The `deps.uow` branch (commands.rs:528-535) and the `deps.repo` branch (536-565) both need the conflict→reload→supersede check. `persist_upsert` conflict surfaces as the same optimistic-lock `Err`; handle both.
3. **Return value on supersede:** return `Ok(RuntimeCommandResult::RouteStateChanged { route_id, status: "Failed" })`. The caller (`CamelContext::start`) must not fail-fast on it — it is Ok. Confirm the start-loop treats status="Failed" as non-fatal (it already continues on Ok).
4. **E2E observability (Round 9 Important):** correct — `inferred_lifecycle_label` (route_helpers.rs:162-171) cannot emit Failed. The test MUST register+start via the real `RuntimeBus` and poll `ask(RouteStatus)` to `Failed` with a 2s bound. Do not assert through handle-liveness.
5. **Watcher spawn site:** unchanged from the async-watcher design — detached, inside the trait start/resume/aggregate paths after `await_startup`. `start_invoked` oneshot + grace-gating retained (prevents grace elapsing before `start()` runs). Sole-publisher retained.
6. **Delete** the in-tree **Design 1** synchronous grace: `ImmediateLatches::wait`'s use inside `await_immediate_startup` must return to a pre-resolved handshake so the actor never yields on Immediate (the reentrancy fix). The latches struct is retained but consumed only by the detached watcher.

---

## One-paragraph verdict

Round 9's diagnosis is correct and the post-confirm hook (Option 1) would work but pays for it with a `RuntimeExecutionPort` signature change, all-mocks churn, and a genuinely new conflict-path hazard. The simpler architecture you could not see is to **stop trying to make the watcher's write win the race, and instead make the loser (Phase 2a `confirm_start`) recognize when it has been legitimately superseded**: on optimistic-lock conflict, re-load and, if the state is terminal `Failed`, return Ok. Localized to `handle_lifecycle_start`, deterministic, no port changes, no reentrancy risk. Single phase. Keep the reentrancy barrier scenario verbatim; reword Requirement 2's retry rationale and rewrite the confirmation-race scenario to target Phase 2a. Ship it.
