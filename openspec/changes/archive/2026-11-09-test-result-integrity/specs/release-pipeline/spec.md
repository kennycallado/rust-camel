## ADDED Requirements

### Requirement: CI preflight gates the tag pipeline on a tested commit

The tag wrapper (`release.yml`) SHALL contain a `ci-preflight` job, the
first job in the workflow, that fails closed before any build or publish
side effect unless the pushed tag points at a tested commit:

- It SHALL resolve the tagged commit (`git rev-parse 'HEAD^{commit}'`,
  correct for annotated and lightweight tags).
- It SHALL query the GitHub Actions API for `ci.yml` workflow runs with
  `head_sha` equal to that commit (`repos/{owner}/{repo}/actions/workflows/ci.yml/runs?head_sha=<sha>&per_page=100`,
  reading `workflow_runs[].id`, `.conclusion`, `.event`, and
  `.head_branch`), and keep only completed successful runs that are
  **push-triggered on main** (`event == "push"`,
  `head_branch == "main"`, `conclusion == "success"`). PR-triggered
  runs SHALL NOT qualify as evidence even when their conclusion reads
  `success`, because they test a synthetic merge commit rather than the
  tagged tree. For each qualifying run query its jobs
  (`repos/{owner}/{repo}/actions/runs/<id>/jobs?per_page=100`, reading
  `jobs[].name`, `jobs[].conclusion`, `jobs[].steps[].name`,
  `jobs[].steps[].conclusion`).
- It SHALL consider the commit tested if and only if at least one
  qualifying (push-on-main, successful) CI run has a job named
  `Full Tests (ubuntu-latest)` whose conclusion is `success` and whose
  `Test (full workspace)` and `Test camel-cli (exec flavor)` steps both
  concluded `success`.
- It SHALL treat as NOT tested — and fail the job with an error naming
  the commit — every other case: no CI run for the sha, no qualifying
  run, only PR-triggered runs, a cancelled or failed run, the Full
  Tests job absent, failed, or skipped, or either test step not
  concluded `success`.
- It SHALL fail on any API error (nonzero `gh` exit under
  `set -euo pipefail`; no `|| true`, no `continue-on-error`).
- It SHALL authenticate with the built-in `GITHUB_TOKEN` and declare the
  `actions: read` permission (plus `contents: read` for checkout), with
  a comment stating that scope; no additional secret SHALL be required.
- The `call-release-matrix` job SHALL declare
  `needs: [ci-preflight]`, so an untested SHA fails the run before the
  build matrix, the GitHub Release, the docker images, or the crates.io
  publish start. The `publish` job SHALL keep `needs: call-release-matrix`
  (transitively gated).

#### Scenario: tag on an untested SHA fails closed before any side effect

- **GIVEN** a tag `v*` whose commit has no successful CI run (never ran,
  or its latest runs are failed or cancelled)
- **WHEN** the tag push triggers `release.yml`
- **THEN** the `ci-preflight` job fails with an error naming the commit,
  and neither `call-release-matrix` nor `publish` starts (no matrix
  build, no GitHub Release, no docker push, no crates.io publish)

#### Scenario: a skipped Full Tests step blocks the release

- **GIVEN** a tag whose commit's only successful-looking CI run has the
  Full Tests job's workspace test step skipped (an earlier step failed)
- **WHEN** the preflight inspects that run's job steps
- **THEN** the step conclusion `skipped` fails the "tests ran and
  passed" check, the preflight fails, and the release does not start

#### Scenario: PR-triggered evidence alone does not qualify

- **GIVEN** a tag whose commit has CI runs triggered only by
  `pull_request` events (all otherwise green)
- **WHEN** the preflight filters the runs list on
  `event == "push"` and `head_branch == "main"`
- **THEN** no run qualifies, the preflight fails with an error naming
  the commit, and the release does not start (PR runs test a synthetic
  merge commit, not the tagged tree)

#### Scenario: tag on a tested SHA proceeds unchanged

- **GIVEN** a tag `v*` whose commit has a completed successful
  push-on-main CI run in which the Full Tests job and both test steps
  concluded `success`
- **WHEN** the tag push triggers `release.yml`
- **THEN** the preflight passes and the pipeline behaves exactly as
  before this change (same matrix, same assets, same docker and publish
  behaviour)

## MODIFIED Requirements

### Requirement: Release build matrix has a single reusable source of truth

The release build matrix (legs, targets, runners, feature
composition, probes) SHALL live in exactly one reusable workflow
(`release-matrix.yml`, `on: workflow_call`) invoked by wrappers.
The tag wrapper (`release.yml`) SHALL contain no inline matrix: it
declares the tag trigger, the publish permissions
(`contents: write`, `packages: write`, `id-token: write`), the
secrets handoff (`secrets: inherit`), a single call with
`publish: true`, and the fail-closed `ci-preflight` job of
Requirement: CI preflight gates the tag pipeline on a tested commit.
The tag wrapper SHALL also home the crates.io
publish job (see Requirement: Publish side effects are gated on the
publish input) — this homing is a hard constraint: crates.io
trusted publishing matches the OIDC `job_workflow_ref` (the
workflow file containing the publish job) against each crate's
registered entry, and the 60+ published crates are registered
against `.github/workflows/release.yml`.

The tag matrix SHALL build the three-flavor topology (12 entries,
11 uploading): slim on x86_64/aarch64 musl; regular on the 4 Linux
targets plus a macOS compile-guard entry (upload: false, protects
cargo-install source builds); full on gnu ×2 (aarch64 on the native
`ubuntu-24.04-arm` runner), macOS ×2, and Windows. Desktop platforms
ship full-only (owner ruling post-rc.2 CI rehearsal). Each leg SHALL select its closure exclusively via the
`flavor-<x>` marker feature (`--features flavor-<x>`); no leg SHALL
compose feature lists by string interpolation. The reusable
workflow SHALL accept a `dev-profile` boolean input (default
`false`); when true, the build matrix reduces to the 3-leg
representative dev subset.

#### Scenario: tag wrapper is thin

- **GIVEN** `.github/workflows/release.yml` and the reusable
  `.github/workflows/release-matrix.yml`
- **WHEN** the tag wrapper file is inspected
- **THEN** it declares the `v*` tag trigger, the three publish
  permissions, `secrets: inherit`, exactly one job that calls the
  reusable workflow with `publish: true`, the crates.io
  publish job (with `environment: crates-io` and
  `needs: call-release-matrix`), and the `ci-preflight` job gating the
  reusable-workflow call (`call-release-matrix` carries
  `needs: [ci-preflight]`), and it contains no build matrix and
  no probe steps

#### Scenario: dev wrapper calls the same matrix

- **GIVEN** `.github/workflows/release-dev.yml`
- **WHEN** a push to a tracked branch or a `workflow_dispatch` runs
  it
- **THEN** it invokes the identical reusable workflow with
  `publish: false` and `dev-profile: true`, references no secrets,
  and carries `concurrency` with `cancel-in-progress: true`

#### Scenario: dev profile trims the build matrix

- **GIVEN** the reusable workflow invoked with `publish: false` and
  `dev-profile: true`
- **WHEN** the build matrix is expanded
- **THEN** exactly 3 build legs run — x86_64-unknown-linux-gnu
  (full), x86_64-unknown-linux-musl (regular), x86_64-apple-darwin
  (regular) — and the closure-check job runs unchanged; with
  `dev-profile: false` (tag path) all 12 entries run
