# Proposal: audit-fix-docdrift-t1-baseline

## Why

The T1 fidelity audit (`docs/audits/modules/*-quality-2026-08-05.md`) found
`FC-DOC-DRIFT` findings across the five T1 crates (camel-api, camel-config,
camel-dsl, camel-cli, camel-builder). These are stale TODO comments, phantom
rustdoc references, README lists that lag the shipped command/flag surface, a
dead citation, and a version-stale error string. Owner decision 2026-08-07
made clean documentation a freeze requirement, so this baseline must close
before the `lint-context-citations` enforcement gate (D2, rc-9h5a) can land.

## What Changes

**In scope (code-stream doc drift in the five T1 crates):**
- camel-api: fix phantom rustdoc references to `CamelError::NotFound(...)` in
  `claim_check.rs` (the variant does not exist; only `ComponentNotFound` does);
  resolve the `TODO(API-006)` re-export comment in `lib.rs`.
- camel-config: remove/update the three stale `TODO(CONFIG-004)` comments that
  claim hot-reload is not implemented (it is implemented and consumed).
- camel-dsl: sync the README step tables with the shipped step surface.
- camel-cli: sync the README top-level command list with the `Commands` enum
  (add `plugin`/`openapi`), fix the malformed `## Overview` block, add the
  missing `camel run` OTel flags; remove the dead `TODO(PROC-004)` citation in
  `CONTEXT.md`.
- camel-builder: update the version-stale `"canonical v1"` error strings to
  `v2` (ADR-0016 v2 is current) and the coupled test assertions (M4).

**Explicitly excluded:**
- `CONTEXT.md` semantic drift already resolved by the oracle in real time.
- The `lint-context-citations` xtask itself (rc-9h5a, change D2, blocked-by
  this baseline).
- T2/T3 doc-drift crates (rc-acd3, separate change D3).
- builder M2 (BUILDER-003/006 numbering collision + untracked open items):
  split to bd rc-z6zw (discovered-from rc-bwbg). M2 is numbering-reconciliation
  + beads-tracking, a different work shape from this mechanical baseline.

## Acceptance criteria

- All FC-DOC-DRIFT T1 findings (api I1/M5, config M1, dsl M2, cli M2,
  builder M4) closed: stale TODOs removed or updated to actual state,
  READMEs synced with the shipped surface, dead citations removed, `canonical
  v2` strings consistent across source and tests. Builder M2 is split to
  rc-z6zw (not closed by this change).
- `cargo fmt --check` and `cargo clippy` green on the touched crates.
- No structural change. The only user-visible change is the M4 error-message
  text (`canonical v1` → `canonical v2`): an intentional diagnostic
  compatibility change that preserves rejection and compilation semantics
  (the same steps are rejected the same way; only the version label in the
  message changes). Exact-string consumers (tests, downstream matchers) must
  update from `v1` to `v2` in lockstep.

## Risk budget

- Zero structural change. The M4 edit changes observable diagnostic text
  (`canonical v1` → `canonical v2`). It preserves rejection and compilation
  semantics — the same steps are rejected the same way; only the version
  label moves. Any exact-string consumer (tests, downstream matchers) must
  update in lockstep. This is an intentional diagnostic-text change, not a
  silent contract break.
- Out of bounds: any change to trait/Service signatures, step semantics,
  canonical compilation behavior, or the set of steps that canonical rejects.

## Bd

- rc-bwbg (child of rc-w5yo, the FC-DOC-DRIFT T1 epic).
