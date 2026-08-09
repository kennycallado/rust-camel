# Bless verdict: audit-fix-async-lifecycle

**Reviewer:** e_opus (senior Rust architect, escalated)
**Skill:** self-grill-proposals
**Round:** 2 (re-grill after C1/C2/I1/I2/M1/M2 fixes)
**Artifacts (SHA-256 verified):**
- proposal.md `d4f0e9a3553b875b663319bd03e1813af1eedba9668f03eb38d3d4ed905dcd59`
- design.md `7797d1004e1cb011e9de1298a8fe0dbc123e455fdb9e26051cac7b5696de4e2a`
- spec.md `8c7d6a82c180ea485cd7efe2e9b51adf2e123057ff7d9390521a91051699d43e`

---

## VERDICT: BLESS

All six round-1 findings are resolved. Each fix was re-verified against
source, not just against the edit description. Both `JoinHandle` payload
shapes (health `JoinHandle<()>`, master `JoinHandle<Result<(), CamelError>>`)
are now correctly distinguished, and the two auth hazards (lock-scope race,
capacity-gated starvation) are closed with explicit invariants. The change
is ready for tasks.md authoring and plan-bless.

---

## Finding resolution

### C1 — Fix 1 health match nesting — RESOLVED

design.md:22-28 now prescribes **three** arms matching the actual
`JoinHandle<()>` shape: `Ok(Ok(()))` (clean join), `Ok(Err(join_err))`
(JoinError, logged at `error!`), `Err(_)` (timeout → `abort()` + await).
Source confirmed: the spawned closure (server.rs:111-124) returns `()` — the
axum serve error is logged *inside* the task and never escapes — so
`timeout(dur, handle).await` yields `Result<Result<(), JoinError>, Elapsed>`,
two levels. The former three-level `Ok(Ok(Ok(())))` compile error is gone.
(`crates/camel-health/src/server.rs:111-124`, design.md:22-28)

### C2 — spec bridge-drain over-specification — RESOLVED

spec.md:74-76 now reads: the epoch-bridge is "drained within its own
`drain_timeout` window (or aborted if that window elapses)". This decouples
the bridge drain from the delegate's timeout budget. Source confirms the
bridge uses a *fresh* `timeout(drain_timeout, &mut bridge)` (leadership.rs:123)
independent of the delegate's consumed budget — a conforming two-independent-
window implementation no longer appears to violate the scenario under strict
reading. (`crates/components/camel-master/src/leadership.rs:123`, spec.md:74-76)

### I1 — Fix 5 auth cleanup lock-scope/race — RESOLVED

design.md:128-135 now states cleanup acquires the outer `in_flight` mutex as
a single critical section (iterate + `strong_count == 1` test + `remove`),
and is race-free because both get-or-insert (step 3) and cleanup (step 9)
acquire the same outer mutex — no `Arc` clone can be issued between the test
and the removal. This is the correct invariant, sound because the outer lock
is never held across an await. (design.md:119-135)

### I2 — Fix 5 cleanup starvation under low pressure — RESOLVED

design.md:126-127 now runs in-flight cleanup on **every** cache miss (before
the cache insert at step 7), explicitly "NOT gated by the result-cache
capacity check in `evict_if_needed()`". Source confirms both
`evict_if_needed` impls early-return at `cache.len() < max`
(introspection.rs:171-173, permission_cache.rs:103-107), so the former
hang-off-eviction design would have starved cleanup. Now decoupled;
unbounded-growth path closed. (`introspection.rs:170-173`,
`permission_cache.rs:103-107`, design.md:126-131)

### M1 — producer.rs line citation — RESOLVED

design.md:73 and proposal.md now cite `producer.rs:143`. Source confirms
line 143 is `Poll::Ready(Err(_)) => Poll::Ready(Err(CamelError::ConsumerStopping))`.
No stale-line lint risk. (`crates/components/camel-jms/src/producer.rs:143`)

### M2 — Fix 4 delegate-outcome mapping — RESOLVED

design.md:88-93 enumerates all five delegate outcomes → stored value:
- `Ok(Ok(Ok(())))` → `None`
- `Ok(Ok(Err(err)))` → `Some(err)`
- `Ok(Err(e)) if e.is_panic()` → `Some(ProcessorError)`
- `Ok(Err(e))` (cancelled) → `Some(ProcessorError)`
- `Err(_)` (timeout) → `None` after `abort()`

Source confirms the delegate handle is `JoinHandle<Result<(), CamelError>>`
(leadership.rs:95-116 arms: `Ok(Ok(Ok(())))`, `Ok(Ok(Err(err)))`,
`Ok(Err(_)) if is_panic`, `Ok(Err(_))`, `Err(_)`). The three-level nesting
here is correct for the *delegate* handle and does not contradict Fix 1's
two-level *health* handle — they are genuinely different payload types.
(`crates/components/camel-master/src/leadership.rs:95-116`, design.md:88-93)

---

## Self-grill records (round 2)

### C1 — health

**Questions:**
1. [cross-ref] Does the spawned closure actually return `()`, making the
   two-level `Result<Result<(), JoinError>, Elapsed>` shape correct?
2. [sharpen] Is the `error!`-level logging arm placed on the JoinError case
   (not a nonexistent delegate-error case)?

**Answers:**
1. Yes. server.rs:111-124 closure logs the axum serve error inside the task
   and returns no value. `timeout(dur, handle)` → two levels. design.md:24
   states this explicitly. (`crates/camel-health/src/server.rs:111-124`)
2. Yes. design.md:26 maps `Ok(Err(join_err))` → `error!`. The removed
   `Ok(Ok(Err(_)))` "delegate error" arm does not exist for a `JoinHandle<()>`.
   (design.md:26)

**Outcome:** confirm.

### C2 — master spec

**Questions:**
1. [scenario] Does the reworded scenario admit a two-independent-window
   implementation without appearing to violate "aborted if elapses"?
2. [cross-ref] Does source use a shared or independent drain budget?

**Answers:**
1. Yes. "within its own `drain_timeout` window (or aborted if that window
   elapses)" scopes the abort to the bridge's own window, not a shared
   budget. spec.md:75-76.
2. Independent. leadership.rs:123 opens a fresh `timeout(drain_timeout,
   &mut bridge)`. Wording now matches. (`leadership.rs:123`)

**Outcome:** confirm.

### I1/I2 — auth

**Questions:**
1. [sharpen] Is the cleanup critical section (strong_count test + remove)
   held under a single lock that also guards get-or-insert?
2. [scenario] Under zero cache pressure (len never reaches max), does
   in-flight cleanup still run?

**Answers:**
1. Yes. design.md:128-135: cleanup acquires the outer `in_flight` mutex as a
   single critical section; get-or-insert (step 3) holds the same mutex.
   Race-free. (design.md:128-135)
2. Yes. design.md:126-127: cleanup runs on every cache miss, not gated by
   `evict_if_needed()`'s capacity check (which early-returns at
   `len < max`, introspection.rs:172). Starvation closed. (design.md:126-127)

**Outcome:** confirm.

### M1/M2 — jms + master

**Questions:**
1. [cross-ref] Is producer.rs:143 the ConsumerStopping site?
2. [cross-ref] Does the enumerated five-outcome mapping match the delegate
   handle's `Result<(), CamelError>` payload?

**Answers:**
1. Yes. producer.rs:143 = `Poll::Ready(Err(CamelError::ConsumerStopping))`.
   design.md:73 cites it. (`producer.rs:143`)
2. Yes. leadership.rs:95-116 has exactly the five arms design.md:88-93
   enumerates; the three-level nesting is correct for the delegate handle.
   (`leadership.rs:95-116`)

**Outcome:** confirm.

---

## Standing confirmations from round 1 (unchanged)

- Fix 2 (function) — best-effort drain, types resolve, `rollback_start`
  precedent real. BLESS.
- Fix 3 (JMS) — `ConsumerStopping` unit-variant string-loss noted in
  design.md:71-72; ADR-0024 conformance confirmed. BLESS.
- ADR discipline — no new ADR; all five are within-crate lifecycle
  corrections under existing ADRs 0024/0007/0035. Correct.
- Spec requirements map 1:1 to the five fixes; scenarios concrete and
  falsifiable; acceptance criteria testable.

---

## Required changes before plan-bless

None. All six round-1 findings resolved. Proceed to tasks.md authoring.
