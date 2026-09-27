# Proposal: archengine — scenario rename/drop in MODIFIED requirements

## Why

The openspec archive engine (upstream `@fission-ai/openspec@1.7.0`, nix-pinned,
read-only here) treats a MODIFIED requirement block as a scenario superset: at
archive time every scenario name in the canonical block must appear in the
delta block, and the delta block then replaces the canon block wholesale. A
delta that renames or intentionally drops a scenario is refused
("current spec contains scenario(s) not present in the modified block"). The
same requirement name in both MODIFIED and REMOVED is also forbidden, so there
is no escape hatch. Recent landings paid 5 manual scenario-carries and one
`--skip-specs` + manual sync (r2embed, splitjson) — error-prone conductor
surgery each time (bd rc-bayx, rc-63aj lineage).

## What Changes

Tooling only; no product spec deltas (skip_specs marker on this change).

1. New xtask subcommand `cargo xtask archive <change>` in `scripts/xtask`
   that wraps the pinned `openspec archive`:
   - A MODIFIED requirement block may carry an explicit
     `<!-- openspec-scenario-ops ... -->` directive comment declaring
     scenario `renamed:` (old -> new) and `dropped:` (name | one-line
     justification) operations.
   - The wrapper validates the directives against the canonical spec with a
     strict guard matrix, pre-syncs the affected canon requirement blocks
     (verbatim block splice), then execs upstream `openspec archive
     <change> --json --yes`. The engine's own "early-sync" idempotency then
     accepts the change without weakening any upstream validation.
   - Without directives the wrapper is a pure passthrough: today's strict
     behavior, unchanged.
2. Unit tests for parser/guard/splice; `#[ignore]` integration tests that run
   the real `openspec` binary on a scratch openspec root (rename+drop happy
   path, strict refusal preserved, MODIFIED+REMOVED validate conflict intact,
   idempotent re-run).
3. Usage note in `.opencode/skills/openspec-archive-change/SKILL.md` (the
   archive-discipline doc).

## Acceptance Criteria (bd rc-bayx)

- Archive of a delta that renames and drops scenarios inside a MODIFIED
  requirement succeeds (no "not present in the modified block" refusal) and
  `openspec/specs/` canon reflects the rename and the drop.
- `openspec validate` still rejects the same requirement name in both MODIFIED
  and REMOVED (parser guard untouched).
- Round-trip test proves delta → archive → canon sync for rename+drop.
- No directives anywhere = byte-identical passthrough behavior (regression
  gate: all 120 canonical specs validate, untouched by the change itself).

## Risk Budget

- Engine is upstream and pinned (1.7.0 via nix): the wrapper depends only on
  stable apply semantics (wholesale replace + superset guard + early-sync
  no-ops); drift is caught loudly by the ignored integration tests wired into
  the mission gate.
- Canon writes happen only after the full guard matrix passes and after
  `openspec validate --json` gates the change; a post-pre-sync engine failure
  leaves canon recoverable and the wrapper prints an explicit idempotent
  re-run instruction.
- Upstream contribution of native scenario ops = follow-up bd (deferral), not
  this change.
