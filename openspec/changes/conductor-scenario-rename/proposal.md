# Proposal: conductor-scenario-rename

## Why

Follow-up to `conductor-stage-rename` (bd rc-u2pt). The canon
`conductor-workflow` spec still carries the legacy scenario header
"mid-PHASE-3 compaction recovery" and a Stage Terminology exception
clause documenting why: `openspec archive` 1.7.0 refuses to drop or rename
an existing scenario header via a MODIFIED block.

The in-tree `openspec-sync-specs` skill sanctions a manual canon sync
where the delta is intent, not wholesale replacement. Applying the rename
through that path removes the last stage-meaning "PHASE" token from
living documents and retires the exception clause.

## What Changes

- Canon `openspec/specs/conductor-workflow/spec.md`: scenario header
  "mid-PHASE-3 compaction recovery" → "mid-STAGE-3 compaction recovery"
  (body already used STAGE vocabulary); exception paragraph removed from
  the Stage Terminology requirement.
- A matching delta spec is committed alongside, so `openspec archive`
  validates idempotently after merge (canon already equals the delta
  result; the matcher finds all canon scenarios present).

## Impact

Docs only: one canon spec file plus this change's artifacts. The archived
`2026-08-22-conductor-stage-rename` copy keeps the historical exception
text untouched. No code, no CLI upgrade required.
