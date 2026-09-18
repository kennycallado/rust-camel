# Proposal: release-restructure

## Why

The release build matrix currently lives only in `release.yml`, the
tag-triggered publishing workflow. Every matrix or leg change is
verified solely by the real tag pipeline — there is no way to exercise
the full build/probe path on a branch, so drift between "what we
think the release does" and "what the tag run does" accumulates until
a release breaks (the rc-vnm8 sed-composition class, the rc-2ii8l
cross-target break both shipped through this gap). bd rc-5t5fo.4
(e_opus final-gate notes already recorded on the ticket) and the
distribution-flavor verdict
(docs/audits/2026-09-17-distribution-flavor-verdict.md) call for a
single reusable workflow with a publish toggle before the three-flavor
matrix switch (rc-5t5fo.5) lands on top.

## What Changes

- Extract the build matrix + probes + publish steps of
  `.github/workflows/release.yml` into a reusable workflow
  (`workflow_call`) with a `publish` boolean input
  (`.github/workflows/release-matrix.yml`).
- `release.yml` becomes a thin tag-triggered wrapper that calls the
  reusable workflow with `publish: true`, declares
  `contents:write, packages:write, id-token:write` permissions, and
  passes `secrets: inherit`.
- Add `.github/workflows/release-dev.yml`: `push` to select branches +
  `workflow_dispatch` trigger calling the same reusable workflow with
  `publish: false`; NO logins/credentials, no registry pushes, no GH
  release, no crates.io publish; docker uses `buildx --load` + a local
  smoke run; `concurrency: cancel-in-progress` on the dev wrapper ONLY.
- Inside the reusable workflow, the entire publish job (credential
  provider install, logins, pushes) is gated on `if: inputs.publish`.
- The dev harness runs the feature-closure test
  (resolver-2 unification tripwire) as part of its checks.
- Explicitly excluded: flavor-matrix changes (rc-5t5fo.5 — legs keep
  their current single-flavor shape), artifact renaming, docker tag
  remaps (rc-5t5fo.6/.9), binstall metadata (rc-5t5fo.8),
  smoke/capability-assert hardening beyond the local docker smoke
  (rc-5t5fo.2).

## Acceptance criteria

- `release.yml` contains no inline matrix: it only wraps
  (trigger + permissions + call). Tag semantics byte-equivalent for
  v-next (same legs, same probes, same publish outputs).
- `release-dev.yml` runs the identical matrix and probes with
  `publish: false`, zero secrets referenced, `cancel-in-progress: true`.
- Reusable workflow: every publish-path step carries
  `if: inputs.publish`; non-publish steps run in both modes.
- Dev-mode docker step uses `--load` + local run; no `push` verb
  reachable when `publish: false`.
- Feature-closure test executes in the dev harness.
- Both wrappers parse as valid YAML; `workflow_call` inputs declared
  exactly once.

## Risk budget

- Acceptable: workflow refactor churn inside `.github/workflows/`
  only; a temporarily larger diff while extracting; CI-only risk
  (no product code touched).
- Out of bounds: any change to build flags, feature composition, or
  artifact names (those belong to rc-5t5fo.5/.6/.8/.9); any secret
  exposure in the dev path; `cancel-in-progress` on the tag wrapper
  (cancelled `imagetools create` = half-published manifests).

Affected crates: none (`.github/workflows/` only).
bd: rc-5t5fo.4 (blocks rc-5t5fo.5; epic rc-5t5fo).
