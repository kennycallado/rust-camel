# Design: release-publisher-homing

## Context

e_glm adjudication (2026-09-18, full record:
`/tmp/nix-shell.vg8W2X/opencode/release-publisher-homing-adjudication.md`,
mirrored in bd rc-s926l): current main breaks trusted publishing on
the next tag. `job_workflow_ref` = the file CONTAINING the publish
job; the 60+ crates.io registrations name `release.yml`. The publish
job is self-contained (`publish-crates.sh` → `cargo xtask publish`
consumes workspace state only; no secrets beyond the job's own OIDC
token, no job outputs, no tag plumbing — `Extract version` is
docker-only). Crates.io has no trusted-publishing API: manual UI
re-registration × 60+ is the only alternative to homing.

## Goals / Non-Goals

- Goals: restore `job_workflow_ref = .github/workflows/release.yml`;
  zero crates.io re-registration; document the homing constraint so
  no future refactor re-breaks it.
- Non-Goals: dead `Docker metadata` step cleanup (bd rc-t2h1a);
  slim-http alias expiry (rc-n6iop); any change to dev wrapper or
  changelog.rs (the hardened version grep lives in the matrix
  release job, which stays).

## Decisions

### D1 — Home the publish job in the tag wrapper (homing)

`release.yml` gains a second job `publish`:
`needs: call-release-matrix`, `runs-on: ubuntu-latest`,
`environment: crates-io`, job-level `permissions: { contents: read,
id-token: write }`, and the 6 steps moved verbatim from the matrix
(checkout, toolchain, trustpub credential-provider install,
`scripts/publish-crates.sh`, …). The `if: inputs.publish` gate is
DROPPED — the wrapper only runs on `v*` tags, which IS the gate
(structural, not conditional). Mixed call-job + normal job with
`needs:` is legal GitHub Actions.

### D2 — Ordering coupling accepted (publish waits ALL matrix jobs)

`needs: call-release-matrix` waits for every matrix job (build legs,
docker, release). A docker failure now blocks crates publishing.
No partial-dependency mechanism exists in GitHub Actions. Accepted:
crates.io is the least reversible side effect; failing late-but-
together is safer than publishing while images failed. Declared as
an accepted difference in Req 2's "tag run publishes as before"
scenario.

### D3 — Matrix `publish` input semantics shrink

The input still gates every side effect INSIDE the reusable workflow:
docker logins/pushes/attest/imagetools + gh-release job. Its
description text updates to name those (crates.io no longer listed).
The in-file trusted-publisher comment block (~lines 248-253) moves
with the job to the wrapper.

### D4 — Docs revert with the WHY kept; ADR; checklist banner

`oidc-publish-fallback.md`: tuples (3) + `git show` verify command
(~line 94) revert to `release.yml`; intro (4-6) and line ~87
rephrase to pipeline-with-homed-publisher; the WHY sentence is
KEPT, inverted: the publish job MUST remain in `release.yml`
(`job_workflow_ref` coupling to 60+ registrations). bump-version
instruction AND prompt (`.opencode/prompts/` — hidden dir) revert
matrix-holds-publish claims. Small ADR (passes all three criteria:
proven-costly reversal, surprising-without-context, real trade-off)
+ CONTEXT-MAP decisions line. Archived checklist gets an OBSOLETE
banner (not deleted — evidence integrity).

## Risks / Trade-offs

- First-tag hazard: if any crate HAD been pre-flipped to
  release-matrix.yml, homing breaks it instead. Owner verified
  2026-09-18: zero re-registrations occurred.
- Wrapper grows from 18 to ~55 lines — still minimal; the thinness
  goal yields to the homing constraint (recorded in ADR).
- Dev wrapper semantics unchanged; closure-check unchanged.

## Migration Plan

Single change; lands before any `v*` tag push (none exists post-
0.49.0 tag window; no migration steps needed).

## Open Questions

None — e_glm adjudication resolved tier (full change, spec deltas
mandatory: canonical spec literally forbids wrapper-owned publish
steps today), shape, and gotchas.
