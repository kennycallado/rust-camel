# Tasks: review-finding-resolution-gating

## conductor-workflow spec

### Task 1.1: Land the gating requirement in the canon and verify prompt conformance

**Files:**
- `openspec/changes/review-finding-resolution-gating/specs/conductor-workflow/spec.md` (new — the ADDED delta defining the requirement)
- `openspec/specs/conductor-workflow/spec.md` (modified on archive — delta synced into canon by `openspec archive`)
- `.opencode/agents/conductor-light.md` (modified — corrected to conform, including the over-scope escalation fix applied during the holistic review)

**Steps:**
1. Confirm the change delta defines requirement "Review-finding resolution gating" with exactly three scenarios: "Legitimate finding blocks advancement", "Absurd finding may be discarded", "Deferral does not satisfy resolution".
2. Verify prompt conformance for **Scenario 1 (legitimate blocks)**: `.opencode/agents/conductor-light.md` per-task verdict contains a branch that sends a legitimate-minor finding to "resolve BEFORE advancing", and the PHASE 4 holistic review loops back on legitimate-minor findings.
3. Verify prompt conformance for **Scenario 2 (absurd discardable)**: the prompt retains absurd-minor as the only discardable finding (advances the task).
4. Verify prompt conformance for **Scenario 3 (deferral does not satisfy)**: the prompt's minor-resolution step contains no "OR file bd and advance" escape, no "fix exceeds the task's scope" deferral carve-out, and states minors are "blocking, not deferrable"; an in-scope finding whose fix exceeds the task's scope escalates to the human (not defer-and-advance); bd follow-ups are reserved ONLY for genuinely out-of-scope work.
5. Verify **three-site symmetry**: the inter-phase review verdict (step h) also loops back on legitimate-minor findings, matching the per-task (f) and holistic (d) sites — the exact cross-site drift this change exists to eliminate.
6. Run `openspec validate review-finding-resolution-gating` and confirm the delta parses and applies cleanly (exit 0).

**Tests:** (conformance — no Rust; the "system" is the agent workflow defined by the prompt and spec)
- `delta_parses_and_applies`: change exists → run `openspec validate review-finding-resolution-gating` → command exits 0 and reports "valid".
- `no_deferral_escape_in_prompt`: prompt exists → `rg -Fc "Resolve minor issues or file bd" .opencode/agents/conductor-light.md` → exits 1 (zero matches; the old escape-hatch text is gone).
- `no_over_scope_deferral_carveout`: prompt exists → `rg -Fc "or when a fix exceeds the task's scope" .opencode/agents/conductor-light.md` → exits 1 (the contradictory deferral clause is gone).
- `legitimate_minor_blocks_in_prompt`: prompt exists → `rg -Fc "resolve BEFORE advancing" .opencode/agents/conductor-light.md` → exits 0 with at least one match.
- `absurd_minor_discardable_in_prompt`: prompt exists → `rg -Fc "Absurd minor (no real defect)" .opencode/agents/conductor-light.md` → exits 0 with at least one match (fixed-string match; the parentheses are literals, not regex).
- `over_scope_escalates_not_defers`: prompt exists → `rg -Fc "escalate to the human rather than defer-and-advance" .opencode/agents/conductor-light.md` → exits 0 with at least one match.
- `inter_phase_routes_legitimate_minor`: prompt exists → `rg -Fc "important, or legitimate-minor findings" .opencode/agents/conductor-light.md` → exits 0 with at least one match.
- `holistic_routes_legitimate_minor`: prompt exists → `rg -Fc "REJECT / important / legitimate-minor findings" .opencode/agents/conductor-light.md` → exits 0 with at least one match.

**Acceptance:**
- `openspec validate review-finding-resolution-gating` exits 0.
- `rg -Fc "Resolve minor issues or file bd" .opencode/agents/conductor-light.md` exits 1 (no match).
- `rg -Fc "or when a fix exceeds the task's scope" .opencode/agents/conductor-light.md` exits 1 (no match).
- `rg -Fc "resolve BEFORE advancing" .opencode/agents/conductor-light.md` exits 0 (match present).
- `rg -Fc "Absurd minor (no real defect)" .opencode/agents/conductor-light.md` exits 0 (match present).
- `rg -Fc "escalate to the human rather than defer-and-advance" .opencode/agents/conductor-light.md` exits 0 (match present).
- `rg -Fc "important, or legitimate-minor findings" .opencode/agents/conductor-light.md` exits 0 (match present).
- `rg -Fc "REJECT / important / legitimate-minor findings" .opencode/agents/conductor-light.md` exits 0 (match present).
- `openspec status --change review-finding-resolution-gating` reports all artifacts complete (4/4) — the apply-ready state.

- [x] 1.1
