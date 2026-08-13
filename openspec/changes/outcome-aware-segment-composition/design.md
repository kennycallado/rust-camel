# Design: outcome-aware-segment-composition

## Context

`PipelineOutcome` (ADR-0025) has three variants: `Completed(Exchange)`,
`Stopped(Exchange)`, `Failed(CamelError)`. `Stopped` is reserved for the
intentional Stop EIP (semantic halt with exchange state intact); `Failed` is for
errors. Structural EIPs (`cache:`, `recipient_list:`, `multicast`, `split`,
`do_try`, `load_balance`) implement the internal `OutcomePipeline` trait so
`Stopped` propagates across sub-pipeline boundaries without crossing the Tower
`Service<Exchange>` boundary.

Two layers matter for this change:

1. **OutcomePipeline layer** (`PipelineOutcome`): `cache:`, `do_try:`,
   `multicast`, `split`. These return `PipelineOutcome` directly.
2. **Tower Service layer** (`Result<Exchange, CamelError>`):
   `recipient_list` is a Tower `Service<Exchange, Response=Exchange,
   Error=CamelError>` (it predates the OutcomePipeline split and was not migrated
   — see ADR-0025 §"Structural EIPs" enumeration which lists `recipient_list`
   only indirectly via multicast). It is wrapped into an `OutcomeSegment` by an
   adapter: `Ok(ex) → Completed(ex)`, `Err(e) → Failed(e)`.

**The bug (rc-20yn) lives at the Tower→Outcome boundary.** In
`RecipientListService::call` (`camel-processor/src/recipient_list.rs`):
- sequential arm: `Err(e) if config.stop_on_exception => return Err(e)`; on any
  other error `Err(_) => continue`. After the loop, `aggregate_results(strategy,
  original, results)` runs. With zero successes `results` is empty and
  `LastWins` returns `results.into_iter().last().unwrap_or(original)` — the
  **original inbound exchange**.
- parallel arm: same `unwrap_or(original)` fallback in `aggregate_results`.

So all-recipients-failed is laundered to `Ok(original)`, the adapter yields
`Completed(original)`, and `cache:`'s `on_miss` write-back fires unconditionally
on `Completed` (`cache_eip.rs` step 3→4) — poisoning the cache with the inbound
body for the full TTL.

**`cache:` already does the right thing on `Failed`/`Stopped`**
(`cache_eip.rs` step 3): both propagate as-is with NO write-back. The defect is
purely that recipient_list never produces an `Err` on the all-failed path.

## Goals

- Pin the composition invariant in ADR-0058 so future Segment implementations
  have a compliance reference.
- Fix rc-20yn at the Tower→Outcome boundary: all-failed-with-at-least-one-error
  returns `Err(last_error)`, yielding `Failed` upstream and skipping cache
  write-back.
- Verify rc-n8rc and rc-65yi against the landed invariant; fix only where the
  reproducer proves necessary (do not speculatively rewrite).
- Gate the epic close on composed-path integration tests (rc-fgcu) — these
  defects are invisible to unit tests.

## Non-Goals

- Migrate `recipient_list` to the `OutcomePipeline` trait (large refactor; the
  boundary fix suffices and is lower-risk).
- Change `PipelineOutcome` enum shape (ADR-0025 stable).
- Change `stop_on_exception` default (Apache Camel parity).
- HTTP header policy (ADR-0057, epic rc-vy6w).

## The invariant (ADR-0058)

> **When a Segment's attempted work results in zero successes (an operational
> failure), the Segment SHALL report `Failed(error)` — or `Stopped(exchange)`
> only when the zero-success outcome is an intentional halt governed by the Stop
> EIP (ADR-0025 §3). It SHALL NOT report `Completed`.**

The invariant is outcome-based, NOT body-equality-based: a zero-success Segment
may not return `Completed` even if its body differs from the inbound body. A
Segment that attempted NO work (empty recipient list, no sub-pipeline execution,
a no-op passthrough) is NOT an operational failure and MAY return
`Completed(original)`.

Per-Segment definitions (normative scenarios cover the four delivered/repaired;
ADR-0058 enumerates the full governed set):

- **`recipient_list`** (Tower `Service`, reaches the invariant via the
  `Result → PipelineOutcome` adapter): attempt = one recipient call; success =
  call returned `Ok`; operational failure = ≥1 call attempted AND zero `Ok`.
- **`multicast`** (OutcomePipeline sibling): attempt = one branch sub-pipeline;
  success = branch returned `Completed`; operational failure = ≥1 branch AND
  zero `Completed`. Multicast is the parallel OutcomePipeline form and is
  governed identically to recipient_list on the zero-success axis. **Note:** the
  partial-success aggregation policy (some `Completed`, some `Failed`, no
  `Stopped`) is OUT OF SCOPE for this change — current multicast returns
  `Failed` on any branch failure, inconsistent with recipient_list's
  aggregate-on-partial-success; tracked as rc-b41j.
- **`cache`** (propagator, not generator): a propagated `Stopped`/`Failed` from
  `on_miss` is NOT cache's operational failure — cache propagates it as-is with
  no write-back. Cache's own operational failure is limited to repository errors
  (Contract C1). Cache is governed by the write-back trust rule (below).
- **`do_try`**: attempt = try_body; success = try_body `Completed` OR try_body
  `Failed` with a matching catch returning `Completed`; operational failure =
  try_body `Failed` AND no catch matched or all matching catches re-propagated.
  A `Stopped` from try_body is an intentional halt (propagated, skipping catch
  and finally per ADR-0025 §5.1).

**Cache write-back trust rule:** `cache:` may write back only a body a successful
(`Completed`) `on_miss` produced. It MUST skip write-back on `Stopped`/`Failed`
(already true at `cache_eip.rs` step 3). This is the downstream guarantor of the
invariant for the cache-warming pattern.

**Governed Segments (ADR-0058 enumeration):** `recipient_list`, `multicast`,
`cache`, `do_try`, `split`, `streaming-split`, `load_balance`. Normative spec
scenarios cover the first four (this change's deliverables); compliance of
`split`/`streaming-split`/`load_balance` is established by ADR-0058 and verified
separately.

## Decisions

1. **All-failed canonical outcome = `Err(last_error)` → `Failed(last_error)`.**
   NOT `Stopped(original)`. Rationale: `Stopped` is reserved for the intentional
   Stop EIP (ADR-0025 §3); using it for an error hides operational failure
   semantics and would let `do_try` catches keyed on `Failed` miss the error.

2. **"Last error" determinism:** the carried error is the error from the task
   returned by the last `JoinSet::join_next().await` that completed with an
   error (the most recently completed failing task at aggregation time). This is
   a "representative error, not causally-first" and is documented in ADR-0058.
   Tests assert that `Failed` is returned; a specific error identity is asserted
   only when the test controls completion order via a synchronization primitive.

3. **Partial success preserved:** if at least one recipient succeeded, aggregate
   over successes only and return `Ok(aggregated)`. Only the zero-success case
   is an error. This protects the legitimate multicast "continue past failure"
   pattern (`stop_on_exception=false`).

4. **Empty-recipient no-op preserved:** an empty resolved-URI list (expression
   yields `""` or all-empty tokens) returns `Ok(original)` — no work was
   attempted, so the invariant does not apply.

5. **rc-n8rc / rc-65yi are verification-first.** Build the reproducer; only edit
   if the reproducer still fails after rc-20yn's invariant lands. rc-65yi in
   particular may resolve entirely from the invariant (its candidate root causes
   at `cache_eip.rs:217` write-back and `do_try_segment.rs:97`
   `exchange_for_unmatched` are downstream of the recipient_list fix).

6. **Multicast partial-success is OUT OF SCOPE.** `sequential_multicast` /
   `parallel_multicast` return `Failed` on any branch failure even with
   successful branches (inconsistent with recipient_list's
   aggregate-on-partial-success). The zero-success invariant (this change's
   scope) is satisfied by multicast; the partial-success inconsistency is filed
   as rc-b41j and left for a separate change to avoid scope creep.

## Phases

This change is delivered in **four ordered phases**. The full multi-phase
`tasks.md` is plan-blessed once; phases execute in order.

### Phase 1 — Contract authority (rc-yy74)
Author ADR-0058. Lands FIRST so rc-20yn's fix cites it as authority. No code
changes; docs + ADR index/citations only.
**Exit:** ADR-0058 accepted, cross-links ADR-0025, ADR index updated, 0058 number
confirmed reserved (rc-zfov). Single task → no inter-phase review.

### Phase 2 — Keystone correction (rc-20yn + multicast verification)
Two tasks, both establishing the zero-success invariant at their respective
layers:
- **2.1 rc-20yn:** fix `RecipientListService::call` (sequential + parallel
  arms): track errors; when `results` is empty AND ≥1 error occurred, return
  `Err(last_error)`. Add failing-first unit tests (sequential all-failed,
  parallel all-failed, partial-success unchanged, empty-recipient no-op
  unchanged). Add cache composition regression test proving no write-back of
  inbound body on all-failed. Also verify/cite the pre-existing cache
  Stopped-propagation test (cache_eip.rs ~L865) so the cache-propagator scenario
  is owned.
- **2.2 multicast verification (NEW normative coverage, bd follow-up rc-b41j
  filed for the separate partial-success issue):** multicast ALREADY complies
  with the zero-success invariant (verified: `outputs` empty + `last_error` set
  → `Failed`; `Stopped` returns immediately). Add the two zero-success +
  Stopped-wins regression scenarios to lock the behavior. No production-code
  fix expected.
**Exit:** no zero-success path produces `Completed(original)` at either the
Tower layer (recipient_list) or the OutcomePipeline layer (multicast);
`stop_on_exception=false` default unchanged; cache retains seeded stale entry on
Failed on_miss. **Inter-phase `r_glm` review fires** (≥2 tasks): full Phase-2
diff reviewed for cross-task interaction (recipient_list Tower fix vs multicast
OutcomePipeline verification).

### Phase 3 — Error-path verification and repair (rc-n8rc, rc-65yi)
Two tasks, logically sequential within the worktree (one worker at a time):
- **3.1 rc-n8rc:** build the streaming-403 reproducer after Phase 2 lands; ensure
  the error response remains available exactly once with correct status/body.
- **3.2 rc-65yi:** build the composed cache/do_try/cache_peek_stale reproducer;
  repair body ownership only where the reproducer proves necessary (likely
  `cache_eip.rs:217` write-back body takeover and/or `do_try_segment.rs:97`
  `exchange_for_unmatched` pre-clone).
**Exit:** both reproducers pass independently and together. **Inter-phase
`r_glm` review fires** (≥2 tasks): full Phase-3 diff reviewed for cross-task
interaction and emergent drift.

### Phase 4 — Epic integration gates (rc-fgcu)
Add deterministic local 4xx/403 mock server + timer-driven cache-poison test and
stale-serve test in `crates/camel-test/` (or `camel-processor` integration
tests). No external network, no timing races.
**Exit:** both end-to-end scenarios pass repeatedly. Single task → no inter-phase
review.

## Risks

1. **Over-constraining legitimate `Completed(original)`:** the invariant must key
   on "attempted work failed", not body equality. Mitigated by Decision 4 +
   explicit unit test for empty-recipient no-op.
2. **Breaking partial-multicast continue-past-failure:** only the zero-success
   case errors. Mitigated by Decision 3 + partial-success unit test.
3. **Parallel "last error" non-determinism:** documented as representative error
   (Decision 2); test asserts the error is returned, not its exact identity when
   multiple distinct errors race.
4. **rc-65yi reproducer is the hard part:** unit traces of CacheService +
   DoTrySegment do NOT reproduce (per ticket). Only the composed path does.
   Mitigated by Phase 4 integration test owning the composed-path coverage and
   Phase 3.2 requiring a route-level reproducer before any edit.
5. **ADR numbering collision with rc-eoft (epic rc-vy6w):** 0057 = rc-eoft, 0058
   = rc-65fs (reserved rc-zfov). If rc-eoft reuses 0058, conflict. Mitigated by
   the reservation ticket discovered-from both rc-eoft and rc-65fs.

## Spec deltas

- **ADDED capability `segment-outcome-composition`:** the outcome-based
  invariant, per-Segment definitions of attempted work / success / operational
  failure / intentional halt, canonical `Failed(last_error)` outcome, parallel
  last-error determinism (JoinSet `join_next` order), recipient_list and
  multicast zero-success rules, cache write-back trust rule (cross-reference),
  do_try catch body propagation, stream ownership (single-consumption + status
  and body propagation, split into separate requirements/scenarios).
- **ADDED Requirements on existing capability `eip-cache`:** add the write-back
  trust rule (skip on `Stopped`/`Failed`) and the stale-body-preservation
  requirement (rc-65yi) as NEW requirements on the existing eip-cache capability
  (delta header `## ADDED Requirements`). They do not modify the existing
  `Cache EIP face` requirement. Cross-reference the new capability.
- **No MODIFIED, no REMOVED.** ADR-0058 is a doc artefact, not a spec delta
  (documented in tasks, referenced from specs).

## Open questions (resolved before PHASE 1)

- **Q: ADR-0057 doesn't exist yet (repo ends at 0056).** → RESOLVED: reserve
  0058 (rc-zfov); proceed assuming 0057 lands before or alongside (rc-eoft owns
  it, separate agent, separate epic rc-vy6w).
- **Q: `Stopped(original)` vs `Failed(last_error)` as canonical all-failed?** →
  RESOLVED: `Failed(last_error)` (Decision 1).
