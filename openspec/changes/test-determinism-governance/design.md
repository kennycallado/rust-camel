# Design: test-determinism-governance

## Approach

A single ADDED delta on the existing `test-determinism` capability
(`openspec/changes/test-determinism-governance/specs/test-determinism/spec.md`).
Each rule area becomes one `### Requirement:` with a SHALL statement
faithful to ADR-0069 §13.2 (as amended by the e_gpt counter-review) plus
2–5 GIVEN/WHEN/THEN scenarios. The doctrine source is ADR-0069
§13.1–13.2. The taxonomy has SEVEN classes: `unbounded-wait`,
`port-toctou`, `pooled-race`, `global-state`, `platform-timing`,
`sleep-as-sync`, `runner-pollution`. This delta cites the tags it
governs inside requirement text so bd issues can cite back. Two
classes need no new rule here: `unbounded-wait` is already canon (the
`unbounded-wait-bounding` capability and this capability's
deadline-bounded-await and lint-resolution requirements), and
`platform-timing` is a detector class, not a design rule — no
prohibition removes it, and per §13.1 the retry/weekly-coverage layers
carry it.

Layering rule (how duplication with mechanics capabilities is avoided):

- THIS capability carries the design rules — what a deterministic test
  must and must not do — and the quarantine policy.
- Mechanics capabilities keep their contracts unchanged and are cited by
  name: `staged-listener-binding` (ADR-0070 bind-stage-read law),
  `http-test-harness` (consumer-test registry-poll readiness and
  `REGISTRY_TEST_MUTEX` serialization — it does NOT specify the
  outbound loopback-server obligation; that rule lives in this
  delta), `lint-test-sleep` (advisory sleep detection),
  `unbounded-wait-bounding` (deadline-bounded receives and loops),
  `ignore-test-policy` (ADR-0054 closed vocabulary, unchanged by R5).
- R6 per-test ceilings are already canon here ("Bounded per-test
  execution in the Rust library-test job"); the new R7 quarantine
  requirement completes the policy pair.

The R7 requirement states POLICY only: registry field contract (exact
test ID, bd issue, owner, ISO expiry), gating-lint rejection of
missing/malformed/expired entries, the separate non-gating retry job,
the 14-day maximum lifetime, and the explicit bar on `#[ignore]` and
name suffixes as quarantine mechanisms. The registry file format, the
lint implementation, and nextest rollout measurement are the part-3
mechanics change (bd rc-jwp3 notes) and are out of scope here.

## Affected crates

- None. Spec-only change; no crate is built or modified.
- The rules GOVERN the test workspace (`camel-tests`) and the per-crate
  test modules, but enforcement mechanics arrive under their own
  changes (bd rc-3lx2 lineage).

## Architecture boundaries

No runtime, DSL, or component surface changes. This sits entirely in
the governance plane: `openspec/specs/` canon. Data/control plane
boundary untouched.

Single-phase change; no `## Phase N` headings.

## Alternatives considered

- One new capability per rule (six capabilities). Rejected: fragments
  the governance home; the epic needs ONE citable place for R7, and bd
  rc-jwp3 rules against big-split proliferation as much as against one
  big change. One focused delta on the existing capability is the
  ruled shape.
- Distribute rules into each mechanics capability (sleep rule into
  `lint-test-sleep`, port rule into `staged-listener-binding`, …).
  Rejected: rules would be citable only piecemeal; a reviewer could
  not answer "what makes a test deterministic here?" from one spec.
  Mechanics capabilities describe HOW their tool works, not WHAT tests
  must do.
- MODIFIED requirements instead of ADDED. Rejected: no existing
  requirement text contradicts the rules; ADDED is the minimal honest
  delta.
- Wait for part-3 mechanics, then codify. Rejected: the epic
  (rc-99d5) references this governance home NOW; doctrine is settled
  (ADR-0069 §13.2 is normative); spec text must precede the mechanics
  it governs.
