# Execution Strategy — Audit Code-Correction Stream (rust-camel v1.0.0)

**Date:** 2026-08-07
**Author:** architectural escalation (analysis only — no code, no bd mutations)
**Inputs:** AUDIT.md (FC-\* table §785-820, correction flow §352-368), TRIAGE-2026-08-07.md, live `bd list --status open` (verified this session).
**Scope:** determine the execution strategy for the code stream (Critical/Important/Minor findings → OpenSpec → conductor-light). Docs stream (DPs) is already closed by the oracle.

---

## 0. Verified state deltas vs the briefing (read these first)

The live bd graph diverges from the briefing/TRIAGE in five material ways. The plan below is built on the **verified** state:

1. **The epic→child graph is already materialized** (TRIAGE hygiene note #2 is stale). `rc-be30`←{rc-iom7, rc-25j3}, `rc-p0ta`←{rc-1v0s, rc-yaep}, `rc-flny`←{rc-jla3, rc-qaom} all show real `parent-child` edges. `rc-t8kb` is **closed** (superseded by rc-be30). **No hygiene pass needed before execution.**
2. **`rc-w5yo` is scoped to T1 ONLY** (6 crates: api/config/dsl/cli/builder + children rc-bwbg/rc-9h5a). The FC table's "~24 crates" is aspirational — the **T2/T3 doc-drift (~18 crates) has NO bd home**. This is the single biggest structural gap.
3. **`rc-ryl0` (cxf test-gap) is P3, not P2**; **`rc-xctv` (thermo) is P3**. Both are genuinely below the P2 line.
4. **`rc-vh2l`** exists = the ADR-0051 **enforcement-lint task** (analog to rc-ierl for non_exhaustive, rc-9h5a for citations). It explicitly says *extend `cargo xtask lint-secrets`*, and that **field-name detection is rejected** (misses `StateStore.data`, flags `client_key_path`). This is the ADR-0051 sweep's gate task.
5. **`rc-h6yv` (auth async-lifecycle HOL) has NO dependency on rc-c9xo** in the actual graph. Only `rc-fvl5` is `related-to:rc-c9xo` (same Debug-leak shape, same crate). So the "does rc-c9xo unblock h6yv?" question resolves to **no** — h6yv is independent async-lifecycle work.

**ADR-0051 secret-leak bd set (verified):** rc-c9xo (P1), rc-zb1b (P1), rc-fvl5 (P2), rc-2g5v (P2), rc-4tbt (P2), rc-xbl1 (P2, Serialize), rc-ryl0 (P3, test-gap). Plus gate task rc-vh2l. **No separate bd for cxf beyond rc-ryl0.**

---

## 1. Granularity policy (the rule)

**Rule: one OpenSpec change = one coherent fix-shape applied to a bounded crate set, gated by at most one ADR/policy decision.**

Concretely this is **option (e) Hybrid**, resolved by three tie-breakers applied in order:

1. **Fix-shape homogeneity dominates.** If N findings share the *same* correction direction (redact Debug, cache compiled artifact, implement-or-remove dead field), they go in **one** change — the design.md is written once and the tasks are N near-identical worker jobs. This is why the audit *already* built epics around homogeneous FC clusters (rc-flny, rc-p0ta, rc-be30) and *refused* to epic the heterogeneous ones (FC-ASYNC-LIFECYCLE, FC-DEAD-CODE). **Honor that boundary — it is the audit's own signal.**
2. **Freeze-blocker independence overrides clustering.** A P1 that blocks the freeze gets its **own** change even if it shares a fix-shape with P2 siblings, so its merge is not held hostage by slower siblings. (rc-c9xo does not wait for rc-2g5v.)
3. **Blast-radius caps coarseness.** A change may not span so many crates that holistic review becomes unreviewable or the worktree accumulates merge-conflict surface against parallel owner work. Hard cap: **≤ 6 crates per change**, and cross-crate contract edits (metrics contract, non_exhaustive) get their **own** change separate from the mechanical call-site fixes they enable.

**Rejected options and why:**
- **(a) per-bd-issue (~45 changes):** wastes the two-blessing ceremony 45×; ADR-0051's six redactions would re-litigate the same policy six times. Rejected.
- **(b) per-FC-cluster:** right *instinct* but too blunt — it would force heterogeneous "clusters" (async-lifecycle, dead-code) into one incoherent change, and would fold a P1 freeze-blocker into a P2 epic. Rejected as the sole rule; kept as the default for homogeneous clusters.
- **(c) per-ADR-sweep:** correct for ADR-0051 specifically (see change **A1**), but doesn't generalize — most findings aren't ADR-governed. Applied selectively.
- **(d) per-crate:** splits FC-LANG-RECOMPILE (one SPI contract) across three changes and triples the ceremony for an identical fix. Rejected except as the *batching key* for orphan individuals with no cluster.

**Ceremony-cost reasoning:** each conductor-light run pays a fixed cost = spec-blessing + plan-blessing + holistic review, independent of size. With ~45 issues, per-issue = 45× ceremony; the hybrid below = **~16 changes**, cutting ceremony ~65% while keeping each change's internal fix-shape coherent enough that plan-blessing is cheap.

---

## 2. Proposed OpenSpec changes (concrete list — ~16 changes)

Naming: `audit-fix-<slug>`. Complexity S/M/L = worker-task count + review load. "Freeze" column marks the freeze-gate set.

### Freeze-gate set (must complete before v1.0 freeze)

| # | Change | bd issues | Fix-shape / rationale | Cx | Freeze |
|---|---|---|---|---|---|
| **A1** | `audit-fix-secret-leak-sweep` | rc-c9xo, rc-zb1b, rc-fvl5, rc-2g5v, rc-4tbt, rc-xbl1 (+ rc-ryl0 test folds in) | **ADR-0051 policy sweep**, one design.md, N near-identical redact-Debug/Serialize tasks across 5 crates (auth×2, wasm, otel, bridge, kafka). Coherent by *policy*, not FC label. ≤6 crates (auth counts once). **rc-ryl0 (P3 test-gap) folds in free** — same crate family, adds the missing regression test the sweep should establish as pattern. | **L** | ✅ |
| **A2** | `audit-fix-secret-leak-lint` | rc-vh2l | Enforcement gate (extend `lint-secrets`). **Separate change** because it's a contract/tooling decision with its own blessing weight, and it must land *after* A1 so the lint has a clean tree to pass against. Analog to how rc-ierl followed the non_exhaustive execution. | **M** | ✅ |
| **A3** | `audit-fix-http-clippy-gate` | rc-4vx8 | Mechanical: 3 `#[allow(await_holding_lock)]` mirroring 17 existing. Own change — unblocks CI, zero design surface, fastest possible merge. | **S** | ✅ |
| **A4** | `audit-fix-wit-versioning` | rc-aaxe (+ rc-m9nn dead-code, rc-osj0 dup-wit fold in) | ADR-0053 execution (WIT SemVer). rc-m9nn (dead runtime code in contract crate) and rc-osj0 (host dup .wit) are **same-crate, touched-by-the-same-edit** — batch by crate (tie-breaker 4/d). | **M** | ✅ |

### Pre-freeze P2 (security / correctness / lifecycle — strongly recommended before freeze)

| # | Change | bd issues | Fix-shape / rationale | Cx | Freeze |
|---|---|---|---|---|---|
| **B1** | `audit-fix-trust-boundary` | rc-be30 (epic) → rc-iom7, rc-25j3 | Homogeneous (validate untrusted path/query, ADR-0032/0033). Epic already coherent. sql H7 has no bd — **file it as a 3rd child or note explicitly excluded** (owner decision). | **M** | ⚠️ |
| **B2** | `audit-fix-metrics-contract` | rc-asm9, rc-0pyv, rc-7zr3 | Cross-crate metrics posture (ADR-0052 + ADR-0032 amend already committed `0e2b5ca5`). Contract + call-sites in prometheus, touching otel/health surface. rc-7zr3 (status-lies-on-exit) is same-crate same-file. | **M** | ⚠️ |
| **B3** | `audit-fix-lang-recompile` | rc-flny (epic) → rc-jla3, rc-qaom (+ jsonpath, unfiled) | Homogeneous perf: cache compiled artifact in Expression, one SPI contract. **jsonpath has no bd child** — file it before propose, or the change silently drops 1/3. | **M** | ⚠️ |
| **B4** | `audit-fix-dead-config` | rc-p0ta (epic) → rc-1v0s, rc-yaep (+ direct-M3 folds in) | Homogeneous: implement-or-remove per field. Direct M3 is doc-only, folds into the same design. | **S** | ⚠️ |
| **B5** | `audit-fix-wasm-hardening` | rc-cgc8, rc-466y, rc-dzd7 | Single crate (camel-component-wasm), but rc-466y/rc-dzd7 are `blocked-by-decision:ADR-0047`. **Gate:** ADR-0047 must be materialized first (it was a DP-2 pending-oracle item — verify it landed). If ADR-0047 is not yet Accepted, this change is **blocked** and drops to post-freeze. | **M** | ⚠️ |

### Batched orphan individuals (no cluster — batched by crate or by "one-shot mechanical")

| # | Change | bd issues | Fix-shape / rationale | Cx | Freeze |
|---|---|---|---|---|---|
| **C1** | `audit-fix-async-lifecycle` | rc-7wus, rc-b50f, rc-0zsm, rc-97gf, rc-h6yv | **DELIBERATELY grouped despite heterogeneity** — see §5. These 5 are all shutdown/lifecycle-correctness, each small, none sharing a fix-shape. Grouping them as *one change with 5 independent tasks* amortizes ceremony without forcing a false common design (the design.md is a §-per-crate rationale, not a unified fix). This is the one place I override the audit's "no epic" call at the *OpenSpec* layer (not the bd layer). | **M** | ⚠️ |
| **C2** | `audit-fix-auth-lifecycle` | *(rc-h6yv moved to C1)* — n/a | — | — | — |
| **C3** | `audit-fix-mock-correctness` | rc-zx30, rc-wy0y | Same crate (camel-mock): dead feature + clone drops Body::Stream. | **S** | ⚠️ |
| **C4** | `audit-fix-otel-lifecycle` | rc-z0y3, rc-3ixr | Same crate (camel-otel): stale meter binding + global-state leak. (rc-2g5v already in A1.) | **S** | ⚠️ |
| **C5** | `audit-fix-availability-dos` | rc-5qao, rc-wvty | grpc busy-spin + llm unbounded-deserialize. Both "resource-exhaustion / availability", small, different crates — batched as a themed one-shot. | **S** | ⚠️ |
| **C6** | `audit-fix-misc-correctness` | rc-3smd, rc-exa2, rc-gr8k, rc-xvuk, rc-jh8s, rc-sfy1, rc-7ka6, rc-b50f-adjacent | Grab-bag of unrelated single-crate P2 correctness (log multibyte panic, seda forwarder, proto-compiler temp_dir clobber, container cleanup, ws TLS asymmetry, bean non_exhaustive, endpoint-macros trybuild). **Split into 2–3 sub-changes if plan-blessing balks** — this is the overflow bucket. | **L** | ⚠️ |

### Doc-drift (see §5 for the split verdict)

| # | Change | bd issues | Fix-shape / rationale | Cx | Freeze |
|---|---|---|---|---|---|
| **D1** | `audit-fix-docdrift-t1-baseline` | rc-bwbg | Concrete T1 doc fixes, pre-enforcement. | **M** | ❌ |
| **D2** | `audit-fix-docdrift-lint` | rc-9h5a | xtask lint-context-citations; blocked-by D1. | **M** | ❌ |
| **D3** | `audit-fix-docdrift-t2t3` | **UNFILED (~18 crates)** — file new epic first | The T2/T3 residual. **Explicitly post-v1.0** unless owner wants clean docs at freeze. | **L** | ❌ |

### Post-v1.0 (deferred)

| # | Change | bd issues | Rationale | Freeze |
|---|---|---|---|---|
| **P-1** | `audit-fix-thermo` | rc-xctv (P3 epic) | Structural decomposition, no correctness. Explicitly post-v1.0. | ❌ |
| **P-2** | *(none)* | rc-ryl0, rc-omb8, rc-x2gy, rc-2nds, rc-7rup, rc-0xzt, rc-597a, etc. (P3 individuals) | Fold opportunistically into their crate's pre-freeze change (e.g. rc-ryl0 → A1) or defer. | ❌ |

### Special case — NOT a conductor-light change

| Change | bd | Why different |
|---|---|---|
| **rc-krpx** (Saxon sidecar security) | rc-krpx | **This is an investigation, not an implementation.** It has no children and no known fix — it asks "audit the unaudited sidecar." Route it to a **dedicated L3 security-audit spike** (auditor+validator, like the original audit flow), NOT conductor-light. conductor-light implements known changes; it cannot audit an external Java sidecar. Output of the spike may *then* spawn implementation changes. **See §3 phase F.** |

---

## 3. Conductor mode matrix

conductor-light modes: **interactive** (pause at every gate) vs **autopilot** (run flow, pause only on reject/stuck/error; cap = 3 escalations OR 2 consecutive task rejections; terminates in branch, human merges).

| Change / tier | Mode | Justification |
|---|---|---|
| **A3** http-clippy | **autopilot** | Purely mechanical, mirrors existing pattern, near-zero design risk. Ideal autopilot candidate. |
| **A4** wit-versioning | **autopilot** | ADR-0053 already materialized; execution is mechanical (version strings + binding maps). |
| **A1** secret-leak sweep | **interactive at spec-bless, autopilot after** | Security-critical *and* touches a P1 freeze-blocker. Human eyes on the spec (is the redaction contract right? does the test prove non-leakage?) — then autopilot the 6 near-identical redactions. Hybrid: one human gate, mechanical tail. |
| **A2** secret-leak lint | **interactive** | Enforcement policy with false-positive/false-negative trade-offs (rc-vh2l explicitly flags this). A wrong lint blocks the whole workspace. Human blesses the detection contract. |
| **B1 trust-boundary, B2 metrics** | **interactive at spec-bless, autopilot after** | Security-adjacent + cross-crate contract. Bless the contract, then mechanical call-site work. |
| **B3 lang-recompile, B4 dead-config** | **autopilot** | Homogeneous, well-understood fix-shape, single SPI/pattern. Low blast radius. |
| **B5 wasm-hardening** | **interactive** | Sandbox-security semantics + ADR-0047 dependency; gets human review. |
| **C1 async-lifecycle** | **interactive** | Heterogeneous shutdown-race fixes are exactly where subtle deadlock/leak bugs hide. Each task is small but semantically load-bearing (ADR-0024 match, epoch-bump drain). Human reviews each. |
| **C3–C6 orphan correctness** | **autopilot** | Individually small, low-risk single-crate fixes. Let autopilot run; the cap catches anything that goes sideways. |
| **D1–D3 doc-drift** | **autopilot** | Text/citation fixes, but **high fan-out** — cap must be respected. Autopilot with the escalation cap is the right containment. |
| **P-1 thermo** | **interactive** (when eventually run) | Pure refactor of giant files = high merge-conflict risk against owner's parallel work; needs human sequencing. Post-v1.0. |
| **rc-krpx sidecar** | **N/A — not conductor-light** | Dedicated audit spike (see §2 special case). |

**General rule:** *security + cross-crate-contract → human blesses the spec; everything mechanical → autopilot the tail.* The two-blessing gate's value is concentrated at spec-time for these; plan+task time is where autopilot earns its keep.

---

## 4. Execution sequence (phased, with hard gates)

Dependencies drive the order. The controlling facts: **rc-c9xo blocks freeze** (A1), **rc-4vx8 blocks CI** (A3), and A2/D2 lints must land *after* their baselines (A1/D1) so they pass against a clean tree.

### Phase 0 — Unblock CI (day 0, parallel-safe)
- **A3** (http-clippy). Autopilot. **Entry:** none. **Exit:** workspace `cargo clippy -D warnings` green. *This must land first — every other change's holistic review runs clippy.*

### Phase 1 — Freeze-blockers (the freeze-gate critical path)
- **A1** (secret-leak sweep) — the long pole. Interactive spec, autopilot tail.
- **A4** (wit-versioning) — parallel with A1 (disjoint crates).
- **Gate G1 (freeze-security):** A1 merged AND A2 merged.
- **A2** (secret-leak lint) — **entry:** A1 merged (clean tree). Interactive.
- **Exit criterion for Phase 1 = FREEZE-SECURITY GATE:** rc-c9xo + rc-zb1b closed, `lint-secrets` extended and green, rc-aaxe closed. **This is the minimum bar to *consider* freeze.**

### Phase 2 — Pre-freeze P2 (security/correctness; parallelizable, disjoint crates)
Run B1, B2, B3, B4 concurrently in separate worktrees (crate-disjoint → low conflict). B5 only if ADR-0047 confirmed Accepted.
- **Entry:** Phase 1 exit. **Exit:** all B-changes merged; trust-boundary + metrics + lang + dead-config closed.

### Phase 3 — Orphan correctness (parallelizable)
C1 (interactive), C3–C6 (autopilot), concurrent.
- **Entry:** Phase 2 exit (or overlap if worker budget allows — crate-disjoint from most of Phase 2). **Exit:** all C-changes merged.

### Phase 4 — Doc-drift baseline + enforcement
- **D1** → then **D2** (blocked-by D1). Autopilot with cap.
- **Entry:** any time after Phase 0 (independent of code fixes). **Recommended:** overlap with Phase 2/3.
- **Exit:** T1 doc-drift closed, lint-context-citations green in CI.

### Phase F — Sidecar security spike (independent track, start early)
- **rc-krpx** dedicated L3 audit spike. **Not gated by any code phase.** Start in parallel with Phase 1. **This IS a freeze concern** (unaudited XXE surface) — if the spike finds a live hole, it escalates a new freeze-blocker. Owner should treat the *spike completion* (not necessarily any fix) as a freeze checklist item.

### FREEZE DECISION POINT
**Freeze-gate set (hard):** Phase 0 + Phase 1 exits + Phase F spike delivered a verdict (clean OR remediated). 
**Strongly-recommended-before-freeze:** Phase 2 (B1/B2/B3 security+correctness). 
**Nice-to-have:** Phase 3, Phase 4-D1/D2.

### Post-v1.0 (explicitly deferred)
- **P-1** thermo (rc-xctv). **D3** T2/T3 doc-drift (~18 crates, unfiled). P3 individuals.

**Sequencing constraints (explicit):**
- A2 **after** A1; D2 **after** D1 (lint-after-baseline).
- B5 **after** ADR-0047 Accepted (verify — it was pending-oracle).
- rc-h6yv does **NOT** depend on rc-c9xo (verified) — it runs in C1 independently.
- rc-fvl5 **does** share rc-c9xo's fix-shape and crate → it rides A1 (not a separate change).

---

## 5. Epic-structure verdict

**Overall: the bd epic structure is sound at the bd layer. Change it in exactly three places.**

1. **FC-ASYNC-LIFECYCLE — keep as 4 individuals in bd, but group as ONE OpenSpec change (C1).** The audit's "no epic" call is correct *for bd* (heterogeneous fixes, no shared shape → a bd epic would be a false umbrella). But at the *OpenSpec* layer the question is different: ceremony amortization. Five tiny lifecycle changes = 5× two-blessing gates for ~5 small fixes. One change with 5 independent tasks and a §-per-crate design pays the gate once. **This distinction (bd-epic ≠ OpenSpec-change) is the key insight** — don't conflate the two taxonomies. (Add rc-h6yv as the 5th, per verified graph.)

2. **FC-DOC-DRIFT (rc-w5yo) — split by tier, and FILE THE MISSING T2/T3 EPIC.** rc-w5yo is *T1-scoped* (verified: 6 crates). The ~18 T2/T3 doc-drift findings the FC table attributes to it are **not actually in the epic**. **Owner decision required:** file a new `rc-XXXX` epic "FC-DOC-DRIFT T2/T3 residual" (children by crate) and mark it **post-v1.0 (P3)** unless clean docs are a freeze requirement. Do NOT stretch rc-w5yo to 24 crates — its acceptance criteria and children are T1-specific; widening it silently breaks its exit condition. Keep rc-w5yo as-is (D1/D2), add D3 as a distinct post-v1.0 epic.

3. **rc-krpx — reclassify from "epic to implement" to "spike to investigate."** It has no children and no fix — it is an *audit request*, not an implementation cluster. Routing it through `/opsx:propose` → conductor-light is a category error (you can't write a spec for "find out if Saxon is vulnerable"). Run it as a security spike; let its findings spawn real implementation changes if needed.

**Keep unchanged:** rc-be30 (trust-boundary, homogeneous, children materialized), rc-flny (lang-recompile, homogeneous), rc-p0ta (dead-config, homogeneous), rc-xctv (thermo, correctly P3 post-v1.0). **File-before-propose (small gaps):** jsonpath child under rc-flny; sql-H7 child under rc-be30 (or explicit exclusion note). FC-DEAD-CODE stays no-epic (audit call correct; batched by crate into C3/A4).

---

## 6. Risks & caveats

1. **Shared `CARGO_TARGET_DIR` (`/home/shared/rust-camel-target`) across owner worktrees** (AUDIT.md critical clause). Every conductor-light worktree that runs tests contends on the global target. **Mitigation:** each change's worktree must export a dedicated `CARGO_TARGET_DIR=/home/shared/rust-camel-target-fix/<change>` (mirror the audit's per-crate isolation) + rely on sccache. Without this, "cargo test passes" is unreliable under parallel phases 2/3.
2. **Merge-conflict surface against parallel owner work.** The plan runs phases 2–4 concurrently across disjoint crates *by design* to minimize this, but A1 (5 crates) and C6 (grab-bag) are the fattest targets. Keep A1's crates out of any concurrent change (they're all in A1), and split C6 if it grows.
3. **ADR-0047 dependency (B5) may be unmet.** rc-466y/rc-dzd7 are `blocked-by-decision:ADR-0047`, which was a *pending-oracle* DP. **Verify ADR-0047 is Accepted before scheduling B5**; if not, B5 slips to post-freeze and only rc-cgc8 (unblocked) can proceed.
4. **rc-vh2l lint false-positives can wedge the workspace.** A too-aggressive `lint-secrets` extension blocks *every* subsequent change's CI gate. This is why A2 is interactive and lands *after* A1 (clean tree to calibrate against). If A2's detection proves unstable, ship it as **warn-only** first, promote to `-D` post-freeze.
5. **The "batched orphan" changes (C5/C6) risk plan-blessing rejection** for incoherence ("why are grpc busy-spin and llm deserialize in one change?"). Pre-empt: the design.md must frame the *theme* (availability/DoS) not claim a shared fix. If a reviewer balks, split — the overflow is expected.
6. **rc-krpx spike could surface a live freeze-blocker late.** Because it's an investigation, its risk is unbounded until it runs. **Start it in Phase F immediately (parallel to Phase 1)** so a bad finding doesn't ambush the freeze date.
7. **Doc-drift autopilot fan-out** (D1/D3) can burn the escalation cap on trivial-but-numerous edits, terminating mid-flight in a branch. Acceptable (human merges the partial), but budget for a second autopilot pass to finish the tail.
8. **conductor-light terminates in a branch; the human merges.** Per AUDIT.md, **no `git push` by any agent.** Every phase exit = a local merge by the owner. The sequence's "exit criteria" are owner-merge events, not agent actions — the plan cannot self-advance across phase gates.

---

## Owner decisions required (explicit)

1. **File the T2/T3 doc-drift epic (D3)?** New `rc-XXXX`, post-v1.0 P3 — or leave the ~18 crates untracked. Recommendation: file it (untracked drift rots).
2. **sql-H7 (trust-boundary 3rd site):** file as rc-be30 child, or explicitly exclude with a note? Recommendation: file it — the FC table already treats it as in-scope.
3. **jsonpath (FC-LANG-RECOMPILE 3rd):** file as rc-flny child before B3 propose, else the change silently covers 2/3. Recommendation: file it.
4. **Is clean documentation a freeze requirement?** If yes, D1/D2 move into the freeze-gate; if no (recommended), they stay pre-freeze-nice-to-have and D3 goes post-v1.0.
5. **ADR-0047 status** — confirm Accepted before scheduling B5.

---

## Decisions RESOLVED (owner, 2026-08-07)

All 5 decisions resolved. **Net effect: the freeze-gate expands** — clean docs and nice-to-have work are now in scope pre-v1.0.

1. **D3 → PRE-v1.0 (not post).** Clean documentation IS a freeze requirement. Epic **rc-acd3** filed (P2, deps discovered-from:rc-ca8z) covering ~24 T2/T3 doc-drift crates. rc-w5yo stays T1-only (D1/D2); rc-acd3 is the T2/T3 residual (D3). D3 moves from the "Post-v1.0" bucket into the freeze-gate-adjacent set.
2. **sql-H7 → filed rc-qek5** (bug, P2, child of rc-be30, discovered-from:rc-6z4). rc-be30 trust-boundary epic now has 3 children: rc-iom7, rc-25j3, rc-qek5.
3. **jsonpath → filed rc-sg6r** (bug, P2, child of rc-flny, discovered-from:rc-6z4). rc-flny lang-recompile epic now has 3 children: rc-jla3, rc-qaom, rc-sg6r. B3 change covers the full 3/3.
4. **Clean docs = freeze requirement.** D1 (rc-bwbg) + D2 (rc-9h5a) + D3 (rc-acd3) all pre-freeze. Phase 4 (doc-drift) is upgraded from "nice-to-have" to freeze-gate-adjacent. Nice-to-have Phase 3 (orphan correctness C1–C6) also in scope pre-v1.0 per owner ("cover nice-to-have too if possible").
5. **ADR correction — B5 dependency is ADR-0050, NOT ADR-0047.** ADR-0047 = MiniJinja Template Rendering (Accepted, unrelated). The WASM sandbox posture is **ADR-0050** (`wasm-sandbox-capability-posture`, **Estado: Aceptado; implementación pendiente**, Opción B = selective WASI registration per world, references ADR-0011/0014/0031/0032/0033, origin F-camel-component-wasm-I1+I2). **B5 dependency is SATISFIED — B5 (rc-466y + rc-dzd7) can proceed.** The "ADR-0047" references in §2 (B5), §3, §4, §6 caveat #3 above are stale; read them as ADR-0050.

### Updated freeze-gate set (post-decisions)
**Hard freeze-gate:** Phase 0 (A3 clippy) + Phase 1 (A1 secret-sweep + A2 lint, A4 wit) + Phase F (rc-krpx spike verdict) + **Phase 4 (D1+D2+D3 doc-drift, all tiers)**.
**Strongly-recommended pre-freeze:** Phase 2 (B1–B5) + Phase 3 (C1–C6).
**Explicitly post-v1.0:** P-1 thermo (rc-xctv) only. P3 individuals fold opportunistically.

### bd state after hygiene + filing (2026-08-07)
- Superseded: rc-t8kb → rc-be30 (duplicate epic closed).
- Linked: rc-jla3 + rc-qaom → rc-flny (were orphaned).
- Filed: rc-qek5 (sql-H7), rc-sg6r (jsonpath), rc-acd3 (T2/T3 doc-drift epic, pre-v1.0).
- Escalated Minor→Important (expert_gpt 2026-08-07): rc-yv1m (api-M3 Principal Debug), rc-7hgc (http-M1 cookie dead-config), rc-ngnt (http-M2 proxy dead-config + SSRF-adjacent), rc-gbrh (xj-M2 block_in_place panic). rc-7hgc + rc-ngnt are children of rc-p0ta (FC-DEAD-CONFIG).
- rc-acd3 crate list corrected (note added): also covers doc-drift in auth, bean-macros, bridge, component-llm, component-wasm, endpoint-macros, language-js/jsonpath/rhai/xpath, master, otel, test, xslt, xj-M1.
- Phase F sidecar spike COMPLETE (rc-krpx closed): Verdict CLEAN-WITH-GAPS — 0 Critical, 0 Important. Saxon/CXF trust boundary holds. 4 defense-in-depth findings (N1/N2 Low, N3/N4 Nit) tracked in rc-wp9d. Does NOT block v1.0. Report: `docs/audits/modules/sidecar-xml-security-spike-2026-08-07.md`.
- Net open: 56 issues (rc-krpx closed).

---

## Minor handling (amendment, expert_gpt 2026-08-07)

Every Minor finding MUST receive one explicit disposition before the v1.0
freeze: tracked, folded, advisory, or deferred post-v1.0.

- Thermo-nuclear findings remain under rc-xctv and are post-v1.0.
- FC-DOC-DRIFT findings are pre-v1.0 under rc-w5yo or rc-acd3.
- Ponytail findings remain advisory unless triage identifies correctness,
  security, or public-contract impact.
- Every other substantive Minor MUST map to a bd issue or to an existing
  OpenSpec change by exact finding ID. Pure polish can remain advisory.
- Each affected `tasks.md` MUST enumerate the Minor finding IDs that it
  resolves. A generic instruction to "fix a crate's Minors" is not sufficient.

**Freeze invariant: no substantive Minor may remain tracked only in a module
audit report.**

### Escalated this session (Minor → Important, pre-freeze)
| Finding | bd | Rationale |
|---|---|---|
| F-camel-api-M3 | rc-yv1m | `Principal.claims` (untrusted, ADR-0032) derives Debug → PII/custom-claim disclosure. **DISTINCT from ADR-0051/A1** (claims are untrusted data, not credentials) — own change, manual Debug. |
| F-camel-http-M1 | rc-7hgc | `cookieHandling=inmemory` parsed+passed to `build_client` but silently no effect (FC-DEAD-CONFIG, child rc-p0ta). |
| F-camel-http-M2 | rc-ngnt | `proxy_url` deprecated-but-`pub`+TOML-parsed, silently ignored — security-adjacent (routing/SSRF pinning bypass, child rc-p0ta). |
| F-camel-xj-M2 | rc-gbrh | `block_on_result` uses `block_in_place` → panics on current-thread Tokio runtime (latent footgun in `create_endpoint` path). |

### Residual Minor tail (advisory unless triage promotes)
~10-15 genuinely-untracked non-doc Minors remain (version-lies, comments, log-dedup, rustdoc nits). Per the policy above: each gets a disposition during its crate's change (fold with explicit finding-ID enumeration) or stays advisory. The conductor-light `tasks.md` for each change MUST list the folded finding IDs in its acceptance criteria.
