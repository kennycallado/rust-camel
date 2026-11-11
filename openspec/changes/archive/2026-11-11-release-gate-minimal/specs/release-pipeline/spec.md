## REMOVED Requirements

### Requirement: CI preflight gates the tag pipeline on a tested commit

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
Requirement: CI preflight gates the tag pipeline on build and unit-test evidence.
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

## ADDED Requirements

### Requirement: CI preflight gates the tag pipeline on build and unit-test evidence

The tag wrapper (`release.yml`) SHALL contain a `ci-preflight` job, the
first job in the workflow, that fails closed before any build or publish
side effect unless the pushed tag points at a commit with build and
unit-test evidence:

- It SHALL resolve the tagged commit (`git rev-parse 'HEAD^{commit}'`,
  correct for annotated and lightweight tags).
- It SHALL query the GitHub Actions API for `ci.yml` workflow runs with
  `head_sha` equal to that commit
  (`repos/{owner}/{repo}/actions/workflows/ci.yml/runs?head_sha=<sha>&per_page=100`,
  reading `workflow_runs[].id`, `.run_attempt`, `.status`, `.event`, and
  `.head_branch`), and keep only **push-triggered on main** runs
  (`event == "push"`, `head_branch == "main"`, `status == "completed"`)
  **of any conclusion**: a failed run still carries per-job evidence.
  PR-triggered runs SHALL NOT qualify as evidence even when their
  conclusion reads `success`, because they test a synthetic merge commit
  rather than the tagged tree.
- For each qualifying run it SHALL read the attempt count
  (`run_attempt`) and query every attempt's jobs
  (`repos/{owner}/{repo}/actions/runs/<id>/attempts/<n>/jobs?per_page=100`,
  reading `jobs[].name`, `jobs[].conclusion`, `jobs[].steps[].name`,
  `jobs[].steps[].conclusion`).
- It SHALL consider the commit tested if and only if at least one
  attempt of a qualifying run has a job named `Unit Tests
  (ubuntu-latest)` whose conclusion is `success`, containing exactly one
  `Build` step and exactly one `Test (unit only — no Docker required)`
  step, both concluded `success`. Evidence SHALL be unioned across
  attempts: a later attempt that passes the job qualifies even when an
  earlier attempt of the same run failed it, so a flaky first attempt
  does not invalidate an otherwise green commit.
- It SHALL treat the `Full Tests (ubuntu-latest)` job as **advisory**:
  its conclusion SHALL be reported (green, red, or absent) in the
  preflight log, and it SHALL NOT fail the preflight. A red Full Tests
  job on a commit whose build and unit tests are green therefore does
  not block the release.
- It SHALL treat as NOT tested — and fail the job with an error naming
  the commit — every other case: no completed push-on-main run for the
  sha, only PR-triggered runs, no attempt with the `Unit Tests
  (ubuntu-latest)` job, that job failed or skipped, or either required
  step absent, duplicated, skipped, cancelled, or not concluded
  `success`.
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

- **GIVEN** a tag `v*` whose commit has no completed push-on-main CI run,
  or no attempt whose Unit Tests job and both required steps are green
- **WHEN** the tag push triggers `release.yml`
- **THEN** the `ci-preflight` job fails with an error naming the commit,
  and neither `call-release-matrix` nor `publish` starts (no matrix
  build, no GitHub Release, no docker push, no crates.io publish)

#### Scenario: a skipped unit-test step blocks the release

- **GIVEN** a tag whose commit's only push-on-main run has the Unit Tests
  job's `Build` or `Test (unit only — no Docker required)` step skipped
  (an earlier step failed)
- **WHEN** the preflight inspects that attempt's job steps
- **THEN** the step conclusion `skipped` fails the "build and unit tests
  ran and passed" check, the preflight fails, and the release does not
  start

#### Scenario: a red Full Tests does not block a build-and-unit-tested commit

- **GIVEN** a completed push-on-main run whose `Unit Tests
  (ubuntu-latest)` job and both required steps are green, and whose
  `Full Tests (ubuntu-latest)` job failed (for example a wall-clock
  ratio test on a loaded runner)
- **WHEN** the tag push triggers `release.yml`
- **THEN** the preflight passes, logs a warning naming the Full Tests
  conclusion, and the release pipeline starts

#### Scenario: green unit evidence from a later attempt counts

- **GIVEN** a qualifying run whose attempt 1 failed `Unit Tests
  (ubuntu-latest)` and whose attempt 2 (a re-run) has that job and both
  steps green
- **WHEN** the preflight unions job evidence across attempts
- **THEN** the commit counts as tested and the release proceeds

#### Scenario: PR-triggered evidence alone does not qualify

- **GIVEN** a tag whose commit has CI runs triggered only by
  `pull_request` events (all otherwise green)
- **WHEN** the preflight filters the runs list on
  `event == "push"` and `head_branch == "main"`
- **THEN** no run qualifies, the preflight fails with an error naming
  the commit, and the release does not start (PR runs test a synthetic
  merge commit, not the tagged tree)

#### Scenario: tag on a tested SHA proceeds unchanged

- **GIVEN** a tag `v*` whose commit has a completed push-on-main run in
  which `Unit Tests (ubuntu-latest)` and both required steps concluded
  `success`
- **WHEN** the tag push triggers `release.yml`
- **THEN** the preflight passes and the pipeline behaves exactly as
  before this change (same matrix, same assets, same docker and publish
  behaviour)
