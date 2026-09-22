# Proposal: bound-bare-recv-waits

## Why

`cargo xtask lint-unbounded-wait` (bd rc-3lx2, mission 201) ratchets 548
unbounded waits in test function bodies. Of those, 117 are bare
`rx.recv().await` sites: channel receives in `#[test]` / `#[tokio::test]`
bodies with no deadline at the call site. (The mission order said 100; the
original inventory matched `.recv().await` on the finding's start line and
missed 17 multi-line `rx\n.recv()\n.await` shapes — re-derived from the
lint's own AST scanner during the inter-phase-2 review.) Each can park the test forever
when a producer stalls or leaks its sender — the `unbounded-wait` defect
class of ADR-0069 §13.2 R1 ("a wait bounded only by something external
that may never happen"). ADR-0069 already names the remedy:
receive-with-timeout assertions at transport boundaries. The bounding
idiom exists and is proven in-repo
(`route_controller_drainclaim_tests.rs`:
`timeout(Duration::from_secs(2), rx.recv()).await.expect(..).expect(..)`).

This mission (bd rc-1e7sb) bounds all 117 recv-class sites and drops the
ratchet by exactly this mission's delta. It is the companion of mission
210 `loopsweep` (41 `loop {}` findings). Detector-level analysis shows
zero overlap: no recv site lies inside any reported loop span (and no
hunk adjacency), so both missions can land independently. `while let`
drains are recv findings (this mission); `loop {}` keyword loops are
loopsweep's.

## What Changes

- Bound all 117 bare `.recv().await` findings across 16 crates
  (27 files), by family:
  - single `expect`/`unwrap` receives → double-expect
    `tokio::time::timeout` wrapper (drainclaim idiom);
  - `while let Some(..) = rx.recv().await` drains → per-iteration
    deadline `loop { match timeout(..) }` that panics on `Elapsed`
    (silent break would mask a hang as a pass);
  - `if let Some(..) = rx.recv().await` optional receives → `match`
    on the wrapped receive, `Elapsed` ⇒ panic (a stall is a defect);
  - receives inside spawned `async move {}` drainer blocks → bounded
    inside the task, or `// allow-test-wait:` with a per-site
    justification when the drainer is runtime-torn-down fire-and-forget.
- `scripts/xtask/ratchet-unbounded-wait.max`: 548 → 431 (this mission's
  delta only — 117 sites; loopsweep owns the loop-class remainder).
- Test bodies and the ratchet file ONLY. No production code, no public
  API, no lint changes.

## Acceptance criteria

- `lint-unbounded-wait` reports zero unadjudicated `.recv().await`
  findings for the 117 inventoried sites (each either bounded or
  individually justified via `allow-test-wait`).
- Ratchet max = 431 and the lint exits green at that ceiling.
- `cargo fmt --check`, `cargo clippy -p <affected> -- -D warnings`, and
  the affected crates' test suites pass.
- No assertion is weakened: every `Elapsed` path fails the test with a
  message naming the wait and deadline.

## Risk budget

- Acceptable: strictly larger test-failure surface (timeout panic
  replaces silent hang); small flake risk from deadline magnitudes,
  mitigated by 2s floor (5s for container-backed kafka/sql/grpc paths).
- Out of bounds: weakening or removing existing assertions; touching
  `loop {}` sites owned by loopsweep; changing the lint or its detector
  classes; production code changes.

## Affected crates

camel-core, camel-processor, camel-component-api,
camel-component-grpc, camel-sql, camel-kafka, camel-component-seda,
camel-http, camel-direct, camel-master, camel-component-mcp, camel-ws,
camel-timer, camel-component-wasm, camel-test, camel-dsl; plus
`scripts/xtask/ratchet-unbounded-wait.max`.

Bd: rc-1e7sb (epic rc-99d5 adjacent, ADR-0069 §13.2 R1).

## Self-grill record

**Questions generated:**
1. [glossary] Does "family A/B/D" conflict with the lint's own vocabulary,
   and is "bare recv" the canonical term for what the detector flags?
2. [sharpen] "Drops the ratchet by exactly this mission's delta" — is the
   delta 100 sites or the count actually removed from the finding total?
3. [scenario] A `while let` drain and a bare single recv are both listed as
   findings, but the lint reports loops at the `loop` keyword and awaits at
   the recv span. Does the inventory line for a drain point at the line the
   lint actually reports, so a per-site conversion clears the exact finding?
4. [cross-ref] Proposal claims zero overlap with loopsweep (`loop {}`). The
   lint's loop class is `ExprLoop`; `while let` is `ExprWhile`. Do the 100
   recv sites actually avoid the `ExprLoop` subsumption path?

**Answers (with citations):**
1. The lint has no A/B/D taxonomy — these are the change's own AST-derived
   partition (`inventory.md:5-7`), disjoint from the detector's class names
   (`lint_unbounded_wait.rs:214-229`). "Bare recv" = an awaited `recv` method
   call not inside a `timeout` future-arg region (`lint_unbounded_wait.rs:603-604,
   683-689`). No glossary collision. (`inventory.md:5`, `lint_unbounded_wait.rs:215`)
2. Delta = unadjudicated findings removed. All 100 sites are `recv`-method
   awaits reported one-per-site (none are `ExprLoop`, so none are subsumed);
   bounding each removes exactly one finding → 548−100=448. Verified: 100
   unique (file,line) pairs, no double count. (`lint_unbounded_wait.rs:683-689`)
3. `while let` is `syn::ExprWhile`; the detector fires in `visit_expr_await`
   on the `recv` span, NOT the `while` line, and the `ExprLoop` subsumption
   path (`lint_unbounded_wait.rs:654-681`) never runs for `while let`. So the
   inventory's recv-line entry is the exact reported line. Confirmed at
   http:8318/11686 (live source matches inventory line-for-line). Per-site
   conversion clears the exact finding. (`lint_unbounded_wait.rs:683-709`)
4. Confirmed: every one of the 100 is a `recv`-method await or a `while let
   … recv().await` (ExprWhile), never a keyword `loop {}` (ExprLoop). The
   `ExprLoop` path is loopsweep's. Zero AST overlap. (`lint_unbounded_wait.rs:654,683`)

**Outcome:** confirm
**Self-grill mode:** self-grill-proposals skill
