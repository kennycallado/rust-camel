# Proposal: review-finding-resolution-gating

## Why

The conductor-light workflow drifted from its intended review discipline: the agent advanced tasks while legitimate minor review findings stayed unresolved. The root cause is that the rule lived only in the agent prompt (`.opencode/agents/conductor-light.md`), and free-form edits weakened it across three locations over time. The spec canon (`openspec/specs/conductor-workflow/spec.md`) mandates that an inter-phase review runs, but it is silent on what a review verdict obligates. A normative invariant with no protected home erodes under prompt entropy. This change gives the invariant a bless-protected home in the spec canon so it can only be weakened deliberately and reviewably.

## What Changes

- ADD one requirement to the `conductor-workflow` spec: a review finding that identifies a real defect (critical, important, or legitimate minor) blocks advancement to the next task, phase, or workflow gate until resolved and re-reviewed; only absurd-minor findings (no demonstrated defect) may be discarded; filing a deferred follow-up does not satisfy resolution for any in-scope finding.
- The detailed per-task and holistic verdict branching stays in the prompt, which already implements this floor (conformance verified during this change).

Affected crates: none (spec/docs only).

Excluded: changes to the verdict branching prose in the prompt (already corrected in the same session); any Runtime, DSL, Components, or Services code.

## Acceptance criteria

- The `conductor-workflow` spec canon contains a requirement that gates advancement on real-defect findings.
- Three scenarios cover: a legitimate finding blocks advancement; an absurd finding may be discarded; deferral does not satisfy resolution.
- The prompt `.opencode/agents/conductor-light.md` conforms to the requirement, with no escape hatch that advances a task on an unresolved legitimate finding.

## Risk budget

Low. Spec-only change; no Rust, no runtime behavior. The brittleness risk of over-specifying operational detail is held down by elevating only the invariant, not the branching prose. Out of bounds: rewriting the existing inter-phase-review gating or merge-authorization requirements.
