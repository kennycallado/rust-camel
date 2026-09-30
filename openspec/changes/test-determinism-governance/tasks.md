# Tasks: test-determinism-governance

Spec-only change: the deliverable is the blessed delta
(`specs/test-determinism/spec.md`). No crate code, no cargo builds.
Tasks below are verification and landing-readiness work.

## Governance

### Task 1.1: Cross-reference audit of the delta spec

**Files:**
- `openspec/changes/test-determinism-governance/specs/test-determinism/spec.md` (modified — only if an audit finding requires a citation fix)
- `openspec/changes/test-determinism-governance/proposal.md` (modified — same condition)

**Steps:**
1. For each artifact cited in the delta, verify existence and cited
   fact, from the worktree root
   (`/home/shared/rust-camel-worktrees/determinism`):
   - `docs/adr/0069-integration-tier-testing-contract.md` contains
     `#### 13.1 Taxonomy` with all seven class tags (`unbounded-wait`,
     `port-toctou`, `pooled-race`, `global-state`, `platform-timing`,
     `sleep-as-sync`, `runner-pollution`) and `#### 13.2 Rules` with
     R1–R7.
   - `docs/adr/0070-staged-listener-port-determinism.md` exists.
   - Capability dirs exist under `openspec/specs/`:
     `staged-listener-binding`, `http-test-harness`, `lint-test-sleep`,
     `unbounded-wait-bounding`, `ignore-test-policy`,
     `test-determinism`.
   - `http-test-harness` spec.md contains the registry-poll readiness
     requirement and the `REGISTRY_TEST_MUTEX` serialization text.
   - `ignore-test-policy` spec.md references ADR-0054.
2. From the repo root (`/home/kenny/dev/rust-camel`), verify bd
   statuses cited: `bd show rc-s7dyw --json` → `"status": "open"`;
   `bd show rc-1dgvg --json` → `"status": "closed"`; `bd show rc-w1u9
   --json` → exists (the explicit startup handshake; its closure is
   what makes post-start sleeps vestigial).
3. If any check fails, fix the citation in the delta (or proposal) to
   match ground truth; do not change the doctrine. Re-run
   `openspec validate test-determinism-governance --type change --json`
   after any edit.

**Tests:** (executable spec — non-Rust, command + expected)
- `xref-adr0069`: `grep -c 'sleep-as-sync' docs/adr/0069-integration-tier-testing-contract.md` → ≥ 1; `grep -c '#### 13.2 Rules' docs/adr/0069-integration-tier-testing-contract.md` → 1
- `xref-capabilities`: `ls openspec/specs/{staged-listener-binding,http-test-harness,lint-test-sleep,unbounded-wait-bounding,ignore-test-policy,test-determinism}/spec.md` → all six paths list without error
- `xref-http-harness-claim`: `grep -c 'REGISTRY_TEST_MUTEX' openspec/specs/http-test-harness/spec.md` → ≥ 1
- `xref-bd-statuses`: `bd show rc-s7dyw --json | grep '"status": "open"'` → 1 hit; `bd show rc-1dgvg --json | grep '"status": "closed"'` → 1 hit; `bd show rc-w1u9 --json` → command exits 0 and prints a JSON body
- `validate-after-audit`: `openspec validate test-determinism-governance --type change --json` → `"valid": true`, zero issues

**Acceptance:**
- Every test above passes (or the citation was fixed and the test then passes).
- No new edits to the delta beyond citation corrections.
- `openspec validate test-determinism-governance --type change --json` reports `valid: true` at the end of the task.

- [x] 1.1

### Task 1.2: Landing readiness — validation, structure count, gate enumeration

**Files:**
- `openspec/changes/test-determinism-governance/` (read-only for this task)

**Steps:**
1. Run `openspec validate test-determinism-governance --type change --json`; confirm `valid: true`, zero delta-structure issues.
2. Count structure: the delta contains exactly 6 `### Requirement:` headers and every requirement has ≥ 1 `#### Scenario:` block (expected total: 21 scenarios — 3 sleep-as-sync, 3 no-free-port, 4 real-loopback, 3 env-mutation, 3 runner-pollution, 5 quarantine — count and record actuals).
3. Content audit — map EACH of the 21 scenarios to its doctrine anchor
   and record pass/fail per scenario against that anchor:
   - sleep-as-sync scenarios ↔ ADR-0069 §13.1 `sleep-as-sync` entry
     ("sleep stands in for synchronization … a `wait_until` barrier on
     an observable state replaces it") and §13.2 R1's bounded-helper
     context; the handshake scenario ↔ rc-w1u9 explicit handshake.
   - no-free-port scenarios ↔ §13.2 R2 (bind/inspect/close/rebind
     forbidden; port-zero ownership; child-process handoff; named
     exceptions) and ADR-0070's reserved-address / oneshot-placeholder
     exception list.
   - real-loopback scenarios ↔ §13.2 R3 (axum/Hyper/wiremock
     preference; oneshot below network boundary; raw TCP for
     protocol faults; one-response-and-close OR complete request
     loop); readiness scenario ↔ `http-test-harness` registry-poll
     requirement.
   - env-mutation scenarios ↔ §13.2 R4 (direct injection; dedicated
     child process; crate-wide RAII guard + one lock + restore;
     canonical-guard-type lint note).
   - runner-pollution scenarios ↔ TWO anchors: (a) ADR-0069 §13.1
     `runner-pollution` entry ("orphan processes or firewall residue
     from earlier CI steps") supports the leak-prohibition and
     teardown-reaping THEN clauses; (b) the sealed fleet standing
     containment policy (bd rc-tyq9e, 2026-09-30;
     `.opencode/fleet/GOVERNOR.md` Containment + Landing gates:
     scoped invocations via systemd-run with scope collection when a
     run ends, and the zero-orphan condition — `pgrep -c` of known
     battery process names must be 0 before/after a run) supports the
     scope-collection and zero-orphan pass-condition THEN clauses.
     Bounded teardown also anchors on §13.2 R1's bounded-teardown
     clause.
   - quarantine scenarios ↔ §13.2 R7 verbatim: `flaky-result = "fail"`
     retry semantics, registry field contract (exact test ID, bd
     issue, owner, ISO expiry), gating-lint rejection, non-gating
     retry job, 14-day maximum, `#[ignore]`/name-suffix bar.
   Expected result: every scenario maps to an anchor whose text
   supports the scenario's THEN clause; any mismatch is a finding to
   fix in the delta (citation-level only — doctrine changes require a
   fresh spec-bless).
4. Enumerate the Rust quality gates from repo `AGENTS.md ## QUALITY GATES` and record each as `N/A — no Rust changed`. The check MUST cover the complete worktree state — committed range, staged, unstaged, and untracked (including files inside untracked directories): confirm BOTH `git diff --name-only $(git merge-base HEAD main)...HEAD | grep -cE '\.rs$|Cargo\.toml'` → 0 AND `git status --porcelain --untracked-files=all | grep -cE '\.rs$|Cargo\.toml'` → 0. `lint-commits` is skipped by conductor policy (remote fetch). Non-Rust gates: none exist in the registry.
5. Record the gate enumeration result in the mission's parked report at
   `/home/kenny/dev/rust-camel/.opencode/fleet/inbox/determinism-parked.json`
   (fleet inbox; mission 326 writes exactly this file), not in this
   change dir.

**Tests:** (executable spec — non-Rust, command + expected)
- `validate-clean`: `openspec validate test-determinism-governance --type change --json | jq '.items[0].valid'` → `true`
- `structure-count`: `grep -c '^### Requirement:' openspec/changes/test-determinism-governance/specs/test-determinism/spec.md` → 6; `grep -c '^#### Scenario:' openspec/changes/test-determinism-governance/specs/test-determinism/spec.md` → 21
- `scenario-doctrine-audit`: for each of the 21 scenarios, diff its THEN clause against the doctrine anchor named in step 3 (ADR-0069 §13.1/§13.2 R1–R7, ADR-0070 exceptions, `http-test-harness` readiness requirement, rc-w1u9) → 21/21 supported, 0 mismatches
- `no-rust-diff`: `git diff --name-only $(git merge-base HEAD main)...HEAD | grep -cE '\.rs$|Cargo\.toml' || true` → 0 AND `git status --porcelain --untracked-files=all | grep -cE '\.rs$|Cargo\.toml' || true` → 0

**Acceptance:**
- `validate-clean` passes.
- `structure-count` matches (6 requirements, 21 scenarios); any mismatch investigated and resolved before landing.
- `scenario-doctrine-audit` records 21/21 pass; any mismatch fixed at citation level (doctrine changes require fresh spec-bless).
- `no-rust-diff` returns 0 on BOTH checks; all AGENTS.md Rust gates recorded N/A in the parked report.

- [x] 1.2
