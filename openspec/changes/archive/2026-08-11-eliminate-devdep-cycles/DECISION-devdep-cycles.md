# Architectural Decision: eliminate-devdep-cycles (final, decisive)

Authority: escalated architect. Verified against source, not the issue text.
Status: historical SCC analysis that grounded the spec/plan revision. The
authoritative record is the blessed spec/design/tasks + ADR-0055 +
baseline.md (which corrected this doc's 3-edge count to 4).

## TL;DR

The real problem is **3 weak (dev-dep) edges in 2 crates**, not 25 edges / 16 crates.
The "16-crate `no_verify` set" is a **diagnostic artifact of an over-breaking greedy
loop in `resolve_publish_order`**, not 16 real cycles. Fix the 3 edges, fix the
loop, delete the hack. Total ≈ 8 tasks, not 16.

## Ground truth (measured from Cargo.toml, replicating the xtask parser)

The combined normal+weak publishable graph has **exactly two non-trivial SCCs**:

- **SCC-A (5 nodes):** `camel-builder, camel-http, camel-ws, camel-core, camel-otel`
- **SCC-B (2 nodes):** `camel-endpoint, camel-endpoint-macros`

Everything else is already a DAG. The 25 "broken edges" / 16-crate `no_verify`
reported by `cargo xtask publish-order` are collateral: the greedy breaker keeps
snipping weak edges from *any* still-unscheduled crate with a weak incoming edge,
long after the graph is acyclic. **24 of the 25 "broken edges" are not in any
cycle** (verified: the "target" does not normal-reach the "holder").

### Edge-direction semantics (verified in `resolve_publish_order`, L3234-3386)

`adj[di].push((ci, kind))` where **`ci` = the crate that DECLARES the dep**, `di` = the
dep target. `broken_weak_edges.push((crates[di], crates[ci]))` and the print
`{from=di} --dev/build-dep--> {to=ci} (publish {to} --no-verify)`, `no_verify = {ci}`.
**The print reads target-first, holder-second — the OPPOSITE of the actual dev-dep
direction.** The user's ground-truth list was decoded backwards. `no_verify` = the
*holder* crate (the one whose dev-dep got cut).

### The irreducible cycles (holder --dev--> target ; NORMAL backbone)

SCC-A backbone (all normal unless marked):
```
camel-builder --N--> camel-core
camel-http    --N--> camel-otel      (optional dep, `otel` feature)
camel-ws      --N--> camel-otel
camel-otel    --dev--> camel-core     [WEAK]   (tests/integration.rs)
camel-otel    --dev--> camel-builder  [WEAK]   (tests/integration.rs)
camel-core    --dev--> camel-http     [WEAK]   (component_metadata_catalog.rs cfg(test))
camel-core    --dev--> camel-ws       [WEAK]
```
SCC-B:
```
camel-endpoint       --N--> camel-endpoint-macros
camel-endpoint-macros --dev--> camel-endpoint   [WEAK]  (tests/derive_integration.rs + trybuild UI)
camel-endpoint-macros --dev--> camel-api        [WEAK]  (same tests)
```

### Minimum feedback edge set (brute-forced)

- **SCC-A: cut `camel-core --dev--> camel-http` and `camel-core --dev--> camel-ws`** (2 edges, ONE holder crate). This alone makes SCC-A a DAG. Cutting otel's two edges also works but touches a crate with a 484-line integration test — core's two edges are anchored by exactly two `#[cfg(test)]` functions, so core is the cheaper cut.
- **SCC-B: cut `camel-endpoint-macros --dev--> {camel-endpoint, camel-api}`** (1 holder crate).

**Verified:** removing those 3 edges → 0 SCCs → pure Kahn sorts all 59 crates with
**0 broken edges**. This is the whole fix.

## THE PATTERN (decisive)

Two distinct sub-problems, two distinct mechanisms. Do NOT apply one uniformly.

### Pattern 1 — `camel-core` http/ws: **StubComponent for fixture tests + targeted relocation of real-option tests**

The only consumers of `camel-core`'s http/ws dev-deps are `#[cfg(test)]`
functions in `src/component_metadata_catalog.rs`. The remediation splits
by test intent (not one uniform mechanism):

- **Fixture-intent tests** (assert only scheme registration / count /
  `all_metadata().len()` / `query_capabilities`): the real component is
  incidental scaffolding. Replace with a private `StubComponent`
  implementing the `Component` trait with a configurable scheme + synthetic
  metadata. Strictly simpler than relocation (least code that works).

- **Real-option-intent tests** (`all_phase2_schemes_have_options`,
  `no_duplicate_option_names`, and any sibling asserting the REAL http/ws
  option catalog a stub cannot reproduce): relocate those specific
  functions to `crates/camel-test/tests/core_catalog_real_metadata_test.rs`
  (camel-test already normal-deps http; add ws to its `[dev-dependencies]`,
  leaf-safe). This is the one place relocation is genuinely needed — scoped
  to ≤4 functions, not 130 files.

Either way, both `camel-component-http` and `camel-component-ws` leave
camel-core's `[dev-dependencies]`. No `tests/`-file mass relocation; the
`tests/` files use `camel_component_mock`/`timer`/`api` which are
non-cyclic and stay.

### Pattern 2 — `camel-endpoint-macros`: **relocate the derive-integration test to `camel-endpoint`**

`endpoint-macros` is a proc-macro crate. Its `derive_integration.rs` and trybuild UI
tests must compile macro *output*, which references `camel_endpoint` / `camel_api`
types — an inherent proc-macro-testing cycle (the macro's consumer is a heavier
crate). The canonical fix (syn/darling/serde_derive all do this): **the derive
crate's integration tests live in the CONSUMER crate.**

**Prescription:** move `tests/derive_integration.rs` and the `tests/ui/*` +
`tests/ui_tests.rs` trybuild harness from `camel-endpoint-macros` into
`camel-endpoint/tests/` (which already normal-deps both `camel-endpoint-macros` and
`camel-api` — zero new edges). Remove `camel-api` + `camel-endpoint` from
`endpoint-macros` `[dev-dependencies]`. The crate keeps only `trybuild` +
`syn`/`quote`/`proc-macro2`. Pure unit tests in `src/uri_config.rs` stay.

### The `test-support` "crux" — it is a NON-problem here

The feared pattern (`[dev-dependencies] camel-component-api = { features = ["test-support"] }`
used by `src/` `cfg(test)`) is **not cyclic** and needs no change. Verified:
`camel-component-api`'s normal deps are `camel-api, camel-auth, camel-language-api,
camel-endpoint` — none reaches back to `camel-core` or any component. So
`camel-component-api(test-support)` dev-deps never close a cycle. **Retain every one
of them unchanged.** No `*-test-support` split crate is needed. The design.md's
menu-item-4 caveat ("feature can't create/eliminate a dependency") is correct and
already satisfied — the dep is non-cyclic, so it stays.

This dissolves the escalation's stated crux: there is no "I need component-api's
test-support but dev-depending on it is cyclic" case in this workspace. It isn't.

## The xtask over-breaking bug (must fix, or the lint lies)

`resolve_publish_order`'s cycle-breaking loop (L3295-3352) advances whenever it can
break *a* weak edge from an unscheduled source, not whenever a cycle actually
remains. After the true feedback edges are cut, the queue is empty only transiently;
the loop still finds weak edges into not-yet-drained crates and snips them, inflating
`broken_weak_edges`. **This is why cutting the 3 real edges still showed 16 "broken"
in simulation of the CURRENT algorithm.**

**Fix:** before breaking any edge, run Tarjan SCC on the *remaining* combined graph
(restricted to unscheduled nodes) and only break a weak edge that lies **inside a
non-trivial SCC**. When no unscheduled node is in a non-trivial SCC, drain via Kahn
and stop. This makes `no_verify` report the *true* minimal set and makes
`lint-publish-cycles` trustworthy. Ship this in Phase 0 alongside `--show-cycles` —
otherwise the baseline is 16 phantom crates and every phase chases ghosts.

## camel-test leaf status — already true, keep the invariant check

`camel-config --dev--> camel-test` and `camel-dsl --dev--> camel-test` in the user's
list are **phantom over-broken edges**, decoded backwards. Actual direction:
`camel-test --dev--> {config, dsl}` (camel-test is the holder). After the loop fix,
**no publishable crate declares `camel-test` in any dep kind** → camel-test is a
genuine leaf, stays published, never in `no_verify`. Keep the plan's
`lint-publish-cycles` leaf assertion (scan all manifests for a `camel-test` dep) — it
is cheap insurance, not a fix.

## Verdict on the blessed spec/plan

- **Phase structure survives; Phase 1/2 scope collapses.** Phases 0/3/4 (diagnostic,
  ADR+lint, delete hack) are correct as written. Phases 1-2 shrink from "relocate
  ~130-file sprawl across 8 crates" to **3 surgical edits in 2 crates**.
- **e_gpt's REJECT was right for the wrong reason.** The plan wasn't under-scoped on
  effort — it was mis-targeted: it never ran SCC analysis, so it treated collateral
  edges as real and prescribed relocations that touch non-cyclic tests.
- **Split?** No. One change. The 3 edits + loop fix + lint + ADR + hack-deletion are
  cohesive and small. A split adds ceremony without isolation benefit.

## Prescribed task list (≈8 tasks, replaces the 16-task plan)

1. **Phase 0 — `--show-cycles` diagnostic** (keep plan Task 0.1) **+ fix the
   over-breaking loop** (Tarjan-gated edge removal). Baseline after the fix must show
   `no_verify = {camel-core, camel-otel(or n/a), camel-endpoint-macros}`-class real
   set, not 16.
2. **Fix `camel-core` http/ws:** StubComponent in `component_metadata_catalog.rs`
   cfg(test); drop `camel-component-http` + `camel-component-ws` from core
   `[dev-dependencies]`. (Optionally also relocate `otel/tests/integration.rs` to
   camel-test to make the SCC-A cut redundant-safe; NOT required once core's 2 edges
   are cut.)
3. **Fix `camel-endpoint-macros`:** move `derive_integration.rs` + `ui/` trybuild
   harness into `camel-endpoint/tests/`; drop `camel-api` + `camel-endpoint` from
   endpoint-macros `[dev-dependencies]`.
4. **Verify DAG:** `--show-cycles` → `no_verify` empty; `cargo test -p camel-core -p
   camel-endpoint -p camel-endpoint-macros -p camel-otel` green.
5. **ADR-0055** + CONTEXT-MAP citation (keep plan Task 3.1).
6. **`lint-publish-cycles`** xtask subcommand incl. camel-test leaf assertion (keep
   plan Task 3.2) — now trustworthy because of the Task-1 loop fix.
7. **Wire gate into AGENTS.md QUALITY GATES** (keep plan Task 3.3).
8. **Delete the hack:** remove `comment_out_camel_dev_deps`,
   `is_weak_dependency_section`, strip/restore loop; simplify `publish_crates` to
   linear topo sort (keep plan Task 4.1).

Drop plan Tasks 1.2, 1.3, 1.4, 1.5 mass-relocation and ALL of Phase 2 (2.1-2.5):
those crates (llm, surrealdb, template, validator, exec, wasm, builder) are **not in
any real SCC** — their only "cyclic" dev-deps in the issue are phantom over-broken
edges. Verify each is absent from the post-loop-fix `--show-cycles` before touching
it; expected result: all N/A.

## Trade-offs

- Optimized for: minimalism (3 edits vs 130-file churn), correctness of the
  diagnostic (loop fix), and a trustworthy lint.
- Deprioritized: pre-emptive relocation "hygiene" for non-cyclic tests — explicitly
  rejected as churn with no topology benefit.
- Risk: the loop fix is the one non-trivial code change. It is well-bounded (Tarjan
  on ≤59 nodes) and directly testable with a synthetic cyclic fixture.
