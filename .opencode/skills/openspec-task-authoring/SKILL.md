---
name: openspec-task-authoring
description: Quality discipline for authoring OpenSpec tasks.md task blocks. Loaded by conductor-light in STAGE 2 before plan-bless. Enforces no-placeholders, executable test specs, concrete acceptance, NEW-symbol consistency, and phase-boundary coherence. Use when writing or revising task blocks under `## Phase N` headings in an expert-gated OpenSpec change.
---

# openspec-task-authoring

## Purpose

Quality discipline for authoring OpenSpec `tasks.md` task blocks. Loaded by `conductor-light` during STAGE 2 of the expert-gated flow, before the plan-bless. It enforces a no-placeholders rule, a per-task quality bar, and a self-review pass that catches the failure modes downstream workers and reviewers cannot recover from cheaply.

## No-placeholders rules

The following patterns are forbidden in a task block. If any of them appear, the block fails self-review and the plan-bless is rejected.

- `TBD`, `TODO`, `implement later`, `fill in details`, `...` (ellipsis used as content), `<placeholder>` style tokens.
- `add appropriate error handling`, `add validation`, `handle edge cases` without naming the specific cases.
- `write tests for the above` without an executable test spec (name, setup, action, assert, command, expected).
- `similar to Task N` — repeat the content inline; do not defer by reference.
- A step that describes WHAT without HOW. Example (bad): `Implement the parser.` Example (good): `Add a `fn parse_route(seg: &str) -> Result<Route, ParseError>` in `camel-dsl/src/parser.rs` that matches `seg` against the grammar in `design.md §3` and returns `ParseError::UnknownToken` for unmatched input.`
- A reference to a type, function, or module that no task introduces and no spec defines. New symbols must be introduced in some task block; references to EXISTING project code symbols are fine.

## Per-task-block quality

Every task block has the form already defined by the tasks template (`Files`, `Steps`, `Tests`, `Acceptance`, `- [ ] <id>`). The skill tightens each field:

- **Files**: exact repo-relative paths. Mark each as `(new)`, `(modified)`, or `(deleted)`. No ranges, no globs.
- **Steps**: numbered, each a single concrete action. The result of each step must be observable (a file, a function, a passing test, a committed doc).
- **Tests**: executable test specs expressed as:
  - `name` — the `#[test]` fn name (or a free-form name if a non-Rust crate).
  - `setup` — what exists before the test runs.
  - `action` — what the test does.
  - `assert` — the exact expected outcome.
  - `command` — the `cargo test ...` (or equivalent) invocation that exercises it.
  - `expected` — pass/fail expectation before the implementation exists.
  - Include test code only where semantics are ambiguous. Do NOT require fully compilable code for every test; the spec is the contract, the worker fills the syntax.
- **Acceptance**: concrete, machine-checkable criteria. Examples that pass: `cargo clippy -p camel-core -- -D warnings` exits 0, `cargo test -p camel-core --lib` passes, `cargo xtask schema --check` exits 0, no new `unwrap()` introduced (verifiable by `cargo xtask lint-unwrap`). Examples that fail: `code is clean`, `tests are good`, `looks right`.

## Self-review checklist

Run this before requesting the plan-bless. Each item is a hard pass/fail.

1. **Spec coverage.** For every requirement in the blessed specs, at least one task block must cite it. Walk the spec `## ADDED Requirement` and `## MODIFIED Requirement` sections; for each `### Scenario`, confirm a task exercises that scenario. A scenario with no owning task fails the checklist.
2. **Placeholder scan.** `rg` for the forbidden patterns listed above over the draft `tasks.md`. Zero hits required. Re-run after any edit.
3. **NEW-symbol consistency.** Symbols (types, functions, modules, error variants) introduced in one task must match the names and signatures used wherever later tasks reference them. References to EXISTING project code (already committed symbols the change does not own) are not in scope; only NEW symbols introduced by this change must agree.
4. **Phase-boundary coherence.** For each `## Phase N: <name>` group, the tasks inside share a coherent goal. The phase boundary must match the `## Phases` section in `design.md` (one phase = one deliverable, with the same goal/deps/externally-visible types/exit-criteria the design recorded).

## Scope-check (STAGE 2 start)

The scope-check runs ONCE at the start of STAGE 2, AFTER phases were fixed at design time. It does NOT author the phase decomposition (that is a design-time decision in `design.md`, blessed with the spec); it only validates size and coherence.

- If a phase is too large (e.g. mixes independent subsystems, or contains > ~8 tasks without a sub-deliverable boundary), flag it.
- If independent subsystems were collapsed into a single phase, flag it.
- The flag is binary: phase is acceptable, or it is not.

**Consequence of a failed scope-check (escape hatch).** A phase-validation failure is a SPEC-LEVEL defect, not a tasks-level defect. Do not patch `tasks.md` to mask a bad boundary. Instead:

1. Delete the draft `tasks.md`.
2. Return to STAGE 1.
3. Revise `design.md` (and `specs/` if needed) to fix the phase decomposition.
4. Obtain a fresh spec-bless.
5. Restart STAGE 2 with a regenerated `tasks.md`.

This rule exists because the once-blessed-full-plan design is load-bearing — the apply gate trusts the plan, and a bad boundary hidden by patching surfaces later as cross-phase drift the holistic review cannot cheaply untangle.

## What this skill does NOT do

- It does NOT author the phase decomposition. Phases are a design decision recorded in `design.md ## Phases` and blessed with the spec. The skill validates but does not invent.
- It does NOT replace the reviewer loop (`r_glm` on the plan) or the plan-bless (`e_gpt`). It is the conductor-side quality pass that precedes them.
- It does NOT own the commit-message format. `caveman-commit` does. This skill never specifies commit subjects.
- It does NOT touch the apply gate or `schema.yaml`. Those are out of scope by design (per `design.md` §Architecture boundaries).
