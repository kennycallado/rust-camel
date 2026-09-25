# Proposal: devceil — formal verification of the dev wrapper permission ceiling

## Why

bd rc-myx4r (P2). The release-publisher-homing change (commit b56f9681,
2026-09-18) moved crates publishing out of the reusable
`release-matrix.yml` into the tag wrapper `release.yml`. After that move,
the dev wrapper `release-dev.yml` is the only caller with
`publish: false`, and its top-level permissions block is the request
ceiling for every nested job permission request in the reusable
workflow. GitHub validates nested permission requests at workflow load,
even for jobs gated off by `if:` — the rc-42pkx hotfix class. Slimming
the ceiling re-breaks every main push.

Three open questions needed formal answers:

1. Does the dev wrapper ceiling still cover the union of nested job
   permission requests after the homing move?
2. The homing change artifacts appeared missing from the
   dev-perms-hotfix worktree. Were they lost, or only absent from that
   worktree's base?
3. The homing tasks.md Task 1 acceptance says
   "`release-dev.yml` untouched". Does that claim hold against
   post-homing history?

Two ride-along dispositions complete the record: the test-harness
question (the mission forbids inventing CI) and the hardening question
(the docker job requests `packages: write` + `id-token: write` at load
even in dev runs, where the elevated scopes stay unconsumed).

Fresh evidence 2026-09-25: the Release (dev) workflow ran green on the
v0.54.0 main push — the ceiling holds empirically today. This is the
formal record, not an emergency.

## What Changes

Documentation only. This change dir records verified findings:

- Permissions union computation for all four jobs of
  `release-matrix.yml` (build, release, docker, closure-check), with
  line refs at main dbff0a90, and the verdict that `release-dev.yml`
  L17-20 carries exactly the required trio.
- Location of the homing artifacts: present under
  `openspec/changes/archive/2026-09-18-release-publisher-homing/` since
  commit 83870932. The bd "not found" symptom came from the
  dev-perms-hotfix worktree being cut from a base predating the archive
  commit. No re-baseline fabrication — history is intact.
- T1 re-baseline: the full git history of `release-dev.yml` has exactly
  two commits (02eec1be, dea5c9c7). The permissions block is
  byte-identical since 02eec1be. Post-homing drift is one added line
  (`dev-profile: true`), a call input, not a permissions change.
- Harness disposition: no workflow-YAML test harness exists in the
  repo. The documented manual verification matrix in design.md is the
  accepted evidence, cross-checked by the green v0.54.0 dev run.
- Hardening ride-along disposition: KEEP the current docker job
  permissions. Rationale recorded in design.md.

Zero code changes. Zero workflow edits.

## Impact

- Affected: `openspec/changes/devceil/` (this record) only. No Rust, no
  workflows, no scripts.
- Closes the verification scope of bd rc-myx4r; the record gives future
  maintainers the exact line refs and history needed before touching
  the dev wrapper ceiling.

## Spec delta

None (`skip_specs: true`). The canonical release-pipeline spec does
not encode the ceiling: its "dev wrapper calls the same matrix"
scenario requires only `publish: false`, `dev-profile: true`, no
secrets, and concurrency — it never mentions the permission trio. The
trio mandate is documented in the `release-dev.yml` comment (added by
02eec1be) and bd rc-42pkx. The mission expected no spec delta, and
none is needed. Current workflows conform.
